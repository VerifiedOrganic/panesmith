#![cfg(unix)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[allow(dead_code)]
mod support;

use panesmith_core::{
    CellWidth, ColorSpec, CursorPosition, HostInput, OwnedPaneSnapshot, OwnedScrollbackSnapshot,
    PaneConfig, PaneId, PaneManager, PaneManagerConfig, PaneState, ReproDumpOptions, Size,
    SurfaceRow, SurfaceSnapshot, TerminalViewport, TranscriptConfig, TranscriptMode,
};
use panesmith_ratatui::TerminalPaneWidget;
use ratatui::{
    backend::TestBackend,
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier},
    Terminal,
};
use support::{fixture_path, POLL_INTERVAL, WAIT_TIMEOUT};

static NEXT_TEMP_ID: AtomicU64 = AtomicU64::new(1);

fn unique_output_path(label: &str) -> PathBuf {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should move forward")
        .as_nanos();
    let suffix = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);

    std::env::temp_dir().join(format!(
        "panesmith-{label}-{}-{timestamp}-{suffix}.out",
        std::process::id()
    ))
}

fn wait_for_path(path: &Path, timeout: Duration) {
    let deadline = Instant::now() + timeout;

    while Instant::now() < deadline {
        if path.exists() {
            return;
        }
        thread::sleep(POLL_INTERVAL);
    }

    panic!("timed out waiting for {}", path.display());
}

fn spawn_manager_fixture(name: &str, config: PaneConfig) -> (PaneManager, PaneId) {
    let mut manager = PaneManager::new(PaneManagerConfig::default());
    let pane_id = manager
        .spawn(config)
        .unwrap_or_else(|error| panic!("{name} fixture should spawn through PaneManager: {error}"));
    let mut spawn_events = Vec::new();
    manager.drain_events(&mut spawn_events);

    (manager, pane_id)
}

fn fixture_config(name: &str, size: Size) -> PaneConfig {
    PaneConfig::command(fixture_path(name).to_string_lossy().into_owned())
        .with_size(size)
        .with_transcript(TranscriptConfig::new(TranscriptMode::Both))
}

fn wait_for_snapshot(
    manager: &mut PaneManager,
    pane_id: PaneId,
    label: &str,
    predicate: impl Fn(&OwnedPaneSnapshot) -> bool,
) -> OwnedPaneSnapshot {
    let deadline = Instant::now() + WAIT_TIMEOUT;
    let mut last = None;

    while Instant::now() < deadline {
        let snapshot = manager
            .snapshot(pane_id)
            .expect("pane should remain available while waiting")
            .to_owned_snapshot();
        if predicate(&snapshot) {
            return snapshot;
        }

        last = Some(snapshot);
        thread::sleep(POLL_INTERVAL);
    }

    let snapshot = last.expect("wait loop should sample at least once");
    panic!(
        "timed out waiting for {label}; state={:?}; surface:\n{}",
        snapshot.state,
        surface_text(&snapshot.surface)
    );
}

fn wait_for_snapshot_and_scrollback(
    manager: &mut PaneManager,
    pane_id: PaneId,
    label: &str,
    predicate: impl Fn(&OwnedPaneSnapshot, &OwnedScrollbackSnapshot) -> bool,
) -> (OwnedPaneSnapshot, OwnedScrollbackSnapshot) {
    let deadline = Instant::now() + WAIT_TIMEOUT;
    let mut last = None;

    while Instant::now() < deadline {
        let snapshot = manager
            .snapshot(pane_id)
            .expect("pane should remain available while waiting")
            .to_owned_snapshot();
        let scrollback = manager
            .scrollback(pane_id)
            .expect("pane scrollback should remain available while waiting")
            .to_owned_snapshot();

        if predicate(&snapshot, &scrollback) {
            return (snapshot, scrollback);
        }

        last = Some((snapshot, scrollback));
        thread::sleep(POLL_INTERVAL);
    }

    let (snapshot, scrollback) = last.expect("wait loop should sample at least once");
    panic!(
        "timed out waiting for {label}; state={:?}; surface:\n{}\nscrollback:\n{}",
        snapshot.state,
        surface_text(&snapshot.surface),
        scrollback_text(&scrollback)
    );
}

fn wait_for_exit(manager: &mut PaneManager, pane_id: PaneId, label: &str) -> OwnedPaneSnapshot {
    wait_for_snapshot(manager, pane_id, label, |snapshot| {
        matches!(snapshot.state, PaneState::Exited { code: Some(0) })
    })
}

fn row_text(row: &SurfaceRow<'_>) -> String {
    let mut out = String::new();

    for cell in &row.cells {
        if cell.text.is_empty() || matches!(cell.width, CellWidth::Continuation) {
            out.push(' ');
        } else {
            out.push_str(cell.text.as_ref());
        }
    }

    out
}

fn surface_text(surface: &SurfaceSnapshot<'_>) -> String {
    surface
        .rows
        .iter()
        .map(row_text)
        .collect::<Vec<_>>()
        .join("\n")
}

