//! Pane identity, size, state, and configuration types.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use crate::{DirtyRows, PaneError, Result};

/// Stable pane identifier type shared across the workspace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Serialize, serde::Deserialize),
    serde(transparent)
)]
pub struct PaneId(u64);

impl PaneId {
    /// Creates a pane identifier from a raw numeric value.
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Returns the raw numeric value backing this identifier.
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// Terminal surface dimensions in rows and columns.
///
/// Both dimensions must be at least 1. Use [`Size::try_new`] for a checked
/// constructor, or [`Size::new`] when the values are known to be valid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct Size {
    /// Number of terminal rows.
    pub rows: u16,
    /// Number of terminal columns.
    pub cols: u16,
}

#[cfg(feature = "serde")]
impl<'de> serde::Deserialize<'de> for Size {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(serde::Deserialize)]
        struct Raw {
            rows: u16,
            cols: u16,
        }

        let raw = Raw::deserialize(deserializer)?;
        Size::try_new(raw.rows, raw.cols).map_err(serde::de::Error::custom)
    }
}

impl Size {
    /// Creates a size without runtime validation.
    ///
    /// Prefer [`Size::try_new`] when the dimensions are computed or sourced
    /// from external input.
    pub const fn new(rows: u16, cols: u16) -> Self {
        Self { rows, cols }
    }

    /// Creates a size, rejecting zero rows or columns.
    ///
    /// # Errors
    ///
    /// Returns [`PaneError::InvalidSize`] when either dimension is zero.
    pub fn try_new(rows: u16, cols: u16) -> Result<Self> {
        if rows == 0 || cols == 0 {
            Err(PaneError::InvalidSize { rows, cols })
        } else {
            Ok(Self { rows, cols })
        }
    }
}

/// Lifecycle state of a pane's child process.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum PaneState {
    /// The child process is being started.
    Starting,
    /// The child process is running.
    Running,
    /// The child process exited with an optional status code.
    Exited {
        /// The process exit code, if available.
        code: Option<i32>,
    },
    /// The child process failed to start or crashed.
    Failed {
        /// The error that caused the failure.
        error: PaneError,
    },
    /// The child process was killed.
    Killed {
        /// The reason the pane was killed.
        reason: KillReason,
    },
}

/// Reasons a pane can be killed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum KillReason {
    /// The user explicitly requested termination.
    UserRequested,
    /// The host application requested termination.
    HostRequested,
    /// A configured limit was reached.
    ConfigLimit,
}

/// Interaction mode for a pane, independent of process state.
///
/// A pane can exit while attached; the attach bridge handles this by
/// showing the exit state and allowing detach.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum PaneInteractionMode {
    /// The pane is rendered in an embedded widget.
    Embedded,
    /// The pane is transitioning to fullscreen attach.
    Attaching,
    /// The pane has fullscreen control of the real terminal.
    Attached,
    /// The pane is transitioning back to embedded rendering.
    Detaching,
}

/// Describes the program and arguments to spawn inside a pane.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct CommandSpec {
    /// The executable or shell command to run.
    pub program: String,
    /// Arguments passed to the program.
    pub args: Vec<String>,
}

impl CommandSpec {
    /// Creates a command spec with no arguments.
    pub fn new(program: impl Into<String>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
        }
    }

    /// Creates a command spec with an argument list.
    pub fn with_args<I, S>(program: impl Into<String>, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            program: program.into(),
            args: args.into_iter().map(Into::into).collect(),
        }
    }

    /// Validates that the program is non-empty.
    pub fn validate(&self) -> Result<()> {
        if self.program.trim().is_empty() {
            Err(PaneError::Spawn {
                message: "command program must not be empty".into(),
            })
        } else {
            Ok(())
        }
    }
}

/// Configuration for terminal scrollback retention.
///
/// The default is unbounded retention, preserving the historical behavior for
/// callers that have not opted into a policy. Use
/// [`ScrollbackConfig::bounded_lines`] to cap retained history or
/// [`ScrollbackConfig::disabled`] to retain no scrollback history.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ScrollbackConfig {
    /// Maximum number of history lines to retain.
    ///
    /// `None` means unbounded retention. `Some(0)` disables scrollback.
    /// Positive values retain at most that many history lines. The visible
    /// terminal screen is never counted against this limit.
    pub max_lines: Option<usize>,
}

impl ScrollbackConfig {
    /// Creates an unbounded scrollback policy.
    pub const fn unlimited() -> Self {
        Self { max_lines: None }
    }

    /// Creates a bounded scrollback policy.
    ///
    /// # Errors
    ///
    /// Returns [`PaneError::Spawn`] when `max_lines` is zero.
    pub fn bounded_lines(max_lines: usize) -> Result<Self> {
        if max_lines == 0 {
            Err(PaneError::Spawn {
                message: "scrollback max_lines must be > 0".into(),
            })
        } else {
            Ok(Self {
                max_lines: Some(max_lines),
            })
        }
    }

