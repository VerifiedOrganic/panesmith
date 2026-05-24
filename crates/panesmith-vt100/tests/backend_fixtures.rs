use std::fmt::Write as _;
use std::fs;
use std::path::PathBuf;

use panesmith_core::{
    CellAttrs, CellStyle, CellWidth, ColorSpec, CursorPosition, DirtyRows, MouseMode, PaneError,
    PaneId, ScrollbackConfig, Size, SurfaceBackend, SurfaceRow, SurfaceSnapshot,
};
use panesmith_vt100::Vt100Backend;

#[test]
fn colors_fixture_matches_snapshot() {
    assert_fixture("colors", Size::new(2, 20));
}

#[test]
fn cursor_moves_fixture_matches_snapshot() {
    assert_fixture("cursor-moves", Size::new(3, 12));
}

#[test]
fn clear_line_fixture_matches_snapshot() {
    assert_fixture("clear-line", Size::new(2, 16));
}

#[test]
fn clear_screen_fixture_matches_snapshot() {
    assert_fixture("clear-screen", Size::new(3, 12));
}

#[test]
fn wrapped_lines_fixture_matches_snapshot() {
    assert_fixture("wrapped-lines", Size::new(3, 5));
}

#[test]
fn alternate_screen_fixture_matches_snapshot() {
    assert_fixture("alternate-screen", Size::new(3, 12));
}

#[test]
fn backend_resize_updates_snapshot_size_and_preserves_visible_content() {
    let mut backend = Vt100Backend::new(PaneId::new(7), Size::new(2, 10)).unwrap();
    backend
        .feed(b"hello\r\nworld")
        .expect("fixture should parse");

    backend
        .resize(Size::new(3, 10))
        .expect("resize should succeed");

    let snapshot = backend.snapshot().to_owned_snapshot();
    assert_eq!(snapshot.size, Size::new(3, 10));
    assert_eq!(snapshot.rows.len(), 3);
    assert_eq!(row_text(&snapshot.rows[0]), "hello     ");
    assert_eq!(row_text(&snapshot.rows[1]), "world     ");
    assert_eq!(row_text(&snapshot.rows[2]), "          ");
}

#[test]
fn backend_reports_title_modes_and_cursor_visibility() {
    let mut backend = Vt100Backend::new(PaneId::new(8), Size::new(3, 20)).unwrap();

    let update = backend
        .feed(b"\x1b]2;Pane\x07\x1b[?1004;2004h\x1b[?1003;1006h\x1b[?1h\x1b[?25l")
        .expect("control sequences should parse");

    let snapshot = backend.snapshot().to_owned_snapshot();
    assert!(update.title_changed);
    assert!(update.modes_changed);
    assert!(update.cursor_changed);
    assert_eq!(snapshot.title.as_deref(), Some("Pane"));
    assert_eq!(snapshot.cursor.position, Some(CursorPosition::new(0, 0)));
    assert!(!snapshot.cursor.visible);
    assert!(snapshot.modes.bracketed_paste);
    assert!(snapshot.modes.focus_events);
    assert_eq!(snapshot.modes.mouse, MouseMode::Sgr);
    assert!(snapshot.modes.application_cursor);
}

#[test]
fn backend_terminal_reset_clears_overlay_tracked_modes() {
    let mut backend = Vt100Backend::new(PaneId::new(28), Size::new(2, 10)).unwrap();

    backend
        .feed(b"\x1b[?1004;2004;1000;1006h")
        .expect("mode enable sequence should parse");
    let enabled = backend.snapshot().to_owned_snapshot();
    assert!(enabled.modes.focus_events);
    assert!(enabled.modes.bracketed_paste);
    assert_eq!(enabled.modes.mouse, MouseMode::Sgr);

    let update = backend
        .feed(b"\x1bcplain")
        .expect("terminal reset should parse");
    let reset = backend.snapshot().to_owned_snapshot();

    assert!(update.modes_changed);
    assert!(!reset.modes.focus_events);
    assert!(!reset.modes.bracketed_paste);
    assert_eq!(reset.modes.mouse, MouseMode::None);
    assert_eq!(row_text(&reset.rows[0]), "plain     ");
}

