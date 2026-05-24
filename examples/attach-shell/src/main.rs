//! Attach Shell Demo
//!
//! A ratatui host application that keeps a live embedded PTY preview and can
//! hand the same shell over to the real terminal via fullscreen attach.
//! Embedded mode is for preview and routine input. Attach mode is the
//! correctness path for complex interactive TUIs.
//!
//! Controls
//! ========
//! - Type characters to send them to the embedded shell preview.
//! - Press `Ctrl+F` to attach the shell fullscreen.
//! - Press `Ctrl+]` while attached to return to the dashboard.
//! - Press `Ctrl+Q` to quit the demo.
//! - Paste, focus changes, mouse events, and resize are forwarded in embedded
//!   mode using the current terminal-mode flags reported by the child.

use std::env;
use std::io;
use std::io::Write as _;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::{cursor, ExecutableCommand, QueueableCommand};
use panesmith_attach::{
    AttachOptions, BlockingAttachOutcome, BlockingAttachSession, CrosstermTerminalControl,
    StdioAttachTerminal,
};
use panesmith_core::{
    CursorPosition, HostInput, InputConfig, OwnedPaneSnapshot, OwnedScrollbackSnapshot, PaneConfig,
    PaneId, PaneInteractionMode, PaneState, PaneStats, PortablePtyBackend, PortablePtyProcess,
    PtyBackend, PtyFrame, PtyProcess, Size, SurfaceBackend, XtermEncoder,
};
use panesmith_ratatui::TerminalPaneWidget;
use panesmith_vt100::Vt100Backend;
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Margin, Rect},
    style::{Color, Style},
    text::{Line, Span, Text},
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
struct TerminalGuard {
    active: bool,
}

impl TerminalGuard {
    fn prepare() -> io::Result<Self> {
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
    pane: DemoPane,
    pane_area: Rect,
    should_quit: bool,
    focus: bool,
    guard: TerminalGuard,
    attach_event_seq: u64,
    status_line: String,
}

struct DemoPane {
    id: PaneId,
    process: PortablePtyProcess,
    surface: Vt100Backend,
    encoder: XtermEncoder,
    exited: bool,
    exit_code: Option<i32>,
}

impl DemoPane {
    fn new(id: PaneId, size: Size) -> io::Result<Self> {
        let shell = env::var("SHELL").unwrap_or_else(|_| "sh".to_string());
        let process = PortablePtyBackend
            .spawn(&PaneConfig::command(shell).with_size(size))
            .map_err(io::Error::other)?;
        let surface = Vt100Backend::new(id, size).map_err(io::Error::other)?;

        Ok(Self {
            id,
            process,
            surface,
            encoder: XtermEncoder::new(InputConfig::default()),
            exited: false,
            exit_code: None,
        })
    }

    fn size(&self) -> Size {
        self.surface.size()
    }

    fn is_exited(&self) -> bool {
        self.exited
    }

    fn kill(&mut self) -> io::Result<()> {
        self.process.kill().map_err(io::Error::other)
    }

    fn write_host_input(&mut self, input: HostInput) -> io::Result<()> {
        let bytes = self
            .encoder
            .encode(&input, &self.surface.modes())
            .map_err(io::Error::other)?;
        if bytes.is_empty() {
            return Ok(());
        }
        self.process
            .writer()
            .write_bytes(&bytes)
            .map_err(io::Error::other)
    }

    fn resize(&mut self, size: Size) -> io::Result<()> {
        self.process.resize(size).map_err(io::Error::other)?;
        self.surface.resize(size).map_err(io::Error::other)?;
        Ok(())
    }

    fn drain_output(&mut self) -> io::Result<()> {
        while let Some(frame) = self.process.try_recv() {
            match frame {
                PtyFrame::Output { bytes, .. } => {
                    self.surface.feed(&bytes).map_err(io::Error::other)?;
                }
                PtyFrame::Exited { code, .. } => {
                    self.exited = true;
                    self.exit_code = code;
                }
                PtyFrame::Overflow {
                    dropped_frames,
                    dropped_bytes,
                    ..
                } => eprintln!(
                    "warning: PTY overflow dropped {dropped_frames} frame(s) / {dropped_bytes} byte(s)"
                ),
                PtyFrame::Error { message, .. } => {
                    eprintln!("warning: PTY runtime error: {message}");
                }
                PtyFrame::CursorPositionRequest { .. } => {}
            }
        }

        Ok(())
    }

    fn snapshot(&self) -> OwnedPaneSnapshot {
        OwnedPaneSnapshot {
            id: self.id,
            title: Some("Shell".into()),
            state: if self.exited {
                PaneState::Exited {
                    code: self.exit_code,
                }
            } else {
                PaneState::Running
            },
            interaction_mode: PaneInteractionMode::Embedded,
            size: self.surface.size(),
            surface: self.surface.snapshot().to_owned_snapshot(),
            cursor: self.surface.cursor(),
            modes: self.surface.modes(),
            stats: PaneStats,
        }
    }

