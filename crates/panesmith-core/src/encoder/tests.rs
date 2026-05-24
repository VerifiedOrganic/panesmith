use super::*;
use crate::{
    HostInput, KeyCode, KeyEventKind, KeyInput, KeyModifiers, MouseButton, MouseEventKind,
    MouseInput,
};

// ------------------------------------------------------------------
// Helpers
// ------------------------------------------------------------------

fn default_modes() -> TerminalModes {
    TerminalModes::default()
}

fn modes_with_bracketed_paste() -> TerminalModes {
    TerminalModes {
        bracketed_paste: true,
        ..default_modes()
    }
}

fn modes_with_focus_events() -> TerminalModes {
    TerminalModes {
        focus_events: true,
        ..default_modes()
    }
}

fn modes_with_app_cursor() -> TerminalModes {
    TerminalModes {
        application_cursor: true,
        ..default_modes()
    }
}

fn modes_with_sgr_mouse() -> TerminalModes {
    TerminalModes {
        mouse: MouseMode::Sgr,
        ..default_modes()
    }
}

fn key(code: KeyCode) -> KeyInput {
    KeyInput {
        code,
        modifiers: KeyModifiers::default(),
        kind: KeyEventKind::Press,
    }
}

fn key_ctrl(code: KeyCode) -> KeyInput {
    KeyInput {
        code,
        modifiers: KeyModifiers {
            control: true,
            ..KeyModifiers::default()
        },
        kind: KeyEventKind::Press,
    }
}

fn key_alt(code: KeyCode) -> KeyInput {
    KeyInput {
        code,
        modifiers: KeyModifiers {
            alt: true,
            ..KeyModifiers::default()
        },
        kind: KeyEventKind::Press,
    }
}

fn key_shift(code: KeyCode) -> KeyInput {
    KeyInput {
        code,
        modifiers: KeyModifiers {
            shift: true,
            ..KeyModifiers::default()
        },
        kind: KeyEventKind::Press,
    }
}

fn encoder() -> XtermEncoder {
    XtermEncoder::default()
}

// ------------------------------------------------------------------
// Printable characters
// ------------------------------------------------------------------

#[test]
fn ascii_char_encodes_as_utf8() {
    let enc = encoder();
    assert_eq!(
        enc.encode_key(&key(KeyCode::Char('a')), &default_modes())
            .unwrap(),
        b"a"
    );
    assert_eq!(
        enc.encode_key(&key(KeyCode::Char('Z')), &default_modes())
            .unwrap(),
        b"Z"
    );
    assert_eq!(
        enc.encode_key(&key(KeyCode::Char('1')), &default_modes())
            .unwrap(),
        b"1"
    );
}

#[test]
fn unicode_char_encodes_as_utf8() {
    let enc = encoder();
    assert_eq!(
        enc.encode_key(&key(KeyCode::Char('ñ')), &default_modes())
            .unwrap(),
        "ñ".as_bytes()
    );
    assert_eq!(
        enc.encode_key(&key(KeyCode::Char('€')), &default_modes())
            .unwrap(),
        "€".as_bytes()
    );
}

// ------------------------------------------------------------------
// Control keys
// ------------------------------------------------------------------

#[test]
fn ctrl_a_through_z() {
    let enc = encoder();
    for (ch, expected) in (b'a'..=b'z').zip(0x01u8..=0x1a) {
        let result = enc
            .encode_key(&key_ctrl(KeyCode::Char(ch as char)), &default_modes())
            .unwrap();
        assert_eq!(
            result,
            &[expected],
            "Ctrl+{} should be 0x{:02x}",
            ch as char,
            expected
        );
    }
}

