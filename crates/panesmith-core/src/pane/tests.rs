use super::*;

#[test]
fn pane_config_default_is_shell() {
    let config = PaneConfig::default();
    if cfg!(windows) {
        assert_eq!(config.program(), "cmd");
    } else {
        assert_eq!(config.program(), "sh");
    }
    assert!(config.args().is_empty());
    assert_eq!(config.size, Size::new(24, 80));
    assert_eq!(config.scrollback, ScrollbackConfig::default());
    assert_eq!(config.transcript.mode, TranscriptMode::Disabled);
    assert_eq!(config.kill, KillConfig::default());
    assert!(config.id.is_none());
    assert!(config.title.is_none());
    assert!(config.cwd.is_none());
    assert!(config.env.is_empty());
}

#[test]
fn pane_config_command() {
    let config = PaneConfig::command("bash");
    assert_eq!(config.program(), "bash");
    assert!(config.args().is_empty());
}

#[test]
fn pane_config_command_with_args() {
    let config = PaneConfig::command_with_args("python", ["-q"]);
    assert_eq!(config.program(), "python");
    assert_eq!(config.args(), ["-q"]);
}

#[test]
fn pane_config_shell() {
    let config = PaneConfig::shell();
    if cfg!(windows) {
        assert_eq!(config.program(), "cmd");
    } else {
        assert_eq!(config.program(), "sh");
    }
}

#[test]
fn pane_config_fluent_setters() {
    let config = PaneConfig::command("bash")
        .with_id(PaneId::new(42))
        .with_title("my pane")
        .with_cwd("/tmp")
        .with_env("FOO", "bar")
        .with_size(Size::new(10, 20))
        .with_scrollback(ScrollbackConfig::new(5_000, 512 * 1024).unwrap())
        .with_transcript(TranscriptConfig::new(TranscriptMode::RawBytes))
        .with_surface(SurfaceConfig)
        .with_input(InputConfig::default())
        .with_attach(AttachConfig)
        .with_kill(KillConfig::new(Duration::from_secs(10), false));

    assert_eq!(config.id, Some(PaneId::new(42)));
    assert_eq!(config.title, Some("my pane".into()));
    assert_eq!(config.cwd, Some(PathBuf::from("/tmp")));
    assert_eq!(config.env.get("FOO"), Some(&"bar".into()));
    assert_eq!(config.size, Size::new(10, 20));
    assert_eq!(config.scrollback.max_lines, 5_000);
    assert_eq!(config.scrollback.max_bytes, 512 * 1024);
    assert_eq!(config.transcript.mode, TranscriptMode::RawBytes);
    assert_eq!(config.kill.term_grace, Duration::from_secs(10));
    assert!(!config.kill.kill_descendants);
}

#[test]
fn pane_config_validate_empty_program_fails() {
    let config = PaneConfig::command("");
    let err = config.validate().expect_err("empty program should fail");
    assert!(
        matches!(&err, PaneError::Spawn { message } if message.contains("empty")),
        "expected Spawn error about empty program, got {err:?}"
    );
}

#[test]
fn pane_config_validate_whitespace_program_fails() {
    let config = PaneConfig::command("   ");
    let err = config
        .validate()
        .expect_err("whitespace-only program should fail");
    assert!(
        matches!(&err, PaneError::Spawn { message } if message.contains("empty")),
        "expected Spawn error about empty program, got {err:?}"
    );
}

#[test]
fn pane_config_validate_zero_size_fails() {
    let config = PaneConfig::command("sh").with_size(Size::new(0, 80));
    let err = config.validate().expect_err("zero rows should fail");
    assert!(
        matches!(err, PaneError::InvalidSize { rows: 0, cols: 80 }),
        "expected InvalidSize error, got {err:?}"
    );
}

#[test]
fn pane_config_validate_valid_passes() {
    let config = PaneConfig::command("sh").with_size(Size::new(24, 80));
    config.validate().expect("valid config should pass");
}

#[test]
fn scrollback_config_new_rejects_zero_lines() {
    let err = ScrollbackConfig::new(0, 1024).expect_err("zero lines should fail");
    assert!(
        matches!(&err, PaneError::Spawn { message } if message.contains("scrollback")),
        "expected Spawn error about scrollback, got {err:?}"
    );
}

#[test]
fn scrollback_config_new_rejects_zero_bytes() {
    let err = ScrollbackConfig::new(100, 0).expect_err("zero bytes should fail");
    assert!(
        matches!(&err, PaneError::Spawn { message } if message.contains("scrollback")),
        "expected Spawn error about scrollback, got {err:?}"
    );
}

#[test]
fn scrollback_config_new_accepts_valid() {
    let sc = ScrollbackConfig::new(100, 1024).expect("valid scrollback should succeed");
    assert_eq!(sc.max_lines, 100);
    assert_eq!(sc.max_bytes, 1024);
    assert!(sc.is_enabled());
}

#[test]
fn scrollback_config_disabled_is_not_enabled() {
    let sc = ScrollbackConfig::disabled();
    assert!(!sc.is_enabled());
}

#[test]
fn scrollback_config_default_is_enabled() {
    let sc = ScrollbackConfig::default();
    assert!(sc.is_enabled());
    assert_eq!(sc.max_lines, 10_000);
    assert_eq!(sc.max_bytes, 1024 * 1024);
}

#[test]
fn kill_config_default() {
    let kc = KillConfig::default();
    assert_eq!(kc.term_grace, Duration::from_secs(5));
    assert!(kc.kill_descendants);
}

