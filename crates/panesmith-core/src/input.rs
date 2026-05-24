//! Host-independent input types and backend conversions.
//!
//! This module defines [`HostInput`] and its subtypes so that Panesmith is not
//! permanently tied to any specific terminal backend. Crossterm conversions are
//! available behind the `crossterm` feature flag.

use crate::Size;

/// Host input representation for sending to a pane.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub enum HostInput {
    /// A keyboard key event.
    Key(KeyInput),
    /// A mouse event.
    Mouse(MouseInput),
    /// A paste event carrying the pasted text.
    Paste(String),
    /// The terminal gained focus.
    FocusGained,
    /// The terminal lost focus.
    FocusLost,
    /// The terminal was resized.
    Resize(Size),
    /// Raw bytes to send directly to the PTY.
    Raw(Vec<u8>),
}

/// A keyboard input event.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct KeyInput {
    /// The key that was pressed.
    pub code: KeyCode,
    /// Modifier keys held during the event.
    pub modifiers: KeyModifiers,
    /// Whether this is a press, repeat, or release.
    pub kind: KeyEventKind,
}

impl KeyInput {
    /// Creates a keyboard input event from its code, modifiers, and event kind.
    pub const fn new(code: KeyCode, modifiers: KeyModifiers, kind: KeyEventKind) -> Self {
        Self {
            code,
            modifiers,
            kind,
        }
    }
}

/// Identifies a keyboard key independent of any backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub enum KeyCode {
    /// Backspace key.
    Backspace,
    /// Enter key.
    Enter,
    /// Left arrow.
    Left,
    /// Right arrow.
    Right,
    /// Up arrow.
    Up,
    /// Down arrow.
    Down,
    /// Home key.
    Home,
    /// End key.
    End,
    /// Page Up key.
    PageUp,
    /// Page Down key.
    PageDown,
    /// Tab key.
    Tab,
    /// Back-tab (Shift+Tab).
    BackTab,
    /// Delete key.
    Delete,
    /// Insert key.
    Insert,
    /// Escape key.
    Esc,
    /// Caps Lock key.
    CapsLock,
    /// Scroll Lock key.
    ScrollLock,
    /// Num Lock key.
    NumLock,
    /// Print Screen key.
    PrintScreen,
    /// Pause key.
    Pause,
    /// Menu key.
    Menu,
    /// Keypad begin key.
    KeypadBegin,
    /// A character key.
    Char(char),
    /// A function key (F1–F24).
    F(u8),
    /// Null key.
    Null,
    /// A media key.
    Media(MediaKeyCode),
    /// A modifier key on its own.
    Modifier(ModifierKeyCode),
}

/// Media keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub enum MediaKeyCode {
    /// Play media.
    Play,
    /// Pause media.
    Pause,
    /// Toggle play/pause.
    PlayPause,
    /// Reverse playback.
    Reverse,
    /// Stop media.
    Stop,
    /// Fast forward.
    FastForward,
    /// Rewind.
    Rewind,
    /// Next track.
    TrackNext,
    /// Previous track.
    TrackPrevious,
    /// Record.
    Record,
    /// Lower volume.
    LowerVolume,
    /// Raise volume.
    RaiseVolume,
    /// Mute volume.
    MuteVolume,
}

/// Modifier keys reported as standalone events.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub enum ModifierKeyCode {
    /// Left Shift.
    LeftShift,
    /// Left Control.
    LeftControl,
    /// Left Alt.
    LeftAlt,
    /// Left Super (Windows/Command).
    LeftSuper,
    /// Left Hyper.
    LeftHyper,
    /// Left Meta.
    LeftMeta,
    /// Right Shift.
    RightShift,
    /// Right Control.
    RightControl,
    /// Right Alt.
    RightAlt,
    /// Right Super (Windows/Command).
    RightSuper,
    /// Right Hyper.
    RightHyper,
    /// Right Meta.
    RightMeta,
    /// ISO Level 3 Shift (AltGr).
    IsoLevel3Shift,
    /// ISO Level 5 Shift.
    IsoLevel5Shift,
}

/// Modifier key state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct KeyModifiers {
    /// Shift is held.
    pub shift: bool,
    /// Control is held.
    pub control: bool,
    /// Alt is held.
    pub alt: bool,
    /// Super (Windows/Command) is held.
    pub super_key: bool,
    /// Hyper is held.
    pub hyper: bool,
    /// Meta is held.
    pub meta: bool,
}

/// Whether a key event is a press, repeat, or release.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub enum KeyEventKind {
    /// Key was pressed.
    #[default]
    Press,
    /// Key is being held down (repeating).
    Repeat,
    /// Key was released.
    Release,
}

/// A mouse input event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct MouseInput {
    /// The kind of mouse event.
    pub kind: MouseEventKind,
    /// Column position (0-based).
    pub column: u16,
    /// Row position (0-based).
    pub row: u16,
    /// Modifier keys held during the event.
    pub modifiers: KeyModifiers,
}