#[test]
fn ctrl_brackets_and_misc() {
    let enc = encoder();
    assert_eq!(
        enc.encode_key(&key_ctrl(KeyCode::Char('[')), &default_modes())
            .unwrap(),
        &[0x1b]
    );
    assert_eq!(
        enc.encode_key(&key_ctrl(KeyCode::Char('\\')), &default_modes())
            .unwrap(),
        &[0x1c]
    );
    assert_eq!(
        enc.encode_key(&key_ctrl(KeyCode::Char(']')), &default_modes())
            .unwrap(),
        &[0x1d]
    );
    assert_eq!(
        enc.encode_key(&key_ctrl(KeyCode::Char('^')), &default_modes())
            .unwrap(),
        &[0x1e]
    );
    assert_eq!(
        enc.encode_key(&key_ctrl(KeyCode::Char('_')), &default_modes())
            .unwrap(),
        &[0x1f]
    );
    assert_eq!(
        enc.encode_key(&key_ctrl(KeyCode::Char('@')), &default_modes())
            .unwrap(),
        &[0x00]
    );
    assert_eq!(
        enc.encode_key(&key_ctrl(KeyCode::Char(' ')), &default_modes())
            .unwrap(),
        &[0x00]
    );
}

// ------------------------------------------------------------------
// Enter and Backspace policies
// ------------------------------------------------------------------

#[test]
fn enter_default_is_cr() {
    let enc = encoder();
    assert_eq!(
        enc.encode_key(&key(KeyCode::Enter), &default_modes())
            .unwrap(),
        b"\r"
    );
}

#[test]
fn enter_lf_encoding() {
    let enc = XtermEncoder::new(InputConfig {
        enter: EnterEncoding::Lf,
        ..InputConfig::default()
    });
    assert_eq!(
        enc.encode_key(&key(KeyCode::Enter), &default_modes())
            .unwrap(),
        b"\n"
    );
}

#[test]
fn enter_crlf_encoding() {
    let enc = XtermEncoder::new(InputConfig {
        enter: EnterEncoding::CrLf,
        ..InputConfig::default()
    });
    assert_eq!(
        enc.encode_key(&key(KeyCode::Enter), &default_modes())
            .unwrap(),
        b"\r\n"
    );
}

#[test]
fn backspace_default_is_del() {
    let enc = encoder();
    assert_eq!(
        enc.encode_key(&key(KeyCode::Backspace), &default_modes())
            .unwrap(),
        &[0x7f]
    );
}

#[test]
fn backspace_bs_encoding() {
    let enc = XtermEncoder::new(InputConfig {
        backspace: BackspaceEncoding::Bs,
        ..InputConfig::default()
    });
    assert_eq!(
        enc.encode_key(&key(KeyCode::Backspace), &default_modes())
            .unwrap(),
        &[0x08]
    );
}

// ------------------------------------------------------------------
// Tab and Escape
// ------------------------------------------------------------------

#[test]
fn tab_is_literal_tab() {
    let enc = encoder();
    assert_eq!(
        enc.encode_key(&key(KeyCode::Tab), &default_modes())
            .unwrap(),
        b"\t"
    );
}

#[test]
fn shift_tab_is_backtab_sequence() {
    let enc = encoder();
    assert_eq!(
        enc.encode_key(&key_shift(KeyCode::Tab), &default_modes())
            .unwrap(),
        b"\x1b[Z"
    );
}

#[test]
fn escape_is_literal_esc() {
    let enc = encoder();
    assert_eq!(
        enc.encode_key(&key(KeyCode::Esc), &default_modes())
            .unwrap(),
        &[0x1b]
    );
}

// ------------------------------------------------------------------
// Arrow keys
// ------------------------------------------------------------------

#[test]
fn arrows_normal_cursor_mode() {
    let enc = encoder();
    assert_eq!(
        enc.encode_key(&key(KeyCode::Up), &default_modes()).unwrap(),
        b"\x1b[A"
    );
    assert_eq!(
        enc.encode_key(&key(KeyCode::Down), &default_modes())
            .unwrap(),
        b"\x1b[B"
    );
    assert_eq!(
        enc.encode_key(&key(KeyCode::Right), &default_modes())
            .unwrap(),
        b"\x1b[C"
    );
    assert_eq!(
        enc.encode_key(&key(KeyCode::Left), &default_modes())
            .unwrap(),
        b"\x1b[D"
    );
}

#[test]
fn arrows_application_cursor_mode() {
    let enc = encoder();
    let modes = modes_with_app_cursor();
    assert_eq!(
        enc.encode_key(&key(KeyCode::Up), &modes).unwrap(),
        b"\x1bOA"
    );
    assert_eq!(
        enc.encode_key(&key(KeyCode::Down), &modes).unwrap(),
        b"\x1bOB"
    );
    assert_eq!(
        enc.encode_key(&key(KeyCode::Right), &modes).unwrap(),
        b"\x1bOC"
    );
    assert_eq!(
        enc.encode_key(&key(KeyCode::Left), &modes).unwrap(),
        b"\x1bOD"
    );
}

