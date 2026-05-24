use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

const CHANGELOG: &str = include_str!("../../../CHANGELOG.md");
const LICENSE: &str = include_str!("../../../LICENSE");
const LICENSE_APACHE: &str = include_str!("../../../LICENSE-APACHE");
const LICENSE_MIT: &str = include_str!("../../../LICENSE-MIT");
const RELEASE_CHECKLIST: &str = include_str!("../../../docs/release.md");
const PANESMITH_LIB_RS: &str = include_str!("../src/lib.rs");
const PANESMITH_README: &str = include_str!("../README.md");
const ATTACH_README: &str = include_str!("../../panesmith-attach/README.md");
const CORE_README: &str = include_str!("../../panesmith-core/README.md");
const RATATUI_README: &str = include_str!("../../panesmith-ratatui/README.md");
const VT100_README: &str = include_str!("../../panesmith-vt100/README.md");

#[test]
fn publishable_crates_have_release_metadata() {
    let packages = workspace_packages();

    for (name, expected) in publishable_expectations() {
        let package = packages
            .get(name)
            .unwrap_or_else(|| panic!("missing {name}"));

        assert_eq!(
            package.license.as_deref(),
            Some("MIT OR Apache-2.0"),
            "{name} should declare the dual-license SPDX expression"
        );
        assert_eq!(
            package.repository.as_deref(),
            Some("https://github.com/VerifiedOrganic/panesmith"),
            "{name} should expose the canonical repository URL"
        );
        assert_eq!(
            package.documentation.as_deref(),
            Some(expected.documentation),
            "{name} should expose the matching docs.rs URL"
        );
        assert_eq!(
            package.readme.as_deref(),
            Some("README.md"),
            "{name} should point to its crate-local README"
        );
        assert_eq!(
            package
                .categories
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            expected.categories,
            "{name} should keep the expected category slugs"
        );
        assert_eq!(
            package
                .keywords
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            expected.keywords,
            "{name} should keep the expected keyword set"
        );
        assert_eq!(
            package.publish.first().map(String::as_str),
            Some("crates-io"),
            "{name} should be dry-runnable through cargo publish"
        );

        assert_eq!(
            package.docs_rs_all_features,
            Some(true),
            "{name} should build docs.rs with all features"
        );
        assert_eq!(
            package.docs_rs_default_target.as_deref(),
            Some("x86_64-unknown-linux-gnu"),
            "{name} should pin the default docs.rs target"
        );
        assert_eq!(
            package.docs_rs_targets_len,
            Some(0),
            "{name} should keep docs.rs on the default target only"
        );
    }
}

#[test]
fn packaged_crates_include_license_files_and_skip_workspace_only_tests() {
    for (name, _) in publishable_expectations() {
        let package_files = package_file_list(name);

        for required in ["LICENSE", "LICENSE-APACHE", "LICENSE-MIT", "README.md"] {
            assert!(
                package_files.iter().any(|path| path == required),
                "{name} package should include {required}"
            );
        }

        if name == "panesmith" {
            for excluded in ["tests/public_docs.rs", "tests/release_metadata.rs"] {
                assert!(
                    package_files.iter().all(|path| path != excluded),
                    "panesmith package should exclude {excluded}"
                );
            }
        }
    }
}

#[test]
fn crate_license_files_match_workspace_roots() {
    for (name, expected) in publishable_expectations() {
        let crate_root = workspace_root().join(expected.manifest_dir);

        for license_name in ["LICENSE", "LICENSE-APACHE", "LICENSE-MIT"] {
            let workspace_text = fs::read_to_string(workspace_root().join(license_name))
                .unwrap_or_else(|error| panic!("failed to read workspace {license_name}: {error}"));
            let crate_text = fs::read_to_string(crate_root.join(license_name))
                .unwrap_or_else(|error| panic!("failed to read {name} {license_name}: {error}"));

            assert_eq!(
                crate_text, workspace_text,
                "{name} should ship the workspace {license_name} text unchanged"
            );
        }
    }
}

