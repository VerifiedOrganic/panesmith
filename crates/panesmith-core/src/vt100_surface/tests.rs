use super::*;

#[test]
fn shared_surface_tracks_title_modes_scrollback_and_snapshot() {
    let mut surface = Vt100Surface::new(
        PaneId::new(1),
        Size::new(2, 10),
        DEFAULT_VT100_SCROLLBACK_ROWS,
        "vt100 backend",
    )
    .unwrap();

    let update = surface
        .feed(b"\x1b]2;Pane\x07\x1b[?1004;2004;1006hhello\r\nworld\r\n")
        .unwrap();
    let snapshot = surface.snapshot().to_owned_snapshot();
    let scrollback = surface.scrollback().to_owned_snapshot();

    assert!(update.title_changed);
    assert!(update.modes_changed);
    assert!(update.scrollback_changed);
    assert_eq!(snapshot.title.as_deref(), Some("Pane"));
    assert!(snapshot.modes.focus_events);
    assert!(snapshot.modes.bracketed_paste);
    assert_eq!(snapshot.modes.mouse, MouseMode::Sgr);
    assert_eq!(scrollback.lines[0].text.as_ref(), "hello");
}

#[test]
fn shared_surface_validation_label_is_wrapper_defined() {
    let result = Vt100Surface::new(
        PaneId::new(1),
        Size::new(1, 5),
        DEFAULT_VT100_SCROLLBACK_ROWS,
        "custom wrapper",
    );
    let err = match result {
        Ok(_) => panic!("single-row surfaces should be rejected"),
        Err(error) => error,
    };

    assert_eq!(
        err,
        PaneError::Surface {
            message: "custom wrapper requires at least two rows; got rows=1 cols=5".into()
        }
    );
}
