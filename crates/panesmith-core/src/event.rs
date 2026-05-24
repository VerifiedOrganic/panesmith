//! Event types emitted by pane lifecycles.
//!
//! Every event is pane-scoped, sequence-numbered, and timestamped so that
//! consumers can reconstruct ordering and detect gaps.

use std::time::SystemTime;

use crate::{PaneError, PaneId, PaneState, Size};

/// A pane-scoped event with sequence metadata.
///
/// Events are ordered by [`seq`](Self::seq) within a single pane. Timestamps
/// are wall-clock and may not be monotonic across system clock adjustments.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct PaneEvent {
    /// Pane that emitted the event.
    pub pane_id: PaneId,
    /// Monotonically-increasing sequence number for this pane.
    pub seq: u64,
    /// Wall-clock timestamp when the event was created.
    pub at: SystemTime,
    /// The kind of event and its payload.
    pub kind: PaneEventKind,
}

impl PaneEvent {
    /// Returns a synthetic placeholder event for testing and wiring.
    ///
    /// The sequence number is `0` and the timestamp is the Unix epoch.
    pub fn placeholder(pane_id: PaneId) -> Self {
        Self {
            pane_id,
            seq: 0,
            at: SystemTime::UNIX_EPOCH,
            kind: PaneEventKind::StateChanged(Box::new(StateChangedEvent {
                old: PaneState::Starting,
                new: PaneState::Starting,
            })),
        }
    }

    /// Compares this event's sequence number with another's.
    ///
    /// Returns `None` when the two events belong to different panes, because
    /// sequence numbers are only meaningful within a single pane.
    pub fn seq_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        if self.pane_id != other.pane_id {
            return None;
        }
        Some(self.seq.cmp(&other.seq))
    }

    /// Returns `true` if this event occurred after `other` by sequence number.
    ///
    /// Returns `false` when the two events belong to different panes.
    pub fn is_after(&self, other: &Self) -> bool {
        self.seq_cmp(other) == Some(std::cmp::Ordering::Greater)
    }

    /// Returns `true` if this event occurred before `other` by sequence number.
    ///
    /// Returns `false` when the two events belong to different panes.
    pub fn is_before(&self, other: &Self) -> bool {
        self.seq_cmp(other) == Some(std::cmp::Ordering::Less)
    }
}

/// The kind of pane event and its associated payload.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum PaneEventKind {
    /// The pane's child process was spawned.
    Spawned(SpawnedEvent),
    /// The pane's lifecycle state changed.
    StateChanged(Box<StateChangedEvent>),
    /// The pane received output from its PTY.
    Output(OutputEvent),
    /// The pane's terminal surface changed.
    SurfaceChanged(SurfaceChangedEvent),
    /// Input was sent to the pane's PTY.
    InputSent(InputSentEvent),
    /// The pane was resized.
    Resized(ResizedEvent),
    /// Fullscreen attach started.
    AttachStarted(AttachStartedEvent),
    /// Fullscreen attach ended.
    AttachEnded(AttachEndedEvent),
    /// Transcript scrollback was rotated/dropped.
    TranscriptRotated(TranscriptRotatedEvent),
    /// An internal queue overflowed and data was dropped.
    Overflow(OverflowEvent),
    /// An error occurred in the pane.
    Error(ErrorEvent),
    /// The pane's child process exited.
    Exited(ExitedEvent),
}

/// Event payload for a spawned child process.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SpawnedEvent {
    /// The program that was executed.
    pub program: String,
}

/// Event payload for a state transition.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct StateChangedEvent {
    /// The previous state.
    pub old: PaneState,
    /// The new state.
    pub new: PaneState,
}

/// Metadata about output received from the PTY.
///
/// Raw bytes are **not** included by default to avoid leaking secrets or
/// capturing huge payloads. Use the payload configuration to opt in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct OutputEvent {
    /// Number of bytes in the output frame.
    pub bytes_len: usize,
    /// Absolute offset into the transcript, if recorded.
    ///
    /// In plain-text mode this is a byte offset into the logical plain-text
    /// transcript stream. In raw-byte and both mode this is a byte offset into
    /// the raw PTY byte stream.
    pub transcript_offset: Option<u64>,
    /// Whether the output contains escape sequences.
    pub contains_escape: bool,
}

/// Policy for how much output data to capture and surface in events.
///
/// This is a configuration type, not an event payload. Event producers decide
/// how much payload data they can safely include.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum OutputCapturePolicy {
    /// Only metadata is included; no raw bytes.
    #[default]
    MetadataOnly,
    /// A preview of escaped bytes up to a limit.
    EscapedPreview {
        /// Maximum number of bytes in the preview.
        max_bytes: usize,
    },
    /// All raw bytes are included.
    RawBytes,
}