/// The kind of mouse event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub enum MouseEventKind {
    /// A mouse button was pressed.
    Down(MouseButton),
    /// A mouse button was released.
    Up(MouseButton),
    /// A mouse button is being held while moving.
    Drag(MouseButton),
    /// The mouse moved without any button held.
    Moved,
    /// Scroll wheel moved down.
    ScrollDown,
    /// Scroll wheel moved up.
    ScrollUp,
    /// Scroll wheel moved left.
    ScrollLeft,
    /// Scroll wheel moved right.
    ScrollRight,
}

/// A mouse button.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub enum MouseButton {
    /// Left mouse button.
    Left,
    /// Right mouse button.
    Right,
    /// Middle mouse button.
    Middle,
}

/// Error when a terminal backend event cannot be converted to [`HostInput`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnsupportedEventError {
    /// Description of the unsupported event.
    pub message: String,
}

impl std::fmt::Display for UnsupportedEventError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "unsupported event: {}", self.message)
    }
}

impl std::error::Error for UnsupportedEventError {}

#[cfg(feature = "crossterm")]
mod crossterm_impl {
    use super::*;
    use crossterm::event::{
        Event as CtEvent, KeyCode as CtKeyCode, KeyEvent as CtKeyEvent,
        KeyEventKind as CtKeyEventKind, KeyModifiers as CtKeyModifiers,
        MediaKeyCode as CtMediaKeyCode, ModifierKeyCode as CtModifierKeyCode,
        MouseButton as CtMouseButton, MouseEvent as CtMouseEvent,
        MouseEventKind as CtMouseEventKind,
    };

    impl TryFrom<CtEvent> for HostInput {
        type Error = UnsupportedEventError;
        fn try_from(event: CtEvent) -> std::result::Result<Self, Self::Error> {
            match event {
                CtEvent::Key(key) => Ok(HostInput::Key(key.into())),
                CtEvent::Mouse(mouse) => Ok(HostInput::Mouse(mouse.into())),
                CtEvent::Paste(text) => Ok(HostInput::Paste(text)),
                CtEvent::FocusGained => Ok(HostInput::FocusGained),
                CtEvent::FocusLost => Ok(HostInput::FocusLost),
                CtEvent::Resize(cols, rows) => {
                    let size = Size::try_new(rows, cols).map_err(|_| UnsupportedEventError {
                        message: format!(
                            "resize with invalid dimensions: rows={rows}, cols={cols}"
                        ),
                    })?;
                    Ok(HostInput::Resize(size))
                }
            }
        }
    }

    impl From<CtKeyEvent> for KeyInput {
        fn from(key: CtKeyEvent) -> Self {
            KeyInput {
                code: key.code.into(),
                modifiers: key.modifiers.into(),
                kind: key.kind.into(),
            }
        }
    }

    impl From<CtKeyCode> for KeyCode {
        fn from(code: CtKeyCode) -> Self {
            match code {
                CtKeyCode::Backspace => KeyCode::Backspace,
                CtKeyCode::Enter => KeyCode::Enter,
                CtKeyCode::Left => KeyCode::Left,
                CtKeyCode::Right => KeyCode::Right,
                CtKeyCode::Up => KeyCode::Up,
                CtKeyCode::Down => KeyCode::Down,
                CtKeyCode::Home => KeyCode::Home,
                CtKeyCode::End => KeyCode::End,
                CtKeyCode::PageUp => KeyCode::PageUp,
                CtKeyCode::PageDown => KeyCode::PageDown,
                CtKeyCode::Tab => KeyCode::Tab,
                CtKeyCode::BackTab => KeyCode::BackTab,
                CtKeyCode::Delete => KeyCode::Delete,
                CtKeyCode::Insert => KeyCode::Insert,
                CtKeyCode::Esc => KeyCode::Esc,
                CtKeyCode::CapsLock => KeyCode::CapsLock,
                CtKeyCode::ScrollLock => KeyCode::ScrollLock,
                CtKeyCode::NumLock => KeyCode::NumLock,
                CtKeyCode::PrintScreen => KeyCode::PrintScreen,
                CtKeyCode::Pause => KeyCode::Pause,
                CtKeyCode::Menu => KeyCode::Menu,
                CtKeyCode::KeypadBegin => KeyCode::KeypadBegin,
                CtKeyCode::Char(c) => KeyCode::Char(c),
                CtKeyCode::F(n) => KeyCode::F(n),
                CtKeyCode::Null => KeyCode::Null,
                CtKeyCode::Media(m) => KeyCode::Media(m.into()),
                CtKeyCode::Modifier(m) => KeyCode::Modifier(m.into()),
            }
        }
    }

