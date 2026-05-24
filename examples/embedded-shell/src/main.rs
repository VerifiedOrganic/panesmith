//! Embedded Shell Demo
//!
//! A minimal ratatui host application that embeds a live PTY-backed shell pane.
//! Embedded mode is for preview and routine input. Use attach mode when you
//! need native fullscreen behavior from complex terminal UIs.
//!
//! Controls
//! ========
//! - Type characters to send them to the shell.
//! - Press `Ctrl+Q` to quit the demo.
//! - The demo also handles paste, focus events, and terminal resize.
//! - The viewport follows live output; scrollback navigation is not
//!   implemented in this demo.
//!
//! The shell pane is rendered inside a bordered widget. A focus indicator is
//! shown in the border title. The real terminal cursor is positioned at the
//! pane's reported cursor position when visible.

use std::io;
use std::io::Write as _;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::{cursor, ExecutableCommand, QueueableCommand};
use panesmith_core::{
    CursorPosition, HostInput, OwnedPaneSnapshot, PaneConfig, PaneEventKind, PaneId, PaneManager,
    PaneManagerConfig, Size,
};
use panesmith_ratatui::TerminalPaneWidget;
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Margin, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Widget},
    Terminal,
};

fn main() -> io::Result<()> {
    let mut app = App::new()?;
    let result = app.run();
    app.restore_terminal()?;
    result
}

/// Restores terminal state on drop if not explicitly deactivated.
///
/// This ensures that a panic or early error during `App::new` does not leave
/// the user's terminal in raw mode / alternate screen.
struct TerminalGuard {
    active: bool,
}

impl TerminalGuard {
    fn prepare() -> io::Result<Self> {
        // Arm the guard *before* the first fallible change so that if any
        // subsequent setup step fails the Drop impl still cleans up whatever
        // state was successfully entered.
        let guard = Self { active: true };
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        stdout.execute(EnterAlternateScreen)?;
        stdout.execute(cursor::Hide)?;
        stdout.execute(event::EnableMouseCapture)?;
        stdout.execute(event::EnableBracketedPaste)?;
        stdout.execute(event::EnableFocusChange)?;
        Ok(guard)
    }

    fn deactivate(&mut self) {
        self.active = false;
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        if self.active {
            let mut stdout = io::stdout();
            warn_on_err(
                stdout.execute(event::DisableFocusChange),
                "disable focus change",
            );
            warn_on_err(
                stdout.execute(event::DisableBracketedPaste),
                "disable bracketed paste",
            );
            warn_on_err(
                stdout.execute(event::DisableMouseCapture),
                "disable mouse capture",
            );
            warn_on_err(stdout.execute(cursor::Show), "show cursor");
            warn_on_err(
                stdout.execute(LeaveAlternateScreen),
                "leave alternate screen",
            );
            warn_on_err(disable_raw_mode(), "disable raw mode");
        }
    }
}

struct App {
    terminal: Terminal<CrosstermBackend<io::Stdout>>,
    manager: PaneManager,
    pane_id: PaneId,
    pane_area: Rect,
    should_quit: bool,
    focus: bool,
    guard: TerminalGuard,
    pane_exited: bool,
    snapshot_warned: bool,
}

impl App {
    fn new() -> io::Result<Self> {
        let guard = TerminalGuard::prepare()?;

        let backend = CrosstermBackend::new(io::stdout());
        let terminal = Terminal::new(backend)?;

        let mut manager = PaneManager::new(PaneManagerConfig::default());
        let pane_id = manager
            .spawn(PaneConfig::default())
            .map_err(io::Error::other)?;

        // Resize the pane to match the initial terminal dimensions.
        let (cols, rows) = crossterm::terminal::size()?;
        let size = compute_pane_size(rows, cols);
        warn_on_err(manager.resize(pane_id, size), "resize pane");

        Ok(Self {
            terminal,
            manager,
            pane_id,
            pane_area: Rect::default(),
            should_quit: false,
            focus: true,
            guard,
            pane_exited: false,
            snapshot_warned: false,
        })
    }

    fn restore_terminal(&mut self) -> io::Result<()> {
        let backend = self.terminal.backend_mut();
        backend.execute(event::DisableFocusChange)?;
        backend.execute(event::DisableBracketedPaste)?;
        backend.execute(event::DisableMouseCapture)?;
        backend.execute(cursor::Show)?;
        backend.execute(LeaveAlternateScreen)?;
        disable_raw_mode()?;
        // Only disarm the guard once every restore step has succeeded.
        // If a step above fails the guard stays active so Drop gets a
        // second chance at cleanup when App is eventually dropped.
        self.guard.deactivate();
        Ok(())
    }

