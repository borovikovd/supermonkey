#![expect(
    clippy::expect_used,
    reason = "integration tests fail via expect on broken invariants"
)]

use std::fs;
use std::path::{Path, PathBuf};

fn collect_rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).expect("read directory").flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rs_files(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
            out.push(path);
        }
    }
}

fn prod_source(src: &str) -> &str {
    src.split("#[cfg(test)]").next().unwrap_or(src)
}

fn mentions_crate(source: &str, name: &str) -> bool {
    let patterns = [
        format!("use {name}::"),
        format!("use {name};"),
        format!("{name}::"),
    ];
    source.lines().any(|line| {
        let trimmed = line.trim();
        patterns.iter().any(|p| trimmed.contains(p))
    })
}

fn mentions_module(source: &str, module: &str) -> bool {
    let pattern = format!("crate::{module}");
    source.lines().any(|line| line.contains(&pattern))
}

#[test]
fn workspace_has_only_app_and_fake_gmail() {
    let manifest = fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml"))
        .expect("read Cargo.toml");
    assert!(
        manifest.contains("\"crates/sm-fake-gmail\""),
        "workspace must include sm-fake-gmail"
    );
    for old in [
        "sm-domain",
        "sm-mime",
        "sm-gmail",
        "sm-store",
        "sm-ipc",
        "sm-core",
        "sm-bridge",
        "sm-testkit",
    ] {
        assert!(
            !manifest.contains(old),
            "old crate {old} must not exist in Cargo.toml"
        );
    }
}

#[test]
#[expect(
    clippy::too_many_lines,
    clippy::cognitive_complexity,
    reason = "single table-driven pass enforces external crate and module layering across src-tauri/src"
)]
fn module_and_external_crate_boundaries_hold() {
    let src_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    collect_rs_files(&src_root, &mut files);
    assert!(!files.is_empty(), "src/ must contain Rust files");

    for path in files {
        let rel = path
            .strip_prefix(&src_root)
            .expect("path inside src")
            .to_string_lossy()
            .replace('\\', "/");
        let full = fs::read_to_string(&path).expect("read source file");
        let prod = prod_source(&full);

        let top = rel.split('/').next().unwrap_or("");

        if mentions_crate(prod, "rusqlite") {
            assert_eq!(
                top, "store",
                "rusqlite may only be used in store/ (found in {rel})"
            );
        }
        if mentions_crate(prod, "reqwest") {
            assert_eq!(
                top, "gmail",
                "reqwest may only be used in gmail/ (found in {rel})"
            );
        }
        if mentions_crate(prod, "tauri") {
            assert_eq!(
                top, "shell",
                "tauri may only be used in shell/ (found in {rel})"
            );
        }
        if mentions_crate(prod, "axum") {
            assert!(
                rel == "session/web.rs"
                    || top == "testkit"
                    || rel == "bin/e2e_server.rs"
                    || rel == "shell/mod.rs",
                "axum may only be used in session/web.rs, shell/mod.rs dev bridge, testkit/, or e2e_server (found in {rel})"
            );
        }

        match top {
            "domain" => {
                for forbidden in [
                    "mime", "gmail", "store", "ipc", "core", "session", "shell", "testkit",
                ] {
                    assert!(
                        !mentions_module(prod, forbidden),
                        "domain/ must not depend on {forbidden} (found in {rel})"
                    );
                }
            }
            "mime" => {
                for forbidden in [
                    "gmail", "store", "ipc", "core", "session", "shell", "testkit",
                ] {
                    assert!(
                        !mentions_module(prod, forbidden),
                        "mime/ must not depend on {forbidden} (found in {rel})"
                    );
                }
            }
            "gmail" => {
                for forbidden in ["store", "ipc", "core", "session", "shell", "testkit"] {
                    assert!(
                        !mentions_module(prod, forbidden),
                        "gmail/ must not depend on {forbidden} (found in {rel})"
                    );
                }
            }
            "store" => {
                for forbidden in [
                    "mime", "gmail", "ipc", "core", "session", "shell", "testkit",
                ] {
                    assert!(
                        !mentions_module(prod, forbidden),
                        "store/ must not depend on {forbidden} (found in {rel})"
                    );
                }
            }
            "ipc" => {
                for forbidden in [
                    "mime", "gmail", "store", "core", "session", "shell", "testkit",
                ] {
                    assert!(
                        !mentions_module(prod, forbidden),
                        "ipc/ must not depend on {forbidden} (found in {rel})"
                    );
                }
            }
            "core" => {
                for forbidden in ["session", "shell", "testkit"] {
                    assert!(
                        !mentions_module(prod, forbidden),
                        "core/ must not depend on {forbidden} (found in {rel})"
                    );
                }
            }
            "session" => {
                for forbidden in ["mime", "gmail", "store", "shell", "testkit"] {
                    assert!(
                        !mentions_module(prod, forbidden),
                        "session/ must not depend on {forbidden} (found in {rel})"
                    );
                }
            }
            _ => {}
        }
    }
}