fn snapshot_text(snapshot: &OwnedPaneSnapshot) -> String {
    surface_text(&snapshot.surface)
}

fn scrollback_text(scrollback: &OwnedScrollbackSnapshot) -> String {
    scrollback
        .lines
        .iter()
        .map(|line| line.text.as_ref())
        .collect::<Vec<_>>()
        .join("\n")
}

fn assert_repro_replays(manager: &mut PaneManager, pane_id: PaneId) {
    let dump = manager
        .dump_repro(pane_id, ReproDumpOptions::default())
        .expect("manager should dump a fixture-derived repro");
    let raw = dump
        .raw_transcript
        .as_ref()
        .expect("fixture repro should include raw transcript bytes");
    assert!(
        !raw.bytes.is_empty(),
        "fixture repro should retain raw output bytes"
    );

    let replayed = dump
        .replay()
        .expect("fixture repro should build a replay")
        .final_surface()
        .expect("fixture repro replay should reconstruct the final surface");

    assert_eq!(replayed, dump.final_surface);
}

fn render_snapshot(snapshot: &OwnedPaneSnapshot) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(snapshot.size.cols, snapshot.size.rows))
        .expect("test backend should initialize");
    terminal
        .draw(|frame| {
            frame.render_widget(
                TerminalPaneWidget::new(snapshot).focused(false),
                Rect::new(0, 0, snapshot.size.cols, snapshot.size.rows),
            );
        })
        .expect("widget should render into the test backend");

    terminal.backend().buffer().clone()
}

fn render_scrollback_view(
    snapshot: &OwnedPaneSnapshot,
    scrollback: &OwnedScrollbackSnapshot,
    scroll_offset: usize,
) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(snapshot.size.cols, snapshot.size.rows))
        .expect("test backend should initialize");
    terminal
        .draw(|frame| {
            frame.render_widget(
                TerminalPaneWidget::new(snapshot)
                    .focused(false)
                    .with_scrollback(scrollback)
                    .with_viewport(TerminalViewport {
                        scroll_offset,
                        follow_tail: false,
                    }),
                Rect::new(0, 0, snapshot.size.cols, snapshot.size.rows),
            );
        })
        .expect("widget should render scrollback into the test backend");

    terminal.backend().buffer().clone()
}

fn buffer_text(buffer: &Buffer) -> String {
    let mut lines = Vec::new();

    for y in buffer.area.y..buffer.area.y + buffer.area.height {
        let mut line = String::new();
        for x in buffer.area.x..buffer.area.x + buffer.area.width {
            line.push_str(buffer[(x, y)].symbol());
        }
        lines.push(line);
    }

    lines.join("\n")
}

#[test]
fn manager_alt_screen_snapshot_modes_and_repro_stay_consistent() {
    let size = Size::new(4, 40);
    let config = fixture_config("alt-screen", size);
    let (mut manager, pane_id) = spawn_manager_fixture("alt-screen", config);

    let snapshot = wait_for_snapshot(
        &mut manager,
        pane_id,
        "active alt-screen snapshot",
        |snapshot| {
            snapshot.modes.alternate_screen && snapshot_text(snapshot).contains("alt-screen ready")
        },
    );

    assert_eq!(snapshot.size, size);
    assert!(snapshot.cursor.visible);
    assert_eq!(
        snapshot.cursor.position,
        Some(CursorPosition::new(1, 0)),
        "fixture CRLF should leave the cursor at the start of the next row"
    );
    assert_eq!(snapshot.surface.rows.len(), usize::from(size.rows));
    assert_repro_replays(&mut manager, pane_id);

    manager
        .write_bytes(pane_id, b"__PANESMITH_EXIT__\n")
        .expect("alt-screen exit command should write through PaneManager");
    wait_for_exit(&mut manager, pane_id, "alt-screen exit");
}

