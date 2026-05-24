use super::*;
use crate::{CellWidth, ColorSpec, MouseMode, PaneError, SurfaceBackend};

#[test]
fn default_surface_backend_rejects_single_row_surface() {
    let err =
        DefaultSurfaceBackend::new(PaneId::new(1), Size::new(1, 5), ScrollbackConfig::default())
            .expect_err("single-row surfaces should be rejected");

    assert_eq!(
        err,
        PaneError::Surface {
            message: "default surface backend requires at least two rows; got rows=1 cols=5"
                .to_string(),
        }
    );
}

#[test]
fn default_surface_backend_resize_rejects_single_row_surface() {
    let mut backend =
        DefaultSurfaceBackend::new(PaneId::new(2), Size::new(2, 5), ScrollbackConfig::default())
            .expect("two-row surface should initialize");

    let err = backend
        .resize(Size::new(1, 5))
        .expect_err("single-row resize should be rejected");

    assert_eq!(
        err,
        PaneError::Surface {
            message: "default surface backend requires at least two rows; got rows=1 cols=5"
                .to_string(),
        }
    );
}

#[test]
fn default_surface_scrollback_preserves_styled_ansi_output() {
    let mut backend =
        DefaultSurfaceBackend::new(PaneId::new(3), Size::new(2, 6), ScrollbackConfig::default())
            .expect("two-row surface should initialize");

    backend
        .feed(b"\x1b[31;44;1mred\x1b[0m\r\nplain\r\n")
        .expect("ansi output should parse");

    let scrollback = backend.scrollback().to_owned_snapshot();
    let line = scrollback
        .lines
        .iter()
        .find(|line| line.text.as_ref() == "red")
        .expect("styled line should be retained in scrollback");

    assert_eq!(line.row.cells[0].text.as_ref(), "r");
    assert_eq!(line.row.cells[0].style.fg, Some(ColorSpec::Indexed(1)));
    assert_eq!(line.row.cells[0].style.bg, Some(ColorSpec::Indexed(4)));
}

#[test]
fn default_surface_tracks_focus_sgr_mouse_and_reset_modes() {
    let mut backend =
        DefaultSurfaceBackend::new(PaneId::new(4), Size::new(2, 8), ScrollbackConfig::default())
            .expect("two-row surface should initialize");

    let enabled = backend
        .feed(b"\x1b[?1004;2004;1000;1006h")
        .expect("mode enable sequence should parse");
    let modes = backend.snapshot().modes;
    assert!(enabled.modes_changed);
    assert!(modes.focus_events);
    assert!(modes.bracketed_paste);
    assert_eq!(modes.mouse, MouseMode::Sgr);

    let reset = backend
        .feed(b"\x1bcplain")
        .expect("terminal reset should parse");
    let modes = backend.snapshot().modes;
    assert!(reset.modes_changed);
    assert!(!modes.focus_events);
    assert!(!modes.bracketed_paste);
    assert_eq!(modes.mouse, MouseMode::None);
}

#[test]
fn default_surface_matches_shared_engine_for_representative_fixture() {
    let pane_id = PaneId::new(5);
    let size = Size::new(2, 12);
    let scrollback = ScrollbackConfig::default();
    let mut wrapper = DefaultSurfaceBackend::new(pane_id, size, scrollback).unwrap();
    let mut shared = Vt100Surface::new(
        pane_id,
        size,
        configured_scrollback_rows(scrollback),
        VALIDATION_NAME,
    )
    .unwrap();
    let fixture = b"\x1b]2;Pane\x07\x1b[?1004;2004;1006h\x1b[32;45;3mhello\x1b[0m\r\nworld\r\n";

    let wrapper_update = wrapper.feed(fixture).unwrap();
    let shared_update = shared.feed(fixture).unwrap();

    assert_eq!(wrapper_update, shared_update);
    assert_eq!(
        wrapper.snapshot().to_owned_snapshot(),
        shared.snapshot().to_owned_snapshot()
    );
    assert_eq!(
        wrapper.scrollback().to_owned_snapshot(),
        shared.scrollback().to_owned_snapshot()
    );
    assert_eq!(wrapper.cursor(), shared.cursor());
    assert_eq!(wrapper.modes(), shared.modes());
    assert_eq!(
        wrapper.scrollback().to_owned_snapshot().lines[0].row.cells[0].width,
        CellWidth::Single
    );
}