#[test]
fn arrows_respect_config_app_cursor() {
    let enc = XtermEncoder::new(InputConfig {
        application_cursor_keys: true,
        ..InputConfig::default()
    });
    assert_eq!(
        enc.encode_key(&key(KeyCode::Up), &default_modes()).unwrap(),
        b"\x1bOA"
    );
}

// ------------------------------------------------------------------
// Navigation keys
// ------------------------------------------------------------------

#[test]
fn home_and_end_normal_mode() {
    let enc = encoder();
    assert_eq!(
        enc.encode_key(&key(KeyCode::Home), &default_modes())
            .unwrap(),
        b"\x1b[H"
    );
    assert_eq!(
        enc.encode_key(&key(KeyCode::End), &default_modes())
            .unwrap(),
        b"\x1b[F"
    );
}

#[test]
fn home_and_end_application_mode() {
    let enc = encoder();
    let modes = modes_with_app_cursor();
    assert_eq!(
        enc.encode_key(&key(KeyCode::Home), &modes).unwrap(),
        b"\x1bOH"
    );
    assert_eq!(
        enc.encode_key(&key(KeyCode::End), &modes).unwrap(),
        b"\x1bOF"
    );
}

#[test]
fn page_up_down_insert_delete() {
    let enc = encoder();
    assert_eq!(
        enc.encode_key(&key(KeyCode::PageUp), &default_modes())
            .unwrap(),
        b"\x1b[5~"
    );
    assert_eq!(
        enc.encode_key(&key(KeyCode::PageDown), &default_modes())
            .unwrap(),
        b"\x1b[6~"
    );
    assert_eq!(
        enc.encode_key(&key(KeyCode::Insert), &default_modes())
            .unwrap(),
        b"\x1b[2~"
    );
    assert_eq!(
        enc.encode_key(&key(KeyCode::Delete), &default_modes())
            .unwrap(),
        b"\x1b[3~"
    );
}

// ------------------------------------------------------------------
// Alt prefix
// ------------------------------------------------------------------

#[test]
fn alt_char_prefixes_with_esc() {
    let enc = encoder();
    assert_eq!(
        enc.encode_key(&key_alt(KeyCode::Char('x')), &default_modes())
            .unwrap(),
        b"\x1bx"
    );
}

#[test]
fn alt_arrow_prefixes_with_esc() {
    let enc = encoder();
    assert_eq!(
        enc.encode_key(&key_alt(KeyCode::Up), &default_modes())
            .unwrap(),
        b"\x1b\x1b[A"
    );
}

#[test]
fn alt_prefix_disabled_when_alt_sends_escape_is_false() {
    let enc = XtermEncoder::new(InputConfig {
        alt_sends_escape: false,
        ..InputConfig::default()
    });
    assert_eq!(
        enc.encode_key(&key_alt(KeyCode::Char('x')), &default_modes())
            .unwrap(),
        b"x"
    );
}

// ------------------------------------------------------------------
// Key release is ignored
// ------------------------------------------------------------------

#[test]
fn key_release_produces_nothing() {
    let enc = encoder();
    let release = KeyInput {
        code: KeyCode::Char('a'),
        modifiers: KeyModifiers::default(),
        kind: KeyEventKind::Release,
    };
    assert_eq!(enc.encode_key(&release, &default_modes()).unwrap(), b"");
}

// ------------------------------------------------------------------
// Paste
// ------------------------------------------------------------------

#[test]
fn paste_without_bracketed_paste_is_plain_text() {
    let enc = encoder();
    assert_eq!(
        enc.encode_paste("hello world", &default_modes()),
        b"hello world"
    );
}

#[test]
fn paste_preserves_newlines() {
    let enc = encoder();
    assert_eq!(
        enc.encode_paste("line1\nline2", &default_modes()),
        b"line1\nline2"
    );
}

#[test]
fn bracketed_paste_wraps_exactly() {
    let enc = encoder();
    let modes = modes_with_bracketed_paste();
    assert_eq!(
        enc.encode_paste("hello", &modes),
        b"\x1b[200~hello\x1b[201~"
    );
}