    fn scrollback(&self) -> OwnedScrollbackSnapshot {
        self.surface.scrollback().to_owned_snapshot()
    }
}

impl App {
    fn new() -> io::Result<Self> {
        let guard = TerminalGuard::prepare()?;
        let backend = CrosstermBackend::new(io::stdout());
        let terminal = Terminal::new(backend)?;
        let (cols, rows) = crossterm::terminal::size()?;
        let pane_size = compute_pane_size(rows, cols);

        Ok(Self {
            terminal,
            pane: DemoPane::new(PaneId::new(1), pane_size)?,
            pane_area: Rect::default(),
            should_quit: false,
            focus: true,
            guard,
            attach_event_seq: 0,
            status_line: "Embedded preview live. Press Ctrl+F to attach fullscreen.".into(),
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
        self.guard.deactivate();
        Ok(())
    }

    fn run(&mut self) -> io::Result<()> {
        let tick_rate = Duration::from_millis(16);

        while !self.should_quit {
            if event::poll(tick_rate)? {
                self.process_event(event::read()?);
                while event::poll(Duration::ZERO)? {
                    self.process_event(event::read()?);
                }
            }

            self.pane.drain_output()?;
            if self.pane.is_exited() {
                self.should_quit = true;
            }

            let snapshot = self.pane.snapshot();
            let scrollback = self.pane.scrollback();
            let focus = self.focus;
            let pane_area = &mut self.pane_area;
            let status_line = self.status_line.clone();
            let terminal = &mut self.terminal;

            terminal.draw(|frame| {
                let area = frame.area();
                let chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([Constraint::Min(3), Constraint::Length(2)])
                    .split(area);

                let main_area = chunks[0];
                let footer_area = chunks[1];

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
                TerminalPaneWidget::new(&snapshot)
                    .with_scrollback(&scrollback)
                    .render(inner, frame.buffer_mut());

                let footer = Text::from(vec![
                    Line::from(vec![
                        Span::styled("Status:", Style::default().fg(Color::Cyan)),
                        Span::raw(" "),
                        Span::raw(status_line),
                    ]),
                    Line::from(vec![
                        Span::styled("Ctrl+F", Style::default().fg(Color::Yellow)),
                        Span::raw(" attach  |  "),
                        Span::styled("Ctrl+]", Style::default().fg(Color::Yellow)),
                        Span::raw(" detach fullscreen  |  "),
                        Span::styled("Ctrl+Q", Style::default().fg(Color::Yellow)),
                        Span::raw(" quit"),
                    ]),
                ]);
                frame.render_widget(Paragraph::new(footer), footer_area);
            })?;

            self.update_cursor_position(&snapshot)?;
        }

        if !self.pane.is_exited() {
            warn_on_err(self.pane.kill(), "kill pane");
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
                    self.pane.write_host_input(HostInput::FocusGained),
                    "send focus gained",
                );
            }
            Event::FocusLost => {
                self.focus = false;
                warn_on_err(
                    self.pane.write_host_input(HostInput::FocusLost),
                    "send focus lost",
                );
            }
            Event::Resize(cols, rows) => self.handle_resize(rows, cols),
        }
    }

    fn handle_key(&mut self, key: KeyEvent) {
        if key.code == KeyCode::Char('q') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.should_quit = true;
            return;
        }

        if key.code == KeyCode::Char('f') && key.modifiers.contains(KeyModifiers::CONTROL) {
            if let Err(error) = self.attach_fullscreen() {
                self.status_line = format!("Attach failed: {error}");
                warn_on_err::<(), _>(Err(error), "attach fullscreen");
            }
            return;
        }

        let event = Event::Key(key);
        if let Ok(input) = HostInput::try_from(event) {
            warn_on_err(self.pane.write_host_input(input), "send key");
        }
    }

    fn handle_mouse(&mut self, mouse: crossterm::event::MouseEvent) {
        if let Some(translated) = translate_mouse_event(mouse, self.pane_area) {
            let event = Event::Mouse(translated);
            if let Ok(input) = HostInput::try_from(event) {
                warn_on_err(self.pane.write_host_input(input), "send mouse");
            }
        }
    }

    fn handle_paste(&mut self, text: &str) {
        warn_on_err(
            self.pane.write_host_input(HostInput::Paste(text.into())),
            "send paste",
        );
    }

    fn handle_resize(&mut self, rows: u16, cols: u16) {
        warn_on_err(
            self.terminal
                .resize(ratatui::layout::Rect::new(0, 0, cols, rows)),
            "resize terminal",
        );

        let size = compute_pane_size(rows, cols);
        warn_on_err(self.pane.resize(size), "resize pane");
    }

