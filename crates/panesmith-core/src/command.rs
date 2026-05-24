//! Command types for controlling panes.
//!
//! [`PaneCommand`] is the internal command enum used by async implementations
//! and testing. It mirrors the manager's public API as discrete messages.

use std::time::Duration;

use crate::event::DetachReason;
use crate::input::HostInput;
use crate::{KillReason, PaneId, Size};

/// Internal command enum for pane operations.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum PaneCommand {
    /// Spawn a new pane with the given configuration.
    Spawn(crate::PaneConfig),
    /// Resize an existing pane.
    Resize {
        /// Target pane.
        pane_id: PaneId,
        /// New size.
        size: Size,
    },
    /// Write raw bytes to a pane's PTY.
    WriteBytes {
        /// Target pane.
        pane_id: PaneId,
        /// Bytes to write.
        bytes: Vec<u8>,
    },
    /// Send encoded input to a pane.
    Input {
        /// Target pane.
        pane_id: PaneId,
        /// The input to send.
        input: HostInput,
    },
    /// Kill a pane.
    Kill {
        /// Target pane.
        pane_id: PaneId,
        /// Reason for killing.
        reason: KillReason,
    },
    /// Attach a pane to the real terminal fullscreen.
    Attach {
        /// Target pane.
        pane_id: PaneId,
        /// Attach options.
        options: AttachOptions,
    },
    /// Detach a pane from the real terminal.
    Detach {
        /// Target pane.
        pane_id: PaneId,
        /// Reason for detaching.
        reason: DetachReason,
    },
}

/// Options for controlling fullscreen attach behavior.
///
/// Controls detach chord, screen policy, resize behavior, output forwarding,
/// and terminal restoration.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct AttachOptions {
    /// Configuration for the detach chord and timeout.
    pub detach: DetachConfig,
    /// Policy for handling the terminal screen.
    pub screen: crate::event::AttachScreenPolicy,
    /// Policy for resizing the child PTY during attach.
    pub resize: AttachResizePolicy,
    /// Policy for forwarding child output during attach.
    pub output: AttachOutputPolicy,
    /// Policy for terminal state restoration after detach.
    pub restore: RestorePolicy,
}

/// Configuration for the detach chord and partial-match timeout.
///
/// A single-byte chord (e.g. `Ctrl+]`) detaches immediately. A multi-byte
/// chord (e.g. `Ctrl-A d`) holds the first byte until the second arrives or
/// the timeout expires.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct DetachConfig {
    /// Byte sequence that triggers detach.
    pub chord: Vec<u8>,
    /// How long to hold a partial chord before forwarding held bytes.
    pub partial_timeout: Duration,
}

impl Default for DetachConfig {
    fn default() -> Self {
        Self {
            chord: vec![0x1d], // Ctrl+]
            partial_timeout: Duration::from_millis(500),
        }
    }
}

/// Policy for resizing the child PTY during attach.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum AttachResizePolicy {
    /// Resize the child PTY to match the real terminal size.
    #[default]
    UseRealTerminalSize,
    /// Keep the embedded size; do not resize on attach.
    KeepEmbeddedSize,
}

/// Policy for forwarding child PTY output while attached.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum AttachOutputPolicy {
    /// Write output to real stdout and also feed the surface backend.
    #[default]
    FanoutToSurfaceAndStdout,
    /// Write only to stdout; replay to surface after detach.
    StdoutOnlyThenReplay,
}

/// Policy for terminal state restoration after detach.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum RestorePolicy {
    /// Restore all terminal state (screen, cursor, raw mode, size).
    #[default]
    Full,
    /// Restore only essential terminal state.
    Minimal,
}
