# Getting Started

Use the umbrella crate unless you have a reason to depend on focused crates
directly.

```toml
[dependencies]
panesmith = "0.1"
ratatui = "0.30"
```

Enable optional integrations when you need them:

```toml
panesmith = { version = "0.1", features = ["crossterm", "serde"] }
```

## Minimal Embedded Pane

The smallest host spawns a pane, snapshots it, and renders that snapshot.

```bash
cargo run -p panesmith --example embedded_shell_minimal
```

That example is intentionally tiny. It is useful when you want to see the API
surface without reading a full event loop.

## Live Embedded Demo

Run the interactive embedded shell demo:

```bash
cargo run -p embedded-shell
```

It demonstrates:

- a live PTY-backed shell
- ratatui rendering
- keyboard, paste, mouse, and resize forwarding
- cursor placement inside the host layout
- clean process shutdown

## Fullscreen Attach Demo

Run the attach demo:

```bash
cargo run -p attach-shell
```

Controls:

- `Ctrl+F` attaches the shell fullscreen
- `Ctrl+]` detaches back to the dashboard
- `Ctrl+Q` quits the demo

Use attach when the child program needs native terminal behavior, such as an
editor, alternate-screen UI, rich paste handling, or terminal-mode changes that
should not be approximated inside a widget.

## Choosing Crates

- Use `panesmith` for application code and examples.
- Use `panesmith-core` when you need runtime control without rendering.
- Use `panesmith-ratatui` when you already have a manager snapshot and want a
  ratatui widget.
- Use `panesmith-attach` when you need fullscreen attach primitives directly.
- Use `panesmith-vt100` when you want the public vt100 surface adapter.

## Next Reading

- [Embedded Panes](embedding.md)
- [Fullscreen Attach](attach.md)
- [Events and Transcripts](events-and-transcripts.md)
- [Architecture](architecture.md)