    /// Creates a bounded scrollback policy.
    ///
    /// This constructor is retained for source compatibility with earlier
    /// Panesmith prereleases that accepted a byte limit. The backing terminal
    /// history is bounded by `max_lines`; `max_bytes` must still be non-zero
    /// to catch stale mixed-zero configurations.
    ///
    /// # Errors
    ///
    /// Returns [`PaneError::Spawn`] when either argument is zero.
    pub fn new(max_lines: usize, max_bytes: usize) -> Result<Self> {
        if max_bytes == 0 {
            return Err(PaneError::Spawn {
                message: "scrollback max_bytes must be > 0".into(),
            });
        }
        Self::bounded_lines(max_lines)
    }

    /// Disables scrollback entirely.
    pub const fn disabled() -> Self {
        Self { max_lines: Some(0) }
    }

    /// Returns `true` when the policy retains no history.
    pub const fn is_disabled(&self) -> bool {
        matches!(self.max_lines, Some(0))
    }

    /// Returns `true` when history retention is unbounded.
    pub const fn is_unlimited(&self) -> bool {
        self.max_lines.is_none()
    }

    /// Returns `true` when a positive line bound is configured.
    pub const fn is_bounded(&self) -> bool {
        matches!(self.max_lines, Some(lines) if lines > 0)
    }

    /// Returns `true` when some history can be retained.
    pub const fn is_enabled(&self) -> bool {
        !self.is_disabled()
    }

    /// Returns the configured line limit.
    ///
    /// `None` means unbounded. `Some(0)` means disabled.
    pub const fn line_limit(&self) -> Option<usize> {
        self.max_lines
    }
}

impl Default for ScrollbackConfig {
    fn default() -> Self {
        Self::unlimited()
    }
}

/// Modes for transcript recording.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum TranscriptMode {
    /// No transcript is recorded.
    #[default]
    Disabled,
    /// Plain text transcript (ANSI stripped).
    PlainText,
    /// Raw PTY bytes.
    RawBytes,
    /// Both plain text and raw bytes.
    Both,
}

/// Configuration for transcript recording behavior.
///
/// Defaults to [`TranscriptMode::Disabled`] with 10,000 line and 1 MiB byte
/// retention limits. A limit of `0` disables that bound.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct TranscriptConfig {
    /// The selected transcript mode.
    pub mode: TranscriptMode,
    /// Maximum number of lines to retain in the transcript.
    ///
    /// In Both mode, each chunk contributes `max(raw_newlines, plain_newlines)`
    /// to avoid double-counting one logical line. A value of `0` disables the
    /// line limit.
    #[cfg_attr(feature = "serde", serde(default = "default_transcript_max_lines"))]
    pub max_lines: usize,
    /// Maximum total bytes to retain in the transcript.
    #[cfg_attr(feature = "serde", serde(default = "default_transcript_max_bytes"))]
    pub max_bytes: usize,
}

impl TranscriptConfig {
    /// Creates a transcript config with the given mode and default limits.
    pub fn new(mode: TranscriptMode) -> Self {
        Self {
            mode,
            ..Self::default()
        }
    }

    /// Sets the transcript line limit.
    pub fn with_max_lines(mut self, max_lines: usize) -> Self {
        self.max_lines = max_lines;
        self
    }

    /// Sets the transcript byte limit.
    pub fn with_max_bytes(mut self, max_bytes: usize) -> Self {
        self.max_bytes = max_bytes;
        self
    }
}

impl Default for TranscriptConfig {
    fn default() -> Self {
        Self {
            mode: TranscriptMode::Disabled,
            max_lines: 10_000,
            max_bytes: 1024 * 1024,
        }
    }
}

#[cfg(feature = "serde")]
fn default_transcript_max_lines() -> usize {
    10_000
}

#[cfg(feature = "serde")]
fn default_transcript_max_bytes() -> usize {
    1024 * 1024
}

/// Configuration for surface rendering behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SurfaceConfig;

/// Configuration for input handling behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct InputConfig {
    /// How the Enter key is encoded.
    pub enter: EnterEncoding,
    /// How the Backspace key is encoded.
    pub backspace: BackspaceEncoding,
    /// Whether Alt+key sends ESC prefix.
    pub alt_sends_escape: bool,
    /// Whether the host sends application cursor sequences.
    pub application_cursor_keys: bool,
    /// How newlines in pasted text are handled.
    pub paste_newline: PasteNewlinePolicy,
}

/// How the Enter/Return key is encoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub enum EnterEncoding {
    /// Carriage return (`\r`).
    #[default]
    Cr,
    /// Line feed (`\n`).
    Lf,
    /// Carriage return + line feed (`\r\n`).
    CrLf,
}

/// How the Backspace key is encoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub enum BackspaceEncoding {
    /// DEL (`0x7f`).
    #[default]
    Del,
    /// Backspace (`0x08`).
    Bs,
}

