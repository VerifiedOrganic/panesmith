# Panesmith

Panesmith is a Rust toolkit for hosting real command-line programs inside
terminal user interfaces.

It gives a host application two paths:

- embedded panes for dashboards, previews, routine input, and observability
- fullscreen attach when the child program needs native terminal ownership

## Why This Exists

Embedding a terminal program inside another terminal UI sounds simple until the
edges matter. A correct host has to coordinate a PTY, terminal emulation,
rendering, input encoding, resize handling, scrollback, lifecycle events, and
terminal restoration after fullscreen handoff.

Most projects end up wiring those pieces together locally. Panesmith exists so
that boundary can be shared, tested, and reused without forcing every host to
become a terminal multiplexer.

The central rule is practical:

- embedded mode is for preview and routine interaction
- attach mode is the fidelity path for complex terminal programs

## Status

Panesmith is pre-1.0. The core runtime, vt100-backed surface, ratatui widget,
event stream, transcript capture, repro dumps, and Unix fullscreen attach path
are implemented. The public API is usable, but some higher-level host features
such as selection, search, and built-in scrollback browsing are intentionally
left to applications for now.

## Quick Start

```rust
use std::io;

use panesmith::{PaneConfig, PaneManager, PaneManagerConfig, Size, TerminalPaneWidget};
use ratatui::{backend::CrosstermBackend, Terminal};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut manager = PaneManager::new(PaneManagerConfig::default());
    let pane_id = manager.spawn(
        PaneConfig::shell()
            .with_title("Shell")
            .with_size(Size::new(24, 80)),
    )?;

    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    let snapshot = manager.snapshot(pane_id)?;
    terminal.draw(|frame| {
        frame.render_widget(TerminalPaneWidget::new(&snapshot), frame.area());
    })?;
    Ok(())
}
```

This renders one embedded shell frame and exits. For live loops and fuller
examples, see [Getting Started](docs/getting-started.md).

## Crates

The workspace is split by boundary rather than by feature pile.

```text
crates/panesmith-core       pane runtime, ports, events, transcripts, repros
crates/panesmith-vt100      vt100-backed surface adapter
crates/panesmith-ratatui    ratatui widget adapter
crates/panesmith-attach     fullscreen attach/detach support
crates/panesmith            umbrella crate with common re-exports
```

Most applications should start with the umbrella crate:

```rust
use panesmith::{PaneConfig, PaneManager, TerminalPaneWidget};
```

Use the focused crates directly when you want tighter dependency control.

## Documentation

- [Architecture](docs/architecture.md)
- [Getting Started](docs/getting-started.md)
- [Embedded Panes](docs/embedding.md)
- [Fullscreen Attach](docs/attach.md)
- [Events and Transcripts](docs/events-and-transcripts.md)
- [Testing](docs/testing.md)
- [Release Checklist](docs/release.md)

## Examples

Small reference examples live in `crates/panesmith/examples`:

- `embedded_shell_minimal` renders one shell pane in under 50 code lines
- `dashboard_two_panes` renders two independent panes side by side
- `manager_attach` attaches a manager-owned pane fullscreen
- `transcript_capture` records raw and plain-text transcript buffers
- `event_consumption` drains ordered pane events

Run one with:

```bash
cargo run -p panesmith --example embedded_shell_minimal
```

Interactive demos live in the workspace `examples` directory:

```bash
cargo run -p embedded-shell
cargo run -p attach-shell
```

## Feature Flags

The umbrella crate exposes the common optional integrations:

- `crossterm` enables crossterm input conversion and attach helpers
- `serde` enables serialization support for public data types from
  `panesmith-core`

Focused crates expose the same features only where they apply.

## Design Principles

- Keep the runtime behind explicit ports: PTY, surface, input, attach terminal,
  and event delivery.
- Keep adapters replaceable. The default stack uses `portable-pty`, `vt100`,
  `ratatui`, and optional `crossterm`, but those choices are not baked into the
  core data model.
- Keep terminal ownership explicit. The host owns embedded rendering; the child
  owns the real terminal only during attach.
- Keep concurrency boring. `PaneManager` is movable across threads, but callers
  should use external synchronization when sharing it.
- Keep files and modules scoped to a single job. Tests live beside the modules
  they validate instead of being embedded in large runtime files.

## Local Validation

```bash
cargo fmt --all
cargo check --workspace --all-features
cargo test --workspace
```

Attach-heavy work should also run the ignored checks described in
[Testing](docs/testing.md).

## Limitations

- Embedded panes are not a promise of perfect behavior for every fullscreen
  terminal application.
- Fullscreen attach currently targets Unix terminals.
- The live attach helper temporarily takes exclusive ownership of process
  stdin while attached.
- Built-in scrollback search, selection, and copy workflows are not included.
- The ignored live attach smoke test requires a real terminal.

## License

Panesmith is dual-licensed under MIT or Apache-2.0. See `LICENSE`,
`LICENSE-APACHE`, and `LICENSE-MIT`.
