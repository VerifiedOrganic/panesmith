# panesmith-attach

`panesmith-attach` provides the fullscreen attach and detach bridge for a live
Panesmith pane. It is the correctness path for interactive terminal programs
that need native terminal ownership.

## What this crate covers

Use this crate when your host TUI needs to suspend its own drawing, hand the
real terminal to a child PTY, and then restore the host cleanly on detach or
child exit.

## Feature flags

This crate keeps live-terminal integrations behind explicit feature flags.

- `crossterm` enables the `crossterm` host-terminal implementation and the
  Unix `StdioAttachTerminal` helper for real-terminal attach sessions.
