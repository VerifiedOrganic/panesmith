#![doc = include_str!("../README.md")]

pub mod command;
pub(crate) mod default_surface;
#[doc(hidden)]
pub mod detach_encoding;
pub mod encoder;
pub mod error;
pub mod event;
pub mod input;
pub mod input_transaction;
pub mod manager;
#[doc(hidden)]
pub mod mode_overlay;
pub mod pane;
pub mod pty;
pub mod repro;
pub mod transcript;
#[doc(hidden)]
pub mod vt100_surface;

pub use command::{
    AttachOptions, AttachOutputPolicy, AttachResizePolicy, DetachConfig, PaneCommand, RestorePolicy,
};
pub use encoder::{InputEncoder, XtermEncoder};
pub use error::IoOperation;
pub use error::PaneError as Error;
pub use error::{PaneError, Result};
pub use event::{
    AttachEndedEvent, AttachScreenPolicy, AttachStartedEvent, DetachReason, DirtyRows, ErrorEvent,
    ExitedEvent, InputKind, InputSentEvent, OutputCapturePolicy, OutputEvent, OverflowEvent,
    OverflowQueue, PaneEvent, PaneEventKind, ResizedEvent, SpawnedEvent, StateChangedEvent,
    SurfaceChangedEvent, TranscriptRotatedEvent,
};
pub use input::{
    HostInput, KeyCode, KeyEventKind, KeyInput, KeyModifiers, MediaKeyCode, ModifierKeyCode,
    MouseButton, MouseEventKind, MouseInput, UnsupportedEventError,
};
pub use input_transaction::{
    input_echo_hash, InputIntent, InputOutcome, InputRetryConfig, InputTransaction,
    InputTransactionError, InputVerification,
};
pub use manager::{
    PaneAttachError, PaneAttachInputChunk, PaneAttachOutcome, PaneAttachTerminal,
    PaneAttachTerminalControl, PaneExit, PaneHandle, PaneManager, PaneManagerConfig,
    ScrollbackReader, TranscriptReader,
};
pub use pane::{
    AttachConfig, BackspaceEncoding, CellAttrs, CellStyle, CellWidth, ChildEnvironmentPolicy,
    ColorSpec, CommandSpec, CursorPosition, CursorState, EnterEncoding, InputConfig, KillConfig,
    KillReason, MouseMode, OwnedPaneSnapshot, OwnedScrollbackLine, OwnedScrollbackSnapshot,
    OwnedSurfaceCell, OwnedSurfaceRow, OwnedSurfaceSnapshot, PaneConfig, PaneId,
    PaneInteractionMode, PaneSnapshot, PaneState, PaneStats, PasteNewlinePolicy, ReplayBackendKind,
    ScrollbackConfig, ScrollbackLine, ScrollbackSnapshot, Size, SurfaceBackend,
    SurfaceBackendMetadata, SurfaceCell, SurfaceConfig, SurfaceRow, SurfaceSnapshot, SurfaceUpdate,
    TerminalModes, TerminalViewport, TerminalViewportMetrics, TranscriptConfig, TranscriptMode,
};
pub use pty::{
    OverflowStats, PortablePtyBackend, PortablePtyProcess, PtyBackend, PtyFrame, PtyHandle,
    PtyProcess, PtyWriter,
};
pub use repro::{PaneReplay, ReproDump, ReproDumpOptions, ReproRawTranscript, ReproSizeEvent};
pub use transcript::{strip_ansi, Transcript, TranscriptRecord, TranscriptRotation};

#[cfg(test)]
mod tests;
