use std::cell::Cell;
use std::collections::VecDeque;
use std::env;
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use panesmith_attach::{
    AttachInputChunk, AttachOptions, AttachOutputPolicy, AttachResizePolicy, AttachScreenPolicy,
    AttachSurfaceSink, AttachTerminal, BlockingAttachSession, CrosstermTerminalControl,
    HostTerminalControl, RawModeOps, StdioAttachTerminal,
};
use panesmith_core::{
    DetachReason, PaneConfig, PaneId, PortablePtyBackend, PtyBackend, PtyFrame, PtyProcess, Size,
};
use panesmith_core::{PaneAttachTerminal, SurfaceBackend};
use panesmith_vt100::Vt100Backend;

const REAL_PTY_DETACH_DELAY: Duration = Duration::from_millis(750);

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

fn real_pty_test_lock() -> MutexGuard<'static, ()> {
    static REAL_PTY_TEST_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    REAL_PTY_TEST_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn normalize(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).replace('\r', "")
}

#[derive(Debug)]
struct ScheduledTerminal {
    stdin: VecDeque<AttachInputChunk>,
    stdout: Vec<Vec<u8>>,
    size: Size,
    size_calls: Cell<usize>,
    drop_first_ready_stdin_before_size_calls: Option<usize>,
}

impl ScheduledTerminal {
    fn new(size: Size) -> Self {
        Self {
            stdin: VecDeque::new(),
            stdout: Vec::new(),
            size,
            size_calls: Cell::new(0),
            drop_first_ready_stdin_before_size_calls: None,
        }
    }

    fn queue_stdin(&mut self, at: Instant, bytes: impl Into<Vec<u8>>) {
        self.stdin.push_back(AttachInputChunk {
            at,
            bytes: bytes.into(),
        });
    }

    fn with_drop_first_ready_stdin_before_size_calls(mut self, required_size_calls: usize) -> Self {
        self.drop_first_ready_stdin_before_size_calls = Some(required_size_calls);
        self
    }

    fn stdout_bytes(&self) -> Vec<u8> {
        self.stdout.concat()
    }
}

impl AttachTerminal for ScheduledTerminal {
    type Error = &'static str;

