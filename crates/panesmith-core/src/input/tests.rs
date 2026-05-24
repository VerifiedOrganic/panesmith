use super::*;

#[test]
fn host_input_variants_are_constructible() {
    let _key = HostInput::Key(KeyInput {
        code: KeyCode::Char('a'),
        modifiers: KeyModifiers::default(),
        kind: KeyEventKind::Press,
    });
    let _mouse = HostInput::Mouse(MouseInput {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 5,
        row: 10,
        modifiers: KeyModifiers::default(),
    });
    let _paste = HostInput::Paste("hello\nworld".into());
    let _focus_gained = HostInput::FocusGained;
    let _focus_lost = HostInput::FocusLost;
    let _resize = HostInput::Resize(Size::new(24, 80));
    let _raw = HostInput::Raw(vec![0x01, 0x02]);
}

#[test]
fn key_modifiers_bitfield_combinations() {
    let m = KeyModifiers {
        shift: true,
        control: true,
        alt: false,
        super_key: true,
        hyper: false,
        meta: true,
    };
    assert!(m.shift);
    assert!(m.control);
    assert!(!m.alt);
    assert!(m.super_key);
    assert!(!m.hyper);
    assert!(m.meta);
}

#[test]
fn mouse_event_kind_variants() {
    let _down = MouseEventKind::Down(MouseButton::Left);
    let _up = MouseEventKind::Up(MouseButton::Right);
    let _drag = MouseEventKind::Drag(MouseButton::Middle);
    let _moved = MouseEventKind::Moved;
    let _scroll_down = MouseEventKind::ScrollDown;
    let _scroll_up = MouseEventKind::ScrollUp;
    let _scroll_left = MouseEventKind::ScrollLeft;
    let _scroll_right = MouseEventKind::ScrollRight;
}

#[test]
fn key_code_media_and_modifier_variants() {
    let _media = KeyCode::Media(MediaKeyCode::Play);
    let _modifier = KeyCode::Modifier(ModifierKeyCode::LeftAlt);
    let _f = KeyCode::F(12);
    let _char = KeyCode::Char('ñ');
}

#[test]
fn unsupported_event_error_implements_error() {
    let err = UnsupportedEventError {
        message: "test".into(),
    };
    let display = format!("{err}");
    assert!(display.contains("unsupported event"));
    assert!(display.contains("test"));
}

#[cfg(feature = "crossterm")]
mod crossterm_tests {
    use super::*;
    use crossterm::event::{
        Event as CtEvent, KeyCode as CtKeyCode, KeyEvent as CtKeyEvent,
        KeyEventKind as CtKeyEventKind, KeyModifiers as CtKeyModifiers,
        MouseButton as CtMouseButton, MouseEvent as CtMouseEvent,
        MouseEventKind as CtMouseEventKind,
    };

    #[test]
    fn key_char_converts() {
        let ct = CtEvent::Key(CtKeyEvent::from(CtKeyCode::Char('x')));
        let host: HostInput = ct.try_into().expect("char key should convert");
        assert!(
            matches!(
                host,
                HostInput::Key(KeyInput {
                    code: KeyCode::Char('x'),
                    modifiers: KeyModifiers {
                        shift: false,
                        control: false,
                        alt: false,
                        ..
                    },
                    kind: KeyEventKind::Press,
                })
            ),
            "got {host:?}"
        );
    }

    #[test]
    fn key_with_modifiers_converts() {
        let ct = CtEvent::Key(CtKeyEvent::new(
            CtKeyCode::Char('c'),
            CtKeyModifiers::CONTROL,
        ));
        let host: HostInput = ct.try_into().expect("ctrl-c should convert");
        assert!(
            matches!(
                host,
                HostInput::Key(KeyInput {
                    code: KeyCode::Char('c'),
                    modifiers: KeyModifiers { control: true, .. },
                    kind: KeyEventKind::Press,
                })
            ),
            "got {host:?}"
        );
    }

