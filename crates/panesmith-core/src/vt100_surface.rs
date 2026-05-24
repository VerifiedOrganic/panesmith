//! Shared vt100-backed surface implementation.
//!
//! This module is hidden from the public API docs because it is an internal
//! workspace reuse point for the core default surface and the public
//! `panesmith-vt100` wrapper.

use std::borrow::Cow;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use vt100::{Callbacks, Color, MouseProtocolMode, Parser, Screen};

use crate::mode_overlay::TerminalModeOverlay;
use crate::{
    CellAttrs, CellStyle, CellWidth, ColorSpec, CursorPosition, CursorState, DirtyRows, MouseMode,
    PaneError, PaneId, Result, ScrollbackLine, ScrollbackSnapshot, Size, SurfaceCell, SurfaceRow,
    SurfaceSnapshot, SurfaceUpdate, TerminalModes,
};

/// Default scrollback capacity used by the public vt100 backend.
#[doc(hidden)]
pub const DEFAULT_VT100_SCROLLBACK_ROWS: usize = usize::MAX;

/// Shared terminal-aware vt100 surface engine.
#[doc(hidden)]
pub struct Vt100Surface {
    pane_id: PaneId,
    parser: Parser<BackendCallbacks>,
    scrollback_state: ScrollbackState,
    previous_byte_was_escape: bool,
    mode_overlay: TerminalModeOverlay,
    validation_name: &'static str,
    scrollback_capacity: usize,
}

#[derive(Debug, Default)]
struct BackendCallbacks {
    title: Option<String>,
    icon_name: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct ScrollbackState {
    len: usize,
    digest: Option<u64>,
}

impl BackendCallbacks {
    fn title(&self) -> Option<&str> {
        self.title.as_deref().or(self.icon_name.as_deref())
    }
}

impl Callbacks for BackendCallbacks {
    fn set_window_icon_name(&mut self, _: &mut Screen, icon_name: &[u8]) {
        self.icon_name = Some(String::from_utf8_lossy(icon_name).into_owned());
    }

    fn set_window_title(&mut self, _: &mut Screen, title: &[u8]) {
        self.title = Some(String::from_utf8_lossy(title).into_owned());
    }
}

impl Vt100Surface {
    /// Creates a shared vt100 surface with the provided scrollback capacity.
    pub fn new(
        pane_id: PaneId,
        size: Size,
        scrollback_capacity: usize,
        validation_name: &'static str,
    ) -> Result<Self> {
        let size = validate_supported_size(size, validation_name)?;
        let mut parser = Parser::new_with_callbacks(
            size.rows,
            size.cols,
            scrollback_capacity,
            BackendCallbacks::default(),
        );
        let scrollback_state =
            scrollback_state_from_screen(parser.screen_mut(), scrollback_capacity);

        Ok(Self {
            pane_id,
            parser,
            scrollback_state,
            previous_byte_was_escape: false,
            mode_overlay: TerminalModeOverlay::default(),
            validation_name,
            scrollback_capacity,
        })
    }

    /// Returns the pane identifier associated with this surface.
    pub const fn pane_id(&self) -> PaneId {
        self.pane_id
    }

    /// Returns the current terminal size.
    pub fn size(&self) -> Size {
        size_from_screen(self.parser.screen())
    }

    /// Validates a requested resize for this surface.
    pub fn validate_resize(&self, size: Size) -> Result<()> {
        validate_supported_size(size, self.validation_name).map(|_| ())
    }

    /// Resizes the terminal parser.
    pub fn resize(&mut self, size: Size) -> Result<()> {
        validate_supported_size(size, self.validation_name)?;
        let screen = self.parser.screen_mut();
        screen.set_size(size.rows, size.cols);
        self.scrollback_state = scrollback_state_from_screen(screen, self.scrollback_capacity);
        Ok(())
    }

