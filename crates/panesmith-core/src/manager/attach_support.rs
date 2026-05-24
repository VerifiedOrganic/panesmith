use std::fmt;
use std::time::{Duration, Instant};

use crate::detach_encoding::parse_ctrl_detach_encoding;
use crate::{
    AttachScreenPolicy, CellAttrs, CellStyle, CellWidth, ColorSpec, DetachConfig, DetachReason,
    IoOperation, PaneError, PaneSnapshot, ScrollbackLine, Size, SurfaceCell, SurfaceRow,
    TerminalViewport,
};

use super::ATTACH_SCREEN_RESET;

/// A chunk of raw stdin bytes captured while a pane is attached fullscreen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneAttachInputChunk {
    /// Capture timestamp used for detach-chord timeout handling.
    pub at: Instant,
    /// Raw bytes read from the host terminal.
    pub bytes: Vec<u8>,
}

/// Host terminal I/O contract used by [`PaneManager::attach_blocking`].
pub trait PaneAttachTerminal {
    /// Backend-specific terminal I/O error type.
    type Error: fmt::Debug;

    /// Attempts to read the next raw stdin chunk without blocking forever.
    fn read_stdin(&mut self) -> std::result::Result<Option<PaneAttachInputChunk>, Self::Error>;

    /// Writes child PTY output to the host terminal stdout.
    fn write_stdout(&mut self, bytes: &[u8]) -> std::result::Result<(), Self::Error>;

    /// Returns the current real-terminal size.
    fn size(&self) -> std::result::Result<Size, Self::Error>;
}

/// Host terminal suspend/restore contract used by [`PaneManager::attach_blocking`].
///
/// The host terminal profile is caller-owned. Panesmith drives the attached
/// pane's PTY polling, transcript, surface updates, viewport controls, and
/// attach lifecycle, but it cannot infer the raw mode, alternate-screen,
/// mouse, bracketed-paste, keyboard-enhancement, cursor, or redraw policy that
/// an embedding TUI expects after detach.
///
/// Implementations should save the caller's expected terminal profile in
/// [`suspend_for_attach`](Self::suspend_for_attach) and restore that exact
/// profile in [`restore_after_attach`](Self::restore_after_attach). Restoring
/// to a generic "sane" terminal state is not sufficient for hosts that already
/// own enhanced keyboard or mouse input modes.
pub trait PaneAttachTerminalControl {
    /// Backend-specific terminal control error type.
    type Error: fmt::Debug;
    /// Opaque token representing the terminal state saved before attach.
    type RestoreToken;

    /// Suspends host drawing and prepares the real terminal for attach.
    ///
    /// The returned token should contain enough caller-owned state to restore
    /// the host's expected terminal profile after attach ends.
    fn suspend_for_attach(
        &mut self,
        policy: AttachScreenPolicy,
    ) -> std::result::Result<Self::RestoreToken, Self::Error>;

    /// Restores the host terminal after attach ends.
    ///
    /// This must return the real terminal to the caller's expected profile,
    /// including raw mode, alternate-screen state, mouse capture, bracketed
    /// paste, keyboard enhancement flags, and any other host-owned mode needed
    /// for its input parser and redraw loop to continue without a full reset.
    fn restore_after_attach(
        &mut self,
        token: &mut Self::RestoreToken,
    ) -> std::result::Result<(), Self::Error>;
}

/// Structured failure returned by [`PaneManager::attach_blocking`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaneAttachError {
    /// The pane was missing or not attachable.
    Pane { error: PaneError },
    /// Suspending the host terminal failed.
    Suspend { message: String },
    /// Restoring the host terminal failed.
    Restore { message: String },
    /// Reading host stdin failed.
    TerminalInput { message: String },
    /// Writing child output to host stdout failed.
    TerminalOutput { message: String },
    /// Reading the real-terminal size failed.
    TerminalSize { message: String },
    /// A PTY/runtime frame reported an error.
    PtyRuntime { message: String },
    /// Writing bytes to the child PTY failed.
    PtyWrite { message: String },
    /// Resizing the child PTY failed.
    PtyResize { message: String },
    /// Feeding the embedded surface failed.
    SurfaceFeed { message: String },
    /// Resizing the embedded surface failed.
    SurfaceResize { message: String },
}

