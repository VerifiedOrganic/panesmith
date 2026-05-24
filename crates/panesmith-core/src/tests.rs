use super::*;

#[test]
fn core_placeholders_are_constructible() {
    let pane_id = PaneId::new(7);
    let config = PaneConfig::command_with_args("sh", ["-lc", "true"]);
    let mut manager = PaneManager::new(PaneManagerConfig::default());
    let event = PaneEvent::placeholder(pane_id);

    assert_eq!(pane_id.get(), 7);
    assert_eq!(config.program(), "sh");
    assert_eq!(config.args(), ["-lc", "true"]);
    assert_eq!(manager.alloc_placeholder_id().get(), 1);
    assert_eq!(event.pane_id, pane_id);
    assert_eq!(
        format!("{}", PaneError::NotFound { pane_id }),
        "pane 7 was not found"
    );
}

#[test]
fn size_try_new_accepts_nonzero_dimensions() {
    let size = Size::try_new(24, 80).expect("valid size should succeed");
    assert_eq!(size.rows, 24);
    assert_eq!(size.cols, 80);
}

#[test]
fn size_try_new_rejects_zero_rows() {
    let err = Size::try_new(0, 80).expect_err("zero rows should fail");
    assert!(
        matches!(err, PaneError::InvalidSize { rows: 0, cols: 80 }),
        "expected InvalidSize error, got {err:?}"
    );
}

#[test]
fn size_try_new_rejects_zero_cols() {
    let err = Size::try_new(24, 0).expect_err("zero cols should fail");
    assert!(
        matches!(err, PaneError::InvalidSize { rows: 24, cols: 0 }),
        "expected InvalidSize error, got {err:?}"
    );
}

#[test]
fn size_try_new_rejects_both_zero() {
    let err = Size::try_new(0, 0).expect_err("zero dimensions should fail");
    assert!(
        matches!(err, PaneError::InvalidSize { rows: 0, cols: 0 }),
        "expected InvalidSize error, got {err:?}"
    );
}

#[test]
fn pane_error_display_covers_all_variants() {
    let pane_id = PaneId::new(42);

    assert_eq!(
        format!(
            "{}",
            PaneError::Spawn {
                message: "execvp failed".into()
            }
        ),
        "spawn failed: execvp failed"
    );

    assert_eq!(
        format!(
            "{}",
            PaneError::Io {
                operation: IoOperation::Write,
                message: "broken pipe".into()
            }
        ),
        "io failed (write): broken pipe"
    );

    assert_eq!(
        format!(
            "{}",
            PaneError::Surface {
                message: "parse error".into()
            }
        ),
        "surface failed: parse error"
    );

    assert_eq!(
        format!(
            "{}",
            PaneError::InputEncoding {
                message: "unknown key".into()
            }
        ),
        "input encoding failed: unknown key"
    );

    assert_eq!(
        format!(
            "{}",
            PaneError::Attach {
                message: "stdout busy".into()
            }
        ),
        "attach failed: stdout busy"
    );

    assert_eq!(
        format!("{}", PaneError::NotFound { pane_id }),
        "pane 42 was not found"
    );

    assert_eq!(
        format!(
            "{}",
            PaneError::InvalidState {
                expected: "Running".into(),
                actual: "Exited".into()
            }
        ),
        "invalid state: expected Running, got Exited"
    );

    assert_eq!(
        format!("{}", PaneError::InvalidSize { rows: 0, cols: 5 }),
        "invalid size: rows=0, cols=5 (both must be >= 1)"
    );
}

#[test]
fn pane_error_implements_std_error() {
    let err: Box<dyn std::error::Error> = Box::new(PaneError::NotFound {
        pane_id: PaneId::new(1),
    });
    assert!(err.source().is_none());
}

#[test]
fn pane_error_debug_is_available() {
    let err = PaneError::Io {
        operation: IoOperation::Read,
        message: "eof".into(),
    };
    let debug = format!("{err:?}");
    assert!(debug.contains("Io"));
    assert!(debug.contains("Read"));
    assert!(debug.contains("eof"));
}

#[test]
fn pane_id_is_ordered() {
    let a = PaneId::new(1);
    let b = PaneId::new(2);
    assert!(a < b);
    assert!(b > a);
    assert_eq!(a, PaneId::new(1));
}