#[test]
fn backend_reports_active_alternate_screen_mode() {
    let mut backend = Vt100Backend::new(PaneId::new(9), Size::new(2, 10)).unwrap();

    let update = backend
        .feed(b"main\x1b[?1049halt")
        .expect("alternate screen should parse");

    let snapshot = backend.snapshot().to_owned_snapshot();
    assert!(update.modes_changed);
    assert!(snapshot.modes.alternate_screen);
    assert_eq!(row_text(&snapshot.rows[0]), "alt       ");
}

#[test]
fn backend_exposes_scrollback_and_marks_scrollback_updates() {
    let mut backend = Vt100Backend::new(PaneId::new(10), Size::new(2, 5)).unwrap();

    let update = backend
        .feed(b"one\r\ntwo\r\nthree")
        .expect("fixture should parse");

    let scrollback = backend.scrollback().to_owned_snapshot();
    assert!(update.scrollback_changed);
    assert_eq!(scrollback.lines.len(), 1);
    assert_eq!(scrollback.lines[0].text.as_ref(), "one");
    assert_eq!(
        row_text(&backend.snapshot().to_owned_snapshot().rows[0]),
        "two  "
    );
    assert_eq!(
        row_text(&backend.snapshot().to_owned_snapshot().rows[1]),
        "three"
    );
}

#[test]
fn backend_scrollback_preserves_styled_ansi_output() {
    let mut backend = Vt100Backend::new(PaneId::new(11), Size::new(2, 6)).unwrap();

    backend
        .feed(b"\x1b[32;45;3mgreen\x1b[0m\r\nplain\r\n")
        .expect("ansi output should parse");

    let scrollback = backend.scrollback().to_owned_snapshot();
    let line = scrollback
        .lines
        .iter()
        .find(|line| line.text.as_ref() == "green")
        .expect("styled line should be retained in scrollback");

    assert_eq!(line.row.cells[0].text.as_ref(), "g");
    assert_eq!(line.row.cells[0].style.fg, Some(ColorSpec::Indexed(2)));
    assert_eq!(line.row.cells[0].style.bg, Some(ColorSpec::Indexed(5)));
    assert!(line.row.cells[0].style.attrs.italic);
}

#[test]
fn backend_marks_scrollback_changed_when_full_buffer_evicts_history() {
    let mut backend = Vt100Backend::new_with_scrollback(
        PaneId::new(15),
        Size::new(2, 5),
        ScrollbackConfig::bounded_lines(10_000).unwrap(),
    )
    .unwrap();
    let seed = format!("{}bbbbb", "aaaaa\n".repeat(10_001));

    backend
        .feed(seed.as_bytes())
        .expect("seed content should parse");
    let before = backend.scrollback().to_owned_snapshot();
    assert_eq!(before.lines.len(), 10_000);

    let update = backend
        .feed(b"ccccc\n")
        .expect("overflowing content should parse");
    let after = backend.scrollback().to_owned_snapshot();

    assert!(update.scrollback_changed);
    assert_eq!(update.scrollback_lines_dropped, 2);
    assert_eq!(after.lines.len(), 10_000);
    assert_eq!(
        row_text(&backend.snapshot().to_owned_snapshot().rows[0]),
        "ccccc"
    );
}

#[test]
fn backend_records_autowrap_scrollback_without_trailing_newline() {
    let mut backend = Vt100Backend::new(PaneId::new(18), Size::new(2, 5)).unwrap();

    let update = backend
        .feed(b"abcdefghijk")
        .expect("autowrap content should parse");

    let scrollback = backend.scrollback().to_owned_snapshot();
    let snapshot = backend.snapshot().to_owned_snapshot();
    assert!(update.scrollback_changed);
    assert_eq!(scrollback.lines.len(), 1);
    assert_eq!(scrollback.lines[0].text.as_ref(), "abcde");
    assert_eq!(row_text(&snapshot.rows[0]), "fghij");
    assert_eq!(row_text(&snapshot.rows[1]), "k    ");
}

