//! Two-pane dashboard reference example.
//!
//! Embedded mode is for preview and routine input. Attach mode is the
//! correctness path for complex interactive TUIs.

use std::io;

use panesmith::{PaneConfig, PaneManager, PaneManagerConfig, Size, TerminalPaneWidget};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    Terminal,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut manager = PaneManager::new(PaneManagerConfig::default());
    let left = manager.spawn(
        PaneConfig::shell()
            .with_title("left")
            .with_size(Size::new(24, 40)),
    )?;
    let right = manager.spawn(
        PaneConfig::shell()
            .with_title("right")
            .with_size(Size::new(24, 40)),
    )?;

    let left_snapshot = manager.snapshot(left)?.to_owned_snapshot();
    let right_snapshot = manager.snapshot(right)?.to_owned_snapshot();
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    terminal.draw(|frame| {
        let areas = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(frame.area());
        frame.render_widget(TerminalPaneWidget::new(&left_snapshot), areas[0]);
        frame.render_widget(TerminalPaneWidget::new(&right_snapshot), areas[1]);
    })?;
    Ok(())
}
