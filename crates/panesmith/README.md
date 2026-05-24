# panesmith

`panesmith` is the umbrella crate for the Panesmith workspace. It re-exports
the core pane runtime, the fullscreen attach bridge, the `vt100` surface
backend, and the ratatui widget layer behind one import path.

## What this crate covers

Use this crate when you want the shortest path from a host TUI to a live
PTY-backed pane. It keeps the first-release API discoverable while the
workspace stays split into focused implementation crates.

## Examples

The crate ships small reference examples under its package-local `examples/`
directory. The repository also keeps larger interactive demos in its top-level
`examples/` directory.

- `embedded_shell_minimal` draws one embedded shell frame in under 50 lines.
- `manager_attach` hands an existing `PaneManager` pane to fullscreen attach.
- `dashboard_two_panes` shows a simple two-pane dashboard layout.
- `transcript_capture` records plain-text and raw-byte transcripts.
- `event_consumption` drains ordered pane events from the manager.

## Feature flags

This crate re-exports a small set of feature flags so you can stay on the
umbrella crate when you need common optional integrations.

- `crossterm` enables the re-exported `CrosstermTerminalControl` attach
  helper for simple hosts, the Unix-only `StdioAttachTerminal` helper, the
  `RawModeOps` trait for custom raw-mode implementations, the
  `SystemRawModeOps` default implementation, and the `crossterm` input helpers
  from `panesmith-core`. Embedded TUIs that already own a precise terminal
  profile should provide their own `PaneAttachTerminalControl`.
- `serde` enables `serde` support for the re-exported public data types from
  `panesmith-core`.