impl fmt::Display for PaneAttachError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Pane { error } => write!(f, "{error}"),
            Self::Suspend { message } => write!(f, "attach suspend failed: {message}"),
            Self::Restore { message } => write!(f, "attach restore failed: {message}"),
            Self::TerminalInput { message } => write!(f, "attach stdin read failed: {message}"),
            Self::TerminalOutput { message } => {
                write!(f, "attach stdout write failed: {message}")
            }
            Self::TerminalSize { message } => write!(f, "attach terminal size failed: {message}"),
            Self::PtyRuntime { message } => write!(f, "attach PTY runtime failed: {message}"),
            Self::PtyWrite { message } => write!(f, "attach PTY write failed: {message}"),
            Self::PtyResize { message } => write!(f, "attach PTY resize failed: {message}"),
            Self::SurfaceFeed { message } => write!(f, "attach surface feed failed: {message}"),
            Self::SurfaceResize { message } => {
                write!(f, "attach surface resize failed: {message}")
            }
        }
    }
}

impl std::error::Error for PaneAttachError {}

/// Successful result from [`PaneManager::attach_blocking`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneAttachOutcome {
    /// Why attach ended.
    pub reason: DetachReason,
    /// Exit code observed while attached, if the child exited.
    pub child_exit_code: Option<i32>,
    /// The real-terminal size used during attach.
    pub terminal_size: Size,
    /// The embedded size restored on detach.
    pub restored_size: Size,
    /// Bytes read after the detach chord in the same stdin chunk.
    pub remaining_input: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ManagerDetachMatcher {
    pub(super) chord: Vec<u8>,
    timeout: Duration,
    held: Vec<u8>,
    held_at: Vec<Instant>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ManagerDetachFeedResult<'a> {
    pub(super) forward: Vec<u8>,
    pub(super) detached: bool,
    pub(super) remaining: &'a [u8],
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ManagerDetachFeedStep {
    ForwardNone,
    ForwardByte(u8),
    ForwardBytes(Vec<u8>),
    Detach,
}

impl ManagerDetachMatcher {
    pub(super) fn new(config: &DetachConfig) -> Self {
        Self {
            chord: config.chord.clone(),
            timeout: config.partial_timeout,
            held: Vec::new(),
            held_at: Vec::new(),
        }
    }

    pub(super) fn feed_bytes<'a>(
        &mut self,
        bytes: &'a [u8],
        now: Instant,
    ) -> ManagerDetachFeedResult<'a> {
        let mut forward = Vec::with_capacity(bytes.len());

        if let Some(held) = self.check_timeout(now) {
            forward.extend_from_slice(&held);
        }

        let mut idx = 0;
        while idx < bytes.len() {
            let (byte, consumed) =
                if let Some(matched) = parse_ctrl_detach_encoding(&bytes[idx..], &self.chord) {
                    (matched.normalized, matched.consumed)
                } else {
                    (bytes[idx], 1)
                };

            match self.feed_byte_inner(byte, now) {
                ManagerDetachFeedStep::ForwardNone => {}
                ManagerDetachFeedStep::ForwardByte(byte) => forward.push(byte),
                ManagerDetachFeedStep::ForwardBytes(mut bytes) => forward.append(&mut bytes),
                ManagerDetachFeedStep::Detach => {
                    return ManagerDetachFeedResult {
                        forward,
                        detached: true,
                        remaining: &bytes[idx + consumed..],
                    };
                }
            }
            idx += consumed;
        }

        ManagerDetachFeedResult {
            forward,
            detached: false,
            remaining: &[],
        }
    }

    pub(super) fn check_timeout(&mut self, now: Instant) -> Option<Vec<u8>> {
        debug_assert_eq!(
            self.held.len(),
            self.held_at.len(),
            "held bytes and timestamps must stay aligned"
        );

        if let Some(start) = self.held_at.first().copied() {
            debug_assert!(
                !self.held.is_empty(),
                "held timestamp set but held is empty"
            );
            if now.duration_since(start) >= self.timeout {
                self.held_at.clear();
                let held = std::mem::take(&mut self.held);
                if !held.is_empty() {
                    return Some(held);
                }
            }
        }
        None
    }

    fn feed_byte_inner(&mut self, byte: u8, now: Instant) -> ManagerDetachFeedStep {
        if self.chord.is_empty() {
            return ManagerDetachFeedStep::ForwardByte(byte);
        }

        if self.held.is_empty() {
            if byte == self.chord[0] {
                self.held.push(byte);
                self.held_at.push(now);
                if self.chord.len() == 1 {
                    self.reset();
                    return ManagerDetachFeedStep::Detach;
                }
                return ManagerDetachFeedStep::ForwardNone;
            }
            return ManagerDetachFeedStep::ForwardByte(byte);
        }

        let next_idx = self.held.len();
        if next_idx < self.chord.len() && byte == self.chord[next_idx] {
            self.held.push(byte);
            self.held_at.push(now);
            if self.held.len() == self.chord.len() {
                self.reset();
                return ManagerDetachFeedStep::Detach;
            }
            return ManagerDetachFeedStep::ForwardNone;
        }

        let mut combined = std::mem::take(&mut self.held);
        let mut combined_at = std::mem::take(&mut self.held_at);
        combined.push(byte);
        combined_at.push(now);

        let mut suffix_len = 0;
        for k in (1..=combined.len().min(self.chord.len())).rev() {
            if combined[combined.len() - k..] == self.chord[0..k] {
                suffix_len = k;
                break;
            }
        }

        let forward = if suffix_len == 0 {
            combined
        } else {
            let split_idx = combined.len() - suffix_len;
            let held = combined.split_off(split_idx);
            let held_at = combined_at.split_off(split_idx);
            self.held = held;
            self.held_at = held_at;
            combined
        };

        if forward.is_empty() {
            ManagerDetachFeedStep::ForwardNone
        } else if forward.len() == 1 {
            ManagerDetachFeedStep::ForwardByte(forward[0])
        } else {
            ManagerDetachFeedStep::ForwardBytes(forward)
        }
    }

    fn reset(&mut self) {
        self.held.clear();
        self.held_at.clear();
    }
}