    #[test]
    fn arrow_keys_convert() {
        for (ct_code, expected) in [
            (CtKeyCode::Up, KeyCode::Up),
            (CtKeyCode::Down, KeyCode::Down),
            (CtKeyCode::Left, KeyCode::Left),
            (CtKeyCode::Right, KeyCode::Right),
            (CtKeyCode::Home, KeyCode::Home),
            (CtKeyCode::End, KeyCode::End),
            (CtKeyCode::PageUp, KeyCode::PageUp),
            (CtKeyCode::PageDown, KeyCode::PageDown),
            (CtKeyCode::Insert, KeyCode::Insert),
            (CtKeyCode::Delete, KeyCode::Delete),
            (CtKeyCode::Backspace, KeyCode::Backspace),
            (CtKeyCode::Enter, KeyCode::Enter),
            (CtKeyCode::Tab, KeyCode::Tab),
            (CtKeyCode::Esc, KeyCode::Esc),
        ] {
            let ct = CtEvent::Key(CtKeyEvent::from(ct_code));
            let host: HostInput = ct.try_into().expect("arrow key should convert");
            assert!(
                matches!(
                    host,
                    HostInput::Key(KeyInput {
                        code: ref c,
                        modifiers: KeyModifiers { shift: false, control: false, alt: false, .. },
                        kind: KeyEventKind::Press,
                    }) if *c == expected
                ),
                "expected {expected:?} for crossterm {ct_code:?}, got {host:?}"
            );
        }
    }

    #[test]
    fn function_keys_convert() {
        for n in 1..=24 {
            let ct = CtEvent::Key(CtKeyEvent::from(CtKeyCode::F(n)));
            let host: HostInput = ct.try_into().expect("F-key should convert");
            assert!(
                matches!(host, HostInput::Key(KeyInput { code: KeyCode::F(f), .. }) if f == n),
                "F{n} did not convert correctly: {host:?}"
            );
        }
    }

    #[test]
    fn paste_converts_and_preserves_text() {
        let text = "line1\nline2\ttabbed";
        let ct = CtEvent::Paste(text.into());
        let host: HostInput = ct.try_into().expect("paste should convert");
        assert!(
            matches!(host, HostInput::Paste(ref s) if s == text),
            "paste text should be preserved, got {host:?}"
        );
    }

    #[test]
    fn focus_events_convert() {
        let gained: HostInput = CtEvent::FocusGained.try_into().unwrap();
        assert!(matches!(gained, HostInput::FocusGained));

        let lost: HostInput = CtEvent::FocusLost.try_into().unwrap();
        assert!(matches!(lost, HostInput::FocusLost));
    }

    #[test]
    fn resize_converts_dimensions() {
        let ct = CtEvent::Resize(100, 50);
        let host: HostInput = ct.try_into().expect("resize should convert");
        assert!(
            matches!(
                host,
                HostInput::Resize(Size {
                    rows: 50,
                    cols: 100
                })
            ),
            "crossterm Resize(cols, rows) should map to Size {{ rows, cols }}, got {host:?}"
        );
    }

    #[test]
    fn mouse_down_converts() {
        let ct = CtEvent::Mouse(CtMouseEvent {
            kind: CtMouseEventKind::Down(CtMouseButton::Left),
            column: 10,
            row: 20,
            modifiers: CtKeyModifiers::SHIFT,
        });
        let host: HostInput = ct.try_into().expect("mouse down should convert");
        assert!(
            matches!(
                host,
                HostInput::Mouse(MouseInput {
                    kind: MouseEventKind::Down(MouseButton::Left),
                    column: 10,
                    row: 20,
                    modifiers: KeyModifiers { shift: true, .. },
                })
            ),
            "got {host:?}"
        );
    }

    #[test]
    fn mouse_scroll_converts() {
        let ct = CtEvent::Mouse(CtMouseEvent {
            kind: CtMouseEventKind::ScrollUp,
            column: 0,
            row: 0,
            modifiers: CtKeyModifiers::empty(),
        });
        let host: HostInput = ct.try_into().expect("mouse scroll should convert");
        assert!(
            matches!(
                host,
                HostInput::Mouse(MouseInput {
                    kind: MouseEventKind::ScrollUp,
                    column: 0,
                    row: 0,
                    modifiers: KeyModifiers {
                        shift: false,
                        control: false,
                        alt: false,
                        ..
                    },
                })
            ),
            "got {host:?}"
        );
    }

