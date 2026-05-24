#![cfg(unix)]

use std::cell::Cell;
use std::collections::VecDeque;
use std::env;
use std::io;
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use panesmith_attach::{
    AttachInputChunk, AttachOptions, AttachSurfaceSink, AttachTerminal, BlockingAttachSession,
    CrosstermTerminalControl, RawModeOps,
};
use panesmith_core::{
    DetachReason, PaneConfig, PaneId, PortablePtyBackend, PtyBackend, PtyFrame, PtyProcess, Size,
};

const STRESS_ATTACH_DETACH_LOOPS: usize = 50;

fn fixture_path() -> PathBuf {
    env::var("CARGO_BIN_EXE_echo-tui")
        .or_else(|_| env::var("CARGO_BIN_EXE_echo_tui"))
        .map(PathBuf::from)
        .expect("echo-tui fixture path should be available to integration tests")
}

fn spawn_fixture(config: PaneConfig) -> impl PtyProcess {
    PortablePtyBackend
        .spawn(&config)
        .expect("fixture should spawn through portable PTY backend")
}

fn normalize(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).replace('\r', "")
}

fn wait_for_output(process: &mut impl PtyProcess, needle: &str, timeout: Duration) -> String {
    let deadline = Instant::now() + timeout;
    let mut seen = String::new();

    while Instant::now() < deadline {
        while let Some(frame) = process.try_recv() {
            match frame {
                PtyFrame::Output { bytes, .. } => {
                    seen.push_str(&normalize(&bytes));
                    if seen.contains(needle) {
                        return seen;
                    }
                }
                PtyFrame::Overflow {
                    dropped_frames,
                    dropped_bytes,
                    ..
                } => {
                    panic!(
                        "unexpected PTY overflow while waiting for {needle:?}: dropped {dropped_frames} frame(s) / {dropped_bytes} byte(s)"
                    );
                }
                PtyFrame::Error { message, .. } => {
                    panic!("unexpected PTY error while waiting for {needle:?}: {message}");
                }
                PtyFrame::Exited { code, .. } => {
                    panic!("process exited early with code {code:?} while waiting for {needle:?}");
                }
                PtyFrame::CursorPositionRequest { .. } => {}
            }
        }

        thread::sleep(Duration::from_millis(10));
    }

    panic!("timed out waiting for {needle:?}; saw output {seen:?}");
}

fn wait_for_exit(process: &mut impl PtyProcess, timeout: Duration) -> Option<i32> {
    let deadline = Instant::now() + timeout;

    while Instant::now() < deadline {
        while let Some(frame) = process.try_recv() {
            match frame {
                PtyFrame::Exited { code, .. } => return code,
                PtyFrame::Error { message, .. } => {
                    panic!("unexpected PTY error while waiting for exit: {message}");
                }
                PtyFrame::Output { .. }
                | PtyFrame::Overflow { .. }
                | PtyFrame::CursorPositionRequest { .. } => {}
            }
        }

        thread::sleep(Duration::from_millis(10));
    }

    panic!("timed out waiting for process exit");
}

fn wait_for_pid_absent(pid: u32, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    loop {
        let output = Command::new("ps")
            .args(["-o", "stat=", "-p", &pid.to_string()])
            .output()
            .expect("ps should run for process verification");
        let state = String::from_utf8_lossy(&output.stdout).trim().to_string();

        if state.is_empty() {
            return;
        }

        if Instant::now() >= deadline {
            panic!("pid {pid} was not reaped before timeout; ps state={state:?}");
        }

        thread::sleep(Duration::from_millis(20));
    }
}

fn child_pid(process: &mut impl PtyProcess) -> u32 {
    process
        .writer()
        .write_bytes(b"__PANESMITH_PID__\n")
        .expect("pid probe should write");
    let output = wait_for_output(process, "pid:", Duration::from_secs(3));
    output
        .lines()
        .find_map(|line| line.strip_prefix("pid:"))
        .expect("pid probe should report a pid line")
        .parse::<u32>()
        .expect("fixture pid should be numeric")
}

#[derive(Debug)]
struct ScheduledTerminal {
    stdin: VecDeque<AttachInputChunk>,
    stdout: Vec<Vec<u8>>,
    size: Size,
    size_calls: Cell<usize>,
}

impl ScheduledTerminal {
    fn new(size: Size) -> Self {
        Self {
            stdin: VecDeque::new(),
            stdout: Vec::new(),
            size,
            size_calls: Cell::new(0),
        }
    }

    fn queue_stdin(&mut self, at: Instant, bytes: impl Into<Vec<u8>>) {
        self.stdin.push_back(AttachInputChunk {
            at,
            bytes: bytes.into(),
        });
    }

    fn stdout_bytes(&self) -> Vec<u8> {
        self.stdout.concat()
    }
}

impl AttachTerminal for ScheduledTerminal {
    type Error = &'static str;

    fn read_stdin(&mut self) -> Result<Option<AttachInputChunk>, Self::Error> {
        match self.stdin.front() {
            Some(chunk) if chunk.at <= Instant::now() => Ok(self.stdin.pop_front()),
            _ => Ok(None),
        }
    }

    fn write_stdout(&mut self, bytes: &[u8]) -> Result<(), Self::Error> {
        self.stdout.push(bytes.to_vec());
        Ok(())
    }