#[test]
fn manager_slash_menu_preview_scrollback_repro_and_ratatui_stay_consistent() {
    let size = Size::new(4, 60);
    let config = fixture_config("slash-menu", size);
    let (mut manager, pane_id) = spawn_manager_fixture("slash-menu", config);

    let initial = wait_for_snapshot(&mut manager, pane_id, "slash-menu prompt", |snapshot| {
        snapshot.modes.focus_events
            && snapshot.modes.bracketed_paste
            && snapshot_text(snapshot).contains("prompt> ")
    });
    assert!(initial.cursor.position.is_some());
    assert!(initial.cursor.visible);

    manager
        .write_bytes(pane_id, b"/")
        .expect("slash-menu trigger should write through PaneManager");
    let (_menu_snapshot, menu_scrollback) = wait_for_snapshot_and_scrollback(
        &mut manager,
        pane_id,
        "slash-menu frame",
        |snapshot, scrollback| {
            let visible = snapshot_text(snapshot);
            let history = scrollback_text(scrollback);
            visible.contains("/status") || history.contains("/status")
        },
    );
    assert!(
        scrollback_text(&menu_scrollback).contains("menu:")
            || scrollback_text(&menu_scrollback).contains("/status"),
        "small embedded viewport should retain menu lines in scrollback"
    );

    manager
        .send_input(
            pane_id,
            HostInput::Paste("cargo test --workspace".to_string()),
        )
        .expect("paste should encode according to the fixture's bracketed-paste mode");
    let (snapshot, scrollback) = wait_for_snapshot_and_scrollback(
        &mut manager,
        pane_id,
        "slash-menu pasted payload",
        |snapshot, _scrollback| snapshot_text(snapshot).contains("paste:cargo test --workspace"),
    );

    let visible = snapshot_text(&snapshot);
    let history = scrollback_text(&scrollback);
    assert!(
        visible.contains("paste:cargo test --workspace"),
        "expected pasted payload in visible snapshot, got {visible:?}"
    );
    assert!(
        history.contains("menu:") || history.contains("/status"),
        "expected menu history in scrollback, got {history:?}"
    );
    assert!(snapshot.modes.focus_events);
    assert!(snapshot.modes.bracketed_paste);
    assert!(snapshot.cursor.position.is_some());
    assert_repro_replays(&mut manager, pane_id);

    let rendered = render_snapshot(&snapshot);
    let rendered_text = buffer_text(&rendered);
    assert!(
        rendered_text.contains("paste:cargo test --workspace"),
        "ratatui render should preserve fixture-derived visible text, got {rendered_text:?}"
    );

    let history_buffer =
        render_scrollback_view(&snapshot, &scrollback, snapshot.size.rows as usize);
    let history_text = buffer_text(&history_buffer);
    assert!(
        history_text.contains("menu:") || history_text.contains("/status"),
        "ratatui render should preserve fixture-derived scrollback text, got {history_text:?}"
    );

    manager
        .write_bytes(pane_id, b"__PANESMITH_EXIT__\r")
        .expect("slash-menu exit command should write through PaneManager");
    wait_for_exit(&mut manager, pane_id, "slash-menu exit");
}

#[test]
fn manager_resize_reporter_updates_surface_size_and_visible_rows() {
    let initial_size = Size::new(12, 34);
    let resized = Size::new(20, 70);
    let ready_path = unique_output_path("manager-resize-reporter-ready");
    let config = fixture_config("resize-reporter", initial_size).with_env(
        "PANESMITH_READY_FILE",
        ready_path.to_string_lossy().into_owned(),
    );
    let (mut manager, pane_id) = spawn_manager_fixture("resize-reporter", config);

    wait_for_path(&ready_path, WAIT_TIMEOUT);
    manager
        .resize(pane_id, resized)
        .expect("PaneManager should resize the PTY and surface");

    let snapshot = wait_for_snapshot(&mut manager, pane_id, "resize report", |snapshot| {
        snapshot.size == resized && snapshot_text(snapshot).contains("size:20x70")
    });
    assert_eq!(snapshot.size, resized);
    assert_eq!(snapshot.surface.size, resized);
    assert_eq!(snapshot.surface.rows.len(), usize::from(resized.rows));
    assert!(snapshot.cursor.position.is_some());
    assert_repro_replays(&mut manager, pane_id);

    manager
        .write_bytes(pane_id, b"__PANESMITH_EXIT__\n")
        .expect("resize-reporter exit command should write through PaneManager");
    wait_for_exit(&mut manager, pane_id, "resize-reporter exit");
    let _ = fs::remove_file(&ready_path);
}

#[test]
fn manager_styled_wrap_fixture_preserves_styles_wrapping_repro_and_ratatui() {
    let size = Size::new(3, 8);
    let config = fixture_config("styled-wrap", size);
    let (mut manager, pane_id) = spawn_manager_fixture("styled-wrap", config);

    let snapshot = wait_for_snapshot(&mut manager, pane_id, "styled wrapped output", |snapshot| {
        matches!(snapshot.state, PaneState::Exited { code: Some(0) })
            && snapshot_text(snapshot).contains("plain")
    });

    let text = snapshot_text(&snapshot);
    assert!(
        text.contains("redwrap") && text.contains("plain"),
        "expected styled wrapped payload in snapshot, got {text:?}"
    );
    assert!(
        snapshot.surface.rows[0].wrapped,
        "expected first row to be marked as autowrapped"
    );
    let styled_cell = &snapshot.surface.rows[0].cells[0];
    assert_eq!(styled_cell.text.as_ref(), "r");
    assert_eq!(styled_cell.style.fg, Some(ColorSpec::Indexed(1)));
    assert!(styled_cell.style.attrs.bold);
    assert_repro_replays(&mut manager, pane_id);

    let rendered = render_snapshot(&snapshot);
    assert_eq!(rendered[(0, 0)].symbol(), "r");
    assert_eq!(rendered[(0, 0)].fg, Color::Indexed(1));
    assert!(rendered[(0, 0)].modifier.contains(Modifier::BOLD));
    let rendered_text = buffer_text(&rendered);
    assert!(
        rendered_text.contains("plain"),
        "ratatui render should preserve wrapped fixture text, got {rendered_text:?}"
    );
}
