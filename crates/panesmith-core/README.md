# panesmith-core

`panesmith-core` provides the PTY pane runtime, event model, transcript
retention, repro dump types, and surface abstractions that the rest of the
workspace builds on.

## What this crate covers

Use this crate when you need pane lifecycle control, snapshots, transcripts,
and event streams without pulling in a specific rendering layer.

## Pane lifecycle cleanup

`PaneManager::kill` terminates the child and leaves the pane in the manager for
post-kill inspection. Call `PaneManager::remove` to drop manager-owned pane
state, including PTY/process and surface resources. Reset/restart callers that
do not need to inspect the killed pane can use `PaneManager::kill_and_remove`
to perform both steps.

## Feature flags

This crate exposes a small feature surface so you can opt into integrations
deliberately.

- `serde` enables `serde` support for public data types that need structured
  serialization.
- `crossterm` enables input conversion helpers for hosts that capture terminal
  events through `crossterm`.