#[test]
fn backend_records_autowrap_scrollback_with_trailing_newline() {
    let mut backend = Vt100Backend::new(PaneId::new(19), Size::new(2, 5)).unwrap();

    let update = backend
        .feed(b"abcdefghijk\n")
        .expect("autowrap content should parse");

    let scrollback = backend.scrollback().to_owned_snapshot();
    let snapshot = backend.snapshot().to_owned_snapshot();
    assert!(update.scrollback_changed);
    assert_eq!(scrollback.lines.len(), 2);
    assert_eq!(scrollback.lines[0].text.as_ref(), "abcde");
    assert_eq!(scrollback.lines[1].text.as_ref(), "fghij");
    assert_eq!(row_text(&snapshot.rows[0]), "k    ");
    assert_eq!(row_text(&snapshot.rows[1]), "     ");
}

#[test]
fn backend_clear_screen_does_not_report_scrollback_change() {
    let mut backend = Vt100Backend::new(PaneId::new(22), Size::new(2, 5)).unwrap();
    backend
        .feed(b"one\r\ntwo\r\nthree")
        .expect("fixture should parse");
    let before = backend.scrollback().to_owned_snapshot();

    let update = backend
        .feed(b"\x1b[2J\x1b[H")
        .expect("clear-screen control sequence should parse");
    let after = backend.scrollback().to_owned_snapshot();

    assert_eq!(update.dirty_rows, DirtyRows::All);
    assert!(!update.scrollback_changed);
    assert_eq!(after, before);
}

#[test]
fn backend_detects_scrollback_change_after_terminal_reset_refill() {
    let mut backend = Vt100Backend::new(PaneId::new(24), Size::new(2, 5)).unwrap();
    backend.feed(b"olddd\n12345").expect("initial feed");

    let update = backend
        .feed(b"\x1bcnewww\nxxxxx")
        .expect("reset and refill");

    let scrollback = backend.scrollback().to_owned_snapshot();
    assert!(
        update.scrollback_changed,
        "ESC c reset followed by refill to same scrollback length must detect the change"
    );
    assert_eq!(scrollback.lines.len(), 1);
    assert_eq!(scrollback.lines[0].text.as_ref(), "newww");
}

#[test]
fn backend_detects_scrollback_change_when_reset_refill_keeps_newest_history_line() {
    let mut backend = Vt100Backend::new(PaneId::new(26), Size::new(2, 5)).unwrap();
    backend
        .feed(b"11111\n22222\n33333")
        .expect("seed content should parse");

    let before = backend.scrollback().to_owned_snapshot();
    assert_eq!(before.lines.len(), 3);
    assert_eq!(before.lines[0].text.as_ref(), "11111");
    assert_eq!(before.lines[1].text.as_ref(), "");
    assert_eq!(before.lines[2].text.as_ref(), "22222");

    let update = backend
        .feed(b"\x1bcAAAAA\n22222\n33333")
        .expect("reset content should parse");

    let after = backend.scrollback().to_owned_snapshot();
    assert!(
        update.scrollback_changed,
        "reset+refill that preserves the newest retained line must still report scrollback changes"
    );
    assert_eq!(after.lines.len(), 3);
    assert_eq!(after.lines[0].text.as_ref(), "AAAAA");
    assert_eq!(after.lines[1].text.as_ref(), "");
    assert_eq!(after.lines[2].text.as_ref(), "22222");
    assert_ne!(after, before);
}

#[test]
fn backend_marks_scrollback_changed_when_reset_sequence_spans_feed_boundary() {
    let mut backend = Vt100Backend::new(PaneId::new(25), Size::new(2, 5)).unwrap();
    backend
        .feed(b"olddd\n12345\x1b")
        .expect("seed content should parse");

    let update = backend
        .feed(b"cnewww\nxxxxx")
        .expect("reset content should parse");

    let scrollback = backend.scrollback().to_owned_snapshot();
    assert!(update.scrollback_changed);
    assert_eq!(scrollback.lines.len(), 1);
    assert_eq!(scrollback.lines[0].text.as_ref(), "newww");
}