/// How newlines in pasted text are handled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub enum PasteNewlinePolicy {
    /// Preserve newlines exactly.
    #[default]
    Preserve,
    /// Normalize to LF (`\n`).
    NormalizeToLf,
    /// Normalize to CR (`\r`).
    NormalizeToCr,
}

impl Default for InputConfig {
    fn default() -> Self {
        Self {
            enter: EnterEncoding::default(),
            backspace: BackspaceEncoding::default(),
            alt_sends_escape: true,
            application_cursor_keys: false,
            paste_newline: PasteNewlinePolicy::default(),
        }
    }
}

/// Terminal modes reported by the surface backend.
///
/// The encoder uses these to decide whether to emit bracketed paste
/// wrappers, focus event sequences, application cursor sequences, etc.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct TerminalModes {
    /// Whether the child has requested bracketed paste.
    pub bracketed_paste: bool,
    /// Whether the child has requested focus events.
    pub focus_events: bool,
    /// Whether the child is in application cursor mode.
    pub application_cursor: bool,
    /// Whether the child is in the alternate screen buffer.
    pub alternate_screen: bool,
    /// The active mouse protocol, if any.
    pub mouse: MouseMode,
}

/// Mouse protocols that a terminal surface can enable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub enum MouseMode {
    /// No mouse reporting.
    #[default]
    None,
    /// X10 compatibility mode.
    X10,
    /// Normal tracking mode.
    Normal,
    /// Button-event tracking mode.
    ButtonEvent,
    /// Any-event tracking mode.
    AnyEvent,
    /// SGR-encoded extended coordinates.
    Sgr,
}

/// Configuration for attach/detach behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct AttachConfig;

/// Read-only snapshot of a pane's terminal surface state.
///
/// Snapshot rows and cell text may borrow from a backend, but the collection
/// containers are owned so host renderers can keep the shape simple.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SurfaceSnapshot<'a> {
    /// Visible terminal size represented by this snapshot.
    pub size: Size,
    /// Visible surface rows, top to bottom.
    pub rows: Vec<SurfaceRow<'a>>,
    /// Cursor metadata captured alongside the visible rows.
    pub cursor: CursorState,
    /// Terminal modes active when the snapshot was taken.
    pub modes: TerminalModes,
    /// Optional terminal-managed title, if the backend can expose it.
    pub title: Option<Cow<'a, str>>,
}

/// Owned surface snapshot that can outlive a backend borrow.
pub type OwnedSurfaceSnapshot = SurfaceSnapshot<'static>;

impl<'a> SurfaceSnapshot<'a> {
    /// Creates a snapshot from explicit surface parts.
    pub fn new(
        size: Size,
        rows: Vec<SurfaceRow<'a>>,
        cursor: CursorState,
        modes: TerminalModes,
        title: Option<Cow<'a, str>>,
    ) -> Self {
        Self {
            size,
            rows,
            cursor,
            modes,
            title,
        }
    }

    /// Creates a blank snapshot with the requested size.
    pub fn blank(size: Size) -> Self {
        Self {
            size,
            rows: vec![SurfaceRow::default(); usize::from(size.rows)],
            cursor: CursorState::default(),
            modes: TerminalModes::default(),
            title: None,
        }
    }

    /// Clones this snapshot into an owned form.
    pub fn to_owned_snapshot(&self) -> OwnedSurfaceSnapshot {
        OwnedSurfaceSnapshot {
            size: self.size,
            rows: self.rows.iter().map(SurfaceRow::to_owned_row).collect(),
            cursor: self.cursor,
            modes: self.modes,
            title: self
                .title
                .as_ref()
                .map(|title| Cow::Owned(title.clone().into_owned())),
        }
    }

    /// Converts this snapshot into an owned form.
    pub fn into_owned(self) -> OwnedSurfaceSnapshot {
        OwnedSurfaceSnapshot {
            size: self.size,
            rows: self.rows.into_iter().map(SurfaceRow::into_owned).collect(),
            cursor: self.cursor,
            modes: self.modes,
            title: self.title.map(|title| Cow::Owned(title.into_owned())),
        }
    }
}

impl Default for SurfaceSnapshot<'_> {
    fn default() -> Self {
        Self::blank(Size::new(1, 1))
    }
}

/// A visible surface row.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SurfaceRow<'a> {
    /// Cells that make up this row.
    pub cells: Vec<SurfaceCell<'a>>,
    /// Whether this row ends with a wrap into the next row.
    pub wrapped: bool,
}

/// Owned row data that can outlive backend borrows.
pub type OwnedSurfaceRow = SurfaceRow<'static>;

impl<'a> SurfaceRow<'a> {
    /// Creates a row from a list of cells.
    pub fn new(cells: Vec<SurfaceCell<'a>>) -> Self {
        Self {
            cells,
            wrapped: false,
        }
    }

    /// Marks whether this row wrapped into the next row.
    pub fn with_wrapped(mut self, wrapped: bool) -> Self {
        self.wrapped = wrapped;
        self
    }

