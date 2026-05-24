#![doc = include_str!("../README.md")]

pub mod attach;
#[cfg(feature = "crossterm")]
pub mod crossterm_host;
pub mod detach;
pub mod session;
#[doc(hidden)]
pub mod test_harness;

// Re-export core attach types so consumers only need one import.
pub use panesmith_core::{
    AttachOptions, AttachOutputPolicy, AttachResizePolicy, AttachScreenPolicy, DetachConfig,
    PaneAttachError, PaneAttachInputChunk, PaneAttachOutcome, PaneAttachTerminal,
    PaneAttachTerminalControl, RestorePolicy,
};

// Re-export local attach types.
pub use attach::{
    AttachBridge, AttachGuard, AttachState, HostTerminalControl, TerminalRestoreToken,
};
#[cfg(all(feature = "crossterm", unix))]
pub use crossterm_host::StdioAttachTerminal;
#[cfg(feature = "crossterm")]
pub use crossterm_host::{CrosstermTerminalControl, RawModeOps, SystemRawModeOps};
pub use detach::{DetachMatcher, FeedBytesResult, MatchResult};
pub use session::{
    AttachInputChunk, AttachPtyEndpoint, AttachSurfaceSink, AttachTerminal, BlockingAttachError,
    BlockingAttachOutcome, BlockingAttachSession,
};

#[cfg(test)]
mod tests;