#[test]
fn backend_marks_scrollback_changed_when_resize_splits_reset_sequence() {
    let mut backend = Vt100Backend::new(PaneId::new(27), Size::new(2, 5)).unwrap();
    backend
        .feed(b"olddd\n12345\x1b")
        .expect("seed content should parse");
    backend
        .resize(Size::new(2, 5))
        .expect("resize should preserve pending ESC state");

    let update = backend
        .feed(b"cnewww\nxxxxx")
        .expect("reset content should parse");

    let scrollback = backend.scrollback().to_owned_snapshot();
    assert!(update.scrollback_changed);
    assert_eq!(scrollback.lines.len(), 1);
    assert_eq!(scrollback.lines[0].text.as_ref(), "newww");
}

#[test]
fn backend_marks_scrollback_changed_when_full_scrollback_rotates_blank_screen() {
    let mut backend = Vt100Backend::new_with_scrollback(
        PaneId::new(23),
        Size::new(2, 5),
        ScrollbackConfig::bounded_lines(10_000).unwrap(),
    )
    .unwrap();
    let seed = format!("{}bbbbb", "aaaaa\n".repeat(10_001));

    backend
        .feed(seed.as_bytes())
        .expect("seed content should parse");
    backend
        .feed(b"\x1b[2J\x1b[2;1H")
        .expect("clear-screen control sequence should parse");

    let before_snapshot = backend.snapshot().to_owned_snapshot();
    let before_scrollback = backend.scrollback().to_owned_snapshot();
    assert_eq!(before_scrollback.lines.len(), 10_000);

    let update = backend.feed(b"\n").expect("blank scroll should parse");

    let after_snapshot = backend.snapshot().to_owned_snapshot();
    let after_scrollback = backend.scrollback().to_owned_snapshot();

    assert_eq!(update.dirty_rows, DirtyRows::None);
    assert!(update.scrollback_changed);
    assert_eq!(after_snapshot.rows, before_snapshot.rows);
    assert_eq!(after_scrollback.lines.len(), 10_000);
    assert_ne!(after_scrollback, before_scrollback);
}

#[test]
fn backend_marks_dirty_row_ranges_for_partial_updates() {
    let mut backend = Vt100Backend::new(PaneId::new(11), Size::new(3, 10)).unwrap();
    backend.feed(b"abc").expect("initial content should parse");

    let update = backend
        .feed(b"\x1b[2;1Hxyz")
        .expect("partial update should parse");

    assert_eq!(update.dirty_rows, DirtyRows::Range { start: 1, end: 2 });
}

#[test]
fn backend_uses_latest_icon_name_until_title_is_set() {
    let mut backend = Vt100Backend::new(PaneId::new(12), Size::new(2, 10)).unwrap();

    let first = backend
        .feed(b"\x1b]1;A\x07")
        .expect("first icon update should parse");
    let second = backend
        .feed(b"\x1b]1;B\x07")
        .expect("second icon update should parse");

    assert!(first.title_changed);
    assert!(second.title_changed);
    assert_eq!(backend.snapshot().title.as_deref(), Some("B"));

    backend
        .feed(b"\x1b]2;Title\x07")
        .expect("title update should parse");
    backend
        .feed(b"\x1b]1;C\x07")
        .expect("icon fallback update should parse");

    assert_eq!(backend.snapshot().title.as_deref(), Some("Title"));
}

#[test]
fn backend_preserves_wide_character_cell_widths() {
    let mut backend = Vt100Backend::new(PaneId::new(13), Size::new(2, 3)).unwrap();
    backend
        .feed("あ".as_bytes())
        .expect("wide character should parse");

    let snapshot = backend.snapshot().to_owned_snapshot();
    assert_eq!(snapshot.rows.len(), 2);
    assert_eq!(snapshot.rows[0].cells[0].text.as_ref(), "あ");
    assert_eq!(snapshot.rows[0].cells[0].width, CellWidth::Double);
    assert_eq!(snapshot.rows[0].cells[1].width, CellWidth::Continuation);
    assert_eq!(snapshot.rows[0].cells[2].width, CellWidth::Single);
}

