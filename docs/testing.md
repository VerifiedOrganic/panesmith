# Testing

Run the normal workspace gate before opening a change:

```bash
cargo fmt --all
cargo check --workspace --all-features
cargo test --workspace
cargo clippy --workspace --all-features --all-targets -- -D warnings
```

The workspace also provides an `xtask` gate:

```bash
cargo xtask gate
```

## Focused Checks

Use focused test commands while iterating:

```bash
cargo test -p panesmith-core
cargo test -p panesmith-attach
cargo test -p panesmith-vt100
cargo test -p terminal-edge-cases
```

## Attach Checks

Attach has deterministic coverage in the default workspace tests. These tests
exercise in-memory terminal control, real PTY behavior, detach cleanup, resize
restore, and child-exit handling.

Ignored checks cover slower or manual paths:

```bash
cargo test --workspace -- --ignored
```

The live terminal smoke test must be run in a real terminal:

```bash
cargo test -p panesmith-attach --features crossterm \
  --test live_crossterm_attach \
  manual_live_shell_attach_restores_terminal \
  -- --ignored --nocapture
```

## Fixtures

`fixtures/terminal-edge-cases` contains small terminal programs used to
exercise wrapping, alternate screen, bracketed paste, cursor position reports,
slow output, frame storms, resize reporting, and exit codes.

`fixtures/echo-tui` provides a simple interactive process for PTY and attach
tests.

## Documentation Checks

The umbrella crate includes tests that keep README links, public docs,
package metadata, and examples aligned with the published API.