pub(super) struct PaneAttachGuard<'a, T: PaneAttachTerminalControl + ?Sized> {
    control: &'a mut T,
    token: Option<T::RestoreToken>,
}

impl<'a, T: PaneAttachTerminalControl + ?Sized> PaneAttachGuard<'a, T> {
    pub(super) fn new(control: &'a mut T, token: T::RestoreToken) -> Self {
        Self {
            control,
            token: Some(token),
        }
    }

    pub(super) fn detach(&mut self) -> std::result::Result<(), T::Error> {
        if let Some(ref mut token) = self.token {
            self.control.restore_after_attach(token)?;
            self.token = None;
        }
        Ok(())
    }

    pub(super) fn is_detached(&self) -> bool {
        self.token.is_none()
    }

    pub(super) fn disarm(&mut self) {
        self.token = None;
    }
}

impl<'a, T: PaneAttachTerminalControl + ?Sized> Drop for PaneAttachGuard<'a, T> {
    fn drop(&mut self) {
        if let Some(mut token) = self.token.take() {
            let _ = self.control.restore_after_attach(&mut token);
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum AttachStdinPollResult {
    Idle,
    Forwarded,
    ViewportChanged,
    Detached { remaining_input: Vec<u8> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct AttachOutputDrainResult {
    pub(super) made_progress: bool,
    pub(super) child_exited: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AttachViewportAction {
    ScrollUp(usize),
    ScrollDown(usize),
    PageUp,
    PageDown,
    Home,
    End,
}

pub(super) fn split_attach_viewport_controls(bytes: &[u8]) -> (Vec<u8>, Vec<AttachViewportAction>) {
    let mut forward = Vec::with_capacity(bytes.len());
    let mut actions = Vec::new();
    let mut idx = 0;

    while idx < bytes.len() {
        if let Some((action, consumed)) = parse_attach_viewport_control(&bytes[idx..]) {
            actions.push(action);
            idx += consumed;
        } else {
            forward.push(bytes[idx]);
            idx += 1;
        }
    }

    (forward, actions)
}

fn parse_attach_viewport_control(bytes: &[u8]) -> Option<(AttachViewportAction, usize)> {
    for (sequence, action) in [
        (b"\x1b[5~".as_slice(), AttachViewportAction::PageUp),
        (b"\x1b[6~".as_slice(), AttachViewportAction::PageDown),
        (b"\x1b[H".as_slice(), AttachViewportAction::Home),
        (b"\x1b[1~".as_slice(), AttachViewportAction::Home),
        (b"\x1b[7~".as_slice(), AttachViewportAction::Home),
        (b"\x1bOH".as_slice(), AttachViewportAction::Home),
        (b"\x1b[F".as_slice(), AttachViewportAction::End),
        (b"\x1b[4~".as_slice(), AttachViewportAction::End),
        (b"\x1b[8~".as_slice(), AttachViewportAction::End),
        (b"\x1bOF".as_slice(), AttachViewportAction::End),
    ] {
        if bytes.starts_with(sequence) {
            return Some((action, sequence.len()));
        }
    }

    parse_sgr_mouse_wheel(bytes)
}

fn parse_sgr_mouse_wheel(bytes: &[u8]) -> Option<(AttachViewportAction, usize)> {
    const PREFIX: &[u8] = b"\x1b[<";
    if !bytes.starts_with(PREFIX) {
        return None;
    }

    let end = bytes
        .iter()
        .position(|byte| *byte == b'M' || *byte == b'm')?;
    let body = &bytes[PREFIX.len()..end];
    let button = body.split(|byte| *byte == b';').next()?;
    let button = parse_u16(button)?;
    let action = match button {
        64 => AttachViewportAction::ScrollUp(3),
        65 => AttachViewportAction::ScrollDown(3),
        _ => return None,
    };
    Some((action, end + 1))
}

fn parse_u16(bytes: &[u8]) -> Option<u16> {
    if bytes.is_empty() {
        return None;
    }
    let mut value = 0_u16;
    for &byte in bytes {
        value = value.checked_mul(10)?;
        value = value.checked_add(u16::from(byte.checked_sub(b'0')?))?;
    }
    Some(value)
}

pub(super) fn render_attach_viewport_bytes(
    snapshot: &PaneSnapshot<'_>,
    scrollback: &crate::ScrollbackSnapshot<'_>,
    viewport: TerminalViewport,
    terminal_size: Size,
) -> Vec<u8> {
    let visible_rows = usize::from(terminal_size.rows);
    let max_cols = usize::from(terminal_size.cols);
    let metrics = viewport.metrics(snapshot, Some(scrollback), visible_rows);
    let mut out = Vec::new();
    out.extend_from_slice(ATTACH_SCREEN_RESET);

    for (render_row_idx, logical_row_idx) in (metrics.start_row..metrics.end_row).enumerate() {
        push_cursor_position(&mut out, render_row_idx + 1, 1);

        if let Some(line) = scrollback.lines.get(logical_row_idx) {
            render_attach_scrollback_line(&mut out, line, max_cols);
        } else {
            let surface_row_idx = logical_row_idx.saturating_sub(scrollback.lines.len());
            render_attach_surface_row(
                &mut out,
                snapshot.surface.rows.get(surface_row_idx),
                max_cols,
            );
        }

        out.extend_from_slice(b"\x1b[0m\x1b[K");
    }

    out.extend_from_slice(b"\x1b[0m");
    out
}

fn render_attach_scrollback_line(out: &mut Vec<u8>, line: &ScrollbackLine<'_>, max_cols: usize) {
    if line.row.cells.is_empty() {
        push_plain_text_cells(out, line.text.as_ref(), max_cols);
    } else {
        render_attach_surface_row(out, Some(&line.row), max_cols);
    }
}

fn render_attach_surface_row(out: &mut Vec<u8>, row: Option<&SurfaceRow<'_>>, max_cols: usize) {
    let Some(row) = row else {
        return;
    };

    for (col_idx, cell) in row.cells.iter().take(max_cols).enumerate() {
        render_attach_cell(out, cell, col_idx + 1 == max_cols);
    }
}

fn render_attach_cell(out: &mut Vec<u8>, cell: &SurfaceCell<'_>, at_right_edge: bool) {
    match cell.width {
        CellWidth::Continuation => {}
        CellWidth::Double if at_right_edge => {
            push_style(out, cell.style);
            out.push(b' ');
        }
        CellWidth::Single | CellWidth::Double => {
            push_style(out, cell.style);
            if cell.text.is_empty() {
                out.push(b' ');
            } else {
                out.extend_from_slice(cell.text.as_bytes());
            }
        }
    }
}

fn push_plain_text_cells(out: &mut Vec<u8>, text: &str, max_cols: usize) {
    push_style(out, CellStyle::default());
    for ch in text.chars().take(max_cols) {
        let mut buffer = [0_u8; 4];
        out.extend_from_slice(ch.encode_utf8(&mut buffer).as_bytes());
    }
}

fn push_cursor_position(out: &mut Vec<u8>, row: usize, col: usize) {
    out.extend_from_slice(b"\x1b[");
    push_decimal(out, row);
    out.push(b';');
    push_decimal(out, col);
    out.push(b'H');
}

fn push_style(out: &mut Vec<u8>, style: CellStyle) {
    let mut codes = Vec::new();
    codes.push("0".to_string());
    push_attr_codes(&mut codes, style.attrs);
    push_color_codes(&mut codes, 38, 39, style.fg);
    push_color_codes(&mut codes, 48, 49, style.bg);
    push_color_codes(&mut codes, 58, 59, style.underline_color);

    out.extend_from_slice(b"\x1b[");
    for (idx, code) in codes.iter().enumerate() {
        if idx > 0 {
            out.push(b';');
        }
        out.extend_from_slice(code.as_bytes());
    }
    out.push(b'm');
}

fn push_attr_codes(codes: &mut Vec<String>, attrs: CellAttrs) {
    if attrs.bold {
        codes.push("1".into());
    }
    if attrs.dim {
        codes.push("2".into());
    }
    if attrs.italic {
        codes.push("3".into());
    }
    if attrs.underlined {
        codes.push("4".into());
    }
    if attrs.slow_blink {
        codes.push("5".into());
    }
    if attrs.rapid_blink {
        codes.push("6".into());
    }
    if attrs.reversed {
        codes.push("7".into());
    }
    if attrs.hidden {
        codes.push("8".into());
    }
    if attrs.crossed_out {
        codes.push("9".into());
    }
}

fn push_color_codes(codes: &mut Vec<String>, prefix: u8, reset: u8, color: Option<ColorSpec>) {
    match color {
        None => {}
        Some(ColorSpec::Default) => codes.push(reset.to_string()),
        Some(ColorSpec::Indexed(index)) => {
            codes.push(prefix.to_string());
            codes.push("5".into());
            codes.push(index.to_string());
        }
        Some(ColorSpec::Rgb(r, g, b)) => {
            codes.push(prefix.to_string());
            codes.push("2".into());
            codes.push(r.to_string());
            codes.push(g.to_string());
            codes.push(b.to_string());
        }
    }
}

fn push_decimal(out: &mut Vec<u8>, value: usize) {
    out.extend_from_slice(value.to_string().as_bytes());
}

pub(super) fn pane_attach_error_to_pane_error(error: &PaneAttachError) -> PaneError {
    match error {
        PaneAttachError::Pane { error } => error.clone(),
        PaneAttachError::Suspend { message } => PaneError::Attach {
            message: format!(
                "attach suspend failed: {}",
                normalize_debug_message(message)
            ),
        },
        PaneAttachError::Restore { message } => PaneError::Attach {
            message: format!(
                "attach restore failed: {}",
                normalize_debug_message(message)
            ),
        },
        PaneAttachError::TerminalSize { message } => PaneError::Attach {
            message: format!(
                "attach terminal size failed: {}",
                normalize_debug_message(message)
            ),
        },
        PaneAttachError::PtyRuntime { message } => PaneError::Attach {
            message: format!("attach PTY runtime failed: {message}"),
        },
        PaneAttachError::TerminalInput { message } => PaneError::Io {
            operation: IoOperation::Read,
            message: format!(
                "attach stdin read failed: {}",
                normalize_debug_message(message)
            ),
        },
        PaneAttachError::TerminalOutput { message } => PaneError::Io {
            operation: IoOperation::Write,
            message: format!(
                "attach stdout write failed: {}",
                normalize_debug_message(message)
            ),
        },
        PaneAttachError::PtyWrite { message } => PaneError::Io {
            operation: IoOperation::Write,
            message: format!(
                "attach PTY write failed: {}",
                normalize_debug_message(message)
            ),
        },
        PaneAttachError::PtyResize { message } => PaneError::Io {
            operation: IoOperation::Resize,
            message: format!(
                "attach PTY resize failed: {}",
                normalize_debug_message(message)
            ),
        },
        PaneAttachError::SurfaceFeed { message } => PaneError::Surface {
            message: format!(
                "attach surface feed failed: {}",
                normalize_debug_message(message)
            ),
        },
        PaneAttachError::SurfaceResize { message } => PaneError::Surface {
            message: format!(
                "attach surface resize failed: {}",
                normalize_debug_message(message)
            ),
        },
    }
}

fn normalize_debug_message(message: &str) -> String {
    // Safe: Debug formatting for &str/String always emits ASCII double-quotes.
    if message.len() >= 2 && message.starts_with('"') && message.ends_with('"') {
        message[1..message.len() - 1].to_string()
    } else {
        message.to_string()
    }
}