#[test]
fn backend_preserves_combining_character_cell_contents() {
    let mut backend = Vt100Backend::new(PaneId::new(29), Size::new(2, 4)).unwrap();
    backend
        .feed("e\u{301}x".as_bytes())
        .expect("combining character should parse");

    let snapshot = backend.snapshot().to_owned_snapshot();
    assert_eq!(snapshot.rows[0].cells[0].text.as_ref(), "e\u{301}");
    assert_eq!(snapshot.rows[0].cells[0].width, CellWidth::Single);
    assert_eq!(snapshot.rows[0].cells[1].text.as_ref(), "x");
    assert_eq!(snapshot.cursor.position, Some(CursorPosition::new(0, 2)));
}

#[test]
fn backend_scroll_region_confines_scrolling_to_region() {
    let mut backend = Vt100Backend::new(PaneId::new(30), Size::new(5, 8)).unwrap();
    backend
        .feed(b"top\r\none\r\ntwo\r\nthree\r\nbottom")
        .expect("initial content should parse");

    let update = backend
        .feed(b"\x1b[2;4r\x1b[4;1H\nZ")
        .expect("scroll region update should parse");

    let snapshot = backend.snapshot().to_owned_snapshot();
    assert_eq!(update.dirty_rows, DirtyRows::Range { start: 1, end: 4 });
    assert_eq!(row_text(&snapshot.rows[0]), "top     ");
    assert_eq!(row_text(&snapshot.rows[1]), "two     ");
    assert_eq!(row_text(&snapshot.rows[2]), "three   ");
    assert_eq!(row_text(&snapshot.rows[3]), "Z       ");
    assert_eq!(row_text(&snapshot.rows[4]), "bottom  ");
}

#[test]
fn backend_new_rejects_zero_sized_surface() {
    let err = Vt100Backend::new(PaneId::new(14), Size::new(0, 0))
        .expect_err("zero-sized surface should be rejected");
    assert_eq!(err, PaneError::InvalidSize { rows: 0, cols: 0 });
}

#[test]
fn backend_new_rejects_zero_rows() {
    let err = Vt100Backend::new(PaneId::new(16), Size::new(0, 1))
        .expect_err("zero-row surface should be rejected");
    assert_eq!(err, PaneError::InvalidSize { rows: 0, cols: 1 });
}

#[test]
fn backend_new_rejects_zero_cols() {
    let err = Vt100Backend::new(PaneId::new(17), Size::new(1, 0))
        .expect_err("zero-column surface should be rejected");
    assert_eq!(err, PaneError::InvalidSize { rows: 1, cols: 0 });
}

#[test]
fn backend_new_rejects_single_row_surface() {
    let err = Vt100Backend::new(PaneId::new(20), Size::new(1, 2))
        .expect_err("single-row surface should be rejected");
    assert_eq!(
        err,
        PaneError::Surface {
            message: "vt100 backend requires at least two rows; got rows=1 cols=2".to_string()
        }
    );
}

#[test]
fn backend_resize_rejects_single_row_surface() {
    let mut backend = Vt100Backend::new(PaneId::new(21), Size::new(2, 2)).unwrap();

    let err = backend
        .resize(Size::new(1, 2))
        .expect_err("single-row resize should be rejected");

    assert_eq!(
        err,
        PaneError::Surface {
            message: "vt100 backend requires at least two rows; got rows=1 cols=2".to_string()
        }
    );
}

fn assert_fixture(name: &str, size: Size) {
    let mut backend = Vt100Backend::new(PaneId::new(1), size).unwrap();
    let ansi = fs::read(fixture_dir().join(format!("{name}.ansi"))).expect("fixture must exist");
    backend.feed(&ansi).expect("fixture should parse");

    let snapshot = backend.snapshot().to_owned_snapshot();
    let actual = render_snapshot(&snapshot);
    let expected =
        fs::read_to_string(fixture_dir().join(format!("{name}.snap"))).expect("snapshot exists");

    assert_eq!(actual, expected, "snapshot mismatch for fixture {name}");
}

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/surface")
}

