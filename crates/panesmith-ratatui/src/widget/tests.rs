use super::{CursorRenderMode, TerminalPaneWidget, TerminalViewport};
use panesmith_core::{
    CellAttrs, CellStyle, CellWidth, ColorSpec, CursorPosition, CursorState, OwnedPaneSnapshot,
    OwnedScrollbackSnapshot, PaneId, PaneInteractionMode, PaneState, PaneStats, ScrollbackLine,
    ScrollbackSnapshot, Size, SurfaceCell, SurfaceSnapshot, TerminalModes,
};
use ratatui::{
    backend::TestBackend,
    buffer::{Buffer, Cell},
    layout::Rect,
    style::{Color, Modifier, Style},
    widgets::Widget,
    Terminal,
};

#[test]
fn widget_keeps_track_of_the_target_pane() {
    let snapshot = snapshot(&["pane"]);
    let widget = TerminalPaneWidget::new(&snapshot);

    assert_eq!(widget.pane_id(), PaneId::new(7));
}

#[test]
fn render_draws_basic_ascii_snapshot() {
    let snapshot = snapshot(&["hello", "world"]);
    let expected = Buffer::with_lines(["hello", "world"]);

    assert_render(snapshot, Rect::new(0, 0, 5, 2), &expected);
}

#[test]
fn render_maps_colors_and_common_attributes() {
    let snapshot = styled_snapshot(vec![vec![
        SurfaceCell::new(
            "A",
            CellWidth::Single,
            CellStyle {
                fg: Some(ColorSpec::Indexed(5)),
                bg: Some(ColorSpec::Rgb(1, 2, 3)),
                underline_color: Some(ColorSpec::Rgb(9, 9, 9)),
                attrs: CellAttrs {
                    bold: true,
                    dim: true,
                    italic: true,
                    underlined: true,
                    slow_blink: true,
                    rapid_blink: true,
                    reversed: true,
                    hidden: true,
                    crossed_out: true,
                },
            },
        ),
        SurfaceCell::new(
            "B",
            CellWidth::Single,
            CellStyle {
                fg: Some(ColorSpec::Default),
                bg: Some(ColorSpec::Indexed(10)),
                underline_color: None,
                attrs: CellAttrs {
                    bold: true,
                    ..CellAttrs::default()
                },
            },
        ),
    ]]);

    let mut expected = Buffer::with_lines(["AB"]);
    expected[(0, 0)].set_style(
        Style::new()
            .fg(Color::Indexed(5))
            .bg(Color::Rgb(1, 2, 3))
            .add_modifier(
                Modifier::BOLD
                    | Modifier::DIM
                    | Modifier::ITALIC
                    | Modifier::UNDERLINED
                    | Modifier::SLOW_BLINK
                    | Modifier::RAPID_BLINK
                    | Modifier::REVERSED
                    | Modifier::HIDDEN
                    | Modifier::CROSSED_OUT,
            ),
    );
    expected[(1, 0)].set_style(
        Style::new()
            .bg(Color::Indexed(10))
            .add_modifier(Modifier::BOLD),
    );

    assert_render(snapshot, Rect::new(0, 0, 2, 1), &expected);
}

#[test]
fn render_treats_empty_panes_and_missing_cells_as_spaces() {
    let snapshot = styled_snapshot(vec![
        vec![SurfaceCell::new(
            "h",
            CellWidth::Single,
            CellStyle::default(),
        )],
        vec![],
    ]);
    let widget = TerminalPaneWidget::new(&snapshot);
    let mut buf = Buffer::filled(Rect::new(0, 0, 4, 2), Cell::new("#"));

    widget.render(Rect::new(0, 0, 4, 2), &mut buf);

    let expected = Buffer::with_lines(["h   ", "    "]);
    assert_eq!(buf, expected);
}

#[test]
fn render_clips_to_the_supplied_rect() {
    let snapshot = snapshot(&["abcdef", "ghijkl", "mnopqr"]);
    let mut terminal = Terminal::new(TestBackend::new(8, 4)).unwrap();
    terminal
        .draw(|frame| {
            frame.render_widget(TerminalPaneWidget::new(&snapshot), Rect::new(2, 1, 4, 2));
        })
        .unwrap();

    let expected = Buffer::with_lines(["        ", "  ghij  ", "  mnop  ", "        "]);
    terminal.backend().assert_buffer(&expected);
}

#[test]
fn render_keeps_double_width_cells_inside_bounds() {
    let snapshot = styled_snapshot(vec![vec![
        SurfaceCell::new("界", CellWidth::Double, CellStyle::default()),
        SurfaceCell::new("", CellWidth::Continuation, CellStyle::default()),
        SurfaceCell::new("!", CellWidth::Single, CellStyle::default()),
    ]]);
    let mut expected = Buffer::with_lines(["   "]);
    expected[(0, 0)].set_symbol("界");
    expected[(2, 0)].set_symbol("!");

    assert_render(snapshot, Rect::new(0, 0, 3, 1), &expected);
}

