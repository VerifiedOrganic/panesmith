use super::*;

fn assert_send<T: Send>() {}

#[test]
fn pane_manager_is_send() {
    assert_send::<PaneManager>();
}

#[test]
fn umbrella_reexports_cover_each_workspace_crate() {
    let pane_id = PaneId::new(11);
    let _manager = PaneManager::new(PaneManagerConfig::default());
    let config = PaneConfig::command("bash");
    let snapshot = OwnedPaneSnapshot {
        id: pane_id,
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
    let bridge = AttachBridge::new(pane_id);
    let backend = Vt100Backend::new(pane_id, Size::new(30, 100)).unwrap();
    let error = PaneError::NotFound { pane_id };
    let event = PaneEvent::placeholder(pane_id);

    assert_eq!(config.program(), "bash");
    assert_eq!(widget.pane_id(), pane_id);
    assert_eq!(bridge.pane_id(), pane_id);
    assert_eq!(backend.size(), Size::new(30, 100));
    assert_eq!(event.pane_id, pane_id);
    assert_eq!(format!("{error}"), "pane 11 was not found");
}

#[test]
fn pane_id_reexport_is_the_same_core_type() {
    let pane_id: panesmith_core::PaneId = PaneId::new(42);
    assert_eq!(pane_id, panesmith_core::PaneId::new(42));
}

#[test]
fn feed_bytes_result_is_reexported() {
    let result: FeedBytesResult<'static> = FeedBytesResult {
        forward: vec![0x61],
        detached: false,
        remaining: &[],
    };
    assert_eq!(result.forward, vec![0x61]);
    assert!(!result.detached);
    assert!(result.remaining.is_empty());
}

#[test]
fn umbrella_config_builders_work() {
    use std::time::Duration;

    let config = PaneConfig::command("bash")
        .with_id(PaneId::new(7))
        .with_title("test")
        .with_size(Size::new(10, 20))
        .with_scrollback(ScrollbackConfig::new(1_000, 64 * 1024).unwrap())
        .with_transcript(TranscriptConfig::new(TranscriptMode::PlainText))
        .with_kill(KillConfig::new(Duration::from_secs(3), false));

    assert_eq!(config.program(), "bash");
    assert_eq!(config.id, Some(PaneId::new(7)));
    assert_eq!(config.title, Some("test".into()));
    assert_eq!(config.size, Size::new(10, 20));
    assert_eq!(config.scrollback.max_lines, 1_000);
    assert_eq!(config.transcript.mode, TranscriptMode::PlainText);
    assert_eq!(config.kill.term_grace, Duration::from_secs(3));
    assert!(!config.kill.kill_descendants);
}

#[test]
fn pane_command_attach_uses_core_attach_options() {
    let pane_id = PaneId::new(5);
    let cmd = PaneCommand::Attach {
        pane_id,
        options: CoreAttachOptions::default(),
    };
    assert!(matches!(cmd, PaneCommand::Attach { pane_id: id, .. } if id == pane_id));
}

#[cfg(feature = "crossterm")]
#[test]
fn umbrella_crossterm_attach_helpers_compile() {
    let _control = CrosstermTerminalControl::with_raw_mode_ops(Vec::<u8>::new(), SystemRawModeOps);

    #[cfg(unix)]
    let _ctor: fn(Vec<u8>) -> std::io::Result<StdioAttachTerminal<Vec<u8>>> =
        StdioAttachTerminal::<Vec<u8>>::new;
}