    /// Clones this row into an owned form.
    pub fn to_owned_row(&self) -> OwnedSurfaceRow {
        OwnedSurfaceRow {
            cells: self.cells.iter().map(SurfaceCell::to_owned_cell).collect(),
            wrapped: self.wrapped,
        }
    }

    /// Converts this row into an owned form.
    pub fn into_owned(self) -> OwnedSurfaceRow {
        OwnedSurfaceRow {
            cells: self
                .cells
                .into_iter()
                .map(SurfaceCell::into_owned)
                .collect(),
            wrapped: self.wrapped,
        }
    }
}

/// A visible terminal cell or grapheme cluster.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SurfaceCell<'a> {
    /// Display text stored in this cell.
    pub text: Cow<'a, str>,
    /// How many columns this cell occupies.
    pub width: CellWidth,
    /// Style metadata for the cell.
    pub style: CellStyle,
}

/// Owned cell data that can outlive backend borrows.
pub type OwnedSurfaceCell = SurfaceCell<'static>;

impl<'a> SurfaceCell<'a> {
    /// Creates a surface cell from text, width, and style metadata.
    pub fn new(text: impl Into<Cow<'a, str>>, width: CellWidth, style: CellStyle) -> Self {
        Self {
            text: text.into(),
            width,
            style,
        }
    }

    /// Clones this cell into an owned form.
    pub fn to_owned_cell(&self) -> OwnedSurfaceCell {
        OwnedSurfaceCell {
            text: Cow::Owned(self.text.clone().into_owned()),
            width: self.width,
            style: self.style,
        }
    }

    /// Converts this cell into an owned form.
    pub fn into_owned(self) -> OwnedSurfaceCell {
        OwnedSurfaceCell {
            text: Cow::Owned(self.text.into_owned()),
            width: self.width,
            style: self.style,
        }
    }
}

/// Terminal cell width metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum CellWidth {
    /// Standard single-column cell.
    #[default]
    Single,
    /// Leading cell for a double-width grapheme.
    Double,
    /// Continuation cell consumed by a preceding double-width grapheme.
    Continuation,
}

/// Terminal surface style information.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct CellStyle {
    /// Foreground color override, if known.
    pub fg: Option<ColorSpec>,
    /// Background color override, if known.
    pub bg: Option<ColorSpec>,
    /// Underline color override, if known.
    pub underline_color: Option<ColorSpec>,
    /// Boolean text attributes.
    pub attrs: CellAttrs,
}

/// Surface color representation shared by backends and renderers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum ColorSpec {
    /// Terminal default color.
    #[default]
    Default,
    /// ANSI indexed color.
    Indexed(u8),
    /// Explicit RGB color.
    Rgb(u8, u8, u8),
}

/// Boolean terminal style attributes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct CellAttrs {
    /// Bold/intense text.
    pub bold: bool,
    /// Dim text.
    pub dim: bool,
    /// Italic text.
    pub italic: bool,
    /// Underlined text.
    pub underlined: bool,
    /// Slow blink attribute.
    pub slow_blink: bool,
    /// Rapid blink attribute.
    pub rapid_blink: bool,
    /// Reversed foreground/background.
    pub reversed: bool,
    /// Hidden text.
    pub hidden: bool,
    /// Crossed-out text.
    pub crossed_out: bool,
}

/// Cursor coordinates in zero-based surface space.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct CursorPosition {
    /// Row index within the visible surface.
    pub row: u16,
    /// Column index within the visible surface.
    pub col: u16,
}

impl CursorPosition {
    /// Creates a cursor position in zero-based surface coordinates.
    pub const fn new(row: u16, col: u16) -> Self {
        Self { row, col }
    }
}

/// Cursor position and visibility state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct CursorState {
    /// Cursor position, if the backend can report it.
    pub position: Option<CursorPosition>,
    /// Whether the child cursor is visible.
    pub visible: bool,
}

impl CursorState {
    /// Creates a cursor state from a position and visibility flag.
    pub const fn new(position: Option<CursorPosition>, visible: bool) -> Self {
        Self { position, visible }
    }

    /// Creates a hidden cursor state with no known position.
    pub const fn hidden() -> Self {
        Self {
            position: None,
            visible: false,
        }
    }
}

impl Default for CursorState {
    fn default() -> Self {
        Self::hidden()
    }
}

/// Snapshot of scrollback content separate from the visible surface.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ScrollbackSnapshot<'a> {
    /// Scrollback lines in presentation order, oldest to newest.
    pub lines: Vec<ScrollbackLine<'a>>,
}

/// Owned scrollback snapshot that can outlive a backend borrow.
pub type OwnedScrollbackSnapshot = ScrollbackSnapshot<'static>;

impl<'a> ScrollbackSnapshot<'a> {
    /// Creates a scrollback snapshot from explicit lines.
    pub fn new(lines: Vec<ScrollbackLine<'a>>) -> Self {
        Self { lines }
    }

    /// Clones this scrollback snapshot into an owned form.
    pub fn to_owned_snapshot(&self) -> OwnedScrollbackSnapshot {
        OwnedScrollbackSnapshot {
            lines: self
                .lines
                .iter()
                .map(ScrollbackLine::to_owned_line)
                .collect(),
        }
    }

