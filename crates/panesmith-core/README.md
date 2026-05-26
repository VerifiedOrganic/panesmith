# panesmith-core

`panesmith-core` provides the PTY pane runtime, event model, scrollback and
transcript retention, repro dump types, and surface abstractions that the rest
of the workspace builds on.

## What this crate covers

Use this crate when you need pane lifecycle control, snapshots, transcripts,
and event streams without pulling in a specific rendering layer.

## Pane lifecycle cleanup

`PaneManager::kill` terminates the child and leaves the pane in the manager for
post-kill inspection. Call `PaneManager::remove` to drop manager-owned pane
state, including PTY/process and surface resources. Reset/restart callers that
do not need to inspect the killed pane can use `PaneManager::kill_and_remove`
to perform both steps.

## Child environment policy

`PaneConfig` inherits the parent process environment by default for backward
compatibility. Daemon embedders can use `PaneConfig::with_clear_env` or
`PaneConfig::with_env_allowlist` to prevent parent daemon variables from
reaching child panes, then add explicit variables with `with_env`. Explicit
variables override inherited values, and `with_term_fallback` can provide a
`TERM` value when the selected policy does not.

This policy only controls environment variables. It is not a process sandbox
and does not alter filesystem, user, descriptor, network, or PTY access.

## Feature flags

This crate exposes a small feature surface so you can opt into integrations
deliberately.

- `serde` enables `serde` support for public data types that need structured
  serialization.
- `crossterm` enables input conversion helpers for hosts that capture terminal
  events through `crossterm`.
