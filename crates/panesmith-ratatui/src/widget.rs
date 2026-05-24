//! Ratatui widget types for rendering Panesmith pane snapshots.

use panesmith_core::{
    CellAttrs, CellStyle, CellWidth, ColorSpec, PaneId, PaneSnapshot, ScrollbackLine,
    ScrollbackSnapshot, SurfaceCell, SurfaceRow, TerminalViewport,
};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    widgets::Widget,
};

/// How the widget should render the child cursor when it is visible.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CursorRenderMode {
    /// Do not render the child cursor.
    Hidden,
    /// Invert the target cell to represent the child cursor.
    #[default]
    InvertCell,
}

/// Renders a pane snapshot into a ratatui buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalPaneWidget<'a> {
    snapshot: &'a PaneSnapshot<'a>,
    scrollback: Option<&'a ScrollbackSnapshot<'a>>,
    viewport: TerminalViewport,
    focused: bool,
    cursor_render_mode: CursorRenderMode,
}

impl<'a> TerminalPaneWidget<'a> {
    /// Creates a widget for the provided pane snapshot.
    pub fn new(snapshot: &'a PaneSnapshot<'a>) -> Self {
        Self {
            snapshot,
            scrollback: None,
            viewport: TerminalViewport::default(),
            focused: true,
            cursor_render_mode: CursorRenderMode::default(),
        }
    }

    /// Returns the pane identifier associated with this widget.
    pub fn pane_id(self) -> PaneId {
        self.snapshot.id
    }

    /// Supplies optional scrollback content for viewport rendering.
    pub fn with_scrollback(mut self, scrollback: &'a ScrollbackSnapshot<'a>) -> Self {
        self.scrollback = Some(scrollback);
        self
    }

    /// Configures the viewport used when rendering the pane.
    pub const fn with_viewport(mut self, viewport: TerminalViewport) -> Self {
        self.viewport = viewport;
        self
    }

    /// Marks whether this pane is focused for cursor rendering purposes.
    pub const fn focused(mut self, focused: bool) -> Self {
        self.focused = focused;
        self
    }

    /// Overrides how the child cursor is rendered.
    pub const fn with_cursor_render_mode(mut self, mode: CursorRenderMode) -> Self {
        self.cursor_render_mode = mode;
        self
    }
}

impl Widget for TerminalPaneWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let area = area.intersection(buf.area);
        if area.is_empty() {
            return;
        }

        clear_area(area, buf);

        let surface = &self.snapshot.surface;
        let max_cols = usize::from(area.width.min(surface.size.cols));
        let surface_row_count = usize::from(surface.size.rows);
        let scrollback_lines = self
            .scrollback
            .map_or(&[][..], |scrollback| scrollback.lines.as_slice());
        let visible_rows = usize::from(area.height);
        let metrics = self.viewport.metrics_from_counts(
            surface_row_count,
            scrollback_lines.len(),
            visible_rows,
        );
        let cursor_target = cursor_target(
            self,
            &area,
            scrollback_lines.len(),
            metrics.start_row,
            metrics.end_row,
            max_cols,
        );

        for (render_row_idx, logical_row_idx) in (metrics.start_row..metrics.end_row).enumerate() {
            let y = area.y + render_row_idx as u16;

            if let Some(line) = scrollback_lines.get(logical_row_idx) {
                render_scrollback_line(&area, buf, y, max_cols, line);
                continue;
            }

            let surface_row_idx = logical_row_idx.saturating_sub(scrollback_lines.len());
            render_surface_row(
                &area,
                buf,
                y,
                max_cols,
                surface.rows.get(surface_row_idx),
                cursor_target,
                self.cursor_render_mode,
            );
        }
    }
}

fn clear_area(area: Rect, buf: &mut Buffer) {
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            buf[(x, y)].reset();
        }
    }
}