fn render_snapshot(snapshot: &SurfaceSnapshot<'_>) -> String {
    let mut out = String::new();
    let cursor = snapshot
        .cursor
        .position
        .unwrap_or(CursorPosition::new(0, 0));

    writeln!(out, "size: {}x{}", snapshot.size.rows, snapshot.size.cols).unwrap();
    writeln!(
        out,
        "cursor: row={} col={} visible={}",
        cursor.row, cursor.col, snapshot.cursor.visible
    )
    .unwrap();
    writeln!(
        out,
        "modes: bracketed_paste={} mouse={} focus_events={} application_cursor={} alternate_screen={}",
        snapshot.modes.bracketed_paste,
        mouse_mode_name(snapshot.modes.mouse),
        snapshot.modes.focus_events,
        snapshot.modes.application_cursor,
        snapshot.modes.alternate_screen
    )
    .unwrap();

    match snapshot.title.as_deref() {
        Some(title) => writeln!(out, "title: {:?}", title).unwrap(),
        None => writeln!(out, "title: none").unwrap(),
    }

    writeln!(out, "rows:").unwrap();
    for (row_idx, row) in snapshot.rows.iter().enumerate() {
        writeln!(
            out,
            "  {row_idx:02} wrap={} |{}|",
            row.wrapped,
            row_text(row)
        )
        .unwrap();
    }

    writeln!(out, "styles:").unwrap();
    let mut style_lines = 0usize;
    for (row_idx, row) in snapshot.rows.iter().enumerate() {
        for (col_idx, cell) in row.cells.iter().enumerate() {
            if !interesting_cell(cell.width, cell.style) {
                continue;
            }

            style_lines += 1;
            writeln!(
                out,
                "  {row_idx:02}:{col_idx:02} width={} text={:?} fg={} bg={} attrs={}",
                width_name(cell.width),
                cell.text,
                color_name(cell.style.fg),
                color_name(cell.style.bg),
                attrs_name(cell.style.attrs)
            )
            .unwrap();
        }
    }

    if style_lines == 0 {
        writeln!(out, "  (none)").unwrap();
    }

    out
}

fn row_text(row: &SurfaceRow<'_>) -> String {
    let mut out = String::new();

    for cell in &row.cells {
        if cell.width == CellWidth::Continuation || cell.text.is_empty() {
            out.push(' ');
        } else {
            out.push_str(cell.text.as_ref());
        }
    }

    out
}

fn interesting_cell(width: CellWidth, style: CellStyle) -> bool {
    width != CellWidth::Single || style != CellStyle::default()
}

fn width_name(width: CellWidth) -> &'static str {
    match width {
        CellWidth::Single => "single",
        CellWidth::Double => "double",
        CellWidth::Continuation => "continuation",
    }
}

fn color_name(color: Option<ColorSpec>) -> String {
    match color {
        None | Some(ColorSpec::Default) => "default".to_string(),
        Some(ColorSpec::Indexed(idx)) => format!("idx({idx})"),
        Some(ColorSpec::Rgb(r, g, b)) => format!("rgb({r},{g},{b})"),
    }
}

fn attrs_name(attrs: CellAttrs) -> String {
    let mut parts = Vec::new();

    if attrs.bold {
        parts.push("bold");
    }
    if attrs.dim {
        parts.push("dim");
    }
    if attrs.italic {
        parts.push("italic");
    }
    if attrs.underlined {
        parts.push("underlined");
    }
    if attrs.slow_blink {
        parts.push("slow_blink");
    }
    if attrs.rapid_blink {
        parts.push("rapid_blink");
    }
    if attrs.reversed {
        parts.push("reversed");
    }
    if attrs.hidden {
        parts.push("hidden");
    }
    if attrs.crossed_out {
        parts.push("crossed_out");
    }

    if parts.is_empty() {
        "none".to_string()
    } else {
        parts.join(",")
    }
}

fn mouse_mode_name(mode: MouseMode) -> &'static str {
    match mode {
        MouseMode::None => "none",
        MouseMode::X10 => "x10",
        MouseMode::Normal => "normal",
        MouseMode::ButtonEvent => "button-event",
        MouseMode::AnyEvent => "any-event",
        MouseMode::Sgr => "sgr",
        _ => "unknown",
    }
}