#[test]
fn render_replaces_right_edge_double_width_cells_without_spilling() {
    let snapshot = styled_snapshot(vec![vec![
        SurfaceCell::new("界", CellWidth::Double, CellStyle::default()),
        SurfaceCell::new("", CellWidth::Continuation, CellStyle::default()),
    ]]);
    let widget = TerminalPaneWidget::new(&snapshot);
    let mut buf = Buffer::filled(Rect::new(0, 0, 3, 1), Cell::new("#"));

    widget.render(Rect::new(1, 0, 1, 1), &mut buf);

    let expected = Buffer::with_lines(["# #"]);
    assert_eq!(buf, expected);
}

#[test]
fn render_shows_the_cursor_for_focused_panes() {
    let snapshot = snapshot_with_cursor(
        &["ab"],
        CursorState::new(Some(CursorPosition::new(0, 1)), true),
    );
    let mut expected = Buffer::with_lines(["ab"]);
    expected[(1, 0)].set_style(Style::new().add_modifier(Modifier::REVERSED));

    assert_render(snapshot, Rect::new(0, 0, 2, 1), &expected);
}

#[test]
fn render_toggles_reverse_video_for_cursor_on_reversed_cells() {
    let reversed_style = CellStyle {
        attrs: CellAttrs {
            reversed: true,
            ..CellAttrs::default()
        },
        ..CellStyle::default()
    };
    let snapshot = styled_snapshot_with_cursor(
        vec![vec![
            SurfaceCell::new("A", CellWidth::Single, reversed_style),
            SurfaceCell::new("B", CellWidth::Single, reversed_style),
        ]],
        CursorState::new(Some(CursorPosition::new(0, 0)), true),
    );
    let mut expected = Buffer::with_lines(["AB"]);
    expected[(1, 0)].set_style(Style::new().add_modifier(Modifier::REVERSED));

    assert_render(snapshot, Rect::new(0, 0, 2, 1), &expected);
}

#[test]
fn render_hides_the_cursor_for_unfocused_panes() {
    let snapshot = snapshot_with_cursor(
        &["ab"],
        CursorState::new(Some(CursorPosition::new(0, 1)), true),
    );
    let expected = Buffer::with_lines(["ab"]);

    assert_render_widget(
        TerminalPaneWidget::new(&snapshot).focused(false),
        Rect::new(0, 0, 2, 1),
        &expected,
    );
}

#[test]
fn render_hides_the_cursor_when_mode_is_hidden_even_if_focused() {
    let snapshot = snapshot_with_cursor(
        &["ab"],
        CursorState::new(Some(CursorPosition::new(0, 1)), true),
    );
    let expected = Buffer::with_lines(["ab"]);

    assert_render_widget(
        TerminalPaneWidget::new(&snapshot).with_cursor_render_mode(CursorRenderMode::Hidden),
        Rect::new(0, 0, 2, 1),
        &expected,
    );
}

#[test]
fn render_places_cursor_on_the_leading_cell_for_wide_graphemes() {
    let snapshot = styled_snapshot_with_cursor(
        vec![vec![
            SurfaceCell::new("界", CellWidth::Double, CellStyle::default()),
            SurfaceCell::new("", CellWidth::Continuation, CellStyle::default()),
            SurfaceCell::new("!", CellWidth::Single, CellStyle::default()),
        ]],
        CursorState::new(Some(CursorPosition::new(0, 1)), true),
    );
    let mut expected = Buffer::with_lines(["   "]);
    expected[(0, 0)]
        .set_symbol("界")
        .set_style(Style::new().add_modifier(Modifier::REVERSED));
    expected[(2, 0)].set_symbol("!");

    assert_render(snapshot, Rect::new(0, 0, 3, 1), &expected);
}

#[test]
fn render_uses_scrollback_offset_for_viewports() {
    let snapshot = snapshot(&["live-1", "live-2"]);
    let scrollback = scrollback(&["old-1", "old-2", "old-3"]);
    let viewport = TerminalViewport::scrolled(1);
    let metrics = viewport.metrics(&snapshot, Some(&scrollback), 2);
    let expected = Buffer::with_lines(["old-3 ", "live-1"]);

    assert_eq!(metrics.total_rows, 5);
    assert_eq!(metrics.visible_rows, 2);
    assert_eq!(metrics.max_scroll_offset, 3);
    assert_eq!(metrics.effective_scroll_offset, 1);
    assert_eq!(metrics.start_row, 2);
    assert_eq!(metrics.end_row, 4);
    assert_render_widget(
        TerminalPaneWidget::new(&snapshot)
            .with_scrollback(&scrollback)
            .with_viewport(viewport),
        Rect::new(0, 0, 6, 2),
        &expected,
    );
}