    fn run(&mut self) -> io::Result<()> {
        let tick_rate = Duration::from_millis(16); // ~60 FPS
        let mut events = Vec::new();

        while !self.should_quit {
            // Drain all pending crossterm events before redrawing.
            if event::poll(tick_rate)? {
                self.process_event(event::read()?);
                while event::poll(Duration::ZERO)? {
                    self.process_event(event::read()?);
                }
            }

            // Drain pane lifecycle/output events.
            events.clear();
            self.manager.drain_events(&mut events);
            self.handle_pane_events(&events);

            // Take a snapshot once per frame.
            let maybe_snapshot = match self.manager.snapshot(self.pane_id) {
                Ok(s) => {
                    self.snapshot_warned = false;
                    Some(s.to_owned_snapshot())
                }
                Err(e) => {
                    if !self.snapshot_warned {
                        eprintln!("warning: snapshot pane: {e}");
                        self.snapshot_warned = true;
                    }
                    None
                }
            };

            // Extract disjoint mutable references before drawing.
            let focus = self.focus;
            let pane_area = &mut self.pane_area;
            let terminal = &mut self.terminal;

            terminal.draw(|frame| {
                let area = frame.area();

                // Split area into main pane and a one-row help bar.
                let chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([Constraint::Min(3), Constraint::Length(1)])
                    .split(area);

                let main_area = chunks[0];
                let help_area = chunks[1];

                // Compute the inner area where the pane content will be rendered,
                // leaving room for the border.
                let pane_block = Block::default()
                    .borders(Borders::ALL)
                    .border_style(if focus {
                        Style::default().fg(Color::Green)
                    } else {
                        Style::default().fg(Color::DarkGray)
                    })
                    .title(if focus {
                        " Shell [focused] "
                    } else {
                        " Shell [unfocused] "
                    });

                let inner = main_area.inner(Margin::new(1, 1));
                *pane_area = inner;

                frame.render_widget(pane_block, main_area);

                // Render the shell pane snapshot.
                match maybe_snapshot.as_ref() {
                    Some(snapshot) => {
                        let widget = TerminalPaneWidget::new(snapshot);
                        widget.render(inner, frame.buffer_mut());
                    }
                    None => {
                        let msg = Paragraph::new("Pane not available");
                        msg.render(inner, frame.buffer_mut());
                    }
                }

                // Help bar.
                let help = Line::from(vec![
                    Span::styled("Ctrl+Q", Style::default().fg(Color::Yellow)),
                    Span::raw(
                        " quit  |  type to interact with the shell  |  resize window to resize pane",
                    ),
                ]);
                frame.render_widget(Paragraph::new(help), help_area);
            })?;

            // Position the real terminal cursor at the pane cursor.
            if let Some(ref snapshot) = maybe_snapshot {
                self.update_cursor_position(snapshot)?;
            }
        }

        // Clean up the pane before exiting, but only if it hasn't already
        // finished naturally (e.g. the user typed `exit` inside the shell).
        if !self.pane_exited {
            warn_on_err(
                self.manager
                    .kill(self.pane_id, panesmith_core::KillReason::HostRequested),
                "kill pane",
            );
        }
        Ok(())
    }

    fn process_event(&mut self, event: Event) {
        match event {
            Event::Key(key) => self.handle_key(key),
            Event::Mouse(mouse) => self.handle_mouse(mouse),
            Event::Paste(text) => self.handle_paste(&text),
            Event::FocusGained => {
                self.focus = true;
                warn_on_err(
                    self.manager
                        .send_input(self.pane_id, HostInput::FocusGained),
                    "send focus gained",
                );
            }
            Event::FocusLost => {
                self.focus = false;
                warn_on_err(
                    self.manager.send_input(self.pane_id, HostInput::FocusLost),
                    "send focus lost",
                );
            }
            Event::Resize(cols, rows) => self.handle_resize(rows, cols),
        }
    }

    fn handle_key(&mut self, key: KeyEvent) {
        // Host-level quit chord: Ctrl+Q
        if key.code == KeyCode::Char('q') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.should_quit = true;
            return;
        }

