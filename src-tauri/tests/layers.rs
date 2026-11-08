//! The crate layering of ARCHITECTURE.md, section 3, as a test: every
//! dependency edge between workspace crates must be one the design allows,
//! and the infrastructure crates may only be used where the design puts them.

use std::collections::{BTreeMap, BTreeSet};
use std::process::Command;

/// Each workspace crate and the workspace crates it may depend on.
const ALLOWED: &[(&str, &[&str])] = &[
    ("sm-domain", &[]),
    ("sm-mime", &["sm-domain"]),
    ("sm-gmail", &["sm-domain"]),
    ("sm-store", &["sm-domain"]),
    ("sm-ipc", &["sm-domain"]),
    (
        "sm-core",
        &["sm-domain", "sm-mime", "sm-gmail", "sm-store", "sm-ipc"],
    ),
    ("sm-bridge", &["sm-core", "sm-ipc", "sm-domain"]),
    (
        "sm-testkit",
        &[
            "sm-core",
            "sm-bridge",
            "sm-ipc",
            "sm-domain",
            "sm-gmail",
            "sm-store",
            "sm-fake-gmail",
        ],
    ),
    ("sm-fake-gmail", &[]),
    (
        "supermonkey",
        &[
            "sm-core",
            "sm-bridge",
            "sm-ipc",
            "sm-domain",
            "sm-gmail",
            "sm-testkit",
        ],
    ),
];

/// Third-party crates that only some workspace crates may use directly.
const CONFINED: &[(&str, &[&str])] = &[
    ("rusqlite", &["sm-store", "sm-domain"]),
    ("reqwest", &["sm-gmail", "sm-testkit", "sm-fake-gmail"]),
    ("tauri", &["supermonkey"]),
    ("axum", &["sm-bridge", "sm-testkit", "sm-fake-gmail"]),
];

/// Every workspace crate's direct dependencies by name, each with whether it is
/// a dev-dependency only.
#[expect(
    clippy::expect_used,
    reason = "a test helper: unreadable metadata should fail the test loudly"
)]
fn workspace_deps() -> BTreeMap<String, BTreeSet<(String, bool)>> {
    let out = Command::new(env!("CARGO"))
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("cargo metadata runs");
    assert!(out.status.success(), "cargo metadata failed");
    let meta: serde_json::Value = serde_json::from_slice(&out.stdout).expect("metadata is JSON");
    meta["packages"]
        .as_array()
        .expect("packages")
        .iter()
        .map(|p| {
            let name = p["name"].as_str().expect("name").to_owned();
            let deps = p["dependencies"]
                .as_array()
                .expect("dependencies")
                .iter()
                .map(|d| {
                    let name = d["name"].as_str().expect("dependency name").to_owned();
                    (name, d["kind"].as_str() == Some("dev"))
                })
                .collect();
            (name, deps)
        })
        .collect()
}

#[test]
fn workspace_crates_depend_only_on_lower_layers() {
    let deps = workspace_deps();
    let members: BTreeSet<&str> = ALLOWED.iter().map(|(c, _)| *c).collect();
    for (krate, crate_deps) in &deps {
        let allowed = ALLOWED
            .iter()
            .find(|(c, _)| c == krate)
            .unwrap_or_else(|| panic!("{krate} is not in ARCHITECTURE.md's crate table"))
            .1;
        for (dep, dev_only) in crate_deps
            .iter()
            .filter(|(d, _)| members.contains(d.as_str()))
        {
            // Tests anywhere may run against the fake Google.
            let fake_in_tests = *dev_only && dep == "sm-fake-gmail";
            assert!(
                fake_in_tests || allowed.contains(&dep.as_str()),
                "{krate} may not depend on {dep}"
            );
        }
    }
}

#[test]
fn infrastructure_crates_stay_where_the_design_puts_them() {
    for (krate, crate_deps) in workspace_deps() {
        for (infra, users) in CONFINED {
            if crate_deps
                .iter()
                .any(|(d, dev_only)| d == infra && !dev_only)
            {
                assert!(
                    users.contains(&krate.as_str()),
                    "{krate} may not use {infra}"
                );
            }
        }
    }
}