#[test]
fn pane_config_default_is_shell() {
    let config = PaneConfig::default();
    if cfg!(windows) {
        assert_eq!(config.program(), "cmd");
    } else {
        assert_eq!(config.program(), "sh");
    }
}

#[test]
fn pane_state_transitions_are_valid() {
    // Verify all expected states are constructible
    let _starting = PaneState::Starting;
    let _running = PaneState::Running;
    let _exited = PaneState::Exited { code: Some(0) };
    let _failed = PaneState::Failed {
        error: PaneError::Spawn {
            message: "test".into(),
        },
    };
    let _killed = PaneState::Killed {
        reason: KillReason::UserRequested,
    };
}

#[test]
fn pane_interaction_mode_variants_exist() {
    let _embedded = PaneInteractionMode::Embedded;
    let _attaching = PaneInteractionMode::Attaching;
    let _attached = PaneInteractionMode::Attached;
    let _detaching = PaneInteractionMode::Detaching;
}

#[test]
fn pane_command_variants_are_constructible() {
    let pane_id = PaneId::new(1);
    let size = Size::new(24, 80);

    let _spawn = PaneCommand::Spawn(PaneConfig::command("sh"));
    let _resize = PaneCommand::Resize { pane_id, size };
    let _write = PaneCommand::WriteBytes {
        pane_id,
        bytes: vec![0x0d],
    };
    let _input = PaneCommand::Input {
        pane_id,
        input: HostInput::Raw(vec![0x61]),
    };
    let _kill = PaneCommand::Kill {
        pane_id,
        reason: KillReason::HostRequested,
    };
    let _attach = PaneCommand::Attach {
        pane_id,
        options: AttachOptions::default(),
    };
    let _detach = PaneCommand::Detach {
        pane_id,
        reason: DetachReason::UserChord,
    };
}

#[test]
fn event_sequence_ordering_helpers_work() {
    let pane_id = PaneId::new(1);
    let base_time = std::time::SystemTime::UNIX_EPOCH;

    let a = PaneEvent {
        pane_id,
        seq: 1,
        at: base_time,
        kind: PaneEventKind::Spawned(SpawnedEvent {
            program: "sh".into(),
        }),
    };

    let b = PaneEvent {
        pane_id,
        seq: 2,
        at: base_time,
        kind: PaneEventKind::Exited(ExitedEvent { code: Some(0) }),
    };

    assert!(a.is_before(&b));
    assert!(b.is_after(&a));
    assert_eq!(a.seq_cmp(&b), Some(std::cmp::Ordering::Less));
    assert_eq!(b.seq_cmp(&a), Some(std::cmp::Ordering::Greater));
    assert_eq!(a.seq_cmp(&a), Some(std::cmp::Ordering::Equal));
}

#[test]
fn cross_pane_ordering_returns_none() {
    let base_time = std::time::SystemTime::UNIX_EPOCH;

    let a = PaneEvent {
        pane_id: PaneId::new(1),
        seq: 1,
        at: base_time,
        kind: PaneEventKind::Exited(ExitedEvent { code: Some(0) }),
    };

    let b = PaneEvent {
        pane_id: PaneId::new(2),
        seq: 2,
        at: base_time,
        kind: PaneEventKind::Exited(ExitedEvent { code: Some(0) }),
    };

    assert!(!a.is_before(&b));
    assert!(!b.is_after(&a));
    assert_eq!(a.seq_cmp(&b), None);
    assert_eq!(b.seq_cmp(&a), None);
}

