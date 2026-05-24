use super::TerminalPaneWidget;
use panesmith_core::{
    CursorState, OwnedPaneSnapshot, PaneId, PaneInteractionMode, PaneState, PaneStats, Size,
    SurfaceSnapshot, TerminalModes,
};

#[test]
fn widget_keeps_track_of_the_target_pane() {
    let snapshot = OwnedPaneSnapshot {
        id: PaneId::new(3),
        title: None,
        state: PaneState::Running,
        interaction_mode: PaneInteractionMode::Embedded,
        size: Size::new(1, 1),
        surface: SurfaceSnapshot::blank(Size::new(1, 1)),
        cursor: CursorState::hidden(),
        modes: TerminalModes::default(),
        stats: PaneStats,
    };
    let widget = TerminalPaneWidget::new(&snapshot);
    assert_eq!(widget.pane_id(), PaneId::new(3));
}