#[test]
fn release_artifacts_document_the_publish_gate() {
    assert!(CHANGELOG.contains("## Unreleased"));
    assert!(CHANGELOG.contains("crate readmes"));
    assert!(LICENSE.contains("Apache License, Version 2.0"));
    assert!(LICENSE.contains("MIT license"));
    assert!(LICENSE_APACHE.contains("Apache License"));
    assert!(LICENSE_MIT.contains("MIT License"));

    for needle in [
        "Do not publish until versioning, changelog entries, package dry runs, and docs",
        "cargo publish --dry-run --workspace --allow-dirty",
        "workspace.package.version",
        "fresh release number",
        "cargo check --workspace --all-features",
        "cargo clippy --workspace --all-features --all-targets -- -D warnings",
        "cargo test --workspace",
        "panesmith-core",
        "panesmith-vt100",
        "panesmith-ratatui",
        "panesmith-attach",
        "panesmith",
    ] {
        assert!(
            RELEASE_CHECKLIST.contains(needle),
            "release checklist should mention {needle}"
        );
    }
}

#[test]
fn crate_readmes_cover_feature_docs() {
    let packages = workspace_packages();

    assert!(PANESMITH_README.contains("## Feature flags"));
    assert!(PANESMITH_README.contains("`crossterm`"));
    assert!(PANESMITH_README.contains("`serde`"));
    assert!(PANESMITH_README.contains("`CrosstermTerminalControl`"));
    assert!(PANESMITH_README.contains("`StdioAttachTerminal`"));
    assert!(PANESMITH_README.contains("`RawModeOps`"));
    assert!(PANESMITH_README.contains("`SystemRawModeOps`"));
    assert!(!PANESMITH_README.contains("panesmith-attach/"));
    assert!(!PANESMITH_README.contains("panesmith-core/"));
    assert!(PANESMITH_LIB_RS.contains("pub use panesmith_attach::StdioAttachTerminal;"));
    assert!(PANESMITH_LIB_RS.contains("CrosstermTerminalControl"));

    assert!(
        packages["panesmith"]
            .feature_names
            .contains(&"crossterm".to_owned()),
        "panesmith should expose a crossterm feature when the README documents it"
    );
    assert!(
        packages["panesmith"]
            .feature_names
            .contains(&"serde".to_owned()),
        "panesmith should expose a serde feature when the README documents it"
    );

    assert!(CORE_README.contains("## Feature flags"));
    assert!(CORE_README.contains("`serde`"));
    assert!(CORE_README.contains("`crossterm`"));
    assert!(
        packages["panesmith-core"]
            .feature_names
            .contains(&"serde".to_owned()),
        "panesmith-core should expose a serde feature when the README documents it"
    );
    assert!(
        packages["panesmith-core"]
            .feature_names
            .contains(&"crossterm".to_owned()),
        "panesmith-core should expose a crossterm feature when the README documents it"
    );

    assert!(ATTACH_README.contains("## Feature flags"));
    assert!(ATTACH_README.contains("`crossterm`"));
    assert!(
        packages["panesmith-attach"]
            .feature_names
            .contains(&"crossterm".to_owned()),
        "panesmith-attach should expose a crossterm feature when the README documents it"
    );

    for readme in [RATATUI_README, VT100_README] {
        assert!(readme.contains("## Feature flags"));
        assert!(readme.contains("does not define crate-specific feature flags"));
    }
}

