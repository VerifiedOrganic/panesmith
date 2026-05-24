# Embedded Panes

Embedded panes are the preview path. The host keeps ownership of the real
terminal and renders each child process into a rectangle.

## Lifecycle

1. Build a `PaneConfig`.
2. Spawn it with `PaneManager::spawn`.
3. Poll with `snapshot`, `scrollback`, `transcript`, or `drain_events`.
4. Resize with `PaneManager::resize` when the host layout changes.
5. Send input with `send_input` or `write_bytes`.
6. Kill and remove the pane when it is no longer needed.

The manager drains pending PTY output before returning snapshots, scrollback,
transcripts, and last-sequence information. Hosts do not need a separate
surface polling API for normal rendering.

`PaneManager::kill` requests child termination and transitions the pane to
`PaneState::Killed`, but it intentionally keeps the manager-owned runtime entry
alive. That lets callers inspect final snapshots, scrollback, transcripts,
repro dumps, and events after a kill. `PaneManager::remove` is the cleanup
step that drops the pane's PTY/process handle, surface backend, transcript
buffers, and retained event log; it only accepts panes that are exited, killed,
or failed.

Reset/restart paths that do not need post-kill inspection should use
`PaneManager::kill_and_remove`, which performs the same `kill(); remove()`
sequence and drops manager-owned pane resources before returning.

## Rendering

`TerminalPaneWidget` renders a `PaneSnapshot` into a ratatui area.

```rust
let snapshot = manager.snapshot(pane_id)?;
frame.render_widget(TerminalPaneWidget::new(&snapshot), area);
```

The snapshot includes visible rows, cursor state, title, terminal modes,
scrollback metadata, pane state, and interaction mode.

## Input

Use structured input when the host already has key, mouse, paste, focus, or
resize events:

```rust
manager.send_input(pane_id, input)?;
```

Use raw bytes for host-specific control paths:

```rust
manager.write_bytes(pane_id, b"ls\n")?;
```

Structured input is encoded through the runtime input encoder. Raw bytes are
written directly to the child PTY.

## Resize

Resize is applied to both the PTY and the surface. If the PTY resize fails for a
running pane, the surface is left unchanged so the host does not display a size
the child does not actually have.

## Scrollback

`PaneConfig` controls retained scrollback limits. The manager exposes retained
lines through `PaneManager::scrollback`.

Scrollback is storage, not a full UI. Hosts decide how to implement search,
selection, copying, and viewport controls.

## Embedded Limits

Embedded mode is not a perfect substitute for native terminal ownership. It is
well suited to dashboards, preview panes, logs, command shells, and routine
input. Use attach for programs that depend heavily on fullscreen behavior,
terminal mode changes, alternate screen state, or exact paste semantics.
