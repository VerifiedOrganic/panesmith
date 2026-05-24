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

Repro dumps are intended for terminal-rendering bugs and regression tests, not
for long-term session recording.