    #[test]
    fn key_release_kind_converts() {
        let ct = CtEvent::Key(CtKeyEvent {
            code: CtKeyCode::Char('a'),
            modifiers: CtKeyModifiers::empty(),
            kind: CtKeyEventKind::Release,
            state: crossterm::event::KeyEventState::empty(),
        });
        let host: HostInput = ct.try_into().expect("release key should convert");
        assert!(
            matches!(
                host,
                HostInput::Key(KeyInput {
                    kind: KeyEventKind::Release,
                    ..
                })
            ),
            "got {host:?}"
        );
    }

    #[test]
    fn resize_zero_rows_is_rejected() {
        let ct = CtEvent::Resize(80, 0);
        let result: std::result::Result<HostInput, _> = ct.try_into();
        assert!(
            result.is_err(),
            "resize with zero rows should be rejected, got {result:?}"
        );
        let err = result.unwrap_err();
        assert!(err.message.contains("resize with invalid dimensions"));
        assert!(err.message.contains("rows=0"));
    }

    #[test]
    fn resize_zero_cols_is_rejected() {
        let ct = CtEvent::Resize(0, 24);
        let result: std::result::Result<HostInput, _> = ct.try_into();
        assert!(
            result.is_err(),
            "resize with zero cols should be rejected, got {result:?}"
        );
        let err = result.unwrap_err();
        assert!(err.message.contains("resize with invalid dimensions"));
        assert!(err.message.contains("cols=0"));
    }

    #[test]
    fn resize_both_zero_is_rejected() {
        let ct = CtEvent::Resize(0, 0);
        let result: std::result::Result<HostInput, _> = ct.try_into();
        assert!(
            result.is_err(),
            "resize with both zero should be rejected, got {result:?}"
        );
    }

    #[test]
    fn key_repeat_kind_converts() {
        let ct = CtEvent::Key(CtKeyEvent {
            code: CtKeyCode::Char('a'),
            modifiers: CtKeyModifiers::empty(),
            kind: CtKeyEventKind::Repeat,
            state: crossterm::event::KeyEventState::empty(),
        });
        let host: HostInput = ct.try_into().expect("repeat key should convert");
        assert!(
            matches!(
                host,
                HostInput::Key(KeyInput {
                    kind: KeyEventKind::Repeat,
                    ..
                })
            ),
            "got {host:?}"
        );
    }

    #[test]
    fn media_key_code_converts() {
        let ct = CtEvent::Key(CtKeyEvent::from(CtKeyCode::Media(
            crossterm::event::MediaKeyCode::Play,
        )));
        let host: HostInput = ct.try_into().expect("media key should convert");
        assert!(
            matches!(
                host,
                HostInput::Key(KeyInput {
                    code: KeyCode::Media(MediaKeyCode::Play),
                    ..
                })
            ),
            "got {host:?}"
        );
    }

    #[test]
    fn modifier_key_code_converts() {
        let ct = CtEvent::Key(CtKeyEvent::from(CtKeyCode::Modifier(
            crossterm::event::ModifierKeyCode::LeftAlt,
        )));
        let host: HostInput = ct.try_into().expect("modifier key should convert");
        assert!(
            matches!(
                host,
                HostInput::Key(KeyInput {
                    code: KeyCode::Modifier(ModifierKeyCode::LeftAlt),
                    ..
                })
            ),
            "got {host:?}"
        );
    }

    #[test]
    fn mouse_drag_converts() {
        let ct = CtEvent::Mouse(CtMouseEvent {
            kind: CtMouseEventKind::Drag(CtMouseButton::Right),
            column: 3,
            row: 4,
            modifiers: CtKeyModifiers::CONTROL,
        });
        let host: HostInput = ct.try_into().expect("mouse drag should convert");
        assert!(
            matches!(
                host,
                HostInput::Mouse(MouseInput {
                    kind: MouseEventKind::Drag(MouseButton::Right),
                    column: 3,
                    row: 4,
                    modifiers: KeyModifiers { control: true, .. },
                })
            ),
            "got {host:?}"
        );
    }