    /// Feeds raw terminal bytes into the parser.
    pub fn feed(&mut self, bytes: &[u8]) -> Result<SurfaceUpdate> {
        if bytes.is_empty() {
            return Ok(SurfaceUpdate::default());
        }

        let mut previous_screen = self.parser.screen().clone();
        let previous_cursor = cursor_from_screen(&previous_screen);
        let previous_modes = self.modes();
        let previous_title = self.title().map(str::to_owned);
        let may_reset_scrollback = feed_may_reset_scrollback(bytes, self.previous_byte_was_escape);
        let previous_scrollback_state =
            if may_reset_scrollback && self.scrollback_state.len < self.scrollback_capacity {
                scrollback_state_from_screen_with_digest_policy(
                    &mut previous_screen,
                    self.scrollback_capacity,
                    true,
                )
            } else {
                self.scrollback_state
            };

        self.mode_overlay.update_from_output(bytes);
        let scrollback_lines_dropped = self.process_bytes_and_count_scrollback_drops(bytes);
        self.previous_byte_was_escape = bytes.last().copied() == Some(0x1b);

        let cursor = cursor_from_screen(self.parser.screen());
        let modes = self.modes();
        let dirty_rows = dirty_rows_between(&previous_screen, self.parser.screen());
        let scrollback_state = scrollback_state_after_feed(
            self.parser.screen_mut(),
            previous_scrollback_state,
            may_reset_scrollback,
            self.scrollback_capacity,
        );
        self.scrollback_state = scrollback_state;

        Ok(SurfaceUpdate {
            dirty_rows,
            cursor_changed: cursor != previous_cursor,
            title_changed: self.title() != previous_title.as_deref(),
            modes_changed: modes != previous_modes,
            scrollback_changed: scrollback_state != previous_scrollback_state,
            scrollback_lines_dropped,
        })
    }

    /// Returns the current surface snapshot.
    pub fn snapshot(&self) -> SurfaceSnapshot<'_> {
        let screen = self.parser.screen();
        SurfaceSnapshot::new(
            size_from_screen(screen),
            snapshot_rows(screen),
            cursor_from_screen(screen),
            self.modes(),
            self.title().map(Cow::Borrowed),
        )
    }

    /// Returns the current cursor state.
    pub fn cursor(&self) -> CursorState {
        cursor_from_screen(self.parser.screen())
    }

    /// Returns the current terminal modes.
    pub fn modes(&self) -> TerminalModes {
        self.mode_overlay
            .merge_with(modes_from_screen(self.parser.screen()))
    }

    /// Returns retained scrollback with styled rows.
    pub fn scrollback(&self) -> ScrollbackSnapshot<'static> {
        scrollback_snapshot_from_screen(self.parser.screen())
    }

    fn title(&self) -> Option<&str> {
        self.parser.callbacks().title()
    }

    fn process_bytes_and_count_scrollback_drops(&mut self, bytes: &[u8]) -> u64 {
        if self.scrollback_capacity == usize::MAX {
            self.parser.process(bytes);
            return 0;
        }

        let mut dropped = 0u64;
        let mut before_len = scrollback_len_from_screen(self.parser.screen_mut());
        for byte in bytes {
            let before_newest = (self.scrollback_capacity > 0
                && before_len == self.scrollback_capacity)
                .then(|| newest_scrollback_row_digest_from_screen(self.parser.screen_mut()));

            self.parser.process(std::slice::from_ref(byte));

            let after_len = scrollback_len_from_screen(self.parser.screen_mut());
            if self.scrollback_capacity > 0
                && before_len == self.scrollback_capacity
                && after_len == self.scrollback_capacity
            {
                let after_newest =
                    newest_scrollback_row_digest_from_screen(self.parser.screen_mut());
                if before_newest != Some(after_newest) {
                    dropped = dropped.saturating_add(1);
                }
            }
            before_len = after_len;
        }
        dropped
    }
}

fn validate_supported_size(size: Size, validation_name: &str) -> Result<Size> {
    let size = Size::try_new(size.rows, size.cols)?;
    if size.rows < 2 {
        Err(PaneError::Surface {
            message: format!(
                "{validation_name} requires at least two rows; got rows={} cols={}",
                size.rows, size.cols
            ),
        })
    } else {
        Ok(size)
    }
}

fn size_from_screen(screen: &Screen) -> Size {
    let (rows, cols) = screen.size();
    Size::new(rows, cols)
}

fn cursor_from_screen(screen: &Screen) -> CursorState {
    let (row, col) = screen.cursor_position();
    CursorState::new(Some(CursorPosition::new(row, col)), !screen.hide_cursor())
}

fn modes_from_screen(screen: &Screen) -> TerminalModes {
    TerminalModes {
        bracketed_paste: screen.bracketed_paste(),
        mouse: mouse_mode_from_vt100(screen.mouse_protocol_mode()),
        focus_events: false,
        application_cursor: screen.application_cursor(),
        alternate_screen: screen.alternate_screen(),
    }
}

fn mouse_mode_from_vt100(mode: MouseProtocolMode) -> MouseMode {
    match mode {
        MouseProtocolMode::None => MouseMode::None,
        MouseProtocolMode::Press => MouseMode::X10,
        MouseProtocolMode::PressRelease => MouseMode::Normal,
        MouseProtocolMode::ButtonMotion => MouseMode::ButtonEvent,
        MouseProtocolMode::AnyMotion => MouseMode::AnyEvent,
    }
}

