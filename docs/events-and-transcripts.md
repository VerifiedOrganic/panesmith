# Events and Transcripts

Panesmith records pane activity through ordered events and optional transcript
buffers. These are separate tools: events describe what happened, while
transcripts retain output content.

## Events

`PaneEvent` values are pane-scoped, timestamped, and sequence-numbered.

Hosts can consume them in three ways:

- `PaneManager::drain_events` drains the manager queue.
- `PaneManager::subscribe` receives future events through a channel.
- `PaneManager::set_event_sink` mirrors future events into a callback.

Events cover spawn, state transitions, input, output metadata, resize, surface
changes, overflow, transcript rotation, attach lifecycle, and errors.

Output events do not include raw bytes by default. They include metadata such
as byte length, escape-sequence presence, and transcript offset when transcript
capture is enabled.

`PaneManager::drain_events` drains the live manager queue only. Panesmith also
keeps per-pane retained event history for repro dumps and diagnostics. That
history is controlled separately with
`PaneManagerConfig::with_event_log_retention`:

```rust
# use panesmith_core::{EventLogRetention, PaneManagerConfig};
let config = PaneManagerConfig::default()
    .with_event_log_retention(EventLogRetention::bounded(10_000));
```

The default is unlimited for compatibility. Long-running dashboard hosts, such
as Brehon supervising high-output agent panes, should use bounded retention or
disable retained history with `EventLogRetention::disabled()` when repro dumps
do not need event history. Live event delivery through `drain_events`,
subscribers, and event sinks is unchanged by this setting.

## Transcripts

`TranscriptConfig` controls retained output buffers:

- `Disabled` records no content.
- `PlainText` strips terminal escape sequences.
- `RawBytes` records PTY bytes.
- `Both` records both views.

Buffers can be bounded by lines and bytes. When limits are exceeded, the oldest
content rotates out and the manager emits transcript rotation metadata.

Read retained buffers with:

```rust
let transcript = manager.transcript(pane_id)?;
let text = transcript.plain_text(..);
let bytes = transcript.ansi_bytes(..);
```

Offsets in events are absolute stream offsets. Reader offsets tell you where
the currently retained slice begins, which lets a host detect when old content
has rotated out.

## Repro Dumps

`PaneManager::dump_repro` captures enough pane state to replay a surface from a
retained raw transcript when the backend supports replay. Dumps redact spawn
metadata by default so they are safer to share in issue reports.

Repro dumps include only the retained per-pane event history. When bounded or
disabled event-log retention has omitted older events, the dump's
`event_log_events_dropped` field reports how many events are missing.

Repro dumps are intended for terminal-rendering bugs and regression tests, not
for long-term session recording.
