#![cfg(unix)]

use std::collections::VecDeque;
use std::io;
use std::time::{Duration, Instant};

mod support;

use panesmith_attach::{
    AttachInputChunk, AttachOptions, AttachTerminal, BlockingAttachSession,
    CrosstermTerminalControl, RawModeOps,
};
use panesmith_core::SurfaceBackend;
use panesmith_core::{DetachReason, PaneConfig, PaneId, PtyProcess, Size};
use panesmith_vt100::Vt100Backend;
use support::{fixture_path, normalize, spawn_fixture, wait_for_exit, WAIT_TIMEOUT};

const READINESS_TIMEOUT: Duration = WAIT_TIMEOUT;
const ATTACH_READY_BYTES: &[u8] = b"\x1b[?1004;2004hprompt> ";
const BRACKETED_PASTE_BYTES: &[u8] = b"\x1b[200~cargo test --workspace\x1b[201~";

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

#[derive(Debug)]
struct ScheduledInput {
    chunk: AttachInputChunk,
    requires_stdout: Option<&'static [u8]>,
    ready_deadline: Option<Instant>,
}

#[derive(Debug)]
struct ScheduledTerminal {
    stdin: VecDeque<ScheduledInput>,
    stdout: Vec<u8>,
    size: Size,
}

impl ScheduledTerminal {
    fn new(size: Size) -> Self {
        Self {
            stdin: VecDeque::new(),
            stdout: vec![],
            size,
        }
    }

    fn queue_stdin(&mut self, at: Instant, bytes: impl Into<Vec<u8>>) {
        self.queue_stdin_when(at, bytes, None);
    }

    fn queue_stdin_after_stdout(
        &mut self,
        at: Instant,
        bytes: impl Into<Vec<u8>>,
        required_stdout: &'static [u8],
    ) {
        self.queue_stdin_when(at, bytes, Some(required_stdout));
    }

    fn queue_stdin_when(
        &mut self,
        at: Instant,
        bytes: impl Into<Vec<u8>>,
        requires_stdout: Option<&'static [u8]>,
    ) {
        self.stdin.push_back(ScheduledInput {
            chunk: AttachInputChunk {
                at,
                bytes: bytes.into(),
            },
            requires_stdout,
            ready_deadline: requires_stdout.map(|_| at + READINESS_TIMEOUT),
        });
    }

    fn saw_stdout(&self, needle: &[u8]) -> bool {
        self.stdout
            .windows(needle.len())
            .any(|window| window == needle)
    }

    fn stdout_bytes(&self) -> &[u8] {
        &self.stdout
    }
}

impl AttachTerminal for ScheduledTerminal {
    type Error = &'static str;

    fn read_stdin(&mut self) -> Result<Option<AttachInputChunk>, Self::Error> {
        match self.stdin.front() {
            Some(input) if input.chunk.at <= Instant::now() => {
                if let Some(required_stdout) = input.requires_stdout {
                    if !self.saw_stdout(required_stdout) {
                        if let Some(deadline) = input.ready_deadline {
                            if Instant::now() >= deadline {
                                return Err(
                                    "timed out waiting for attach-ready stdout marker before injecting scheduled input",
                                );
                            }
                        }
                        return Ok(None);
                    }
                }

                Ok(self.stdin.pop_front().map(|input| input.chunk))
            }
            _ => Ok(None),
        }
    }

    fn write_stdout(&mut self, bytes: &[u8]) -> Result<(), Self::Error> {
        self.stdout.extend_from_slice(bytes);
        Ok(())
    }

    fn size(&self) -> Result<Size, Self::Error> {
        Ok(self.size)
    }
}

#[derive(Debug, Clone, Default)]
struct FakeRawModeOps;

impl RawModeOps for FakeRawModeOps {
    fn is_raw_mode_enabled(&self) -> io::Result<bool> {
        Ok(false)
    }

    fn enable_raw_mode(&self) -> io::Result<()> {
        Ok(())
    }

    fn disable_raw_mode(&self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn slash_menu_fixture_survives_attach_and_keeps_preview_current() {
    // NOTE: This spike keeps one happy-path regression. PTY overflow, child
    // exit during attach, idle stdin before detach, and attach-init failures
    // are still explicit follow-up coverage gaps.
    let embedded_size = Size::new(16, 60);
    let terminal_size = Size::new(40, 120);
    let fixture_program = fixture_path("slash-menu").to_string_lossy().into_owned();
    let mut process = spawn_fixture(PaneConfig::command(fixture_program).with_size(embedded_size));
    let mut surface = Vt100Backend::new(PaneId::new(21), embedded_size)
        .expect("surface backend should initialize");
    let mut terminal = ScheduledTerminal::new(terminal_size);

    let start = Instant::now() + Duration::from_millis(20);
    terminal.queue_stdin_after_stdout(start, b"/".to_vec(), ATTACH_READY_BYTES);
    terminal.queue_stdin_after_stdout(
        start + Duration::from_millis(20),
        BRACKETED_PASTE_BYTES.to_vec(),
        ATTACH_READY_BYTES,
    );
    terminal.queue_stdin(start + Duration::from_millis(200), vec![0x1d]);

    let mut control = CrosstermTerminalControl::with_raw_mode_ops(io::sink(), FakeRawModeOps)
        .with_host_alternate_screen(true);
    let mut session =
        BlockingAttachSession::new(PaneId::new(21), AttachOptions::default(), embedded_size, 0)
            .with_poll_interval(Duration::from_millis(1));

    let outcome = session
        .run(&mut terminal, &mut process, &mut surface, &mut control)
        .expect("attach session should detach cleanly");

    assert_eq!(outcome.reason, DetachReason::UserChord);
    assert!(
        terminal.saw_stdout(ATTACH_READY_BYTES),
        "expected attach stdout to include the fixture's bracketed-paste enable and prompt before input injection"
    );

    let stdout = normalize(terminal.stdout_bytes());
    assert!(
        stdout.contains("menu:"),
        "expected attached stdout to show the slash menu, got {stdout:?}"
    );
    assert!(
        stdout.contains("/status"),
        "expected attached stdout to include menu entries, got {stdout:?}"
    );
    assert!(
        stdout.contains("paste:cargo test --workspace"),
        "expected attached stdout to show the pasted payload, got {stdout:?}"
    );

    let preview = surface_text(&surface);
    assert!(
        preview.contains("menu:"),
        "expected the embedded preview to catch up with the slash menu, got {preview:?}"
    );
    assert!(
        preview.contains("paste:cargo test --workspace"),
        "expected the embedded preview to catch up with the pasted payload, got {preview:?}"
    );
    let modes = surface.snapshot().modes;
    assert!(
        modes.focus_events,
        "expected embedded preview surface to retain focus-event mode"
    );
    assert!(
        modes.bracketed_paste,
        "expected embedded preview surface to retain bracketed-paste mode"
    );

    process
        .writer()
        .write_bytes(b"__PANESMITH_EXIT__\r")
        .expect("fixture exit request should write");
    assert_eq!(wait_for_exit(&mut process, WAIT_TIMEOUT), Some(0));
}
