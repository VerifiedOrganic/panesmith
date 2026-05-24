//! Supplementary terminal mode tracking for parser gaps.

use crate::{MouseMode, TerminalModes};

/// Tracks terminal modes that a surface parser may not expose directly.
///
/// This is hidden because it is shared by workspace crates, not intended as a
/// stable application-facing abstraction.
#[doc(hidden)]
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TerminalModeOverlay {
    modes: TerminalModes,
    mouse_modes: MouseModeFlags,
    /// Tail buffer for escape sequences split across feed boundaries.
    tail: Vec<u8>,
}

impl TerminalModeOverlay {
    /// Scans raw output for terminal mode set/reset sequences.
    pub fn update_from_output(&mut self, bytes: &[u8]) -> bool {
        const MODE_TAIL_LIMIT: usize = 64;

        let before = self.modes;
        let mut data = std::mem::take(&mut self.tail);
        data.extend_from_slice(bytes);
        apply_terminal_mode_sequences(&data, &mut self.modes, &mut self.mouse_modes);

        let keep = data.len().min(MODE_TAIL_LIMIT);
        self.tail = data[data.len().saturating_sub(keep)..].to_vec();
        self.modes != before
    }

    /// Merges tracked modes on top of modes reported by a surface backend.
    pub fn merge_with(&self, surface_modes: TerminalModes) -> TerminalModes {
        TerminalModes {
            bracketed_paste: surface_modes.bracketed_paste || self.modes.bracketed_paste,
            focus_events: surface_modes.focus_events || self.modes.focus_events,
            application_cursor: surface_modes.application_cursor || self.modes.application_cursor,
            alternate_screen: surface_modes.alternate_screen || self.modes.alternate_screen,
            mouse: if self.modes.mouse != MouseMode::None {
                self.modes.mouse
            } else {
                surface_modes.mouse
            },
        }
    }
}

/// Tracks individual mouse-mode enable/disable flags so that the highest-priority
/// active mode can be computed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct MouseModeFlags {
    x10: bool,
    normal: bool,
    button_event: bool,
    any_event: bool,
    sgr: bool,
}

impl MouseModeFlags {
    fn apply_private_mode(&mut self, code: u16, enabled: bool) -> bool {
        match code {
            9 => self.x10 = enabled,
            1000 => self.normal = enabled,
            1002 => self.button_event = enabled,
            1003 => self.any_event = enabled,
            1006 => self.sgr = enabled,
            _ => return false,
        }
        true
    }

    fn active_mode(self) -> MouseMode {
        if self.sgr {
            MouseMode::Sgr
        } else if self.any_event {
            MouseMode::AnyEvent
        } else if self.button_event {
            MouseMode::ButtonEvent
        } else if self.normal {
            MouseMode::Normal
        } else if self.x10 {
            MouseMode::X10
        } else {
            MouseMode::None
        }
    }
}

fn apply_terminal_mode_sequences(
    bytes: &[u8],
    modes: &mut TerminalModes,
    mouse_modes: &mut MouseModeFlags,
) {
    let mut index = 0;
    while let Some(relative_esc) = bytes[index..].iter().position(|&byte| byte == 0x1b) {
        let esc = index + relative_esc;
        if bytes[esc..].starts_with(b"\x1bc") {
            *modes = TerminalModes::default();
            *mouse_modes = MouseModeFlags::default();
            index = esc + 2;
            continue;
        }
        if !bytes[esc..].starts_with(b"\x1b[?") {
            index = esc + 1;
            continue;
        }

        let mut cursor = esc + 3;
        let params_start = cursor;
        while cursor < bytes.len() && (bytes[cursor].is_ascii_digit() || bytes[cursor] == b';') {
            cursor += 1;
        }

        if cursor >= bytes.len() {
            break;
        }

        let action = bytes[cursor];
        if action != b'h' && action != b'l' {
            index = esc + 1;
            continue;
        }

        let params = &bytes[params_start..cursor];
        if params.is_empty()
            || params.starts_with(b";")
            || params.ends_with(b";")
            || params
                .split(|&byte| byte == b';')
                .any(|part| part.is_empty())
        {
            index = esc + 1;
            continue;
        }

        let enabled = action == b'h';
        for part in params.split(|&byte| byte == b';') {
            if let Some(code) = parse_private_mode(part) {
                apply_terminal_mode(code, enabled, modes, mouse_modes);
            }
        }

        index = cursor + 1;
    }
}

fn parse_private_mode(part: &[u8]) -> Option<u16> {
    let mut value = 0_u16;
    for &digit in part {
        value = value.checked_mul(10)?;
        value = value.checked_add(u16::from(digit.checked_sub(b'0')?))?;
    }
    Some(value)
}

fn apply_terminal_mode(
    code: u16,
    enabled: bool,
    modes: &mut TerminalModes,
    mouse_modes: &mut MouseModeFlags,
) {
    if mouse_modes.apply_private_mode(code, enabled) {
        modes.mouse = mouse_modes.active_mode();
        return;
    }

    match code {
        1 => modes.application_cursor = enabled,
        1004 => modes.focus_events = enabled,
        47 | 1047 | 1049 => modes.alternate_screen = enabled,
        2004 => modes.bracketed_paste = enabled,
        _ => {}
    }
}

#[cfg(test)]
mod tests;