fn snapshot_rows(screen: &Screen) -> Vec<SurfaceRow<'_>> {
    let size = size_from_screen(screen);
    let mut rows = Vec::with_capacity(usize::from(size.rows));

    for row in 0..size.rows {
        let mut cells = Vec::with_capacity(usize::from(size.cols));
        for col in 0..size.cols {
            let cell = screen.cell(row, col);
            cells.push(surface_cell_from_vt100(cell));
        }

        rows.push(SurfaceRow::new(cells).with_wrapped(screen.row_wrapped(row)));
    }

    rows
}

fn surface_cell_from_vt100(cell: Option<&vt100::Cell>) -> SurfaceCell<'_> {
    let Some(cell) = cell else {
        return SurfaceCell::new("", CellWidth::Single, CellStyle::default());
    };

    SurfaceCell::new(
        cell.contents(),
        width_from_vt100_cell(cell),
        style_from_vt100_cell(cell),
    )
}

fn width_from_vt100_cell(cell: &vt100::Cell) -> CellWidth {
    if cell.is_wide_continuation() {
        CellWidth::Continuation
    } else if cell.is_wide() {
        CellWidth::Double
    } else {
        CellWidth::Single
    }
}

fn style_from_vt100_cell(cell: &vt100::Cell) -> CellStyle {
    CellStyle {
        fg: color_from_vt100(cell.fgcolor()),
        bg: color_from_vt100(cell.bgcolor()),
        underline_color: None,
        attrs: CellAttrs {
            bold: cell.bold(),
            dim: cell.dim(),
            italic: cell.italic(),
            underlined: cell.underline(),
            slow_blink: false,
            rapid_blink: false,
            reversed: cell.inverse(),
            hidden: false,
            crossed_out: false,
        },
    }
}

fn color_from_vt100(color: Color) -> Option<ColorSpec> {
    match color {
        Color::Default => None,
        Color::Idx(idx) => Some(ColorSpec::Indexed(idx)),
        Color::Rgb(r, g, b) => Some(ColorSpec::Rgb(r, g, b)),
    }
}

fn scrollback_state_from_screen(screen: &mut Screen, capacity: usize) -> ScrollbackState {
    scrollback_state_from_screen_with_digest_policy(screen, capacity, false)
}

fn scrollback_len_from_screen(screen: &mut Screen) -> usize {
    let previous_offset = screen.scrollback();
    screen.set_scrollback(usize::MAX);
    let len = screen.scrollback();
    screen.set_scrollback(previous_offset);
    len
}

fn newest_scrollback_row_digest_from_screen(screen: &mut Screen) -> u64 {
    let previous_offset = screen.scrollback();
    screen.set_scrollback(usize::MAX);
    let len = screen.scrollback();
    debug_assert!(len > 0, "newest scrollback digest requires history");
    screen.set_scrollback(1);

    let size = size_from_screen(screen);
    let mut hasher = DefaultHasher::new();
    hash_scrollback_row(screen, 0, size.cols, &mut hasher);
    let digest = hasher.finish();

    screen.set_scrollback(previous_offset);
    digest
}

fn scrollback_state_from_screen_with_digest_policy(
    screen: &mut Screen,
    capacity: usize,
    force_digest_below_capacity: bool,
) -> ScrollbackState {
    let previous_offset = screen.scrollback();
    screen.set_scrollback(usize::MAX);
    let len = screen.scrollback();
    let digest = scrollback_digest_from_screen(screen, len, capacity, force_digest_below_capacity);
    screen.set_scrollback(previous_offset);

    ScrollbackState { len, digest }
}

fn scrollback_state_after_feed(
    screen: &mut Screen,
    previous: ScrollbackState,
    may_reset_scrollback: bool,
    capacity: usize,
) -> ScrollbackState {
    let previous_offset = screen.scrollback();
    screen.set_scrollback(usize::MAX);
    let len = screen.scrollback();
    let force_digest_below_capacity = may_reset_scrollback && len == previous.len && len < capacity;

    let state = if len == previous.len && len < capacity && !force_digest_below_capacity {
        previous
    } else {
        ScrollbackState {
            len,
            digest: scrollback_digest_from_screen(
                screen,
                len,
                capacity,
                force_digest_below_capacity,
            ),
        }
    };

    screen.set_scrollback(previous_offset);
    state
}

fn feed_may_reset_scrollback(bytes: &[u8], previous_byte_was_escape: bool) -> bool {
    (previous_byte_was_escape && bytes.first().copied() == Some(b'c'))
        || bytes.windows(2).any(|window| window == b"\x1bc")
}

