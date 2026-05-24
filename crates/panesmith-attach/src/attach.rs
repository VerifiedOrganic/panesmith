//! Attach bridge types, traits, and state for fullscreen terminal handoff.

use panesmith_core::{AttachOptions, AttachScreenPolicy, PaneId};

/// Lifecycle state of an attach session.
///
/// State transitions are explicit and enforced by [`AttachBridge`]:
///
/// ```text
/// Embedded -> Attaching -> Attached -> Detaching -> Embedded
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AttachState {
    /// Pane is rendered in an embedded widget.
    #[default]
    Embedded,
    /// Transitioning to fullscreen attach.
    Attaching,
    /// Pane has fullscreen control of the real terminal.
    Attached,
    /// Transitioning back to embedded rendering.
    Detaching,
}

impl AttachState {
    /// Returns true if this state is a transitional state.
    pub fn is_transitional(self) -> bool {
        matches!(self, AttachState::Attaching | AttachState::Detaching)
    }

    /// Returns true if the attach bridge is active in this state.
    pub fn is_active(self) -> bool {
        matches!(
            self,
            AttachState::Attaching | AttachState::Attached | AttachState::Detaching
        )
    }
}

/// Token representing saved terminal state from a suspend operation.
///
/// Must be passed to [`HostTerminalControl::restore_after_attach`] to restore
/// the terminal. If dropped without explicit restoration, no automatic action
/// is taken by the token itself; use [`AttachGuard`] for RAII-style cleanup.
///
/// This type is opaque: its fields are private and it should only be created
/// by a [`HostTerminalControl::suspend_for_attach`] implementation.
#[derive(Debug, PartialEq, Eq)]
pub struct TerminalRestoreToken {
    screen_policy: AttachScreenPolicy,
    consumed: bool,
    #[cfg(feature = "crossterm")]
    raw_mode_was_enabled: bool,
    #[cfg(feature = "crossterm")]
    left_host_alternate_screen: bool,
    #[cfg(feature = "crossterm")]
    entered_fresh_alternate_screen: bool,
    #[cfg(feature = "crossterm")]
    keyboard_enhancement_pushed: bool,
    #[cfg(feature = "crossterm")]
    mouse_modes_saved: bool,
}

impl TerminalRestoreToken {
    /// Creates a new restore token for the given screen policy.
    ///
    /// Intended for use by [`HostTerminalControl::suspend_for_attach`]
    /// implementations. The token fields remain private, so this constructor
    /// only allows creating a valid token with the correct screen policy.
    pub fn new(screen_policy: AttachScreenPolicy) -> Self {
        Self {
            screen_policy,
            consumed: false,
            #[cfg(feature = "crossterm")]
            raw_mode_was_enabled: false,
            #[cfg(feature = "crossterm")]
            left_host_alternate_screen: false,
            #[cfg(feature = "crossterm")]
            entered_fresh_alternate_screen: false,
            #[cfg(feature = "crossterm")]
            keyboard_enhancement_pushed: false,
            #[cfg(feature = "crossterm")]
            mouse_modes_saved: false,
        }
    }

    /// Returns the screen policy that was in effect when suspended.
    pub fn screen_policy(&self) -> AttachScreenPolicy {
        self.screen_policy
    }

    /// Marks this token as consumed.
    ///
    /// Intended for use by [`HostTerminalControl::restore_after_attach`]
    /// implementations. Once consumed, the token should not be used again.
    pub fn consume(&mut self) {
        self.consumed = true;
    }

    /// Returns true if this token has already been consumed.
    pub fn is_consumed(&self) -> bool {
        self.consumed
    }

    #[cfg(feature = "crossterm")]
    pub(crate) fn raw_mode_was_enabled(&self) -> bool {
        self.raw_mode_was_enabled
    }

    #[cfg(feature = "crossterm")]
    pub(crate) fn set_raw_mode_was_enabled(&mut self, enabled: bool) {
        self.raw_mode_was_enabled = enabled;
    }

