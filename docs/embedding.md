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

## Child Environment Policy

`PaneConfig` defaults to inheriting the parent process environment and then
applying `PaneConfig::env`, preserving the behavior older Panesmith embedders
expect. Daemon embedders that run untrusted, agent-managed, or tenant-scoped
workloads should set an explicit environment policy instead of relying on that
default.

Use `with_clear_env` to start from an empty child environment:

```rust
# use panesmith_core::PaneConfig;
let pane_config = PaneConfig::command("/opt/agent/bin/agent")
    .with_clear_env()
    .with_env("PATH", "/usr/bin:/bin")
    .with_env("HOME", "/srv/panes/session-42")
    .with_env("PANESMITH_SESSION_ID", "session-42")
    .with_term_fallback("xterm-256color");
```

Use `with_env_allowlist` when a pane should inherit only a small known-safe
set of parent variables:

```rust
# use panesmith_core::PaneConfig;
let pane_config = PaneConfig::shell()
    .with_env_allowlist(["PATH", "HOME", "TERM"])
    .with_env("PANTHEON_RUN_ID", "run-123")
    .with_env("PATH", "/opt/pantheon/bin:/usr/bin:/bin")
    .with_term_fallback("xterm-256color");
```

Policy order is deterministic: Panesmith first applies the inheritance policy,
then fills `TERM` from `with_term_fallback` only when no `TERM` is present, and
finally applies explicit `with_env` values. Explicit values always override
inherited values and the fallback.

This is an environment boundary, not a sandbox. It prevents accidental leakage
of daemon environment variables such as tokens, API keys, socket paths, tracing
configuration, or cloud credentials into the child process. It does not change
the child process user, filesystem access, working directory, open file
descriptors, network access, PTY semantics, or host authorization model.

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

`PaneManagerConfig::with_default_scrollback` controls the retention policy for
new panes that do not set their own policy. `PaneConfig::with_scrollback`
overrides that default for one pane.

```rust
# use panesmith_core::{PaneConfig, PaneManagerConfig, ScrollbackConfig};
let manager_config = PaneManagerConfig::default().with_default_scrollback(
    ScrollbackConfig::bounded_lines(20_000)?,
);

let pane_config = PaneConfig::shell().with_scrollback(
    ScrollbackConfig::bounded_lines(10_000)?,
);
# Ok::<(), panesmith_core::PaneError>(())
```

The default policy is unbounded for compatibility. A bounded policy trims the
backing terminal history buffer; the visible screen rows are not counted
against the limit and are not evicted. The manager exposes retained lines
through `PaneManager::scrollback`, and `PaneSnapshot::stats` reports how many
history lines have been dropped by the retention policy.

Scrollback is storage, not a full UI. Hosts decide how to implement search,
selection, copying, and viewport controls.

## Event Log Retention

`PaneManagerConfig::with_event_log_retention` controls the per-pane event
history retained for diagnostics and repro dumps:

```rust
# use panesmith_core::{EventLogRetention, PaneManagerConfig};
let manager_config = PaneManagerConfig::default()
    .with_event_log_retention(EventLogRetention::bounded(10_000));
let dashboard_config = PaneManagerConfig::default()
    .with_max_event_log_entries(5_000);
let no_history_config = PaneManagerConfig::default()
    .with_event_log_retention(EventLogRetention::disabled());
# let _ = (manager_config, dashboard_config, no_history_config);
```

The default remains unlimited for compatibility. Brehon-style long-running
dashboards that supervise many high-output agent panes should bound or disable
retained event history so diagnostic storage does not grow for the full pane
lifetime. The policy does not change live delivery from
`PaneManager::drain_events`, subscribers, or event sinks. Snapshots and repro
dumps report `event_log_events_dropped` when retained history is partial.

## Embedded Limits

Embedded mode is not a perfect substitute for native terminal ownership. It is
well suited to dashboards, preview panes, logs, command shells, and routine
input. Use attach for programs that depend heavily on fullscreen behavior,
terminal mode changes, alternate screen state, or exact paste semantics.