    /// Converts this scrollback snapshot into an owned form.
    pub fn into_owned(self) -> OwnedScrollbackSnapshot {
        OwnedScrollbackSnapshot {
            lines: self
                .lines
                .into_iter()
                .map(ScrollbackLine::into_owned)
                .collect(),
        }
    }
}

/// One line of scrollback content.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ScrollbackLine<'a> {
    /// Plain text for the scrollback line.
    ///
    /// This is preserved as a compatibility accessor and is also the fallback
    /// rendering source for plain scrollback lines that do not provide styled
    /// cells.
    pub text: Cow<'a, str>,
    /// Styled cells for this historical row, when the backend can expose them.
    #[cfg_attr(feature = "serde", serde(default))]
    pub row: SurfaceRow<'a>,
}

/// Owned scrollback line that can outlive a backend borrow.
pub type OwnedScrollbackLine = ScrollbackLine<'static>;

impl<'a> ScrollbackLine<'a> {
    /// Creates a scrollback line from text.
    pub fn new(text: impl Into<Cow<'a, str>>) -> Self {
        Self {
            text: text.into(),
            row: SurfaceRow::default(),
        }
    }

    /// Creates a scrollback line from plain text plus a styled row.
    pub fn from_row(text: impl Into<Cow<'a, str>>, row: SurfaceRow<'a>) -> Self {
        Self {
            text: text.into(),
            row,
        }
    }

    /// Clones this line into an owned form.
    pub fn to_owned_line(&self) -> OwnedScrollbackLine {
        OwnedScrollbackLine {
            text: Cow::Owned(self.text.clone().into_owned()),
            row: self.row.to_owned_row(),
        }
    }

    /// Converts this line into an owned form.
    pub fn into_owned(self) -> OwnedScrollbackLine {
        OwnedScrollbackLine {
            text: Cow::Owned(self.text.into_owned()),
            row: self.row.into_owned(),
        }
    }
}

/// Viewport state for rendering current screen rows or historical scrollback.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalViewport {
    /// Number of logical rows to scroll upward from the tail/current screen.
    pub scroll_offset: usize,
    /// Whether the viewport should follow the live tail/current screen.
    pub follow_tail: bool,
}

impl Default for TerminalViewport {
    fn default() -> Self {
        Self {
            scroll_offset: 0,
            follow_tail: true,
        }
    }
}

/// Computed viewport bounds for a pane snapshot plus optional scrollback.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TerminalViewportMetrics {
    /// Total logical rows: retained scrollback rows followed by live surface rows.
    pub total_rows: usize,
    /// Number of rows available in the render viewport.
    pub visible_rows: usize,
    /// Highest valid upward scroll offset for the current content and height.
    pub max_scroll_offset: usize,
    /// Scroll offset after applying follow-tail and clamping rules.
    pub effective_scroll_offset: usize,
    /// First logical row rendered for this viewport.
    pub start_row: usize,
    /// Exclusive end logical row rendered for this viewport.
    pub end_row: usize,
}

impl TerminalViewportMetrics {
    /// Returns whether the computed viewport is displaying the live tail.
    pub const fn is_at_tail(self) -> bool {
        self.effective_scroll_offset == 0
    }
}

impl TerminalViewport {
    /// Creates a viewport detached from follow-tail at the provided offset.
    pub const fn scrolled(scroll_offset: usize) -> Self {
        Self {
            scroll_offset,
            follow_tail: false,
        }
    }

    /// Returns this viewport to live-tail following.
    pub const fn follow_tail(self) -> Self {
        Self {
            scroll_offset: 0,
            follow_tail: true,
        }
    }

    /// Computes viewport metrics from a snapshot, optional scrollback, and height.
    pub fn metrics(
        self,
        snapshot: &PaneSnapshot<'_>,
        scrollback: Option<&ScrollbackSnapshot<'_>>,
        visible_rows: usize,
    ) -> TerminalViewportMetrics {
        self.metrics_from_counts(
            usize::from(snapshot.surface.size.rows),
            scrollback.map_or(0, |scrollback| scrollback.lines.len()),
            visible_rows,
        )
    }

    /// Computes viewport metrics from row counts.
    pub fn metrics_from_counts(
        self,
        surface_rows: usize,
        scrollback_rows: usize,
        visible_rows: usize,
    ) -> TerminalViewportMetrics {
        let total_rows = scrollback_rows.saturating_add(surface_rows);
        let max_scroll_offset = total_rows.saturating_sub(visible_rows);
        let effective_scroll_offset = if self.follow_tail {
            0
        } else {
            self.scroll_offset.min(max_scroll_offset)
        };
        let end_row = total_rows.saturating_sub(effective_scroll_offset);
        let start_row = end_row.saturating_sub(visible_rows);

        TerminalViewportMetrics {
            total_rows,
            visible_rows,
            max_scroll_offset,
            effective_scroll_offset,
            start_row,
            end_row,
        }
    }