    #[test]
    fn mouse_moved_converts() {
        let ct = CtEvent::Mouse(CtMouseEvent {
            kind: CtMouseEventKind::Moved,
            column: 7,
            row: 8,
            modifiers: CtKeyModifiers::empty(),
        });
        let host: HostInput = ct.try_into().expect("mouse moved should convert");
        assert!(
            matches!(
                host,
                HostInput::Mouse(MouseInput {
                    kind: MouseEventKind::Moved,
                    column: 7,
                    row: 8,
                    modifiers: KeyModifiers {
                        shift: false,
                        control: false,
                        alt: false,
                        ..
                    },
                })
            ),
            "got {host:?}"
        );
    }

    #[test]
    fn mouse_scroll_left_right_converts() {
        let left = CtEvent::Mouse(CtMouseEvent {
            kind: CtMouseEventKind::ScrollLeft,
            column: 0,
            row: 0,
            modifiers: CtKeyModifiers::empty(),
        });
        let host: HostInput = left.try_into().expect("scroll left should convert");
        assert!(
            matches!(
                host,
                HostInput::Mouse(MouseInput {
                    kind: MouseEventKind::ScrollLeft,
                    ..
                })
            ),
            "got {host:?}"
        );

        let right = CtEvent::Mouse(CtMouseEvent {
            kind: CtMouseEventKind::ScrollRight,
            column: 0,
            row: 0,
            modifiers: CtKeyModifiers::empty(),
        });
        let host: HostInput = right.try_into().expect("scroll right should convert");
        assert!(
            matches!(
                host,
                HostInput::Mouse(MouseInput {
                    kind: MouseEventKind::ScrollRight,
                    ..
                })
            ),
            "got {host:?}"
        );
    }

    #[test]
    fn raw_bytes_variant_still_works() {
        let input = HostInput::Raw(vec![0x1b, 0x5b, 0x41]);
        assert!(matches!(input, HostInput::Raw(ref b) if b == &[0x1b, 0x5b, 0x41]));
    }
}

#[cfg(feature = "serde")]
mod serde_tests {
    use super::*;

    #[test]
    fn serde_host_input_key_roundtrips() {
        let input = HostInput::Key(KeyInput {
            code: KeyCode::Char('x'),
            modifiers: KeyModifiers {
                shift: true,
                control: false,
                alt: true,
                super_key: false,
                hyper: false,
                meta: false,
            },
            kind: KeyEventKind::Press,
        });
        let json = serde_json::to_string(&input).expect("serialize should succeed");
        let decoded: HostInput = serde_json::from_str(&json).expect("deserialize should succeed");
        assert_eq!(input, decoded);
    }

    #[test]
    fn serde_host_input_mouse_roundtrips() {
        let input = HostInput::Mouse(MouseInput {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 10,
            row: 20,
            modifiers: KeyModifiers::default(),
        });
        let json = serde_json::to_string(&input).expect("serialize should succeed");
        let decoded: HostInput = serde_json::from_str(&json).expect("deserialize should succeed");
        assert_eq!(input, decoded);
    }

    #[test]
    fn serde_host_input_paste_roundtrips() {
        let input = HostInput::Paste("hello\nworld".into());
        let json = serde_json::to_string(&input).expect("serialize should succeed");
        let decoded: HostInput = serde_json::from_str(&json).expect("deserialize should succeed");
        assert_eq!(input, decoded);
    }

    #[test]
    fn serde_host_input_focus_gained_roundtrips() {
        let input = HostInput::FocusGained;
        let json = serde_json::to_string(&input).expect("serialize should succeed");
        let decoded: HostInput = serde_json::from_str(&json).expect("deserialize should succeed");
        assert_eq!(input, decoded);
    }

    #[test]
    fn serde_host_input_resize_roundtrips() {
        let input = HostInput::Resize(Size::new(24, 80));
        let json = serde_json::to_string(&input).expect("serialize should succeed");
        let decoded: HostInput = serde_json::from_str(&json).expect("deserialize should succeed");
        assert_eq!(input, decoded);
    }

    #[test]
    fn serde_key_modifiers_roundtrips() {
        let mods = KeyModifiers {
            shift: true,
            control: true,
            alt: false,
            super_key: true,
            hyper: false,
            meta: true,
        };
        let json = serde_json::to_string(&mods).expect("serialize should succeed");
        let decoded: KeyModifiers =
            serde_json::from_str(&json).expect("deserialize should succeed");
        assert_eq!(mods, decoded);
    }
}