#[test]
fn pane_event_kind_variants_are_constructible() {
    let pane_id = PaneId::new(1);
    let size = Size::new(24, 80);

    let _spawned = PaneEventKind::Spawned(SpawnedEvent {
        program: "sh".into(),
    });
    let _state = PaneEventKind::StateChanged(Box::new(StateChangedEvent {
        old: PaneState::Starting,
        new: PaneState::Running,
    }));
    let _output = PaneEventKind::Output(OutputEvent {
        bytes_len: 10,
        transcript_offset: Some(0),
        contains_escape: false,
    });
    let _surface = PaneEventKind::SurfaceChanged(SurfaceChangedEvent {
        generation: 1,
        dirty_rows: DirtyRows::All,
        cursor_changed: true,
        title_changed: false,
        modes_changed: false,
        scrollback_changed: false,
    });
    let _input = PaneEventKind::InputSent(InputSentEvent {
        input_kind: InputKind::Bytes,
        bytes_len: 3,
        recorded: true,
    });
    let _resized = PaneEventKind::Resized(ResizedEvent { size });
    let _attach_start = PaneEventKind::AttachStarted(AttachStartedEvent {
        terminal_size: size,
        embedded_size: size,
        screen_policy: AttachScreenPolicy::ReuseHostAlternateScreen,
    });
    let _attach_end = PaneEventKind::AttachEnded(AttachEndedEvent {
        reason: DetachReason::UserChord,
        restored_size: size,
        duration: std::time::Duration::from_secs(5),
    });
    let _transcript = PaneEventKind::TranscriptRotated(TranscriptRotatedEvent {
        chunks_dropped: 1,
        bytes_dropped: 0,
        raw_bytes_dropped: 0,
        plain_bytes_dropped: 0,
    });
    let _overflow = PaneEventKind::Overflow(OverflowEvent {
        dropped_frames: 2,
        dropped_bytes: 1024,
        queue: OverflowQueue::PtyOutputFrames,
    });
    let _error = PaneEventKind::Error(ErrorEvent {
        error: PaneError::NotFound { pane_id },
    });
    let _exited = PaneEventKind::Exited(ExitedEvent { code: Some(0) });
}

#[cfg(feature = "serde")]
#[test]
fn size_deserialize_rejects_zero_dimensions() {
    let json = r#"{"rows":0,"cols":80}"#;
    let result: std::result::Result<Size, _> = serde_json::from_str(json);
    assert!(result.is_err(), "deserializing zero rows should fail");
}

#[cfg(feature = "serde")]
#[test]
fn size_deserialize_accepts_valid_dimensions() {
    let json = r#"{"rows":24,"cols":80}"#;
    let size: Size = serde_json::from_str(json).expect("valid size should deserialize");
    assert_eq!(size.rows, 24);
    assert_eq!(size.cols, 80);
}

#[cfg(feature = "serde")]
#[test]
fn size_serialize_roundtrips() {
    let size = Size::new(10, 20);
    let json = serde_json::to_string(&size).expect("serialize should succeed");
    let decoded: Size = serde_json::from_str(&json).expect("deserialize should succeed");
    assert_eq!(size, decoded);
}

#[cfg(feature = "serde")]
#[test]
fn pane_config_deserialize_rejects_empty_program() {
    let invalid = PaneConfig::command("");
    let json = serde_json::to_string(&invalid).expect("serialize should succeed");
    let result: std::result::Result<PaneConfig, _> = serde_json::from_str(&json);
    assert!(result.is_err(), "deserializing empty program should fail");
}

#[cfg(feature = "serde")]
#[test]
fn pane_config_deserialize_rejects_whitespace_program() {
    let invalid = PaneConfig::command("   ");
    let json = serde_json::to_string(&invalid).expect("serialize should succeed");
    let result: std::result::Result<PaneConfig, _> = serde_json::from_str(&json);
    assert!(
        result.is_err(),
        "deserializing whitespace-only program should fail"
    );
}

#[cfg(feature = "serde")]
#[test]
fn pane_config_deserialize_rejects_zero_size() {
    let invalid = PaneConfig::command("sh").with_size(Size::new(0, 80));
    let json = serde_json::to_string(&invalid).expect("serialize should succeed");
    let result: std::result::Result<PaneConfig, _> = serde_json::from_str(&json);
    assert!(result.is_err(), "deserializing zero size should fail");
}

#[cfg(feature = "serde")]
#[test]
fn pane_config_deserialize_rejects_mixed_zero_scrollback() {
    let invalid = PaneConfig::command("sh")
        .with_size(Size::new(24, 80))
        .with_scrollback(ScrollbackConfig {
            max_lines: 0,
            max_bytes: 1024,
        });
    let json = serde_json::to_string(&invalid).expect("serialize should succeed");
    let result: std::result::Result<PaneConfig, _> = serde_json::from_str(&json);
    assert!(
        result.is_err(),
        "deserializing mixed-zero scrollback should fail"
    );
}

#[cfg(feature = "serde")]
#[test]
fn pane_config_deserialize_accepts_valid() {
    let valid = PaneConfig::command("sh").with_size(Size::new(24, 80));
    let json = serde_json::to_string(&valid).expect("serialize should succeed");
    let decoded: PaneConfig = serde_json::from_str(&json).expect("valid config should deserialize");
    assert_eq!(valid, decoded);
}