    impl From<CtKeyModifiers> for KeyModifiers {
        fn from(modifiers: CtKeyModifiers) -> Self {
            Self {
                shift: modifiers.contains(CtKeyModifiers::SHIFT),
                control: modifiers.contains(CtKeyModifiers::CONTROL),
                alt: modifiers.contains(CtKeyModifiers::ALT),
                super_key: modifiers.contains(CtKeyModifiers::SUPER),
                hyper: modifiers.contains(CtKeyModifiers::HYPER),
                meta: modifiers.contains(CtKeyModifiers::META),
            }
        }
    }

    impl From<CtKeyEventKind> for KeyEventKind {
        fn from(kind: CtKeyEventKind) -> Self {
            match kind {
                CtKeyEventKind::Press => KeyEventKind::Press,
                CtKeyEventKind::Repeat => KeyEventKind::Repeat,
                CtKeyEventKind::Release => KeyEventKind::Release,
            }
        }
    }

    impl From<CtMediaKeyCode> for MediaKeyCode {
        fn from(code: CtMediaKeyCode) -> Self {
            match code {
                CtMediaKeyCode::Play => MediaKeyCode::Play,
                CtMediaKeyCode::Pause => MediaKeyCode::Pause,
                CtMediaKeyCode::PlayPause => MediaKeyCode::PlayPause,
                CtMediaKeyCode::Reverse => MediaKeyCode::Reverse,
                CtMediaKeyCode::Stop => MediaKeyCode::Stop,
                CtMediaKeyCode::FastForward => MediaKeyCode::FastForward,
                CtMediaKeyCode::Rewind => MediaKeyCode::Rewind,
                CtMediaKeyCode::TrackNext => MediaKeyCode::TrackNext,
                CtMediaKeyCode::TrackPrevious => MediaKeyCode::TrackPrevious,
                CtMediaKeyCode::Record => MediaKeyCode::Record,
                CtMediaKeyCode::LowerVolume => MediaKeyCode::LowerVolume,
                CtMediaKeyCode::RaiseVolume => MediaKeyCode::RaiseVolume,
                CtMediaKeyCode::MuteVolume => MediaKeyCode::MuteVolume,
            }
        }
    }

    impl From<CtModifierKeyCode> for ModifierKeyCode {
        fn from(code: CtModifierKeyCode) -> Self {
            match code {
                CtModifierKeyCode::LeftShift => ModifierKeyCode::LeftShift,
                CtModifierKeyCode::LeftControl => ModifierKeyCode::LeftControl,
                CtModifierKeyCode::LeftAlt => ModifierKeyCode::LeftAlt,
                CtModifierKeyCode::LeftSuper => ModifierKeyCode::LeftSuper,
                CtModifierKeyCode::LeftHyper => ModifierKeyCode::LeftHyper,
                CtModifierKeyCode::LeftMeta => ModifierKeyCode::LeftMeta,
                CtModifierKeyCode::RightShift => ModifierKeyCode::RightShift,
                CtModifierKeyCode::RightControl => ModifierKeyCode::RightControl,
                CtModifierKeyCode::RightAlt => ModifierKeyCode::RightAlt,
                CtModifierKeyCode::RightSuper => ModifierKeyCode::RightSuper,
                CtModifierKeyCode::RightHyper => ModifierKeyCode::RightHyper,
                CtModifierKeyCode::RightMeta => ModifierKeyCode::RightMeta,
                CtModifierKeyCode::IsoLevel3Shift => ModifierKeyCode::IsoLevel3Shift,
                CtModifierKeyCode::IsoLevel5Shift => ModifierKeyCode::IsoLevel5Shift,
            }
        }
    }

    impl From<CtMouseEvent> for MouseInput {
        fn from(mouse: CtMouseEvent) -> Self {
            Self {
                kind: mouse.kind.into(),
                column: mouse.column,
                row: mouse.row,
                modifiers: mouse.modifiers.into(),
            }
        }
    }

    impl From<CtMouseEventKind> for MouseEventKind {
        fn from(kind: CtMouseEventKind) -> Self {
            match kind {
                CtMouseEventKind::Down(btn) => MouseEventKind::Down(btn.into()),
                CtMouseEventKind::Up(btn) => MouseEventKind::Up(btn.into()),
                CtMouseEventKind::Drag(btn) => MouseEventKind::Drag(btn.into()),
                CtMouseEventKind::Moved => MouseEventKind::Moved,
                CtMouseEventKind::ScrollDown => MouseEventKind::ScrollDown,
                CtMouseEventKind::ScrollUp => MouseEventKind::ScrollUp,
                CtMouseEventKind::ScrollLeft => MouseEventKind::ScrollLeft,
                CtMouseEventKind::ScrollRight => MouseEventKind::ScrollRight,
            }
        }
    }

    impl From<CtMouseButton> for MouseButton {
        fn from(button: CtMouseButton) -> Self {
            match button {
                CtMouseButton::Left => MouseButton::Left,
                CtMouseButton::Right => MouseButton::Right,
                CtMouseButton::Middle => MouseButton::Middle,
            }
        }
    }
}

#[cfg(test)]
mod tests;