    /// Clamps this viewport to the provided metrics.
    pub fn clamp(self, metrics: TerminalViewportMetrics) -> Self {
        if self.follow_tail || metrics.effective_scroll_offset == 0 {
            self.follow_tail()
        } else {
            Self::scrolled(metrics.effective_scroll_offset)
        }
    }

    /// Returns whether this viewport is at the live tail for the provided metrics.
    pub const fn is_at_tail(self, metrics: TerminalViewportMetrics) -> bool {
        let _ = self;
        metrics.is_at_tail()
    }

    /// Scrolls upward by `rows` logical rows, clamping to retained content.
    pub fn scroll_up(self, rows: usize, metrics: TerminalViewportMetrics) -> Self {
        let scroll_offset = metrics
            .effective_scroll_offset
            .saturating_add(rows)
            .min(metrics.max_scroll_offset);
        if scroll_offset == 0 {
            self.follow_tail()
        } else {
            Self::scrolled(scroll_offset)
        }
    }

    /// Scrolls downward by `rows` logical rows, returning to follow-tail at the bottom.
    pub fn scroll_down(self, rows: usize, metrics: TerminalViewportMetrics) -> Self {
        let scroll_offset = metrics.effective_scroll_offset.saturating_sub(rows);
        if scroll_offset == 0 {
            self.follow_tail()
        } else {
            Self::scrolled(scroll_offset)
        }
    }

    /// Scrolls upward by one viewport page.
    pub fn page_up(self, metrics: TerminalViewportMetrics) -> Self {
        self.scroll_up(metrics.visible_rows, metrics)
    }

    /// Scrolls downward by one viewport page.
    pub fn page_down(self, metrics: TerminalViewportMetrics) -> Self {
        self.scroll_down(metrics.visible_rows, metrics)
    }
}

/// Metadata returned when a surface backend consumes output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SurfaceUpdate {
    /// Which rows became dirty.
    pub dirty_rows: DirtyRows,
    /// Whether the cursor position changed.
    pub cursor_changed: bool,
    /// Whether the terminal title changed.
    pub title_changed: bool,
    /// Whether terminal modes changed.
    pub modes_changed: bool,
    /// Whether scrollback changed.
    pub scrollback_changed: bool,
    /// Number of retained history lines dropped by the surface policy while
    /// consuming this update.
    pub scrollback_lines_dropped: u64,
}

impl SurfaceUpdate {
    /// Creates a surface update with explicit dirty rows.
    pub const fn new(dirty_rows: DirtyRows) -> Self {
        Self {
            dirty_rows,
            cursor_changed: false,
            title_changed: false,
            modes_changed: false,
            scrollback_changed: false,
            scrollback_lines_dropped: 0,
        }
    }
}

impl Default for SurfaceUpdate {
    fn default() -> Self {
        Self::new(DirtyRows::None)
    }
}

/// Replay backend selector captured in repro dumps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum ReplayBackendKind {
    /// The built-in `PaneManager` default surface backend.
    DefaultSurfaceVt100,
}

/// Identifies a concrete surface backend implementation.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SurfaceBackendMetadata {
    /// Human-readable backend name.
    pub name: String,
    /// Backend version string.
    pub version: String,
    /// Optional replay selector for deterministic repro playback.
    pub replay_kind: Option<ReplayBackendKind>,
}

impl SurfaceBackendMetadata {
    /// Creates backend metadata without replay support.
    pub fn new(name: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            version: version.into(),
            replay_kind: None,
        }
    }

    /// Marks this backend as replayable through [`ReplayBackendKind`].
    pub fn with_replay_kind(mut self, replay_kind: ReplayBackendKind) -> Self {
        self.replay_kind = Some(replay_kind);
        self
    }
}

/// Abstract terminal surface backend contract.
pub trait SurfaceBackend: std::fmt::Debug + Send {
    /// Returns the visible surface size tracked by the backend.
    fn size(&self) -> Size;

    /// Resizes the backend surface.
    fn resize(&mut self, size: Size) -> Result<()>;

    /// Validates a resize without mutating backend state.
    ///
    /// Backends whose [`SurfaceBackend::resize`] implementation can reject
    /// specific sizes should override this hook and return the same error here
    /// before any PTY resize side effects occur.
    fn validate_resize(&self, size: Size) -> Result<()> {
        let _ = size;
        Ok(())
    }

    /// Feeds raw output bytes into the backend.
    fn feed(&mut self, bytes: &[u8]) -> Result<SurfaceUpdate>;

    /// Returns a visible surface snapshot.
    fn snapshot(&self) -> SurfaceSnapshot<'_>;

    /// Returns current cursor metadata.
    fn cursor(&self) -> CursorState;

    /// Returns current terminal modes.
    fn modes(&self) -> TerminalModes;

    /// Returns current scrollback content.
    fn scrollback(&self) -> ScrollbackSnapshot<'_>;