        // Convert crossterm key event to HostInput and route to pane.
        let ct_event = Event::Key(key);
        if let Ok(input) = HostInput::try_from(ct_event) {
            warn_on_err(self.manager.send_input(self.pane_id, input), "send key");
        }
    }

    fn handle_mouse(&mut self, mouse: crossterm::event::MouseEvent) {
        if let Some(translated) = translate_mouse_event(mouse, self.pane_area) {
            let ct_event = Event::Mouse(translated);
            if let Ok(input) = HostInput::try_from(ct_event) {
                warn_on_err(self.manager.send_input(self.pane_id, input), "send mouse");
            }
        }
    }

    fn handle_paste(&mut self, text: &str) {
        warn_on_err(
            self.manager
                .send_input(self.pane_id, HostInput::Paste(text.into())),
            "send paste",
        );
    }

    fn handle_resize(&mut self, rows: u16, cols: u16) {
        // Update the terminal backend size.
        warn_on_err(
            self.terminal
                .resize(ratatui::layout::Rect::new(0, 0, cols, rows)),
            "resize terminal",
        );

        // Resize the pane to match the new widget inner area.
        let size = compute_pane_size(rows, cols);
        warn_on_err(self.manager.resize(self.pane_id, size), "resize pane");
    }

    fn handle_pane_events(&mut self, events: &[panesmith_core::PaneEvent]) {
        for event in events {
            if event.pane_id != self.pane_id {
                continue;
            }
            if let PaneEventKind::Exited(_) = event.kind {
                // When the shell exits, quit the demo.
                self.should_quit = true;
                self.pane_exited = true;
            }
        }
    }

    fn update_cursor_position(&mut self, snapshot: &OwnedPaneSnapshot) -> io::Result<()> {
        let pane_area = self.pane_area;
        let terminal = &mut self.terminal;
        let backend = terminal.backend_mut();

        let show = self.focus && snapshot.cursor.visible;
        if show {
            if let Some(pos) = snapshot.cursor.position {
                if let Some((x, y)) = compute_cursor_screen_position(pos, pane_area) {
                    backend.queue(cursor::MoveTo(x, y))?;
                    backend.queue(cursor::Show)?;
                } else {
                    backend.queue(cursor::Hide)?;
                }
            } else {
                backend.queue(cursor::Hide)?;
            }
        } else {
            backend.queue(cursor::Hide)?;
        }

        backend.flush()?;
        Ok(())
    }
}

// ── Pure helper functions ───────────────────────────────────────────────

/// Compute the inner pane size given the total terminal dimensions.
///
/// The layout reserves 1 row for the help bar and 2 rows + 2 cols for borders.
/// When the terminal is too small for the full bordered layout, the result is
/// clamped to a minimum of 2×1 so the default vt100 surface stays in a
/// supported size range while resize events still reach the pane.
fn compute_pane_size(rows: u16, cols: u16) -> Size {
    let rows = rows.saturating_sub(3).max(2);
    let cols = cols.saturating_sub(2).max(1);
    Size::new(rows, cols)
}

/// Translate a global mouse event into pane-local coordinates.
///
/// Returns `None` if the event falls outside the pane area.
fn translate_mouse_event(
    mouse: crossterm::event::MouseEvent,
    pane_area: Rect,
) -> Option<crossterm::event::MouseEvent> {
    if mouse.column < pane_area.left()
        || mouse.column >= pane_area.right()
        || mouse.row < pane_area.top()
        || mouse.row >= pane_area.bottom()
    {
        return None;
    }
    Some(crossterm::event::MouseEvent {
        kind: mouse.kind,
        column: mouse.column - pane_area.x,
        row: mouse.row - pane_area.y,
        modifiers: mouse.modifiers,
    })
}

/// Map a zero-based cursor position into absolute screen coordinates,
/// clamping to the visible pane area.
///
/// Returns `None` when the pane area has zero width or height.
fn compute_cursor_screen_position(pos: CursorPosition, pane_area: Rect) -> Option<(u16, u16)> {
    if pane_area.width == 0 || pane_area.height == 0 {
        return None;
    }
    let x = pane_area
        .x
        .saturating_add(pos.col)
        .min(pane_area.right().saturating_sub(1));
    let y = pane_area
        .y
        .saturating_add(pos.row)
        .min(pane_area.bottom().saturating_sub(1));
    Some((x, y))
}

/// Log a warning when a `Result` is `Err`.
fn warn_on_err<T, E: std::fmt::Display>(result: Result<T, E>, context: &str) {
    if let Err(e) = result {
        eprintln!("warning: {context}: {e}");
    }
}