    fn size(&self) -> Result<Size, Self::Error> {
        self.size_calls.set(self.size_calls.get() + 1);
        Ok(self.size)
    }
}

#[derive(Debug, Clone, Default)]
struct FakeRawModeOps {
    state: Arc<Mutex<RawModeState>>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct RawModeState {
    enabled: bool,
    enable_calls: u32,
    disable_calls: u32,
    query_calls: u32,
}

impl FakeRawModeOps {
    fn snapshot(&self) -> RawModeState {
        *self.state.lock().expect("raw mode state lock")
    }
}

impl RawModeOps for FakeRawModeOps {
    fn is_raw_mode_enabled(&self) -> io::Result<bool> {
        let mut state = self.state.lock().expect("raw mode state lock");
        state.query_calls += 1;
        Ok(state.enabled)
    }

    fn enable_raw_mode(&self) -> io::Result<()> {
        let mut state = self.state.lock().expect("raw mode state lock");
        state.enable_calls += 1;
        state.enabled = true;
        Ok(())
    }

    fn disable_raw_mode(&self) -> io::Result<()> {
        let mut state = self.state.lock().expect("raw mode state lock");
        state.disable_calls += 1;
        state.enabled = false;
        Ok(())
    }
}

#[derive(Debug, Default)]
struct RecordingSurface {
    resize_calls: Vec<Size>,
    feed_calls: Vec<Vec<u8>>,
}

impl RecordingSurface {
    fn feed_bytes(&self) -> Vec<u8> {
        self.feed_calls.concat()
    }
}

impl AttachSurfaceSink for RecordingSurface {
    type Error = &'static str;

    fn feed_output(&mut self, bytes: &[u8]) -> Result<(), Self::Error> {
        self.feed_calls.push(bytes.to_vec());
        Ok(())
    }

    fn resize(&mut self, size: Size) -> Result<(), Self::Error> {
        self.resize_calls.push(size);
        Ok(())
    }
}

#[test]
#[ignore = "stress test: repeated real-PTY attach/detach loops"]
fn repeated_attach_detach_restores_terminal_and_reaps_child() {
    let embedded_size = Size::new(12, 34);
    let terminal_size = Size::new(40, 120);
    let fixture_program = fixture_path().to_string_lossy().into_owned();
    let mut process = spawn_fixture(PaneConfig::command(fixture_program).with_size(embedded_size));
    let child_pid = child_pid(&mut process);
    let mut surface = RecordingSurface::default();
    let raw_mode = FakeRawModeOps::default();
    let mut next_event_seq = 0;

    for iteration in 0..STRESS_ATTACH_DETACH_LOOPS {
        let mut terminal = ScheduledTerminal::new(terminal_size);
        let start = Instant::now();
        terminal.queue_stdin(start, format!("stress-loop-{iteration}\r"));
        terminal.queue_stdin(start + Duration::from_millis(50), vec![0x1d]);

        let mut session = BlockingAttachSession::new(
            PaneId::new(77),
            AttachOptions::default(),
            embedded_size,
            next_event_seq,
        )
        .with_poll_interval(Duration::from_millis(1));
        let mut control = CrosstermTerminalControl::with_raw_mode_ops(io::sink(), raw_mode.clone())
            .with_host_alternate_screen(true);

        let outcome = session
            .run(&mut terminal, &mut process, &mut surface, &mut control)
            .expect("attach session should detach cleanly during stress loop");
        next_event_seq = session.last_event_seq();

        assert_eq!(
            outcome.reason,
            DetachReason::UserChord,
            "unexpected detach reason on loop {iteration}"
        );
        assert!(
            normalize(&terminal.stdout_bytes()).contains(&format!("stress-loop-{iteration}\n")),
            "expected attached stdout echo on loop {iteration}"
        );
        assert!(
            normalize(&surface.feed_bytes()).contains(&format!("stress-loop-{iteration}\n")),
            "expected surface to mirror attached output on loop {iteration}"
        );
        assert_eq!(
            surface.resize_calls.last().copied(),
            Some(embedded_size),
            "expected detach to restore embedded size on loop {iteration}"
        );

        process
            .writer()
            .write_bytes(b"__PANESMITH_SIZE__\n")
            .expect("post-detach size probe should write");
        let restored = wait_for_output(&mut process, "size:12x34\n", Duration::from_secs(3));
        assert!(
            restored.contains("size:12x34\n"),
            "expected embedded size to be restored after loop {iteration}, got {restored:?}"
        );
    }

    let raw_mode_state = raw_mode.snapshot();
    assert_eq!(
        raw_mode_state.enable_calls,
        STRESS_ATTACH_DETACH_LOOPS as u32
    );
    assert_eq!(
        raw_mode_state.disable_calls,
        STRESS_ATTACH_DETACH_LOOPS as u32
    );
    assert!(!raw_mode_state.enabled);

    process
        .writer()
        .write_bytes(b"__PANESMITH_EXIT__\n")
        .expect("fixture exit request should write");
    assert_eq!(wait_for_exit(&mut process, Duration::from_secs(3)), Some(0));

    drop(process);
    wait_for_pid_absent(child_pid, Duration::from_secs(3));
}
