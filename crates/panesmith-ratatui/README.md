# panesmith-ratatui

`panesmith-ratatui` renders Panesmith pane snapshots into ratatui widgets so
dashboard hosts can preview PTY-backed panes inside normal layouts.

## What this crate covers

Use this crate when your application already uses ratatui and you want a
focused rendering layer that stays separate from PTY management and attach
control.

`TerminalViewport` also owns the canonical scrollback viewport math used by
`TerminalPaneWidget`. Hosts can compute bounds and update scroll state without
duplicating Panesmith's combined scrollback-plus-live-row model:

```rust
use panesmith_ratatui::{TerminalPaneWidget, TerminalViewport};

# fn render_pane(
#     snapshot: &panesmith_core::PaneSnapshot<'_>,
#     scrollback: Option<&panesmith_core::ScrollbackSnapshot<'_>>,
#     height: usize,
#     mut viewport: TerminalViewport,
# ) {
let metrics = viewport.metrics(snapshot, scrollback, height);
viewport = viewport.scroll_up(3, metrics);

let widget = match scrollback {
    Some(scrollback) => TerminalPaneWidget::new(snapshot)
        .with_scrollback(scrollback)
        .with_viewport(viewport),
    None => TerminalPaneWidget::new(snapshot).with_viewport(viewport),
};
# let _ = widget;
# }
```

## Feature flags

This crate does not define crate-specific feature flags today.
