//! Manager-owned fullscreen attach for one pane.
//!
//! Run on Unix with:
//!
//! ```text
//! cargo run -p panesmith --features crossterm --example manager_attach
//! ```
//!
//! Press `Ctrl+]` to detach and return to the host process.

#[cfg(all(feature = "crossterm", unix))]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use std::io;

    use panesmith::{
        AttachOptions, CrosstermTerminalControl, PaneConfig, PaneManager, PaneManagerConfig, Size,
        StdioAttachTerminal,
    };

    let mut manager = PaneManager::new(PaneManagerConfig::default());
    let pane_id = manager.spawn(
        PaneConfig::shell()
            .with_title("Attached shell")
            .with_size(Size::new(24, 80)),
    )?;

    let mut terminal = StdioAttachTerminal::new(io::stdout())?;
    let mut control = CrosstermTerminalControl::new(io::stdout());
    let outcome = manager.attach_blocking(
        pane_id,
        AttachOptions::default(),
        &mut terminal,
        &mut control,
    )?;

    println!(
        "detached: reason={:?} child_exit={:?} trailing_input={} bytes",
        outcome.reason,
        outcome.child_exit_code,
        outcome.remaining_input.len()
    );
    Ok(())
}

#[cfg(not(all(feature = "crossterm", unix)))]
fn main() {
    eprintln!("manager_attach requires Unix and the panesmith `crossterm` feature");
}
