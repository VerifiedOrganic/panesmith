//! Xterm-compatible input encoder.
//!
//! Translates [`HostInput`] events into byte sequences suitable for writing to
//! a child PTY. The encoder respects [`TerminalModes`] so that features like
//! bracketed paste and focus events are only emitted when the child has
//! enabled them.

use crate::{
    BackspaceEncoding, EnterEncoding, InputConfig, KeyCode, KeyEventKind, KeyInput, MouseEventKind,
    MouseInput, MouseMode, PaneError, PasteNewlinePolicy, TerminalModes,
};

/// Trait for encoding host input into terminal byte sequences.
///
/// Implementations decide how keys, paste, and focus events are translated
/// based on the current terminal modes.
pub trait InputEncoder {
    /// Encodes a keyboard event into terminal bytes.
    ///
    /// Unsupported keys (e.g. media keys, unmapped function keys) and key
    /// releases produce an empty byte vector rather than an error. Callers
    /// should treat empty output as "no bytes to send".
    fn encode_key(&self, key: &KeyInput, modes: &TerminalModes) -> Result<Vec<u8>, PaneError>;

    /// Encodes a paste event into terminal bytes.
    ///
    /// Wraps the pasted text in bracketed paste sequences when the child has
    /// enabled bracketed paste.
    fn encode_paste(&self, text: &str, modes: &TerminalModes) -> Vec<u8>;

    /// Encodes a focus-gained event into terminal bytes.
    ///
    /// Returns an empty vector when focus events are not enabled.
    fn encode_focus_gained(&self, modes: &TerminalModes) -> Vec<u8>;

    /// Encodes a focus-lost event into terminal bytes.
    ///
    /// Returns an empty vector when focus events are not enabled.
    fn encode_focus_lost(&self, modes: &TerminalModes) -> Vec<u8>;

    /// Encodes a mouse event into terminal bytes.
    ///
    /// Returns `None` when the child has not enabled a mouse mode.
    fn encode_mouse(
        &self,
        mouse: &MouseInput,
        modes: &TerminalModes,
    ) -> Result<Option<Vec<u8>>, PaneError>;
}

/// Default xterm-compatible encoder.
///
/// Uses [`InputConfig`] for host-side policy (Enter/Backspace encoding,
/// application cursor keys, etc.) and [`TerminalModes`] for child-side
/// feature enablement (bracketed paste, focus events, etc.).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct XtermEncoder {
    /// Host-side input configuration.
    pub config: InputConfig,
}

impl XtermEncoder {
    /// Creates a new encoder with the given configuration.
    pub const fn new(config: InputConfig) -> Self {
        Self { config }
    }

    /// Encodes a full [`HostInput`](crate::HostInput) event by dispatching to
    /// the appropriate trait method.
    pub fn encode(
        &self,
        input: &crate::HostInput,
        modes: &TerminalModes,
    ) -> Result<Vec<u8>, PaneError> {
        match input {
            crate::HostInput::Key(key) => self.encode_key(key, modes),
            crate::HostInput::Paste(text) => Ok(self.encode_paste(text, modes)),
            crate::HostInput::FocusGained => Ok(self.encode_focus_gained(modes)),
            crate::HostInput::FocusLost => Ok(self.encode_focus_lost(modes)),
            crate::HostInput::Mouse(mouse) => self
                .encode_mouse(mouse, modes)
                .map(|opt| opt.unwrap_or_default()),
            crate::HostInput::Resize(_) => Ok(Vec::new()),
            crate::HostInput::Raw(bytes) => Ok(bytes.clone()),
        }
    }
}