    fn read_stdin(&mut self) -> Result<Option<AttachInputChunk>, Self::Error> {
        match self.stdin.front() {
            Some(chunk) if chunk.at <= Instant::now() => {
                if let Some(required_size_calls) =
                    self.drop_first_ready_stdin_before_size_calls.take()
                {
                    if self.size_calls.get() < required_size_calls {
                        let _ = self.stdin.pop_front();
                        return Ok(None);
                    }
                }
                Ok(self.stdin.pop_front())
            }
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

fn surface_text(surface: &impl SurfaceBackend) -> String {
    surface
        .snapshot()
        .rows
        .iter()
        .map(|row| {
            row.cells
                .iter()
                .map(|cell| cell.text.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[derive(Debug, Clone, Default)]
struct SharedWrite(Arc<Mutex<Vec<u8>>>);

impl SharedWrite {
    fn bytes(&self) -> Vec<u8> {
        self.0.lock().expect("shared write lock").clone()
    }
}

#[derive(Debug, Clone, Default)]
struct FlakyWrite {
    state: Arc<Mutex<FlakyWriteState>>,
}

#[derive(Debug, Default)]
struct FlakyWriteState {
    write_failures: VecDeque<io::ErrorKind>,
    flush_failures: VecDeque<io::ErrorKind>,
    bytes: Vec<u8>,
}

impl FlakyWrite {
    fn with_failures(
        write_failures: impl IntoIterator<Item = io::ErrorKind>,
        flush_failures: impl IntoIterator<Item = io::ErrorKind>,
    ) -> Self {
        Self {
            state: Arc::new(Mutex::new(FlakyWriteState {
                write_failures: write_failures.into_iter().collect(),
                flush_failures: flush_failures.into_iter().collect(),
                bytes: Vec::new(),
            })),
        }
    }

    fn bytes(&self) -> Vec<u8> {
        self.state.lock().expect("flaky write lock").bytes.clone()
    }

    fn push_write_failures(&self, failures: impl IntoIterator<Item = io::ErrorKind>) {
        self.state
            .lock()
            .expect("flaky write lock")
            .write_failures
            .extend(failures);
    }

    fn push_flush_failures(&self, failures: impl IntoIterator<Item = io::ErrorKind>) {
        self.state
            .lock()
            .expect("flaky write lock")
            .flush_failures
            .extend(failures);
    }
}

impl Write for FlakyWrite {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut state = self.state.lock().expect("flaky write lock");
        if let Some(kind) = state.write_failures.pop_front() {
            return Err(io::Error::new(kind, "injected write retry"));
        }
        state.bytes.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        let mut state = self.state.lock().expect("flaky write lock");
        if let Some(kind) = state.flush_failures.pop_front() {
            return Err(io::Error::new(kind, "injected flush retry"));
        }
        Ok(())
    }
}

impl Write for SharedWrite {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0
            .lock()
            .expect("shared write lock")
            .extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
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

fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
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
fn crossterm_terminal_control_leaves_and_restores_host_alternate_screen() {
    let sink = SharedWrite::default();
    let raw_mode = FakeRawModeOps::default();
    let mut control = CrosstermTerminalControl::with_raw_mode_ops(sink.clone(), raw_mode.clone())
        .with_host_alternate_screen(true);

    let mut token = control
        .suspend_for_attach(AttachScreenPolicy::LeaveAlternateScreen)
        .expect("suspend should succeed");
    control
        .restore_after_attach(&mut token)
        .expect("restore should succeed");

    let bytes = sink.bytes();
    assert!(contains_bytes(&bytes, b"\x1b[?1049l"));
    assert!(contains_bytes(&bytes, b"\x1b[?1049h"));
    assert!(token.is_consumed());

    let state = raw_mode.snapshot();
    assert_eq!(state.enable_calls, 1);
    assert_eq!(state.disable_calls, 1);
    assert!(!state.enabled);
}

#[test]
fn crossterm_terminal_control_owns_attach_keyboard_and_mouse_modes() {
    let sink = SharedWrite::default();
    let raw_mode = FakeRawModeOps::default();
    let mut control = CrosstermTerminalControl::with_raw_mode_ops(sink.clone(), raw_mode);

    let mut token = control
        .suspend_for_attach(AttachScreenPolicy::ReuseHostAlternateScreen)
        .expect("suspend should succeed");
    control
        .restore_after_attach(&mut token)
        .expect("restore should succeed");

    let bytes = sink.bytes();
    let push_keyboard =
        find_bytes(&bytes, b"\x1b[>0u").expect("attach should push empty keyboard flags");
    let save_mouse =
        find_bytes(&bytes, b"\x1b[?1000s").expect("attach should save mouse tracking state");
    let enable_sgr_mouse =
        find_bytes(&bytes, b"\x1b[?1006h").expect("attach should enable SGR mouse capture");
    let restore_sgr_mouse =
        find_bytes(&bytes, b"\x1b[?1006r").expect("detach should restore SGR mouse state");
    let pop_keyboard = find_bytes(&bytes, b"\x1b[<1u").expect("detach should pop keyboard flags");

    assert!(push_keyboard < pop_keyboard);
    assert!(save_mouse < enable_sgr_mouse);
    assert!(enable_sgr_mouse < restore_sgr_mouse);
    assert!(token.is_consumed());
}

#[test]
fn crossterm_terminal_control_retries_temporary_wouldblock_writes() {
    let writer = FlakyWrite::with_failures(
        [io::ErrorKind::WouldBlock, io::ErrorKind::Interrupted],
        [io::ErrorKind::WouldBlock],
    );
    let raw_mode = FakeRawModeOps::default();
    let mut control = CrosstermTerminalControl::with_raw_mode_ops(writer.clone(), raw_mode);

    let mut token = control
        .suspend_for_attach(AttachScreenPolicy::ReuseHostAlternateScreen)
        .expect("temporary WouldBlock during suspend should be retried");

    writer.push_write_failures([io::ErrorKind::WouldBlock, io::ErrorKind::Interrupted]);
    writer.push_flush_failures([io::ErrorKind::Interrupted]);
    control
        .restore_after_attach(&mut token)
        .expect("temporary WouldBlock during restore should be retried");

    let bytes = writer.bytes();
    assert!(contains_bytes(&bytes, b"\x1b[>0u"));
    assert!(contains_bytes(&bytes, b"\x1b[<1u"));
    assert!(contains_bytes(&bytes, b"\x1b[?1006h"));
    assert!(contains_bytes(&bytes, b"\x1b[?1006r"));
    assert!(token.is_consumed());
}

#[cfg(unix)]
#[test]
fn stdio_attach_terminal_write_stdout_retries_temporary_wouldblock_errors() {
    let attach_writer = FlakyWrite::with_failures(
        [
            io::ErrorKind::WouldBlock,
            io::ErrorKind::Interrupted,
            io::ErrorKind::WouldBlock,
        ],
        [io::ErrorKind::WouldBlock],
    );
    let mut attach_terminal = StdioAttachTerminal::new(attach_writer.clone())
        .expect("stdio attach terminal should initialize");
    <StdioAttachTerminal<FlakyWrite> as AttachTerminal>::write_stdout(
        &mut attach_terminal,
        b"attach terminal bytes",
    )
    .expect("temporary write errors should be retried for AttachTerminal");
    drop(attach_terminal);
    assert_eq!(attach_writer.bytes(), b"attach terminal bytes");

    let manager_writer = FlakyWrite::with_failures(
        [io::ErrorKind::WouldBlock, io::ErrorKind::WouldBlock],
        [io::ErrorKind::Interrupted],
    );
    let mut manager_terminal = StdioAttachTerminal::new(manager_writer.clone())
        .expect("stdio attach terminal should initialize");
    <StdioAttachTerminal<FlakyWrite> as PaneAttachTerminal>::write_stdout(
        &mut manager_terminal,
        b"manager terminal bytes",
    )
    .expect("temporary write errors should be retried for PaneAttachTerminal");
    drop(manager_terminal);
    assert_eq!(manager_writer.bytes(), b"manager terminal bytes");
}

#[test]
fn blocking_attach_enter_fresh_alternate_screen_restores_terminal_sequences() {
    let _pty_lock = real_pty_test_lock();
    let embedded_size = Size::new(12, 34);
    let terminal_size = Size::new(40, 120);
    let fixture_program = fixture_path().to_string_lossy().into_owned();
    let mut process = spawn_fixture(PaneConfig::command(fixture_program).with_size(embedded_size));
    let mut surface = Vt100Backend::new(PaneId::new(10), embedded_size)
        .expect("surface backend should initialize");
    let mut terminal = ScheduledTerminal::new(terminal_size);
    terminal.queue_stdin(Instant::now(), vec![0x1d]);

    let sink = SharedWrite::default();
    let raw_mode = FakeRawModeOps::default();
    let mut control = CrosstermTerminalControl::with_raw_mode_ops(sink.clone(), raw_mode)
        .with_host_alternate_screen(false);
    let mut options = AttachOptions::default();
    options.screen = AttachScreenPolicy::EnterFreshAlternateScreen;
    let mut session = BlockingAttachSession::new(PaneId::new(10), options, embedded_size, 0)
        .with_poll_interval(Duration::from_millis(1));

    session
        .run(&mut terminal, &mut process, &mut surface, &mut control)
        .expect("attach session should detach cleanly");

    let bytes = sink.bytes();
    assert!(contains_bytes(&bytes, b"\x1b[?1049h"));
    assert!(contains_bytes(&bytes, b"\x1b[2J"));
    assert!(contains_bytes(&bytes, b"\x1b[1;1H"));
    assert!(contains_bytes(&bytes, b"\x1b[?1049l"));

    process
        .writer()
        .write_bytes(b"__PANESMITH_EXIT__\n")
        .expect("fixture exit request should write");
    assert_eq!(wait_for_exit(&mut process, Duration::from_secs(3)), Some(0));
}

#[test]
fn blocking_attach_forwards_output_and_restores_embedded_size_on_real_pty() {
    let _pty_lock = real_pty_test_lock();
    let embedded_size = Size::new(12, 34);
    let terminal_size = Size::new(40, 120);
    let fixture_program = fixture_path().to_string_lossy().into_owned();
    let mut process = spawn_fixture(PaneConfig::command(fixture_program).with_size(embedded_size));
    let mut surface = Vt100Backend::new(PaneId::new(1), embedded_size)
        .expect("surface backend should initialize");
    let mut terminal = ScheduledTerminal::new(terminal_size);

    let start = Instant::now();
    terminal.queue_stdin(start, b"__PANESMITH_SIZE__\r".to_vec());
    terminal.queue_stdin(start + Duration::from_millis(5), b"hello attach\r".to_vec());
    terminal.queue_stdin(start + REAL_PTY_DETACH_DELAY, vec![0x1d]);

    let sink = SharedWrite::default();
    let raw_mode = FakeRawModeOps::default();
    let mut control = CrosstermTerminalControl::with_raw_mode_ops(sink, raw_mode.clone())
        .with_host_alternate_screen(true);
    let mut session =
        BlockingAttachSession::new(PaneId::new(1), AttachOptions::default(), embedded_size, 0)
            .with_poll_interval(Duration::from_millis(1));

    let outcome = session
        .run(&mut terminal, &mut process, &mut surface, &mut control)
        .expect("attach session should detach cleanly");

    assert_eq!(outcome.reason, DetachReason::UserChord);
    assert_eq!(outcome.child_exit_code, None);
    assert_eq!(session.state(), panesmith_attach::AttachState::Embedded);

    let stdout = normalize(&terminal.stdout_bytes());
    assert!(
        stdout.contains("size:40x120\n"),
        "expected attached terminal size in stdout, got {stdout:?}"
    );
    assert!(
        stdout.contains("hello attach\n"),
        "expected attached stdout echo, got {stdout:?}"
    );
    assert!(
        surface_text(&surface).contains("hello attach"),
        "expected surface to mirror attached output"
    );

    process
        .writer()
        .write_bytes(b"__PANESMITH_SIZE__\n")
        .expect("post-detach size probe should write");
    let restored = wait_for_output(&mut process, "size:12x34\n", Duration::from_secs(3));
    assert!(restored.contains("size:12x34\n"));

    let state = raw_mode.snapshot();
    assert_eq!(state.enable_calls, 1);
    assert_eq!(state.disable_calls, 1);

    process
        .writer()
        .write_bytes(b"__PANESMITH_EXIT__\n")
        .expect("fixture exit request should write");
    assert_eq!(wait_for_exit(&mut process, Duration::from_secs(3)), Some(0));
}

#[test]
fn blocking_attach_waits_for_post_suspend_barrier_before_immediate_real_pty_input() {
    let _pty_lock = real_pty_test_lock();
    let embedded_size = Size::new(12, 34);
    let terminal_size = Size::new(40, 120);
    let fixture_program = fixture_path().to_string_lossy().into_owned();
    let mut process = spawn_fixture(PaneConfig::command(fixture_program).with_size(embedded_size));
    let mut surface = Vt100Backend::new(PaneId::new(11), embedded_size)
        .expect("surface backend should initialize");
    let mut terminal =
        ScheduledTerminal::new(terminal_size).with_drop_first_ready_stdin_before_size_calls(3);

    let start = Instant::now();
    terminal.queue_stdin(start, b"__PANESMITH_SIZE__\r".to_vec());
    terminal.queue_stdin(start + REAL_PTY_DETACH_DELAY, vec![0x1d]);

    let mut control = CrosstermTerminalControl::with_raw_mode_ops(
        SharedWrite::default(),
        FakeRawModeOps::default(),
    )
    .with_host_alternate_screen(true);
    let mut session =
        BlockingAttachSession::new(PaneId::new(11), AttachOptions::default(), embedded_size, 0)
            .with_poll_interval(Duration::from_millis(1));

    session
        .run(&mut terminal, &mut process, &mut surface, &mut control)
        .expect("attach session should detach cleanly");

    let stdout = normalize(&terminal.stdout_bytes());
    assert!(
        stdout.contains("size:40x120\n"),
        "expected the immediate queued input to be forwarded after the post-suspend barrier, got {stdout:?}"
    );

    process
        .writer()
        .write_bytes(b"__PANESMITH_EXIT__\n")
        .expect("fixture exit request should write");
    assert_eq!(wait_for_exit(&mut process, Duration::from_secs(3)), Some(0));
}

#[test]
fn blocking_attach_keep_embedded_size_skips_terminal_resize() {
    let _pty_lock = real_pty_test_lock();
    let embedded_size = Size::new(12, 34);
    let terminal_size = Size::new(40, 120);
    let fixture_program = fixture_path().to_string_lossy().into_owned();
    let mut process = spawn_fixture(PaneConfig::command(fixture_program).with_size(embedded_size));
    let mut surface = Vt100Backend::new(PaneId::new(3), embedded_size)
        .expect("surface backend should initialize");
    let mut terminal = ScheduledTerminal::new(terminal_size);

    let start = Instant::now();
    terminal.queue_stdin(start, b"__PANESMITH_SIZE__\r".to_vec());
    terminal.queue_stdin(start + REAL_PTY_DETACH_DELAY, vec![0x1d]);

    let mut options = AttachOptions::default();
    options.resize = AttachResizePolicy::KeepEmbeddedSize;
    let mut session = BlockingAttachSession::new(PaneId::new(3), options, embedded_size, 0)
        .with_poll_interval(Duration::from_millis(1));
    let mut control = CrosstermTerminalControl::with_raw_mode_ops(
        SharedWrite::default(),
        FakeRawModeOps::default(),
    )
    .with_host_alternate_screen(true);

    session
        .run(&mut terminal, &mut process, &mut surface, &mut control)
        .expect("attach session should detach cleanly");

    let stdout = normalize(&terminal.stdout_bytes());
    assert!(
        stdout.contains("size:12x34\n"),
        "expected embedded PTY size to remain unchanged, got {stdout:?}"
    );

    process
        .writer()
        .write_bytes(b"__PANESMITH_EXIT__\n")
        .expect("fixture exit request should write");
    assert_eq!(wait_for_exit(&mut process, Duration::from_secs(3)), Some(0));
}

#[test]
fn blocking_attach_stdout_only_replays_to_surface_after_detach() {
    let _pty_lock = real_pty_test_lock();
    let embedded_size = Size::new(12, 34);
    let terminal_size = Size::new(40, 120);
    let fixture_program = fixture_path().to_string_lossy().into_owned();
    let mut process = spawn_fixture(PaneConfig::command(fixture_program).with_size(embedded_size));
    let mut surface = RecordingSurface::default();
    let mut terminal = ScheduledTerminal::new(terminal_size);

    let start = Instant::now();
    terminal.queue_stdin(start, b"hello replay\r".to_vec());
    terminal.queue_stdin(start + REAL_PTY_DETACH_DELAY, vec![0x1d]);

    let mut options = AttachOptions::default();
    options.output = AttachOutputPolicy::StdoutOnlyThenReplay;
    let mut session = BlockingAttachSession::new(PaneId::new(4), options, embedded_size, 0)
        .with_poll_interval(Duration::from_millis(1));
    let mut control = CrosstermTerminalControl::with_raw_mode_ops(
        SharedWrite::default(),
        FakeRawModeOps::default(),
    )
    .with_host_alternate_screen(true);

    session
        .run(&mut terminal, &mut process, &mut surface, &mut control)
        .expect("attach session should detach cleanly");

    assert_eq!(surface.feed_calls.len(), 1);
    assert!(
        normalize(&surface.feed_bytes()).contains("hello replay\n"),
        "expected deferred surface replay after detach"
    );
    assert!(
        normalize(&terminal.stdout_bytes()).contains("hello replay\n"),
        "expected stdout to receive attached output immediately"
    );

    process
        .writer()
        .write_bytes(b"__PANESMITH_EXIT__\n")
        .expect("fixture exit request should write");
    assert_eq!(wait_for_exit(&mut process, Duration::from_secs(3)), Some(0));
}

#[test]
fn blocking_attach_returns_input_after_detach_chord_to_host() {
    let _pty_lock = real_pty_test_lock();
    let embedded_size = Size::new(12, 34);
    let terminal_size = Size::new(40, 120);
    let fixture_program = fixture_path().to_string_lossy().into_owned();
    let mut process = spawn_fixture(PaneConfig::command(fixture_program).with_size(embedded_size));
    let mut surface = Vt100Backend::new(PaneId::new(5), embedded_size)
        .expect("surface backend should initialize");
    let mut terminal = ScheduledTerminal::new(terminal_size);

    terminal.queue_stdin(Instant::now(), vec![0x1d, b'h', b'i']);

    let mut session =
        BlockingAttachSession::new(PaneId::new(5), AttachOptions::default(), embedded_size, 0)
            .with_poll_interval(Duration::from_millis(1));
    let mut control = CrosstermTerminalControl::with_raw_mode_ops(
        SharedWrite::default(),
        FakeRawModeOps::default(),
    )
    .with_host_alternate_screen(true);

    let outcome = session
        .run(&mut terminal, &mut process, &mut surface, &mut control)
        .expect("attach session should detach cleanly");

    assert_eq!(outcome.remaining_input, b"hi".to_vec());

    process
        .writer()
        .write_bytes(b"__PANESMITH_EXIT__\n")
        .expect("fixture exit request should write");
    assert_eq!(wait_for_exit(&mut process, Duration::from_secs(3)), Some(0));
}

#[test]
fn blocking_attach_preserves_restore_path_when_child_exits_while_attached() {
    let _pty_lock = real_pty_test_lock();
    let embedded_size = Size::new(12, 34);
    let terminal_size = Size::new(40, 120);
    let fixture_program = fixture_path().to_string_lossy().into_owned();
    let mut process = spawn_fixture(PaneConfig::command(fixture_program).with_size(embedded_size));
    let mut surface = Vt100Backend::new(PaneId::new(2), embedded_size)
        .expect("surface backend should initialize");
    let mut terminal = ScheduledTerminal::new(terminal_size);

    let start = Instant::now();
    terminal.queue_stdin(start, b"__PANESMITH_EXIT__\r".to_vec());
    terminal.queue_stdin(start + REAL_PTY_DETACH_DELAY, vec![0x1d]);

    let raw_mode = FakeRawModeOps::default();
    let mut control =
        CrosstermTerminalControl::with_raw_mode_ops(SharedWrite::default(), raw_mode.clone())
            .with_host_alternate_screen(true);
    let mut session =
        BlockingAttachSession::new(PaneId::new(2), AttachOptions::default(), embedded_size, 0)
            .with_poll_interval(Duration::from_millis(1));

    let outcome = session
        .run(&mut terminal, &mut process, &mut surface, &mut control)
        .expect("attach session should still detach after child exit");

    assert_eq!(outcome.reason, DetachReason::UserChord);
    assert_eq!(outcome.child_exit_code, Some(0));
    assert_eq!(session.state(), panesmith_attach::AttachState::Embedded);

    let state = raw_mode.snapshot();
    assert_eq!(state.enable_calls, 1);
    assert_eq!(state.disable_calls, 1);
}