    #[cfg(feature = "crossterm")]
    pub(crate) fn left_host_alternate_screen(&self) -> bool {
        self.left_host_alternate_screen
    }

    #[cfg(feature = "crossterm")]
    pub(crate) fn set_left_host_alternate_screen(&mut self, changed: bool) {
        self.left_host_alternate_screen = changed;
    }

    #[cfg(feature = "crossterm")]
    pub(crate) fn entered_fresh_alternate_screen(&self) -> bool {
        self.entered_fresh_alternate_screen
    }

    #[cfg(feature = "crossterm")]
    pub(crate) fn set_entered_fresh_alternate_screen(&mut self, changed: bool) {
        self.entered_fresh_alternate_screen = changed;
    }

    #[cfg(feature = "crossterm")]
    pub(crate) fn keyboard_enhancement_pushed(&self) -> bool {
        self.keyboard_enhancement_pushed
    }

    #[cfg(feature = "crossterm")]
    pub(crate) fn set_keyboard_enhancement_pushed(&mut self, changed: bool) {
        self.keyboard_enhancement_pushed = changed;
    }

    #[cfg(feature = "crossterm")]
    pub(crate) fn mouse_modes_saved(&self) -> bool {
        self.mouse_modes_saved
    }

    #[cfg(feature = "crossterm")]
    pub(crate) fn set_mouse_modes_saved(&mut self, changed: bool) {
        self.mouse_modes_saved = changed;
    }
}

/// Terminal control trait implemented by host backends (e.g. crossterm/ratatui).
///
/// Provides suspend/restore hooks that allow the attach bridge to take over
/// and later return control to the host application.
///
/// The host terminal profile is caller-owned. Implementations are responsible
/// for saving and restoring the exact profile their application expects after
/// attach, including raw mode, alternate-screen state, mouse capture,
/// bracketed paste, keyboard enhancement flags, cursor policy, and redraw
/// expectations. A generic reset to a broadly usable terminal state is not
/// enough for embedded TUIs whose key parser depends on enhanced input modes.
pub trait HostTerminalControl {
    /// The error type returned by suspend/restore operations.
    type Error: std::fmt::Debug;

    /// Suspends host drawing and prepares the real terminal for attach.
    ///
    /// Returns a [`TerminalRestoreToken`] that must later be passed to
    /// [`restore_after_attach`](Self::restore_after_attach). The token should
    /// carry the caller-owned state needed to restore the host's expected
    /// terminal profile exactly.
    fn suspend_for_attach(
        &mut self,
        policy: AttachScreenPolicy,
    ) -> std::result::Result<TerminalRestoreToken, Self::Error>;

    /// Restores the host terminal after attach ends.
    ///
    /// The token is borrowed mutably so that restoration can be retried on
    /// error. On success the implementor or caller should mark the token
    /// consumed; [`AttachGuard`] handles this automatically. On success, the
    /// host should be able to resume its normal input parser and redraw loop
    /// without a separate full terminal reset.
    fn restore_after_attach(
        &mut self,
        token: &mut TerminalRestoreToken,
    ) -> std::result::Result<(), Self::Error>;
}

/// RAII guard that restores the terminal on drop unless explicitly detached.
///
/// Created by suspending host terminal control. If the guard is dropped
/// without calling [`detach`](Self::detach), it automatically attempts
/// restoration.
#[must_use = "dropping the guard immediately would restore the terminal"]
pub struct AttachGuard<'a, T: HostTerminalControl + ?Sized> {
    control: &'a mut T,
    token: Option<TerminalRestoreToken>,
}

impl<'a, T: HostTerminalControl + ?Sized> AttachGuard<'a, T> {
    /// Creates a new guard from a control reference and restore token.
    pub fn new(control: &'a mut T, token: TerminalRestoreToken) -> Self {
        Self {
            control,
            token: Some(token),
        }
    }