fn scrollback_digest_from_screen(
    screen: &mut Screen,
    total_rows: usize,
    capacity: usize,
    force_digest_below_capacity: bool,
) -> Option<u64> {
    if total_rows < capacity && !force_digest_below_capacity {
        return None;
    }

    if total_rows == 0 {
        return None;
    }

    let size = size_from_screen(screen);
    let page_height = usize::from(size.rows).max(1);
    let mut hasher = DefaultHasher::new();
    let mut remaining = total_rows;

    while remaining > 0 {
        screen.set_scrollback(remaining);
        let visible_scrollback_rows = remaining.min(page_height);

        for row in 0..visible_scrollback_rows {
            hash_scrollback_row(screen, row as u16, size.cols, &mut hasher);
        }

        remaining = remaining.saturating_sub(page_height);
    }

    Some(hasher.finish())
}

fn scrollback_snapshot_from_screen(screen: &Screen) -> ScrollbackSnapshot<'static> {
    let size = size_from_screen(screen);
    let page_height = usize::from(size.rows).max(1);
    let mut screen = screen.clone();
    screen.set_scrollback(usize::MAX);
    let total_rows = screen.scrollback();

    if total_rows == 0 {
        return ScrollbackSnapshot::default();
    }

    let mut lines = Vec::with_capacity(total_rows);
    let mut remaining = total_rows;

    while remaining > 0 {
        screen.set_scrollback(remaining);
        let visible_scrollback_rows = remaining.min(page_height);

        for (row, text) in screen
            .rows(0, size.cols)
            .take(visible_scrollback_rows)
            .enumerate()
        {
            lines.push(scrollback_line_from_screen(
                &screen, row as u16, size.cols, text,
            ));
        }

        remaining = remaining.saturating_sub(page_height);
    }

    ScrollbackSnapshot::new(lines)
}

fn scrollback_line_from_screen(
    screen: &Screen,
    row: u16,
    cols: u16,
    text: String,
) -> ScrollbackLine<'static> {
    ScrollbackLine::from_row(text, scrollback_row_from_screen(screen, row, cols))
}

fn scrollback_row_from_screen(screen: &Screen, row: u16, cols: u16) -> SurfaceRow<'static> {
    let cells = (0..cols)
        .map(|col| surface_cell_from_vt100(screen.cell(row, col)).into_owned())
        .collect();

    SurfaceRow::new(cells).with_wrapped(screen.row_wrapped(row))
}

fn hash_scrollback_row(screen: &Screen, row: u16, cols: u16, hasher: &mut impl Hasher) {
    screen.row_wrapped(row).hash(hasher);
    for col in 0..cols {
        match screen.cell(row, col) {
            Some(cell) => hash_vt100_cell(cell, hasher),
            None => false.hash(hasher),
        }
    }
}

fn hash_vt100_cell(cell: &vt100::Cell, hasher: &mut impl Hasher) {
    true.hash(hasher);
    cell.contents().hash(hasher);
    cell.is_wide().hash(hasher);
    cell.is_wide_continuation().hash(hasher);
    hash_vt100_color(cell.fgcolor(), hasher);
    hash_vt100_color(cell.bgcolor(), hasher);
    cell.bold().hash(hasher);
    cell.dim().hash(hasher);
    cell.italic().hash(hasher);
    cell.underline().hash(hasher);
    cell.inverse().hash(hasher);
}

fn hash_vt100_color(color: Color, hasher: &mut impl Hasher) {
    match color {
        Color::Default => 0u8.hash(hasher),
        Color::Idx(idx) => {
            1u8.hash(hasher);
            idx.hash(hasher);
        }
        Color::Rgb(r, g, b) => {
            2u8.hash(hasher);
            r.hash(hasher);
            g.hash(hasher);
            b.hash(hasher);
        }
    }
}

fn dirty_rows_between(previous: &Screen, current: &Screen) -> DirtyRows {
    if previous.size() != current.size() {
        return DirtyRows::All;
    }

    let (rows, cols) = current.size();
    let mut first = None;
    let mut last_exclusive = 0;

    for row in 0..rows {
        if row_differs(previous, current, row, cols) {
            first.get_or_insert(row);
            last_exclusive = row + 1;
        }
    }

    match first {
        None => DirtyRows::None,
        Some(0) if last_exclusive == rows => DirtyRows::All,
        Some(start) => DirtyRows::Range {
            start,
            end: last_exclusive,
        },
    }
}

fn row_differs(previous: &Screen, current: &Screen, row: u16, cols: u16) -> bool {
    if previous.row_wrapped(row) != current.row_wrapped(row) {
        return true;
    }

    for col in 0..cols {
        if previous.cell(row, col) != current.cell(row, col) {
            return true;
        }
    }

    false
}

#[cfg(test)]
mod tests;