#[test]
fn bracketed_paste_preserves_newlines() {
    let enc = encoder();
    let modes = modes_with_bracketed_paste();
    assert_eq!(enc.encode_paste("a\nb", &modes), b"\x1b[200~a\nb\x1b[201~");
}

#[test]
fn paste_normalizes_to_lf() {
    let enc = XtermEncoder::new(InputConfig {
        paste_newline: PasteNewlinePolicy::NormalizeToLf,
        ..InputConfig::default()
    });
    assert_eq!(enc.encode_paste("a\r\nb\rc", &default_modes()), b"a\nb\nc");
}

#[test]
fn paste_normalizes_to_cr() {
    let enc = XtermEncoder::new(InputConfig {
        paste_newline: PasteNewlinePolicy::NormalizeToCr,
        ..InputConfig::default()
    });
    assert_eq!(enc.encode_paste("a\r\nb\nc", &default_modes()), b"a\rb\rc");
}

// ------------------------------------------------------------------
// Focus events
// ------------------------------------------------------------------

#[test]
fn focus_gained_encoded_when_enabled() {
    let enc = encoder();
    assert_eq!(
        enc.encode_focus_gained(&modes_with_focus_events()),
        b"\x1b[I"
    );
}

#[test]
fn focus_gained_omitted_when_disabled() {
    let enc = encoder();
    assert_eq!(enc.encode_focus_gained(&default_modes()), b"");
}

#[test]
fn focus_lost_encoded_when_enabled() {
    let enc = encoder();
    assert_eq!(enc.encode_focus_lost(&modes_with_focus_events()), b"\x1b[O");
}

#[test]
fn focus_lost_omitted_when_disabled() {
    let enc = encoder();
    assert_eq!(enc.encode_focus_lost(&default_modes()), b"");
}

// ------------------------------------------------------------------
// HostInput dispatch
// ------------------------------------------------------------------

#[test]
fn encode_dispatch_key() {
    let enc = encoder();
    let input = HostInput::Key(key(KeyCode::Char('x')));
    assert_eq!(enc.encode(&input, &default_modes()).unwrap(), b"x");
}

#[test]
fn encode_dispatch_paste() {
    let enc = encoder();
    let input = HostInput::Paste("hi".into());
    assert_eq!(
        enc.encode(&input, &modes_with_bracketed_paste()).unwrap(),
        b"\x1b[200~hi\x1b[201~"
    );
}

#[test]
fn encode_dispatch_focus_gained() {
    let enc = encoder();
    let input = HostInput::FocusGained;
    assert_eq!(
        enc.encode(&input, &modes_with_focus_events()).unwrap(),
        b"\x1b[I"
    );
}

#[test]
fn encode_dispatch_focus_lost() {
    let enc = encoder();
    let input = HostInput::FocusLost;
    assert_eq!(
        enc.encode(&input, &modes_with_focus_events()).unwrap(),
        b"\x1b[O"
    );
}

#[test]
fn encode_dispatch_raw() {
    let enc = encoder();
    let input = HostInput::Raw(vec![0x01, 0x02]);
    assert_eq!(
        enc.encode(&input, &default_modes()).unwrap(),
        vec![0x01, 0x02]
    );
}

#[test]
fn encode_dispatch_resize_is_empty() {
    let enc = encoder();
    let input = HostInput::Resize(crate::Size::new(10, 20));
    assert_eq!(enc.encode(&input, &default_modes()).unwrap(), b"");
}

#[test]
fn encode_dispatch_mouse() {
    let enc = encoder();
    let input = HostInput::Mouse(MouseInput {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 0,
        row: 0,
        modifiers: KeyModifiers::default(),
    });
    assert_eq!(
        enc.encode(&input, &modes_with_sgr_mouse()).unwrap(),
        b"\x1b[<0;1;1M"
    );
}

// ------------------------------------------------------------------
// Mouse encoding (SGR only in MVP)
// ------------------------------------------------------------------

#[test]
fn mouse_ignored_when_mode_is_none() {
    let enc = encoder();
    let mouse = MouseInput {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 5,
        row: 10,
        modifiers: KeyModifiers::default(),
    };
    assert_eq!(enc.encode_mouse(&mouse, &default_modes()).unwrap(), None);
}

