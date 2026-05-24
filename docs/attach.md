# Fullscreen Attach

Fullscreen attach temporarily hands the real terminal to a pane's child PTY.
It is the fidelity path for programs that should run as if they were launched
directly in the terminal.

## When To Use Attach

Use attach for:

- editors and pagers
- alternate-screen terminal UIs
- complex paste flows
- programs that change terminal modes
- cases where embedded rendering is useful for preview, but native interaction
  is required for real work

## Manager Attach

The umbrella crate exposes manager-owned attach through
`PaneManager::attach_blocking`.

The flow is:

1. Suspend the host terminal profile through `PaneAttachTerminalControl`.
2. Mark the pane as attached.
3. Bridge terminal input to the child PTY.
4. Bridge child output to the real terminal.
5. Detach on the configured chord.
6. Restore the host terminal profile.
7. Replay any needed output back into the embedded surface.

The pane keeps the same `PaneId`, event sequence, transcript, and surface
state. Hosts do not need to create a second process for fullscreen mode.

## Detach Chord

The default detach chord is `Ctrl+]`. Hosts can configure another chord through
`AttachOptions` and `DetachConfig`.

Partial chords use a timeout so ordinary input is not held forever when the
user starts but does not finish a detach sequence.

## Terminal Control

`PaneAttachTerminalControl` is the host terminal contract. A host should use it
to suspend drawing, raw mode, alternate screen, mouse mode, and any other
terminal state it owns.

The optional crossterm helpers are useful for simple hosts. More complex hosts
should provide their own implementation so attach restore exactly matches the
host's terminal profile.

## Output Policies

Attach can fan output to the real terminal, the embedded surface, or both,
depending on policy. The default manager path preserves enough state for the
embedded view to catch up after detach.

## Validation

Attach behavior has deterministic unit and integration coverage. Live terminal
smoke tests are ignored by default because they require a real terminal and
manual interaction. See [Testing](testing.md).
