use super::*;

#[test]
fn overlay_tracks_focus_bracketed_paste_and_mouse_modes() {
    let mut overlay = TerminalModeOverlay::default();

    assert!(overlay.update_from_output(b"\x1b[?1004;2004;1000;1006h"));
    let modes = overlay.merge_with(TerminalModes::default());

    assert!(modes.focus_events);
    assert!(modes.bracketed_paste);
    assert_eq!(modes.mouse, MouseMode::Sgr);
}

#[test]
fn overlay_restores_lower_priority_mouse_mode_when_sgr_is_disabled() {
    let mut overlay = TerminalModeOverlay::default();

    overlay.update_from_output(b"\x1b[?1000;1006h");
    overlay.update_from_output(b"\x1b[?1006l");
    let modes = overlay.merge_with(TerminalModes::default());

    assert_eq!(modes.mouse, MouseMode::Normal);
}

#[test]
fn overlay_tracks_sequences_split_across_feeds() {
    let mut overlay = TerminalModeOverlay::default();

    assert!(!overlay.update_from_output(b"\x1b[?10"));
    assert!(overlay.update_from_output(b"04h"));

    assert!(overlay.merge_with(TerminalModes::default()).focus_events);
}

#[test]
fn overlay_reset_clears_overlay_tracked_modes() {
    let mut overlay = TerminalModeOverlay::default();

    overlay.update_from_output(b"\x1b[?1004;2004;1006h");
    assert!(overlay.update_from_output(b"\x1bc"));
    let modes = overlay.merge_with(TerminalModes::default());

    assert!(!modes.focus_events);
    assert!(!modes.bracketed_paste);
    assert_eq!(modes.mouse, MouseMode::None);
}

#[test]
fn overlay_reset_can_span_feed_boundary() {
    let mut overlay = TerminalModeOverlay::default();

    overlay.update_from_output(b"\x1b[?1004h\x1b");
    assert!(overlay.update_from_output(b"c"));

    assert!(!overlay.merge_with(TerminalModes::default()).focus_events);
}
