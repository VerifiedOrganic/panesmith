#![doc = include_str!("../README.md")]

pub mod widget;

pub use panesmith_core::{TerminalViewport, TerminalViewportMetrics};
pub use widget::{CursorRenderMode, TerminalPaneWidget};

#[cfg(test)]
mod tests;