#[test]
fn mouse_ignored_when_mode_is_not_sgr() {
    let enc = encoder();
    let mouse = MouseInput {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 5,
        row: 10,
        modifiers: KeyModifiers::default(),
    };
    let modes = TerminalModes {
        mouse: MouseMode::X10,
        ..default_modes()
    };
    assert_eq!(enc.encode_mouse(&mouse, &modes).unwrap(), None);
}

#[test]
fn sgr_mouse_down_left() {
    let enc = encoder();
    let mouse = MouseInput {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 0,
        row: 0,
        modifiers: KeyModifiers::default(),
    };
    let modes = TerminalModes {
        mouse: MouseMode::Sgr,
        ..default_modes()
    };
    assert_eq!(
        enc.encode_mouse(&mouse, &modes).unwrap(),
        Some(b"\x1b[<0;1;1M".to_vec())
    );
}

#[test]
fn sgr_mouse_down_right_with_modifiers() {
    let enc = encoder();
    let mouse = MouseInput {
        kind: MouseEventKind::Down(MouseButton::Right),
        column: 9,
        row: 4,
        modifiers: KeyModifiers {
            shift: true,
            ..KeyModifiers::default()
        },
    };
    let modes = TerminalModes {
        mouse: MouseMode::Sgr,
        ..default_modes()
    };
    assert_eq!(
        enc.encode_mouse(&mouse, &modes).unwrap(),
        Some(b"\x1b[<6;10;5M".to_vec())
    );
}

#[test]
fn sgr_mouse_up() {
    let enc = encoder();
    let mouse = MouseInput {
        kind: MouseEventKind::Up(MouseButton::Left),
        column: 2,
        row: 3,
        modifiers: KeyModifiers::default(),
    };
    let modes = TerminalModes {
        mouse: MouseMode::Sgr,
        ..default_modes()
    };
    assert_eq!(
        enc.encode_mouse(&mouse, &modes).unwrap(),
        Some(b"\x1b[<0;3;4m".to_vec())
    );
}

#[test]
fn sgr_mouse_scroll_down_no_modifiers() {
    let enc = encoder();
    let mouse = MouseInput {
        kind: MouseEventKind::ScrollDown,
        column: 0,
        row: 0,
        modifiers: KeyModifiers::default(),
    };
    let modes = TerminalModes {
        mouse: MouseMode::Sgr,
        ..default_modes()
    };
    assert_eq!(
        enc.encode_mouse(&mouse, &modes).unwrap(),
        Some(b"\x1b[<65;1;1M".to_vec())
    );
}

#[test]
fn sgr_mouse_scroll_up_no_modifiers() {
    let enc = encoder();
    let mouse = MouseInput {
        kind: MouseEventKind::ScrollUp,
        column: 0,
        row: 0,
        modifiers: KeyModifiers::default(),
    };
    let modes = TerminalModes {
        mouse: MouseMode::Sgr,
        ..default_modes()
    };
    assert_eq!(
        enc.encode_mouse(&mouse, &modes).unwrap(),
        Some(b"\x1b[<64;1;1M".to_vec())
    );
}

#[test]
fn sgr_mouse_scroll_down_with_shift() {
    let enc = encoder();
    let mouse = MouseInput {
        kind: MouseEventKind::ScrollDown,
        column: 2,
        row: 3,
        modifiers: KeyModifiers {
            shift: true,
            ..KeyModifiers::default()
        },
    };
    let modes = TerminalModes {
        mouse: MouseMode::Sgr,
        ..default_modes()
    };
    // 65 | 4 = 69
    assert_eq!(
        enc.encode_mouse(&mouse, &modes).unwrap(),
        Some(b"\x1b[<69;3;4M".to_vec())
    );
}

#[test]
fn sgr_mouse_scroll_up_with_control() {
    let enc = encoder();
    let mouse = MouseInput {
        kind: MouseEventKind::ScrollUp,
        column: 0,
        row: 0,
        modifiers: KeyModifiers {
            control: true,
            ..KeyModifiers::default()
        },
    };
    let modes = TerminalModes {
        mouse: MouseMode::Sgr,
        ..default_modes()
    };
    // 64 | 16 = 80
    assert_eq!(
        enc.encode_mouse(&mouse, &modes).unwrap(),
        Some(b"\x1b[<80;1;1M".to_vec())
    );
}

