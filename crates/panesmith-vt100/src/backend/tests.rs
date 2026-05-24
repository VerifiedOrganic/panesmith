use super::*;
use panesmith_core::SurfaceBackend;

#[test]
fn public_backend_matches_shared_engine_for_representative_fixture() {
    let pane_id = PaneId::new(7);
    let size = Size::new(2, 12);
    let mut wrapper = Vt100Backend::new(pane_id, size).unwrap();
    let mut shared = Vt100Surface::new(
        pane_id,
        size,
        DEFAULT_VT100_SCROLLBACK_ROWS,
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
}