    /// Returns backend metadata for repro dumps and diagnostics.
    fn metadata(&self) -> SurfaceBackendMetadata {
        SurfaceBackendMetadata::new(std::any::type_name::<Self>(), env!("CARGO_PKG_VERSION"))
    }
}

/// Runtime statistics for a pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct PaneStats {
    /// Total history lines dropped from the backing scrollback buffer because
    /// of the pane's configured retention policy.
    pub scrollback_lines_dropped: u64,
}

/// A read-only view of pane state.
///
/// Snapshots must be cheap enough for every frame, but they do not need to be
/// zero-copy in every backend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneSnapshot<'a> {
    /// Pane identifier.
    pub id: PaneId,
    /// Optional human-readable title.
    pub title: Option<String>,
    /// Current lifecycle state.
    pub state: PaneState,
    /// Current interaction mode.
    pub interaction_mode: PaneInteractionMode,
    /// Current terminal size.
    pub size: Size,
    /// Surface state snapshot.
    pub surface: SurfaceSnapshot<'a>,
    /// Cursor state.
    pub cursor: CursorState,
    /// Terminal mode flags.
    pub modes: TerminalModes,
    /// Runtime statistics.
    pub stats: PaneStats,
}

/// Owned pane snapshot that can outlive any backend borrow.
pub type OwnedPaneSnapshot = PaneSnapshot<'static>;

impl<'a> PaneSnapshot<'a> {
    /// Clones this snapshot into an owned form.
    pub fn to_owned_snapshot(&self) -> OwnedPaneSnapshot {
        OwnedPaneSnapshot {
            id: self.id,
            title: self.title.clone(),
            state: self.state.clone(),
            interaction_mode: self.interaction_mode,
            size: self.size,
            surface: self.surface.to_owned_snapshot(),
            cursor: self.cursor,
            modes: self.modes,
            stats: self.stats,
        }
    }

    /// Converts this snapshot into an owned form.
    pub fn into_owned(self) -> OwnedPaneSnapshot {
        OwnedPaneSnapshot {
            id: self.id,
            title: self.title,
            state: self.state,
            interaction_mode: self.interaction_mode,
            size: self.size,
            surface: self.surface.into_owned(),
            cursor: self.cursor,
            modes: self.modes,
            stats: self.stats,
        }
    }
}

/// Configuration for how a pane's child process is terminated.
///
/// Defaults to a 5-second grace period between SIGTERM and SIGKILL, with
/// descendant killing enabled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct KillConfig {
    /// Grace period between SIGTERM and SIGKILL.
    pub term_grace: Duration,
    /// Whether to kill descendant processes.
    pub kill_descendants: bool,
}

impl KillConfig {
    /// Creates a new kill config.
    pub fn new(term_grace: Duration, kill_descendants: bool) -> Self {
        Self {
            term_grace,
            kill_descendants,
        }
    }
}

impl Default for KillConfig {
    fn default() -> Self {
        Self {
            term_grace: Duration::from_secs(5),
            kill_descendants: true,
        }
    }
}

/// Describes a child process and pane behavior.
///
/// # Defaults
///
/// | Field       | Default value                                           |
/// |-------------|---------------------------------------------------------|
/// | `id`        | `None` (manager-assigned)                               |
/// | `title`     | `None`                                                  |
/// | `command`   | `"cmd"` on Windows, `"sh"` elsewhere                    |
/// | `cwd`       | `None` (inherits process cwd)                           |
/// | `env`       | empty map                                               |
/// | `size`      | `Size { rows: 24, cols: 80 }`                           |
/// | `scrollback`| `None` (inherit the manager default)                    |
/// | `transcript`| `TranscriptConfig { mode: TranscriptMode::Disabled }`   |
/// | `surface`   | `SurfaceConfig`                                         |
/// | `input`     | `InputConfig`                                           |
/// | `attach`    | `AttachConfig`                                          |
/// | `kill`      | `KillConfig { term_grace: 5s, kill_descendants: true }` |
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct PaneConfig {
    /// Optional stable identifier. When `None`, the manager assigns one.
    pub id: Option<PaneId>,
    /// Optional human-readable title.
    pub title: Option<String>,
    /// The command to spawn.
    pub command: CommandSpec,
    /// Working directory for the child process.
    pub cwd: Option<PathBuf>,
    /// Additional environment variables for the child process.
    pub env: BTreeMap<String, String>,
    /// Initial terminal size.
    pub size: Size,
    /// Optional pane-specific scrollback retention policy.
    ///
    /// `None` means the manager's default scrollback policy is applied when
    /// the pane is spawned.
    pub scrollback: Option<ScrollbackConfig>,
    /// Transcript recording configuration.
    pub transcript: TranscriptConfig,
    /// Surface rendering configuration.
    pub surface: SurfaceConfig,
    /// Input handling configuration.
    pub input: InputConfig,
    /// Attach/detach behavior configuration.
    pub attach: AttachConfig,
    /// Kill behavior configuration.
    pub kill: KillConfig,
    /// Maximum number of PTY output frames to queue before dropping.
    ///
    /// Defaults to 128. A value of 0 is treated as the default.
    pub output_queue_capacity: usize,
}