#[test]
fn viewport_metrics_clamp_when_viewport_is_taller_than_content() {
    let snapshot = snapshot(&["live-1", "live-2"]);
    let viewport = TerminalViewport::scrolled(99);

    let metrics = viewport.metrics(&snapshot, None, 5);

    assert_eq!(metrics.total_rows, 2);
    assert_eq!(metrics.visible_rows, 5);
    assert_eq!(metrics.max_scroll_offset, 0);
    assert_eq!(metrics.effective_scroll_offset, 0);
    assert_eq!(metrics.start_row, 0);
    assert_eq!(metrics.end_row, 2);
    assert!(viewport.is_at_tail(metrics));
    assert_eq!(
        TerminalViewport::default().scroll_up(3, metrics),
        TerminalViewport::default()
    );
}

#[test]
fn viewport_scroll_up_down_clamps_with_scrollback_and_live_rows() {
    let snapshot = snapshot(&["live-1", "live-2"]);
    let scrollback = scrollback(&["old-1", "old-2", "old-3"]);
    let initial_metrics = TerminalViewport::default().metrics(&snapshot, Some(&scrollback), 2);

    let viewport = TerminalViewport::default().scroll_up(2, initial_metrics);
    assert_eq!(viewport, TerminalViewport::scrolled(2));

    let metrics = viewport.metrics(&snapshot, Some(&scrollback), 2);
    assert_eq!(metrics.effective_scroll_offset, 2);
    assert_eq!(
        viewport.scroll_up(99, metrics),
        TerminalViewport::scrolled(3)
    );

    let metrics = TerminalViewport::scrolled(3).metrics(&snapshot, Some(&scrollback), 2);
    assert_eq!(
        TerminalViewport::scrolled(3).scroll_down(1, metrics),
        TerminalViewport::scrolled(2)
    );
    assert_eq!(
        TerminalViewport::scrolled(3).scroll_down(99, metrics),
        TerminalViewport::default()
    );
}

#[test]
fn viewport_page_up_down_uses_visible_height() {
    let snapshot = snapshot(&["live-1", "live-2"]);
    let scrollback = scrollback(&["old-1", "old-2", "old-3", "old-4", "old-5", "old-6"]);
    let metrics = TerminalViewport::default().metrics(&snapshot, Some(&scrollback), 3);

    let viewport = TerminalViewport::default().page_up(metrics);
    assert_eq!(viewport, TerminalViewport::scrolled(3));

    let metrics = viewport.metrics(&snapshot, Some(&scrollback), 3);
    assert_eq!(viewport.page_down(metrics), TerminalViewport::default());
}

#[test]
fn viewport_follow_tail_ignores_stale_scroll_offset() {
    let snapshot = snapshot(&["live-1", "live-2"]);
    let scrollback = scrollback(&["old-1", "old-2", "old-3"]);
    let stale_tail = TerminalViewport {
        scroll_offset: 99,
        follow_tail: true,
    };

    let metrics = stale_tail.metrics(&snapshot, Some(&scrollback), 2);

    assert_eq!(metrics.effective_scroll_offset, 0);
    assert_eq!(metrics.start_row, 3);
    assert_eq!(metrics.end_row, 5);
    assert!(metrics.is_at_tail());
    assert_eq!(stale_tail.clamp(metrics), TerminalViewport::default());
}

#[test]
fn render_scrollback_rows_with_cell_styles_without_cursor() {
    let snapshot = snapshot(&["live-1", "live-2"]);
    let style = CellStyle {
        fg: Some(ColorSpec::Indexed(2)),
        bg: Some(ColorSpec::Indexed(5)),
        attrs: CellAttrs {
            bold: true,
            italic: true,
            ..CellAttrs::default()
        },
        ..CellStyle::default()
    };
    let scrollback = ScrollbackSnapshot::new(vec![ScrollbackLine::from_row(
        "old",
        panesmith_core::SurfaceRow::new(vec![
            SurfaceCell::new("o", CellWidth::Single, style),
            SurfaceCell::new("l", CellWidth::Single, style),
            SurfaceCell::new("d", CellWidth::Single, style),
        ]),
    )])
    .into_owned();
    let mut expected = Buffer::with_lines(["old   ", "live-1"]);
    let expected_style = Style::new()
        .fg(Color::Indexed(2))
        .bg(Color::Indexed(5))
        .add_modifier(Modifier::BOLD | Modifier::ITALIC);
    expected[(0, 0)].set_style(expected_style);
    expected[(1, 0)].set_style(expected_style);
    expected[(2, 0)].set_style(expected_style);

    assert_render_widget(
        TerminalPaneWidget::new(&snapshot)
            .with_scrollback(&scrollback)
            .with_viewport(TerminalViewport {
                scroll_offset: 1,
                follow_tail: false,
            }),
        Rect::new(0, 0, 6, 2),
        &expected,
    );
}