fn render_scrollback_line(
    area: &Rect,
    buf: &mut Buffer,
    y: u16,
    max_cols: usize,
    line: &ScrollbackLine<'_>,
) {
    if line.row.cells.is_empty() {
        buf.set_stringn(
            area.x,
            y,
            line.text.as_ref(),
            usize::from(area.width),
            Style::default(),
        );
        return;
    }

    render_surface_row(
        area,
        buf,
        y,
        max_cols,
        Some(&line.row),
        None,
        CursorRenderMode::Hidden,
    );
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CursorTarget {
    x: u16,
    y: u16,
}

fn cursor_target(
    widget: TerminalPaneWidget<'_>,
    area: &Rect,
    scrollback_rows: usize,
    start_row: usize,
    end_row: usize,
    max_cols: usize,
) -> Option<CursorTarget> {
    if !widget.focused || matches!(widget.cursor_render_mode, CursorRenderMode::Hidden) {
        return None;
    }

    let cursor = widget.snapshot.cursor;
    if !cursor.visible {
        return None;
    }

    let position = cursor.position?;
    let surface_row_idx = usize::from(position.row);
    if surface_row_idx >= usize::from(widget.snapshot.surface.size.rows) {
        return None;
    }

    let surface_row = widget.snapshot.surface.rows.get(surface_row_idx);
    let surface_col_idx = resolve_cursor_col(surface_row, usize::from(position.col));
    if surface_col_idx >= max_cols {
        return None;
    }

    let logical_row_idx = scrollback_rows + surface_row_idx;
    if logical_row_idx < start_row || logical_row_idx >= end_row {
        return None;
    }

    Some(CursorTarget {
        x: area.x + surface_col_idx as u16,
        y: area.y + (logical_row_idx - start_row) as u16,
    })
}

fn resolve_cursor_col(row: Option<&SurfaceRow<'_>>, col: usize) -> usize {
    if matches!(
        row.and_then(|row| row.cells.get(col))
            .map(|cell| cell.width),
        Some(CellWidth::Continuation)
    ) {
        col.saturating_sub(1)
    } else {
        col
    }
}

fn render_surface_row(
    area: &Rect,
    buf: &mut Buffer,
    y: u16,
    max_cols: usize,
    row: Option<&SurfaceRow<'_>>,
    cursor_target: Option<CursorTarget>,
    cursor_render_mode: CursorRenderMode,
) {
    let mut cursor_rendered = false;

    if let Some(row) = row {
        for (col_idx, cell) in row.cells.iter().take(max_cols).enumerate() {
            let x = area.x + col_idx as u16;
            let render_cursor = cursor_target == Some(CursorTarget { x, y });
            render_cell(
                area,
                buf,
                CellRenderTarget {
                    x,
                    y,
                    at_right_edge: col_idx + 1 == max_cols,
                    render_cursor,
                },
                cell,
                cursor_render_mode,
            );
            cursor_rendered |= render_cursor;
        }
    }

    if !cursor_rendered {
        if let Some(target) = cursor_target {
            if target.y == y {
                buf[(target.x, target.y)]
                    .set_style(cursor_style(Style::default(), cursor_render_mode));
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CellRenderTarget {
    x: u16,
    y: u16,
    at_right_edge: bool,
    render_cursor: bool,
}

fn render_cell(
    area: &Rect,
    buf: &mut Buffer,
    target_pos: CellRenderTarget,
    cell: &SurfaceCell<'_>,
    cursor_render_mode: CursorRenderMode,
) {
    let base_style = map_style(cell.style);
    let style = if target_pos.render_cursor {
        cursor_style(base_style, cursor_render_mode)
    } else {
        base_style
    };
    let target = &mut buf[(target_pos.x, target_pos.y)];

    match cell.width {
        CellWidth::Continuation => {}
        CellWidth::Double if target_pos.at_right_edge => {
            target.set_char(' ').set_style(style);
        }
        CellWidth::Single | CellWidth::Double => {
            let symbol = if cell.text.is_empty() {
                " "
            } else {
                cell.text.as_ref()
            };
            target.set_symbol(symbol).set_style(style);

            if matches!(cell.width, CellWidth::Double) {
                let continuation_x = target_pos.x.saturating_add(1);
                if continuation_x < area.right() {
                    buf[(continuation_x, target_pos.y)].reset();
                    buf[(continuation_x, target_pos.y)].set_style(style);
                }
            }
        }
    }
}

fn cursor_style(style: Style, cursor_render_mode: CursorRenderMode) -> Style {
    match cursor_render_mode {
        CursorRenderMode::Hidden => style,
        CursorRenderMode::InvertCell => {
            if style.add_modifier.contains(Modifier::REVERSED)
                && !style.sub_modifier.contains(Modifier::REVERSED)
            {
                style.remove_modifier(Modifier::REVERSED)
            } else {
                style.add_modifier(Modifier::REVERSED)
            }
        }
    }
}

fn map_style(style: CellStyle) -> Style {
    let mut mapped = Style::new();

    if let Some(fg) = map_color(style.fg) {
        mapped = mapped.fg(fg);
    }
    if let Some(bg) = map_color(style.bg) {
        mapped = mapped.bg(bg);
    }

    let modifiers = map_modifiers(style.attrs);
    if !modifiers.is_empty() {
        mapped = mapped.add_modifier(modifiers);
    }

    mapped
}

fn map_color(color: Option<ColorSpec>) -> Option<Color> {
    match color.unwrap_or(ColorSpec::Default) {
        ColorSpec::Default => None,
        ColorSpec::Indexed(index) => Some(Color::Indexed(index)),
        ColorSpec::Rgb(r, g, b) => Some(Color::Rgb(r, g, b)),
    }
}

fn map_modifiers(attrs: CellAttrs) -> Modifier {
    let mut modifiers = Modifier::empty();

    if attrs.bold {
        modifiers |= Modifier::BOLD;
    }
    if attrs.dim {
        modifiers |= Modifier::DIM;
    }
    if attrs.italic {
        modifiers |= Modifier::ITALIC;
    }
    if attrs.underlined {
        modifiers |= Modifier::UNDERLINED;
    }
    if attrs.slow_blink {
        modifiers |= Modifier::SLOW_BLINK;
    }
    if attrs.rapid_blink {
        modifiers |= Modifier::RAPID_BLINK;
    }
    if attrs.reversed {
        modifiers |= Modifier::REVERSED;
    }
    if attrs.hidden {
        modifiers |= Modifier::HIDDEN;
    }
    if attrs.crossed_out {
        modifiers |= Modifier::CROSSED_OUT;
    }

    modifiers
}

#[cfg(test)]
mod tests;