#[cfg(feature = "serde")]
impl<'de> serde::Deserialize<'de> for PaneConfig {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(serde::Deserialize)]
        struct Raw {
            id: Option<PaneId>,
            title: Option<String>,
            command: CommandSpec,
            cwd: Option<PathBuf>,
            env: BTreeMap<String, String>,
            size: Size,
            #[serde(default)]
            scrollback: Option<ScrollbackConfig>,
            transcript: TranscriptConfig,
            surface: SurfaceConfig,
            input: InputConfig,
            attach: AttachConfig,
            kill: KillConfig,
            #[serde(default)]
            output_queue_capacity: usize,
        }

        let raw = Raw::deserialize(deserializer)?;
        let config = PaneConfig {
            id: raw.id,
            title: raw.title,
            command: raw.command,
            cwd: raw.cwd,
            env: raw.env,
            size: raw.size,
            scrollback: raw.scrollback,
            transcript: raw.transcript,
            surface: raw.surface,
            input: raw.input,
            attach: raw.attach,
            kill: raw.kill,
            output_queue_capacity: raw.output_queue_capacity,
        };
        config.validate().map_err(serde::de::Error::custom)?;
        Ok(config)
    }
}

const fn default_shell() -> &'static str {
    if cfg!(windows) {
        "cmd"
    } else {
        "sh"
    }
}

impl PaneConfig {
    /// Creates a config for a program with no arguments.
    pub fn command(program: impl Into<String>) -> Self {
        Self {
            command: CommandSpec::new(program),
            ..Self::default()
        }
    }

    /// Creates a config for a program and argument list.
    pub fn command_with_args<I, S>(program: impl Into<String>, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            command: CommandSpec::with_args(program, args),
            ..Self::default()
        }
    }

    /// Returns a shell-flavored configuration for the host OS.
    pub fn shell() -> Self {
        Self::command(default_shell())
    }

    /// Returns the configured program name.
    pub fn program(&self) -> &str {
        &self.command.program
    }

    /// Returns the configured argument list.
    pub fn args(&self) -> &[String] {
        &self.command.args
    }

    // Fluent setters

    /// Sets the pane identifier.
    pub fn with_id(mut self, id: PaneId) -> Self {
        self.id = Some(id);
        self
    }

    /// Sets the pane title.
    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Sets the working directory.
    pub fn with_cwd(mut self, cwd: impl Into<PathBuf>) -> Self {
        self.cwd = Some(cwd.into());
        self
    }

    /// Adds or replaces an environment variable.
    pub fn with_env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.insert(key.into(), value.into());
        self
    }

    /// Sets the initial terminal size.
    pub fn with_size(mut self, size: Size) -> Self {
        self.size = size;
        self
    }

    /// Sets this pane's scrollback configuration.
    pub fn with_scrollback(mut self, scrollback: ScrollbackConfig) -> Self {
        self.scrollback = Some(scrollback);
        self
    }

    /// Sets the transcript configuration.
    pub fn with_transcript(mut self, transcript: TranscriptConfig) -> Self {
        self.transcript = transcript;
        self
    }

    /// Sets the surface configuration.
    pub fn with_surface(mut self, surface: SurfaceConfig) -> Self {
        self.surface = surface;
        self
    }

    /// Sets the input configuration.
    pub fn with_input(mut self, input: InputConfig) -> Self {
        self.input = input;
        self
    }

    /// Sets the attach configuration.
    pub fn with_attach(mut self, attach: AttachConfig) -> Self {
        self.attach = attach;
        self
    }

    /// Sets the kill configuration.
    pub fn with_kill(mut self, kill: KillConfig) -> Self {
        self.kill = kill;
        self
    }

    /// Sets the PTY output frame queue capacity.
    ///
    /// A value of 0 is treated as the default (128) during spawn.
    pub fn with_output_queue_capacity(mut self, capacity: usize) -> Self {
        self.output_queue_capacity = capacity;
        self
    }

    /// Validates the configuration.
    ///
    /// Checks:
    /// - command program is non-empty
    /// - size rows and cols are non-zero
    pub fn validate(&self) -> Result<()> {
        self.command.validate()?;
        if self.size.rows == 0 || self.size.cols == 0 {
            return Err(PaneError::InvalidSize {
                rows: self.size.rows,
                cols: self.size.cols,
            });
        }
        Ok(())
    }
}

impl Default for PaneConfig {
    fn default() -> Self {
        Self {
            id: None,
            title: None,
            command: CommandSpec::new(default_shell()),
            cwd: None,
            env: BTreeMap::new(),
            size: Size::new(24, 80),
            scrollback: None,
            transcript: TranscriptConfig::default(),
            surface: SurfaceConfig,
            input: InputConfig::default(),
            attach: AttachConfig,
            kill: KillConfig::default(),
            output_queue_capacity: 128,
        }
    }
}

#[cfg(test)]
mod tests;