#[test]
fn sgr_mouse_moved_no_modifiers() {
    let enc = encoder();
    let mouse = MouseInput {
        kind: MouseEventKind::Moved,
        column: 5,
        row: 10,
        modifiers: KeyModifiers::default(),
    };
    let modes = TerminalModes {
        mouse: MouseMode::Sgr,
        ..default_modes()
    };
    assert_eq!(
        enc.encode_mouse(&mouse, &modes).unwrap(),
        Some(b"\x1b[<35;6;11M".to_vec())
    );
}

#[test]
fn sgr_mouse_moved_with_combined_modifiers() {
    let enc = encoder();
    let mouse = MouseInput {
        kind: MouseEventKind::Moved,
        column: 0,
        row: 0,
        modifiers: KeyModifiers {
            shift: true,
            alt: true,
            ..KeyModifiers::default()
        },
    };
    let modes = TerminalModes {
        mouse: MouseMode::Sgr,
        ..default_modes()
    };
    // 35 | 4 | 8 = 47
    assert_eq!(
        enc.encode_mouse(&mouse, &modes).unwrap(),
        Some(b"\x1b[<47;1;1M".to_vec())
    );
}

#[test]
fn sgr_mouse_drag_with_modifiers() {
    let enc = encoder();
    let mouse = MouseInput {
        kind: MouseEventKind::Drag(MouseButton::Left),
        column: 7,
        row: 8,
        modifiers: KeyModifiers {
            shift: true,
            control: true,
            ..KeyModifiers::default()
        },
    };
    let modes = TerminalModes {
        mouse: MouseMode::Sgr,
        ..default_modes()
    };
    // button 0 + drag (32) + shift (4) + control (16) = 52
    assert_eq!(
        enc.encode_mouse(&mouse, &modes).unwrap(),
        Some(b"\x1b[<52;8;9M".to_vec())
    );
}

#[test]
fn sgr_mouse_scroll_left_returns_none() {
    let enc = encoder();
    let mouse = MouseInput {
        kind: MouseEventKind::ScrollLeft,
        column: 0,
        row: 0,
        modifiers: KeyModifiers::default(),
    };
    let modes = TerminalModes {
        mouse: MouseMode::Sgr,
        ..default_modes()
    };
    assert_eq!(enc.encode_mouse(&mouse, &modes).unwrap(), None);
}

#[test]
fn sgr_mouse_scroll_right_returns_none() {
    let enc = encoder();
    let mouse = MouseInput {
        kind: MouseEventKind::ScrollRight,
        column: 0,
        row: 0,
        modifiers: KeyModifiers::default(),
    };
    let modes = TerminalModes {
        mouse: MouseMode::Sgr,
        ..default_modes()
    };
    assert_eq!(enc.encode_mouse(&mouse, &modes).unwrap(), None);
}

// ------------------------------------------------------------------
// Silent drop of unsupported keys
// ------------------------------------------------------------------

#[test]
fn unsupported_function_key_returns_empty() {
    let enc = encoder();
    assert_eq!(
        enc.encode_key(&key(KeyCode::F(25)), &default_modes())
            .unwrap(),
        b""
    );
}

#[test]
fn media_key_returns_empty() {
    let enc = encoder();
    assert_eq!(
        enc.encode_key(
            &key(KeyCode::Media(crate::MediaKeyCode::Play)),
            &default_modes()
        )
        .unwrap(),
        b""
    );
}

#[test]
fn modifier_key_returns_empty() {
    let enc = encoder();
    assert_eq!(
        enc.encode_key(
            &key(KeyCode::Modifier(crate::ModifierKeyCode::LeftAlt)),
            &default_modes()
        )
        .unwrap(),
        b""
    );
}

// ------------------------------------------------------------------
// BackTab standalone
// ------------------------------------------------------------------

#[test]
fn backtab_standalone_code() {
    let enc = encoder();
    assert_eq!(
        enc.encode_key(&key(KeyCode::BackTab), &default_modes())
            .unwrap(),
        b"\x1b[Z"
    );
}
