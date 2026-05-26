# Changelog

This changelog tracks release-facing changes that matter for published
Panesmith crates.

## Unreleased

No unreleased changes.

## 0.2.1 - 2026-05-26

- added explicit child process environment policies for daemon embedders,
  including inherited, cleared, and allowlisted environments with explicit
  variable overrides and `TERM` fallback support in the default portable-pty
  backend

## 0.2.0 - 2026-05-24

- added bounded terminal scrollback retention with manager defaults, per-pane
  overrides, backing-buffer trimming, and dropped-history counters in snapshots

## 0.1.1 - 2026-05-24

This initial public alpha release includes:

- added crate readmes, docs.rs metadata, repository metadata, keywords, and
  categories for the publishable workspace crates
- added dual-license files and a dry-run release checklist for the first alpha
- documented the current publish order for the interdependent crates
- bumped the workspace release version to `0.1.1` so publish dry runs verify
  the current alpha crate set instead of an older published `0.1.0`
- replaced internal planning/spec notes with public documentation for
  architecture, getting started, embedding, attach, events, testing, and release
- moved manager regression tests into `manager/tests.rs` so runtime code and
  test coverage are no longer kept in one large file
- moved large inline unit-test modules into sibling `tests.rs` modules and
  cleaned up clippy warnings across the workspace
- fixed the manager-owned `StdoutOnlyThenReplay` attach path so output is
  buffered while attached and replayed into the embedded surface on detach
