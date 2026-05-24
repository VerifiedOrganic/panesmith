//! Smallest public embedded-shell example.
//!
//! This draws one frame and exits. See `examples/embedded-shell` for a live
//! event loop with input, resize, mouse, and paste handling.

use std::io;

use panesmith::{PaneConfig, PaneManager, PaneManagerConfig, Size, TerminalPaneWidget};
use ratatui::{backend::CrosstermBackend, Terminal};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut manager = PaneManager::new(PaneManagerConfig::default());
    let pane_id = manager.spawn(
        PaneConfig::shell()
            .with_title("Shell")
            .with_size(Size::new(24, 80)),
    )?;

    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    let snapshot = manager.snapshot(pane_id)?;
    terminal.draw(|frame| {
        frame.render_widget(TerminalPaneWidget::new(&snapshot), frame.area());
    })?;
    Ok(())
}