#[test]
fn render_scrollback_view_does_not_project_cursor_onto_history() {
    let snapshot = snapshot_with_cursor(
        &["live"],
        CursorState::new(Some(CursorPosition::new(0, 0)), true),
    );
    let scrollback_style = CellStyle {
        fg: Some(ColorSpec::Indexed(3)),
        ..CellStyle::default()
    };
    let scrollback = ScrollbackSnapshot::new(vec![ScrollbackLine::from_row(
        "old",
        panesmith_core::SurfaceRow::new(vec![
            SurfaceCell::new("o", CellWidth::Single, scrollback_style),
            SurfaceCell::new("l", CellWidth::Single, scrollback_style),
            SurfaceCell::new("d", CellWidth::Single, scrollback_style),
        ]),
    )])
    .into_owned();
    let mut expected = Buffer::with_lines(["old "]);
    expected[(0, 0)].set_style(Style::new().fg(Color::Indexed(3)));
    expected[(1, 0)].set_style(Style::new().fg(Color::Indexed(3)));
    expected[(2, 0)].set_style(Style::new().fg(Color::Indexed(3)));

    assert_render_widget(
        TerminalPaneWidget::new(&snapshot)
            .with_scrollback(&scrollback)
            .with_viewport(TerminalViewport {
                scroll_offset: 1,
                follow_tail: false,
            }),
        Rect::new(0, 0, 4, 1),
        &expected,
    );
}

#[test]
fn render_handles_empty_areas_without_panic() {
    let snapshot = snapshot(&["data"]);
    let widget = TerminalPaneWidget::new(&snapshot);
    let mut buf = Buffer::filled(Rect::new(0, 0, 4, 2), Cell::new("x"));
    let expected = buf.clone();
    let empty_metrics = TerminalViewport::scrolled(1).metrics(&snapshot, None, 0);

    widget.render(Rect::new(0, 0, 0, 2), &mut buf);
    widget.render(Rect::new(0, 0, 4, 0), &mut buf);

    assert_eq!(empty_metrics.visible_rows, 0);
    assert_eq!(empty_metrics.start_row, empty_metrics.end_row);
    assert_eq!(buf, expected);
}

fn assert_render(snapshot: OwnedPaneSnapshot, area: Rect, expected: &Buffer) {
    assert_render_widget(TerminalPaneWidget::new(&snapshot), area, expected);
}

fn assert_render_widget(widget: TerminalPaneWidget<'_>, area: Rect, expected: &Buffer) {
    let mut terminal =
        Terminal::new(TestBackend::new(expected.area.width, expected.area.height)).unwrap();
    terminal
        .draw(|frame| frame.render_widget(widget, area))
        .unwrap();
    terminal.backend().assert_buffer(expected);
}

fn snapshot(rows: &[&str]) -> OwnedPaneSnapshot {
    snapshot_with_cursor(rows, CursorState::hidden())
}

fn snapshot_with_cursor(rows: &[&str], cursor: CursorState) -> OwnedPaneSnapshot {
    styled_snapshot_with_cursor(
        rows.iter()
            .map(|row| {
                row.chars()
                    .map(|ch| {
                        SurfaceCell::new(ch.to_string(), CellWidth::Single, CellStyle::default())
                    })
                    .collect()
            })
            .collect(),
        cursor,
    )
}

fn styled_snapshot(rows: Vec<Vec<SurfaceCell<'static>>>) -> OwnedPaneSnapshot {
    styled_snapshot_with_cursor(rows, CursorState::hidden())
}

fn styled_snapshot_with_cursor(
    rows: Vec<Vec<SurfaceCell<'static>>>,
    cursor: CursorState,
) -> OwnedPaneSnapshot {
    let row_count = rows.len() as u16;
    let col_count = rows.iter().map(Vec::len).max().unwrap_or_default() as u16;
    let rows = rows
        .into_iter()
        .map(|row| panesmith_core::SurfaceRow::new(row.into_iter().collect()))
        .collect::<Vec<_>>();
    let size = Size::new(row_count.max(1), col_count.max(1));

    OwnedPaneSnapshot {
        id: PaneId::new(7),
        title: Some("pane".into()),
        state: PaneState::Running,
        interaction_mode: PaneInteractionMode::Embedded,
        size,
        surface: SurfaceSnapshot::new(size, rows, cursor, TerminalModes::default(), None),
        cursor,
        modes: TerminalModes::default(),
        stats: PaneStats::default(),
    }
}

fn scrollback(lines: &[&str]) -> OwnedScrollbackSnapshot {
    ScrollbackSnapshot::new(lines.iter().copied().map(ScrollbackLine::new).collect()).into_owned()
}
