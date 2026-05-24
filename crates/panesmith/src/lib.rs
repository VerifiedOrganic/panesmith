//! Public entry point for the Panesmith workspace.
//!
//! Embedded mode is for preview and routine input.
//! Attach mode is the correctness path for complex interactive TUIs.
//!
//! ## Embedded shell in under 50 lines
//!
//! ```no_run
//! use std::io;
//!
//! use panesmith::{PaneConfig, PaneManager, PaneManagerConfig, Size, TerminalPaneWidget};
//! use ratatui::{backend::CrosstermBackend, Terminal};
//!
//! fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let mut manager = PaneManager::new(PaneManagerConfig::default());
//!     let pane_id = manager.spawn(
//!         PaneConfig::shell()
//!             .with_title("Shell")
//!             .with_size(Size::new(24, 80)),
//!     )?;
//!
//!     let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
//!     let snapshot = manager.snapshot(pane_id)?;
//!     terminal.draw(|frame| {
//!         frame.render_widget(TerminalPaneWidget::new(&snapshot), frame.area());
//!     })?;
//!     Ok(())
//! }
//! ```
//!
//! This is the smallest public host: spawn one pane, snapshot it, render one
//! frame. The repository also includes larger interactive demos for the live
//! embedded and attach flows.
//!
//! ## Public example map
//!
//! - `examples/embedded_shell_minimal.rs` keeps the smallest possible
//!   shell-pane host copy-pasteable.
//! - `examples/manager_attach.rs` hands an existing manager-owned pane to
//!   fullscreen attach.
//! - `examples/dashboard_two_panes.rs` shows side-by-side pane rendering for
//!   dashboard layouts.
//! - The repository's larger interactive demos cover the live embedded and
//!   fullscreen attach flows end to end.
//! - `examples/transcript_capture.rs` records plain-text and raw-byte
//!   transcript slices.
//! - `examples/event_consumption.rs` drains ordered pane events and matches on
//!   `PaneEventKind`.
//!
//! ## Transcript capture
//!
//! Configure transcript retention on `PaneConfig` with
//! `TranscriptMode::PlainText`, `TranscriptMode::RawBytes`, or
//! `TranscriptMode::Both`, then read the retained buffers back with
//! `PaneManager::transcript`. The `plain_transcript` and `raw_transcript`
//! helpers remain available as convenience accessors for the full retained
//! buffers.
//!
//! ## Event consumption
//!
//! Hosts consume pane lifecycle and I/O activity by polling with
//! `PaneManager::drain_events`. They can also mirror future events through
//! `PaneManager::subscribe` or `PaneManager::set_event_sink` when they want
//! push delivery alongside polling. Events are pane-scoped, sequence-numbered,
//! and timestamped so dashboards can preserve ordering and detect gaps. When
//! hosts use `PaneManager::attach_blocking`, attach lifecycle, input, output,
//! resize, surface, and exit events stay in that same pane-scoped stream.
//!
//! `PaneManager::kill` terminates a child but keeps the pane's manager-owned
//! state available for post-kill inspection. Use `PaneManager::remove` to drop
//! PTY/process and surface resources, or `PaneManager::kill_and_remove` for
//! reset/restart paths that want both steps.
//!
//! ## Limitations
//!
//! - Embedded panes are intended for preview and routine input, not perfect
//!   fullscreen fidelity for every terminal UI.
//! - Attach mode is the escape hatch for editors, slash menus, alternate-screen
//!   apps, and other cases where native terminal ownership matters.
//! - Host terminal state is caller-owned during attach. Embedded TUIs should
//!   restore their exact dashboard profile through `PaneAttachTerminalControl`.
//! - The first release exposes pane runtime building blocks rather than a full
//!   multiplexer, scrollback browser, or selection model.
//! - The live fullscreen attach bridge currently targets Unix terminals.

#[cfg(all(feature = "crossterm", unix))]
pub use panesmith_attach::StdioAttachTerminal;
pub use panesmith_attach::{
    AttachBridge, AttachOptions, DetachMatcher, FeedBytesResult, MatchResult,
};
#[cfg(feature = "crossterm")]
pub use panesmith_attach::{CrosstermTerminalControl, RawModeOps, SystemRawModeOps};
pub use panesmith_core::{
    input_echo_hash, AttachConfig, AttachEndedEvent, AttachOptions as CoreAttachOptions,
    AttachScreenPolicy, AttachStartedEvent, CellAttrs, CellStyle, CellWidth, ColorSpec,
    CommandSpec, CursorPosition, CursorState, DetachReason, DirtyRows, Error, ErrorEvent,
    ExitedEvent, HostInput, InputConfig, InputIntent, InputKind, InputOutcome, InputRetryConfig,
    InputSentEvent, InputTransaction, InputTransactionError, InputVerification, IoOperation,
    KeyCode, KeyEventKind, KeyInput, KeyModifiers, KillConfig, KillReason, MediaKeyCode,
    ModifierKeyCode, MouseButton, MouseEventKind, MouseInput, MouseMode, OutputCapturePolicy,
    OutputEvent, OverflowEvent, OverflowQueue, OwnedPaneSnapshot, OwnedScrollbackLine,
    OwnedScrollbackSnapshot, OwnedSurfaceCell, OwnedSurfaceRow, OwnedSurfaceSnapshot,
    PaneAttachError, PaneAttachInputChunk, PaneAttachOutcome, PaneAttachTerminal,
    PaneAttachTerminalControl, PaneCommand, PaneConfig, PaneError, PaneEvent, PaneEventKind,
    PaneExit, PaneHandle, PaneId, PaneInteractionMode, PaneManager, PaneManagerConfig, PaneReplay,
    PaneSnapshot, PaneState, PaneStats, PortablePtyBackend, PortablePtyProcess, PtyBackend,
    PtyFrame, PtyHandle, PtyProcess, PtyWriter, ReplayBackendKind, ReproDump, ReproDumpOptions,
    ReproRawTranscript, ReproSizeEvent, ResizedEvent, Result, ScrollbackConfig, ScrollbackLine,
    ScrollbackReader, ScrollbackSnapshot, Size, SpawnedEvent, StateChangedEvent, SurfaceBackend,
    SurfaceBackendMetadata, SurfaceCell, SurfaceChangedEvent, SurfaceConfig, SurfaceRow,
    SurfaceSnapshot, SurfaceUpdate, TerminalModes, TranscriptConfig, TranscriptMode,
    TranscriptReader, TranscriptRotatedEvent, UnsupportedEventError,
};
pub use panesmith_ratatui::{
    CursorRenderMode, TerminalPaneWidget, TerminalViewport, TerminalViewportMetrics,
};
pub use panesmith_vt100::Vt100Backend;

#[cfg(feature = "crossterm")]
mod crossterm_compile_smoke {
    use crate::{CrosstermTerminalControl, SystemRawModeOps};

    const _: fn(Vec<u8>) -> CrosstermTerminalControl<Vec<u8>, SystemRawModeOps> =
        CrosstermTerminalControl::<Vec<u8>>::new;

    #[cfg(unix)]
    use crate::StdioAttachTerminal;

    #[cfg(unix)]
    const _: fn(Vec<u8>) -> std::io::Result<StdioAttachTerminal<Vec<u8>>> =
        StdioAttachTerminal::<Vec<u8>>::new;
}

#[cfg(test)]
mod tests;
