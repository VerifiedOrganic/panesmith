# Release Checklist

Use this checklist when preparing a public release. It keeps package metadata,
docs, and publish order aligned across the workspace crates.

## Publishable Crates

- `panesmith-core`
- `panesmith-vt100`
- `panesmith-ratatui`
- `panesmith-attach`
- `panesmith`

Examples, fixtures, and `xtask` are workspace packages only.

## Preflight

Do not publish until versioning, changelog entries, package dry runs, and docs
metadata have been reviewed for the release.

1. Update `CHANGELOG.md`.
2. Confirm `workspace.package.version` is the intended fresh release number.
3. Confirm internal workspace dependency versions match.
4. Run `cargo fmt --all`.
5. Run `cargo check --workspace --all-features`.
6. Run `cargo clippy --workspace --all-features --all-targets -- -D warnings`.
7. Run `cargo test --workspace`.
8. Run ignored attach checks when attach behavior changed.

## Package Dry Run

```bash
cargo publish --dry-run --workspace --allow-dirty
```

Inspect individual package contents when needed:

```bash
cargo package --list -p panesmith
```

Each publishable crate should include:

- `README.md`
- `LICENSE`
- `LICENSE-APACHE`
- `LICENSE-MIT`
- accurate feature documentation
- docs.rs metadata
- repository metadata

## Publish Order

Publish dependency crates first:

1. `panesmith-core`
2. `panesmith-vt100`
3. `panesmith-ratatui`
4. `panesmith-attach`
5. `panesmith`

Wait for crates.io to observe each crate version before publishing the next
crate.

## After Publish

1. Open each crates.io page.
2. Open each docs.rs page.
3. Confirm README rendering, feature notes, license metadata, and repository
   links.
4. Tag the release and push the tag.