impl InputEncoder for XtermEncoder {
    fn encode_key(&self, key: &KeyInput, modes: &TerminalModes) -> Result<Vec<u8>, PaneError> {
        // Ignore key releases; only encode presses and repeats.
        if key.kind == KeyEventKind::Release {
            return Ok(Vec::new());
        }

        let mut buf = Vec::new();
        let app_cursor = modes.application_cursor || self.config.application_cursor_keys;

        match key.code {
            KeyCode::Char(c) => {
                encode_char_key(c, key.modifiers, &mut buf);
            }
            KeyCode::Enter => match self.config.enter {
                EnterEncoding::Cr => buf.push(b'\r'),
                EnterEncoding::Lf => buf.push(b'\n'),
                EnterEncoding::CrLf => buf.extend_from_slice(b"\r\n"),
            },
            KeyCode::Backspace => match self.config.backspace {
                BackspaceEncoding::Del => buf.push(0x7f),
                BackspaceEncoding::Bs => buf.push(0x08),
            },
            KeyCode::Tab => {
                if key.modifiers.shift {
                    buf.extend_from_slice(b"\x1b[Z");
                } else {
                    buf.push(b'\t');
                }
            }
            KeyCode::Esc => buf.push(0x1b),
            KeyCode::Up => {
                if app_cursor {
                    buf.extend_from_slice(b"\x1bOA");
                } else {
                    buf.extend_from_slice(b"\x1b[A");
                }
            }
            KeyCode::Down => {
                if app_cursor {
                    buf.extend_from_slice(b"\x1bOB");
                } else {
                    buf.extend_from_slice(b"\x1b[B");
                }
            }
            KeyCode::Right => {
                if app_cursor {
                    buf.extend_from_slice(b"\x1bOC");
                } else {
                    buf.extend_from_slice(b"\x1b[C");
                }
            }
            KeyCode::Left => {
                if app_cursor {
                    buf.extend_from_slice(b"\x1bOD");
                } else {
                    buf.extend_from_slice(b"\x1b[D");
                }
            }
            KeyCode::Home => {
                if app_cursor {
                    buf.extend_from_slice(b"\x1bOH");
                } else {
                    buf.extend_from_slice(b"\x1b[H");
                }
            }
            KeyCode::End => {
                if app_cursor {
                    buf.extend_from_slice(b"\x1bOF");
                } else {
                    buf.extend_from_slice(b"\x1b[F");
                }
            }
            KeyCode::PageUp => buf.extend_from_slice(b"\x1b[5~"),
            KeyCode::PageDown => buf.extend_from_slice(b"\x1b[6~"),
            KeyCode::Insert => buf.extend_from_slice(b"\x1b[2~"),
            KeyCode::Delete => buf.extend_from_slice(b"\x1b[3~"),
            // Media and modifier keys are not emitted as terminal input.
            KeyCode::Media(_) | KeyCode::Modifier(_) => {}
            // Function keys F1–F4 use SS3 in xterm; F5+ use CSI.
            KeyCode::F(n) => match n {
                1 => buf.extend_from_slice(b"\x1bOP"),
                2 => buf.extend_from_slice(b"\x1bOQ"),
                3 => buf.extend_from_slice(b"\x1bOR"),
                4 => buf.extend_from_slice(b"\x1bOS"),
                5 => buf.extend_from_slice(b"\x1b[15~"),
                6 => buf.extend_from_slice(b"\x1b[17~"),
                7 => buf.extend_from_slice(b"\x1b[18~"),
                8 => buf.extend_from_slice(b"\x1b[19~"),
                9 => buf.extend_from_slice(b"\x1b[20~"),
                10 => buf.extend_from_slice(b"\x1b[21~"),
                11 => buf.extend_from_slice(b"\x1b[23~"),
                12 => buf.extend_from_slice(b"\x1b[24~"),
                13 => buf.extend_from_slice(b"\x1b[25~"),
                14 => buf.extend_from_slice(b"\x1b[26~"),
                15 => buf.extend_from_slice(b"\x1b[28~"),
                16 => buf.extend_from_slice(b"\x1b[29~"),
                17 => buf.extend_from_slice(b"\x1b[31~"),
                18 => buf.extend_from_slice(b"\x1b[32~"),
                19 => buf.extend_from_slice(b"\x1b[33~"),
                20 => buf.extend_from_slice(b"\x1b[34~"),
                _ => {
                    // Unsupported function key; ignore.
                }
            },
            KeyCode::BackTab => buf.extend_from_slice(b"\x1b[Z"),
            KeyCode::CapsLock
            | KeyCode::ScrollLock
            | KeyCode::NumLock
            | KeyCode::PrintScreen
            | KeyCode::Pause
            | KeyCode::Menu
            | KeyCode::KeypadBegin
            | KeyCode::Null => {}
        }

        // Apply Alt prefix uniformly after generating the base sequence.
        if key.modifiers.alt && self.config.alt_sends_escape && !buf.is_empty() {
            buf.insert(0, 0x1b);
        }

        Ok(buf)
    }

    fn encode_paste(&self, text: &str, modes: &TerminalModes) -> Vec<u8> {
        let processed = match self.config.paste_newline {
            PasteNewlinePolicy::Preserve => text.to_string(),
            PasteNewlinePolicy::NormalizeToLf => text.replace("\r\n", "\n").replace('\r', "\n"),
            PasteNewlinePolicy::NormalizeToCr => text.replace("\r\n", "\r").replace('\n', "\r"),
        };

        if modes.bracketed_paste {
            let mut buf = Vec::new();
            buf.extend_from_slice(b"\x1b[200~");
            buf.extend_from_slice(processed.as_bytes());
            buf.extend_from_slice(b"\x1b[201~");
            buf
        } else {
            processed.into_bytes()
        }
    }

    fn encode_focus_gained(&self, modes: &TerminalModes) -> Vec<u8> {
        if modes.focus_events {
            b"\x1b[I".to_vec()
        } else {
            Vec::new()
        }
    }

    fn encode_focus_lost(&self, modes: &TerminalModes) -> Vec<u8> {
        if modes.focus_events {
            b"\x1b[O".to_vec()
        } else {
            Vec::new()
        }
    }

