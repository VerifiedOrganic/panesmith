const README: &str = include_str!("../../../README.md");
const ARCHITECTURE: &str = include_str!("../../../docs/architecture.md");
const GETTING_STARTED: &str = include_str!("../../../docs/getting-started.md");
const EMBEDDING: &str = include_str!("../../../docs/embedding.md");
const ATTACH: &str = include_str!("../../../docs/attach.md");
const EVENTS: &str = include_str!("../../../docs/events-and-transcripts.md");
const TESTING: &str = include_str!("../../../docs/testing.md");
const RELEASE: &str = include_str!("../../../docs/release.md");
const LIB_RS: &str = include_str!("../src/lib.rs");
const EMBEDDED_MINIMAL: &str = include_str!("../examples/embedded_shell_minimal.rs");
const DASHBOARD: &str = include_str!("../examples/dashboard_two_panes.rs");
const TRANSCRIPT: &str = include_str!("../examples/transcript_capture.rs");
const EVENTS_EXAMPLE: &str = include_str!("../examples/event_consumption.rs");

#[test]
fn readme_explains_purpose_without_internal_history() {
    for needle in [
        "## Why This Exists",
        "Embedding a terminal program inside another terminal UI",
        "Most projects end up wiring those pieces together locally.",
        "embedded mode is for preview and routine interaction",
        "attach mode is the fidelity path",
        "## Documentation",
        "docs/architecture.md",
        "docs/getting-started.md",
        "docs/embedding.md",
        "docs/attach.md",
        "docs/events-and-transcripts.md",
        "docs/testing.md",
    ] {
        assert!(README.contains(needle), "README should mention {needle}");
    }
}

#[test]
fn architecture_doc_records_hexagonal_boundaries() {
    for needle in [
        "hexagonal where it matters",
        "## Ports",
        "`PtyBackend`",
        "`SurfaceBackend`",
        "`InputEncoder`",
        "`PaneAttachTerminal`",
        "## Adapters",
        "`PaneManager` is the runtime coordinator",
        "It is `Send`",
        "It is not `Sync`",
        "`manager/attach_support.rs`",
        "`manager/tests.rs`",
    ] {
        assert!(
            ARCHITECTURE.contains(needle),
            "architecture doc should mention {needle}"
        );
    }
}

#[test]
fn user_docs_cover_core_workflows() {
    for needle in [
        "cargo run -p panesmith --example embedded_shell_minimal",
        "cargo run -p embedded-shell",
        "cargo run -p attach-shell",
        "Choosing Crates",
    ] {
        assert!(
            GETTING_STARTED.contains(needle),
            "getting-started doc should mention {needle}"
        );
    }

    for needle in [
        "Embedded panes are the preview path.",
        "`PaneManager::spawn`",
        "`TerminalPaneWidget`",
        "`PaneManager::resize`",
        "`PaneManager::scrollback`",
    ] {
        assert!(
            EMBEDDING.contains(needle),
            "embedding doc should mention {needle}"
        );
    }

    for needle in [
        "`PaneManager::attach_blocking`",
        "`Ctrl+]`",
        "`PaneAttachTerminalControl`",
        "The pane keeps the same `PaneId`",
    ] {
        assert!(
            ATTACH.contains(needle),
            "attach doc should mention {needle}"
        );
    }
}

#[test]
fn observability_testing_and_release_docs_cover_public_contracts() {
    for needle in [
        "`PaneEvent` values are pane-scoped",
        "`PaneManager::drain_events`",
        "`PaneManager::subscribe`",
        "`PaneManager::set_event_sink`",
        "`TranscriptConfig`",
        "`PaneManager::dump_repro`",
    ] {
        assert!(
            EVENTS.contains(needle),
            "events doc should mention {needle}"
        );
    }

    for needle in [
        "cargo fmt --all",
        "cargo check --workspace --all-features",
        "cargo clippy --workspace --all-features --all-targets -- -D warnings",
        "cargo test --workspace",
        "live terminal smoke test",
        "`fixtures/terminal-edge-cases`",
    ] {
        assert!(
            TESTING.contains(needle),
            "testing doc should mention {needle}"
        );
    }

    for needle in [
        "Release Checklist",
        "Do not publish until versioning",
        "cargo publish --dry-run --workspace --allow-dirty",
        "`workspace.package.version`",
        "`panesmith-core`",
        "`panesmith-vt100`",
        "`panesmith-ratatui`",
        "`panesmith-attach`",
        "`panesmith`",
    ] {
        assert!(
            RELEASE.contains(needle),
            "release doc should mention {needle}"
        );
    }
}

#[test]
fn rustdoc_mentions_release_examples_and_limits() {
    for needle in [
        "Embedded mode is for preview and routine input.",
        "Attach mode is the correctness path for complex interactive TUIs.",
        "embedded_shell_minimal",
        "dashboard_two_panes",
        "transcript_capture",
        "event_consumption",
        "Limitations",
    ] {
        assert!(
            LIB_RS.contains(needle),
            "crate docs should mention {needle}"
        );
    }
}

#[test]
fn reference_examples_exist_for_release_topics() {
    for (name, source) in [
        ("embedded_shell_minimal", EMBEDDED_MINIMAL),
        ("dashboard_two_panes", DASHBOARD),
        ("transcript_capture", TRANSCRIPT),
        ("event_consumption", EVENTS_EXAMPLE),
    ] {
        assert!(
            source.contains("fn main"),
            "{name} should stay as a runnable example"
        );
    }
}

#[test]
fn minimal_embedded_shell_stays_under_fifty_code_lines() {
    let code_lines = EMBEDDED_MINIMAL
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with("//"))
        .count();

    assert!(
        code_lines < 50,
        "embedded_shell_minimal should stay copy-pasteable (got {code_lines} code lines)"
    );
}
