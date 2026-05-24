//! Structured error types for Panesmith.

use std::fmt;

use crate::PaneId;

/// Result type alias used throughout Panesmith.
pub type Result<T> = std::result::Result<T, PaneError>;

/// Describes the I/O operation that failed when a [`PaneError::Io`] is produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum IoOperation {
    /// Reading output from a PTY or child process.
    Read,
    /// Writing input to a PTY or child process.
    Write,
    /// Resizing a PTY.
    Resize,
    /// Killing a child process.
    Kill,
    /// Flushing a writer or stdout.
    Flush,
}

impl fmt::Display for IoOperation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read => write!(f, "read"),
            Self::Write => write!(f, "write"),
            Self::Resize => write!(f, "resize"),
            Self::Kill => write!(f, "kill"),
            Self::Flush => write!(f, "flush"),
        }
    }
}

/// Structured error type for all pane-scoped and manager-scoped failures.
///
/// Every variant carries enough context to diagnose the failure without
/// inspecting internal state.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum PaneError {
    /// Spawning a child process failed.
    Spawn {
        /// Human-readable description of what went wrong.
        message: String,
    },
    /// A low-level I/O operation failed.
    Io {
        /// The operation that failed.
        operation: IoOperation,
        /// Human-readable description of what went wrong.
        message: String,
    },
    /// The surface backend failed to parse or apply output.
    Surface {
        /// Human-readable description of what went wrong.
        message: String,
    },
    /// An input event could not be encoded into terminal bytes.
    InputEncoding {
        /// Human-readable description of what went wrong.
        message: String,
    },
    /// An attach or detach operation failed.
    Attach {
        /// Human-readable description of what went wrong.
        message: String,
    },
    /// A pane-scoped operation referenced an unknown pane identifier.
    NotFound {
        /// The pane identifier that could not be resolved.
        pane_id: PaneId,
    },
    /// An operation was requested in a pane state that does not permit it.
    InvalidState {
        /// Description of the state that was expected.
        expected: String,
        /// Description of the actual state encountered.
        actual: String,
    },
    /// A size was constructed with zero rows or columns.
    InvalidSize {
        /// The requested row count.
        rows: u16,
        /// The requested column count.
        cols: u16,
    },
}

impl fmt::Display for PaneError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Spawn { message } => write!(f, "spawn failed: {message}"),
            Self::Io { operation, message } => write!(f, "io failed ({operation}): {message}"),
            Self::Surface { message } => write!(f, "surface failed: {message}"),
            Self::InputEncoding { message } => write!(f, "input encoding failed: {message}"),
            Self::Attach { message } => write!(f, "attach failed: {message}"),
            Self::NotFound { pane_id } => write!(f, "pane {} was not found", pane_id.get()),
            Self::InvalidState { expected, actual } => {
                write!(f, "invalid state: expected {expected}, got {actual}")
            }
            Self::InvalidSize { rows, cols } => {
                write!(
                    f,
                    "invalid size: rows={rows}, cols={cols} (both must be >= 1)"
                )
            }
        }
    }
}

impl std::error::Error for PaneError {}