    fn encode_mouse(
        &self,
        mouse: &MouseInput,
        modes: &TerminalModes,
    ) -> Result<Option<Vec<u8>>, PaneError> {
        if modes.mouse == MouseMode::None {
            return Ok(None);
        }

        // SGR encoding is the only protocol implemented in the MVP.
        if modes.mouse != MouseMode::Sgr {
            return Ok(None);
        }

        let (cb, release) = match mouse.kind {
            MouseEventKind::Down(btn) => {
                let cb = sgr_button_code(btn, false, mouse.modifiers);
                (cb, false)
            }
            MouseEventKind::Up(btn) => {
                let cb = sgr_button_code(btn, false, mouse.modifiers);
                (cb, true)
            }
            MouseEventKind::Drag(btn) => {
                let cb = sgr_button_code(btn, true, mouse.modifiers);
                (cb, false)
            }
            MouseEventKind::Moved => {
                // SGR any-event tracking reports motion with no buttons as
                // button 35 (0b100011), then ORs in modifier bits.
                let cb = 35 | sgr_modifier_bits(mouse.modifiers);
                (cb, false)
            }
            MouseEventKind::ScrollDown => {
                // xterm SGR wheel down uses base code 65 (0b1000001).
                let cb = 65 | sgr_modifier_bits(mouse.modifiers);
                (cb, false)
            }
            MouseEventKind::ScrollUp => {
                // xterm SGR wheel up uses base code 64 (0b1000000).
                let cb = 64 | sgr_modifier_bits(mouse.modifiers);
                (cb, false)
            }
            // Horizontal scroll (buttons 6/7) is not supported in the MVP SGR
            // implementation. Both "mouse mode not enabled" and "scroll
            // direction not supported" return `None`, so callers should treat
            // `None` as "nothing to send".
            MouseEventKind::ScrollLeft | MouseEventKind::ScrollRight => {
                return Ok(None);
            }
        };

        let mut buf = Vec::new();
        buf.push(b'\x1b');
        buf.push(b'[');
        buf.push(b'<');
        write_u16_to_vec(cb.into(), &mut buf);
        buf.push(b';');
        write_u16_to_vec(mouse.column + 1, &mut buf);
        buf.push(b';');
        write_u16_to_vec(mouse.row + 1, &mut buf);
        if release {
            buf.push(b'm');
        } else {
            buf.push(b'M');
        }
        Ok(Some(buf))
    }
}

/// Encodes a character key, applying control mappings.
fn encode_char_key(c: char, modifiers: crate::KeyModifiers, buf: &mut Vec<u8>) {
    if modifiers.control {
        if let Some(b) = control_byte(c) {
            buf.push(b);
            return;
        }
    }

    // No control mapping; encode the raw character.
    let mut tmp = [0u8; 4];
    let s = c.encode_utf8(&mut tmp);
    buf.extend_from_slice(s.as_bytes());
}

/// Maps a control-modified character to its C0 control byte.
///
/// Returns `None` when there is no standard mapping.
fn control_byte(c: char) -> Option<u8> {
    match c {
        ' ' | '@' | '`' => Some(0x00),
        'a'..='z' => Some((c as u8 - b'a' + 1) & 0x1f),
        'A'..='Z' => Some((c as u8 - b'A' + 1) & 0x1f),
        '[' | '{' => Some(0x1b),
        '\\' | '|' => Some(0x1c),
        ']' | '}' => Some(0x1d),
        '^' | '~' => Some(0x1e),
        '_' | '-' => Some(0x1f),
        '?' => Some(0x7f),
        _ => None,
    }
}

/// Computes the SGR button code for a press/drag/release event.
fn sgr_button_code(btn: crate::MouseButton, drag: bool, modifiers: crate::KeyModifiers) -> u8 {
    let mut cb: u8 = match btn {
        crate::MouseButton::Left => 0,
        crate::MouseButton::Middle => 1,
        crate::MouseButton::Right => 2,
    };
    if drag {
        cb |= 0b100000;
    }
    cb | sgr_modifier_bits(modifiers)
}

/// Returns the modifier bit mask for SGR mouse events.
fn sgr_modifier_bits(modifiers: crate::KeyModifiers) -> u8 {
    let mut bits = 0;
    if modifiers.shift {
        bits |= 0b000100;
    }
    if modifiers.alt {
        bits |= 0b001000;
    }
    if modifiers.control {
        bits |= 0b010000;
    }
    bits
}

/// Writes a `u16` into a `Vec<u8>` without heap-allocating a temporary string.
fn write_u16_to_vec(mut n: u16, buf: &mut Vec<u8>) {
    if n == 0 {
        buf.push(b'0');
        return;
    }
    let start = buf.len();
    while n > 0 {
        buf.push(b'0' + (n % 10) as u8);
        n /= 10;
    }
    let end = buf.len();
    buf[start..end].reverse();
}

#[cfg(test)]
mod tests;