fn workspace_packages() -> HashMap<String, PackageRecord> {
    let output = Command::new(env!("CARGO"))
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .current_dir(workspace_root())
        .output()
        .expect("cargo metadata should run");

    assert!(
        output.status.success(),
        "cargo metadata failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let json: Value = serde_json::from_slice(&output.stdout).expect("metadata should be JSON");
    json["packages"]
        .as_array()
        .expect("packages should be an array")
        .iter()
        .map(package_record)
        .collect()
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("workspace root should resolve")
}

fn publishable_expectations() -> [(&'static str, PublishExpectation); 5] {
    [
        (
            "panesmith",
            PublishExpectation {
                documentation: "https://docs.rs/panesmith",
                manifest_dir: "crates/panesmith",
                categories: &["command-line-interface", "development-tools"],
                keywords: &["pty", "terminal", "tui", "ratatui", "attach"],
            },
        ),
        (
            "panesmith-attach",
            PublishExpectation {
                documentation: "https://docs.rs/panesmith-attach",
                manifest_dir: "crates/panesmith-attach",
                categories: &["command-line-interface", "development-tools"],
                keywords: &["pty", "terminal", "attach", "fullscreen", "tui"],
            },
        ),
        (
            "panesmith-core",
            PublishExpectation {
                documentation: "https://docs.rs/panesmith-core",
                manifest_dir: "crates/panesmith-core",
                categories: &["command-line-interface", "development-tools"],
                keywords: &["pty", "terminal", "transcript", "replay", "tui"],
            },
        ),
        (
            "panesmith-ratatui",
            PublishExpectation {
                documentation: "https://docs.rs/panesmith-ratatui",
                manifest_dir: "crates/panesmith-ratatui",
                categories: &["command-line-interface", "gui"],
                keywords: &["terminal", "tui", "ratatui", "widget", "pty"],
            },
        ),
        (
            "panesmith-vt100",
            PublishExpectation {
                documentation: "https://docs.rs/panesmith-vt100",
                manifest_dir: "crates/panesmith-vt100",
                categories: &["command-line-interface", "rendering"],
                keywords: &["terminal", "vt100", "ansi", "pty", "surface"],
            },
        ),
    ]
}

fn package_record(package: &Value) -> (String, PackageRecord) {
    let docs_rs = &package["metadata"]["docs"]["rs"];
    let name = package["name"]
        .as_str()
        .expect("package name should be a string")
        .to_owned();

    (
        name,
        PackageRecord {
            license: package["license"].as_str().map(str::to_owned),
            repository: package["repository"].as_str().map(str::to_owned),
            documentation: package["documentation"].as_str().map(str::to_owned),
            readme: package["readme"].as_str().map(str::to_owned),
            categories: package["categories"]
                .as_array()
                .map(|items| {
                    items
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default(),
            keywords: package["keywords"]
                .as_array()
                .map(|items| {
                    items
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default(),
            publish: package["publish"]
                .as_array()
                .map(|items| {
                    items
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default(),
            docs_rs_all_features: docs_rs["all-features"].as_bool(),
            docs_rs_default_target: docs_rs["default-target"].as_str().map(str::to_owned),
            docs_rs_targets_len: docs_rs["targets"].as_array().map(Vec::len),
            feature_names: package["features"]
                .as_object()
                .map(|features| {
                    let mut names = features.keys().cloned().collect::<Vec<_>>();
                    names.sort();
                    names
                })
                .unwrap_or_default(),
        },
    )
}

fn package_file_list(crate_name: &str) -> Vec<String> {
    let output = Command::new(env!("CARGO"))
        .args(["package", "--list", "--allow-dirty", "-p", crate_name])
        .current_dir(workspace_root())
        .output()
        .unwrap_or_else(|error| panic!("cargo package --list failed for {crate_name}: {error}"));

    assert!(
        output.status.success(),
        "cargo package --list failed for {crate_name}: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    String::from_utf8(output.stdout)
        .expect("package listing should be utf-8")
        .lines()
        .map(str::to_owned)
        .collect()
}

struct PackageRecord {
    license: Option<String>,
    repository: Option<String>,
    documentation: Option<String>,
    readme: Option<String>,
    categories: Vec<String>,
    keywords: Vec<String>,
    publish: Vec<String>,
    docs_rs_all_features: Option<bool>,
    docs_rs_default_target: Option<String>,
    docs_rs_targets_len: Option<usize>,
    feature_names: Vec<String>,
}

#[derive(Clone, Copy)]
struct PublishExpectation {
    documentation: &'static str,
    manifest_dir: &'static str,
    categories: &'static [&'static str],
    keywords: &'static [&'static str],
}