#[cfg(feature = "serde")]
#[test]
fn pane_config_serialize_roundtrips() {
    let config = PaneConfig::default();
    let json = serde_json::to_string(&config).expect("serialize should succeed");
    let decoded: PaneConfig = serde_json::from_str(&json).expect("deserialize should succeed");
    assert_eq!(config, decoded);
}

#[cfg(feature = "serde")]
#[test]
fn serde_representative_event_roundtrips() {
    let pane_id = PaneId::new(42);
    let event = PaneEvent {
        pane_id,
        seq: 7,
        at: std::time::SystemTime::UNIX_EPOCH,
        kind: PaneEventKind::Exited(ExitedEvent { code: Some(0) }),
    };

    let json = serde_json::to_string(&event).expect("serialize should succeed");
    let decoded: PaneEvent = serde_json::from_str(&json).expect("deserialize should succeed");

    assert_eq!(event.pane_id, decoded.pane_id);
    assert_eq!(event.seq, decoded.seq);
    assert!(matches!(
        decoded.kind,
        PaneEventKind::Exited(ExitedEvent { code: Some(0) })
    ));
}

#[cfg(feature = "serde")]
#[test]
fn serde_surface_changed_event_legacy_missing_title_changed_defaults_to_false() {
    // Regression test: payloads serialized before the `title_changed` field
    // existed must deserialize successfully with the field defaulting to false.
    let legacy = r#"{
            "generation": 1,
            "dirty_rows": "All",
            "cursor_changed": true,
            "modes_changed": false,
            "scrollback_changed": false
        }"#;
    let decoded: SurfaceChangedEvent =
        serde_json::from_str(legacy).expect("legacy payload should deserialize");
    assert!(!decoded.title_changed);
    assert_eq!(decoded.generation, 1);
    assert_eq!(decoded.dirty_rows, DirtyRows::All);
    assert!(decoded.cursor_changed);
    assert!(!decoded.modes_changed);
    assert!(!decoded.scrollback_changed);
}

#[cfg(feature = "serde")]
#[test]
fn serde_pane_state_roundtrips() {
    let states = [
        PaneState::Starting,
        PaneState::Running,
        PaneState::Exited { code: Some(42) },
        PaneState::Failed {
            error: PaneError::Spawn {
                message: "boom".into(),
            },
        },
        PaneState::Killed {
            reason: KillReason::ConfigLimit,
        },
    ];

    for state in &states {
        let json = serde_json::to_string(state).expect("serialize should succeed");
        let decoded: PaneState = serde_json::from_str(&json).expect("deserialize should succeed");
        assert_eq!(*state, decoded);
    }
}

#[cfg(feature = "serde")]
#[test]
fn serde_pane_interaction_mode_roundtrips() {
    let modes = [
        PaneInteractionMode::Embedded,
        PaneInteractionMode::Attaching,
        PaneInteractionMode::Attached,
        PaneInteractionMode::Detaching,
    ];

    for mode in &modes {
        let json = serde_json::to_string(mode).expect("serialize should succeed");
        let decoded: PaneInteractionMode =
            serde_json::from_str(&json).expect("deserialize should succeed");
        assert_eq!(*mode, decoded);
    }
}

#[cfg(feature = "serde")]
#[test]
fn serde_pane_command_roundtrips() {
    let pane_id = PaneId::new(7);
    let size = Size::new(24, 80);

    let commands = [
        PaneCommand::Spawn(PaneConfig::command("sh")),
        PaneCommand::Resize { pane_id, size },
        PaneCommand::WriteBytes {
            pane_id,
            bytes: vec![0x0d, 0x0a],
        },
        PaneCommand::Input {
            pane_id,
            input: HostInput::Raw(vec![0x61]),
        },
        PaneCommand::Kill {
            pane_id,
            reason: KillReason::UserRequested,
        },
        PaneCommand::Attach {
            pane_id,
            options: AttachOptions::default(),
        },
        PaneCommand::Detach {
            pane_id,
            reason: DetachReason::HostRequested,
        },
    ];

    for cmd in &commands {
        let json = serde_json::to_string(cmd).expect("serialize should succeed");
        let decoded: PaneCommand = serde_json::from_str(&json).expect("deserialize should succeed");
        assert_eq!(*cmd, decoded);
    }
}
