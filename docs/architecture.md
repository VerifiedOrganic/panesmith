# Architecture

Panesmith is organized around a small runtime core with replaceable adapters.
The shape is hexagonal where it matters: process control, terminal surfaces,
input encoding, attach handoff, rendering, and event delivery cross explicit
ports instead of being hardwired into one host.

## Runtime Boundary

The core runtime owns pane lifecycle and state:

```text
PaneConfig
  -> PaneManager
  -> PtyProcess
  -> SurfaceBackend
  -> PaneSnapshot
  -> host renderer
```

Input follows the reverse path:

```text
HostInput
  -> InputEncoder
  -> PTY writer
  -> child process
```

The host application decides where snapshots are rendered, how events are
stored, and when a pane should be attached fullscreen.

## Ports

`panesmith-core` defines the stable boundaries.

- `PtyBackend` and `PtyProcess` start, resize, write to, and terminate child
  processes.
- `SurfaceBackend` consumes PTY output and produces snapshots, scrollback, and
  terminal mode metadata.
- `InputEncoder` converts structured host input into terminal bytes.
- `PaneAttachTerminal` and `PaneAttachTerminalControl` describe fullscreen
  attach I/O and host terminal suspension.
- `PaneEvent` carries ordered lifecycle, input, output, resize, surface, and
  attach activity.

These ports are intentionally small. They keep the manager from knowing about a
specific terminal emulator, renderer, or host event loop.

## Adapters

The workspace ships practical default adapters:

- `panesmith-core` uses `portable-pty` by default for PTY processes.
- `panesmith-vt100` and the default core surface use `vt100` for terminal
  emulation.
- `panesmith-ratatui` renders snapshots as ratatui widgets.
- `panesmith-attach` provides attach/detach support and optional crossterm
  terminal control helpers.

Applications can replace the PTY backend, surface backend, attach terminal, and
event delivery policy without changing the public pane model.

## Crate Layout

```text
panesmith-core       domain model, manager, ports, events, transcripts, repros
panesmith-vt100      public vt100 surface adapter
panesmith-ratatui    ratatui rendering adapter
panesmith-attach     fullscreen attach bridge
panesmith            umbrella crate
```

The umbrella crate is for convenience. The implementation remains split so
downstream projects can depend on only the boundaries they need.

## Manager Ownership

`PaneManager` is the runtime coordinator. It owns pane state, event queues,
transcripts, and backend handles. It is `Send`, so a host can move it behind an
executor or wrap it in `Arc<Mutex<_>>`. It is not `Sync`; concurrent access
should be explicit and externally synchronized.

This is deliberate. The manager mutates process state, terminal surfaces, and
event queues together. A single mutable owner keeps those transitions easier to
reason about.

## Attach Boundary

Attach is not a second runtime. It is a temporary ownership transfer for a
manager-owned pane:

1. The host suspends its terminal profile.
2. The manager marks the pane as attached.
3. Input and output bridge between the real terminal and the child PTY.
4. Detach restores the host terminal profile.
5. The same pane returns to embedded snapshots and events.

The attach path preserves pane identity and event ordering. Hosts do not need
to spawn a duplicate process to get native terminal behavior.

## File Boundaries

Source files are kept by responsibility:

- `pane.rs` contains public pane data, snapshots, styles, and configuration.
- `pty.rs` contains PTY process boundaries and the portable backend.
- `event.rs` contains event payloads and ordering metadata.
- `input.rs` and `encoder.rs` contain structured input and terminal encoding.
- `transcript.rs` contains retained output buffers.
- `repro.rs` contains replay dumps.
- `manager.rs` contains runtime orchestration.
- `manager/attach_support.rs` contains attach-specific terminal helpers.
- `manager/tests.rs` contains manager-specific regression coverage.

The manager coordinates several ports, but attach helpers and regression tests
are kept out of the runtime file so each file has a clear reason to exist.
Other crate modules follow the same pattern: production code stays in the
module file and larger unit tests live in sibling `tests.rs` modules.