/// Event payload for terminal surface changes.
///
/// Helps renderers avoid unnecessary work and helps tests assert state
/// changes without snapshotting the full surface every time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SurfaceChangedEvent {
    /// Surface generation counter.
    pub generation: u64,
    /// Which rows are dirty, if known.
    pub dirty_rows: DirtyRows,
    /// Whether the cursor position changed.
    pub cursor_changed: bool,
    /// Whether the terminal title changed.
    #[cfg_attr(feature = "serde", serde(default))]
    pub title_changed: bool,
    /// Whether terminal modes changed.
    pub modes_changed: bool,
    /// Whether scrollback content changed.
    pub scrollback_changed: bool,
}

/// Description of dirty rows in a surface update.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum DirtyRows {
    /// The entire surface is dirty.
    All,
    /// A contiguous range of rows is dirty.
    Range {
        /// First dirty row (inclusive).
        start: u16,
        /// Last dirty row (exclusive).
        end: u16,
    },
    /// No rows are dirty (e.g. only modes changed).
    None,
}

/// Event payload for input sent to a pane.
///
/// Full input bytes are **not** included by default to avoid leaking pasted
/// text or passwords.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct InputSentEvent {
    /// The kind of input that was sent.
    pub input_kind: InputKind,
    /// Number of bytes sent.
    pub bytes_len: usize,
    /// Whether the input was recorded in the transcript.
    pub recorded: bool,
}

/// Kinds of input that can be sent to a pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum InputKind {
    /// Raw byte sequence.
    Bytes,
    /// Encoded key press.
    Key,
    /// Pasted text.
    Paste,
}

/// Event payload for a resize operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ResizedEvent {
    /// The new size after resizing.
    pub size: Size,
}

/// Event payload emitted when fullscreen attach starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct AttachStartedEvent {
    /// The real terminal size used during attach.
    pub terminal_size: Size,
    /// The embedded size that will be restored on detach.
    pub embedded_size: Size,
    /// The screen policy applied during attach.
    pub screen_policy: AttachScreenPolicy,
}

/// Policy for handling the terminal screen during attach.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum AttachScreenPolicy {
    /// Reuse the host's alternate screen buffer.
    #[default]
    ReuseHostAlternateScreen,
    /// Leave the alternate screen buffer.
    LeaveAlternateScreen,
    /// Enter a fresh alternate screen buffer.
    EnterFreshAlternateScreen,
}

/// Event payload emitted when fullscreen attach ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct AttachEndedEvent {
    /// The reason attach ended.
    pub reason: DetachReason,
    /// The size restored after detach.
    pub restored_size: Size,
    /// How long the attach session lasted.
    pub duration: std::time::Duration,
}

/// Reasons that fullscreen attach can end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum DetachReason {
    /// The user pressed the detach chord.
    UserChord,
    /// The child process exited.
    ChildExited,
    /// The host application requested detach.
    HostRequested,
    /// An error forced detach.
    Error,
}

/// Event payload for transcript rotation/drop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct TranscriptRotatedEvent {
    /// Number of chunks dropped from the transcript.
    pub chunks_dropped: u64,
    /// Total bytes dropped (raw + plain).
    pub bytes_dropped: u64,
    /// Raw bytes dropped from the raw transcript buffer.
    #[cfg_attr(feature = "serde", serde(default))]
    pub raw_bytes_dropped: u64,
    /// Plain bytes dropped from the plain transcript buffer.
    #[cfg_attr(feature = "serde", serde(default))]
    pub plain_bytes_dropped: u64,
}

/// Event payload for queue overflow.
///
/// PTY output can overwhelm queues. Loss must be explicit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct OverflowEvent {
    /// Number of dropped frames.
    pub dropped_frames: u64,
    /// Number of dropped bytes.
    pub dropped_bytes: u64,
    /// Which queue overflowed.
    pub queue: OverflowQueue,
}

/// Queues that can overflow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum OverflowQueue {
    /// PTY output frames.
    PtyOutputFrames,
    /// Pane event queue.
    PaneEvents,
    /// Transcript chunks.
    TranscriptChunks,
    /// Render cache updates.
    RenderCacheUpdates,
}

/// Event payload for a pane error.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ErrorEvent {
    /// The error that occurred.
    pub error: PaneError,
}

/// Event payload for child process exit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ExitedEvent {
    /// The process exit code, if available.
    pub code: Option<i32>,
}