#[test]
fn command_spec_validate_empty_fails() {
    let spec = CommandSpec::new("");
    let err = spec.validate().expect_err("empty program should fail");
    assert!(
        matches!(&err, PaneError::Spawn { message } if message.contains("empty")),
        "expected Spawn error about empty program, got {err:?}"
    );
}

#[test]
fn command_spec_validate_non_empty_passes() {
    let spec = CommandSpec::new("sh");
    spec.validate().expect("non-empty program should pass");
}

#[test]
fn transcript_config_default_is_disabled() {
    let tc = TranscriptConfig::default();
    assert_eq!(tc.mode, TranscriptMode::Disabled);
}

#[test]
fn pane_config_validate_scrollback_mixed_zero_fails() {
    // lines=0, bytes>0 is inconsistent
    let config = PaneConfig::command("sh")
        .with_size(Size::new(24, 80))
        .with_scrollback(ScrollbackConfig {
            max_lines: 0,
            max_bytes: 1024,
        });
    let err = config
        .validate()
        .expect_err("mixed zero scrollback should fail");
    assert!(
        matches!(&err, PaneError::Spawn { message } if message.contains("scrollback")),
        "expected Spawn error about scrollback, got {err:?}"
    );
}

#[test]
fn pane_config_validate_disabled_scrollback_passes() {
    let config = PaneConfig::command("sh")
        .with_size(Size::new(24, 80))
        .with_scrollback(ScrollbackConfig::disabled());
    config
        .validate()
        .expect("disabled scrollback should pass validation");
}

#[test]
fn snapshot_types_are_constructible() {
    let pane_id = PaneId::new(1);
    let _snapshot = PaneSnapshot {
        id: pane_id,
        title: Some("test".into()),
        state: PaneState::Running,
        interaction_mode: PaneInteractionMode::Embedded,
        size: Size::new(24, 80),
        surface: SurfaceSnapshot::blank(Size::new(24, 80)),
        cursor: CursorState::new(Some(CursorPosition::new(3, 4)), true),
        modes: TerminalModes {
            bracketed_paste: true,
            mouse: MouseMode::AnyEvent,
            focus_events: true,
            application_cursor: false,
            alternate_screen: false,
        },
        stats: PaneStats,
    };
}

#[test]
fn surface_snapshot_defaults_are_constructible() {
    let ss = SurfaceSnapshot::default();
    let _ = format!("{:?}", ss);
}

#[test]
fn surface_snapshot_to_owned_clones_borrowed_text() {
    let snapshot = SurfaceSnapshot::new(
        Size::new(2, 4),
        vec![
            SurfaceRow::new(vec![SurfaceCell::new(
                "hi",
                CellWidth::Single,
                CellStyle::default(),
            )]),
            SurfaceRow::default(),
        ],
        CursorState::new(Some(CursorPosition::new(1, 1)), true),
        TerminalModes::default(),
        Some(Cow::Borrowed("title")),
    );

    let owned = snapshot.to_owned_snapshot();

    assert_eq!(owned.title.as_deref(), Some("title"));
    assert_eq!(owned.rows[0].cells[0].text.as_ref(), "hi");
    assert_eq!(owned.cursor.position, Some(CursorPosition::new(1, 1)));
}

#[test]
fn scrollback_snapshot_to_owned_preserves_text_and_styles() {
    let style = CellStyle {
        fg: Some(ColorSpec::Indexed(2)),
        bg: Some(ColorSpec::Rgb(1, 2, 3)),
        attrs: CellAttrs {
            bold: true,
            italic: true,
            ..CellAttrs::default()
        },
        ..CellStyle::default()
    };
    let snapshot = ScrollbackSnapshot::new(vec![ScrollbackLine::from_row(
        Cow::Borrowed("hi"),
        SurfaceRow::new(vec![SurfaceCell::new(
            Cow::Borrowed("h"),
            CellWidth::Single,
            style,
        )])
        .with_wrapped(true),
    )]);

    let owned = snapshot.to_owned_snapshot();

    assert_eq!(owned.lines[0].text.as_ref(), "hi");
    assert_eq!(owned.lines[0].row.cells[0].text.as_ref(), "h");
    assert_eq!(owned.lines[0].row.cells[0].style, style);
    assert!(owned.lines[0].row.wrapped);
}

#[test]
fn pane_snapshot_to_owned_clones_surface_and_metadata() {
    let snapshot = PaneSnapshot {
        id: PaneId::new(9),
        title: Some("pane".into()),
        state: PaneState::Running,
        interaction_mode: PaneInteractionMode::Embedded,
        size: Size::new(3, 8),
        surface: SurfaceSnapshot::new(
            Size::new(3, 8),
            vec![SurfaceRow::new(vec![SurfaceCell::new(
                "ok",
                CellWidth::Single,
                CellStyle::default(),
            )])],
            CursorState::new(Some(CursorPosition::new(0, 2)), true),
            TerminalModes::default(),
            None,
        ),
        cursor: CursorState::new(Some(CursorPosition::new(0, 2)), true),
        modes: TerminalModes::default(),
        stats: PaneStats,
    };

    let owned = snapshot.to_owned_snapshot();

    assert_eq!(owned.title.as_deref(), Some("pane"));
    assert_eq!(owned.surface.rows[0].cells[0].text.as_ref(), "ok");
    assert_eq!(owned.cursor.position, Some(CursorPosition::new(0, 2)));
}