    /// Explicitly detaches, restoring the terminal.
    ///
    /// Returns an error if restoration fails. The token is kept in the guard
    /// so that callers can retry or let `Drop` attempt restoration again.
    pub fn detach(&mut self) -> std::result::Result<(), T::Error> {
        if let Some(ref mut token) = self.token {
            self.control.restore_after_attach(token)?;
            token.consume();
            self.token = None;
        }
        Ok(())
    }

    /// Returns true if the underlying token has already been consumed.
    pub fn is_detached(&self) -> bool {
        self.token.is_none()
    }

    /// Discards any retained restore token without attempting another restore.
    ///
    /// Intended for abort paths that have already recorded the final explicit
    /// restore attempt and must prevent [`Drop`] from issuing an unobserved
    /// retry.
    pub(crate) fn disarm(&mut self) {
        self.token = None;
    }
}

impl<'a, T: HostTerminalControl + ?Sized> std::fmt::Debug for AttachGuard<'a, T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AttachGuard")
            .field("control", &std::any::type_name::<T>())
            .field("token_present", &self.token.is_some())
            .field("detached", &self.is_detached())
            .finish()
    }
}

impl<'a, T: HostTerminalControl + ?Sized> Drop for AttachGuard<'a, T> {
    fn drop(&mut self) {
        if let Some(ref mut token) = self.token {
            if self.control.restore_after_attach(token).is_ok() {
                token.consume();
            }
        }
    }
}

/// Attach bridge that tracks pane, options, and lifecycle state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachBridge {
    pane_id: PaneId,
    options: AttachOptions,
    state: AttachState,
}

impl AttachBridge {
    /// Creates a bridge with default options in the [`Embedded`] state.
    pub fn new(pane_id: PaneId) -> Self {
        Self::with_options(pane_id, AttachOptions::default())
    }

    /// Creates a bridge with explicit options in the [`Embedded`] state.
    pub fn with_options(pane_id: PaneId, options: AttachOptions) -> Self {
        Self {
            pane_id,
            options,
            state: AttachState::Embedded,
        }
    }

    /// Returns the pane identifier.
    pub const fn pane_id(&self) -> PaneId {
        self.pane_id
    }

    /// Returns the attach options.
    pub const fn options(&self) -> &AttachOptions {
        &self.options
    }

    /// Returns the current attach state.
    pub const fn state(&self) -> AttachState {
        self.state
    }

    /// Transitions to [`AttachState::Attaching`].
    ///
    /// # Panics
    ///
    /// Panics if the current state is not [`AttachState::Embedded`].
    pub fn begin_attach(&mut self) {
        assert_eq!(
            self.state,
            AttachState::Embedded,
            "can only begin attach from Embedded state"
        );
        self.state = AttachState::Attaching;
    }

    /// Transitions to [`AttachState::Attached`].
    ///
    /// # Panics
    ///
    /// Panics if the current state is not [`AttachState::Attaching`].
    pub fn confirm_attached(&mut self) {
        assert_eq!(
            self.state,
            AttachState::Attaching,
            "can only confirm attached from Attaching state"
        );
        self.state = AttachState::Attached;
    }

    /// Transitions to [`AttachState::Detaching`].
    ///
    /// # Panics
    ///
    /// Panics if the current state is not [`AttachState::Attached`].
    pub fn begin_detach(&mut self) {
        assert_eq!(
            self.state,
            AttachState::Attached,
            "can only begin detach from Attached state"
        );
        self.state = AttachState::Detaching;
    }

    /// Transitions to [`AttachState::Embedded`].
    ///
    /// # Panics
    ///
    /// Panics if the current state is not [`AttachState::Detaching`].
    pub fn confirm_detached(&mut self) {
        assert_eq!(
            self.state,
            AttachState::Detaching,
            "can only confirm detached from Detaching state"
        );
        self.state = AttachState::Embedded;
    }

    /// Crossterm-specific helper kept behind an explicit feature gate.
    #[cfg(feature = "crossterm")]
    pub fn keycode_name(key: crossterm::event::KeyCode) -> &'static str {
        match key {
            crossterm::event::KeyCode::Esc => "esc",
            _ => "other",
        }
    }
}