// ── Unit tests for pure helpers ────────────────────────────────────────

#[cfg(test)]
mod tests {
    use std::thread;
    use std::time::{Duration, Instant};

    use super::*;
    use panesmith_core::{KillReason, PaneEventKind, PaneManager, PaneManagerConfig};

    #[test]
    fn test_compute_pane_size_valid() {
        assert_eq!(compute_pane_size(10, 20), Size::new(7, 18));
    }

    #[test]
    fn test_compute_pane_size_clamps_to_minimum() {
        // When the terminal is too small for the full bordered layout,
        // the pane size is clamped to 2×1 so the default surface stays valid.
        assert_eq!(compute_pane_size(3, 2), Size::new(2, 1));
        assert_eq!(compute_pane_size(2, 10), Size::new(2, 8));
        assert_eq!(compute_pane_size(10, 2), Size::new(7, 1));
    }

    #[test]
    fn test_translate_mouse_event_inside() {
        let pane = Rect::new(5, 5, 10, 5);
        let mouse = crossterm::event::MouseEvent {
            kind: crossterm::event::MouseEventKind::Moved,
            column: 7,
            row: 7,
            modifiers: crossterm::event::KeyModifiers::empty(),
        };
        let translated = translate_mouse_event(mouse, pane).unwrap();
        assert_eq!(translated.column, 2);
        assert_eq!(translated.row, 2);
    }

    #[test]
    fn test_translate_mouse_event_outside() {
        let pane = Rect::new(5, 5, 10, 5);
        let mouse_left = crossterm::event::MouseEvent {
            kind: crossterm::event::MouseEventKind::Moved,
            column: 4,
            row: 7,
            modifiers: crossterm::event::KeyModifiers::empty(),
        };
        assert!(translate_mouse_event(mouse_left, pane).is_none());

        let mouse_bottom = crossterm::event::MouseEvent {
            kind: crossterm::event::MouseEventKind::Moved,
            column: 7,
            row: 10,
            modifiers: crossterm::event::KeyModifiers::empty(),
        };
        assert!(translate_mouse_event(mouse_bottom, pane).is_none());
    }

    #[test]
    fn test_compute_cursor_screen_position_basic() {
        let pane = Rect::new(5, 5, 10, 5);
        let pos = CursorPosition::new(2, 3);
        assert_eq!(compute_cursor_screen_position(pos, pane), Some((8, 7)));
    }

    #[test]
    fn test_compute_cursor_screen_position_clamps() {
        let pane = Rect::new(5, 5, 3, 2);
        // (0,0) inside
        assert_eq!(
            compute_cursor_screen_position(CursorPosition::new(0, 0), pane),
            Some((5, 5))
        );
        // Far outside gets clamped to bottom-right of inner area
        assert_eq!(
            compute_cursor_screen_position(CursorPosition::new(100, 100), pane),
            Some((7, 6))
        );
    }

    #[test]
    fn test_compute_cursor_screen_position_empty_pane() {
        let pane = Rect::new(5, 5, 0, 5);
        assert_eq!(
            compute_cursor_screen_position(CursorPosition::new(0, 0), pane),
            None
        );
    }

    /// Regression test: when a pane exits naturally (e.g. the user typed `exit`
    /// inside the embedded shell), the demo should not attempt to kill it again.
    /// Attempting to kill an already-exited pane produces an error, which
    /// previously leaked out as a spurious warning.
    #[test]
    fn demo_does_not_kill_already_exited_pane() {
        let mut manager = PaneManager::new(PaneManagerConfig::default());
        let pane_id = manager
            .spawn(PaneConfig::command_with_args("/bin/sh", ["-c", "exit 0"]))
            .expect("spawn /bin/sh");

        // Wait for the child to exit and the event to be emitted.
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut exited = false;
        while Instant::now() < deadline {
            let mut events = Vec::new();
            manager.drain_events(&mut events);
            for event in &events {
                if event.pane_id == pane_id {
                    if let PaneEventKind::Exited(_) = event.kind {
                        exited = true;
                    }
                }
            }
            if exited {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert!(exited, "pane should have exited naturally");

        // Killing an already-exited pane should fail.
        let result = manager.kill(pane_id, KillReason::HostRequested);
        assert!(
            result.is_err(),
            "killing an already-exited pane should fail"
        );
    }
}