    fn attach_fullscreen(&mut self) -> io::Result<()> {
        if self.pane.is_exited() {
            self.status_line = "Shell already exited; attach is unavailable.".into();
            return Ok(());
        }

        self.pane.drain_output()?;

        let mut terminal = StdioAttachTerminal::new(io::stdout())?;
        let mut control =
            CrosstermTerminalControl::new(io::stdout()).with_host_alternate_screen(true);
        let mut session = BlockingAttachSession::new(
            self.pane.id,
            AttachOptions::default(),
            self.pane.size(),
            self.attach_event_seq,
        )
        .with_poll_interval(Duration::from_millis(1));

        let outcome = session
            .run(
                &mut terminal,
                &mut self.pane.process,
                &mut self.pane.surface,
                &mut control,
            )
            .map_err(io::Error::other)?;
        self.attach_event_seq = session.last_event_seq();
        self.finish_attach(outcome)?;

        Ok(())
    }

    fn finish_attach(&mut self, outcome: BlockingAttachOutcome) -> io::Result<()> {
        self.status_line = format_attach_status(&outcome);
        if outcome.child_exit_code.is_some() {
            self.pane.exited = true;
            self.pane.exit_code = outcome.child_exit_code;
            self.should_quit = true;
        }

        let (cols, rows) = crossterm::terminal::size()?;
        self.handle_resize(rows, cols);
        Ok(())
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

fn format_attach_status(outcome: &BlockingAttachOutcome) -> String {
    let mut status = format!(
        "Detached via {:?}; restored embedded size {}x{}.",
        outcome.reason, outcome.restored_size.rows, outcome.restored_size.cols
    );
    if let Some(code) = outcome.child_exit_code {
        status.push_str(&format!(" Child exited with code {code}."));
    }
    if !outcome.remaining_input.is_empty() {
        status.push_str(" Trailing bytes after the detach chord were returned to the host.");
    }
    status
}

/// Compute the inner pane size given the total terminal dimensions.
///
/// The layout reserves 2 rows for status/help text and 2 cols for borders.
/// When the terminal is too small for the full bordered layout, the result is
/// clamped to a minimum of 2×1 so the default vt100 surface stays in a
/// supported size range while resize events still reach the pane.
fn compute_pane_size(rows: u16, cols: u16) -> Size {
    let rows = rows.saturating_sub(4).max(2);
    let cols = cols.saturating_sub(2).max(1);
    Size::new(rows, cols)
}

/// Translate a global mouse event into pane-local coordinates.
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

/// Map a zero-based cursor position into absolute screen coordinates.
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

fn warn_on_err<T, E: std::fmt::Display>(result: Result<T, E>, context: &str) {
    if let Err(error) = result {
        eprintln!("warning: {context}: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compute_pane_size_reserves_border_and_footer() {
        assert_eq!(compute_pane_size(12, 20), Size::new(8, 18));
    }

    #[test]
    fn compute_pane_size_clamps_tiny_terminals() {
        assert_eq!(compute_pane_size(4, 2), Size::new(2, 1));
        assert_eq!(compute_pane_size(3, 10), Size::new(2, 8));
        assert_eq!(compute_pane_size(10, 1), Size::new(6, 1));
    }

    #[test]
    fn translate_mouse_event_inside_pane_uses_local_coordinates() {
        let pane = Rect::new(5, 5, 10, 5);
        let mouse = crossterm::event::MouseEvent {
            kind: crossterm::event::MouseEventKind::Moved,
            column: 7,
            row: 8,
            modifiers: crossterm::event::KeyModifiers::empty(),
        };

        let translated = translate_mouse_event(mouse, pane).expect("mouse should be inside pane");
        assert_eq!(translated.column, 2);
        assert_eq!(translated.row, 3);
    }

    #[test]
    fn translate_mouse_event_outside_pane_returns_none() {
        let pane = Rect::new(5, 5, 10, 5);
        let mouse = crossterm::event::MouseEvent {
            kind: crossterm::event::MouseEventKind::Moved,
            column: 2,
            row: 8,
            modifiers: crossterm::event::KeyModifiers::empty(),
        };

        assert!(translate_mouse_event(mouse, pane).is_none());
    }

    #[test]
    fn compute_cursor_screen_position_clamps_to_visible_area() {
        let pane = Rect::new(3, 4, 4, 2);

        assert_eq!(
            compute_cursor_screen_position(CursorPosition::new(0, 0), pane),
            Some((3, 4))
        );
        assert_eq!(
            compute_cursor_screen_position(CursorPosition::new(99, 99), pane),
            Some((6, 5))
        );
    }

    #[test]
    fn attach_status_mentions_returned_trailing_input() {
        let status = format_attach_status(&BlockingAttachOutcome {
            reason: panesmith_core::DetachReason::UserChord,
            child_exit_code: None,
            terminal_size: Size::new(40, 120),
            restored_size: Size::new(20, 80),
            remaining_input: b"abc".to_vec(),
        });

        assert!(status.contains("Trailing bytes"));
        assert!(status.contains("20x80"));
    }
}
