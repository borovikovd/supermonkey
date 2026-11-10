//! The build script: bakes the Google OAuth client and the Gmail quota budget
//! into the binary, then runs Tauri's build step.

use std::ffi::OsStr;
use std::path::PathBuf;

/// Reads the Desktop OAuth client from `google-oauth-client.json` (or the
/// path in `SUPERMONKEY_OAUTH_CLIENT`) and bakes its ID and secret into the
/// binary. Google treats a desktop client secret as not confidential.
#[expect(
    clippy::panic,
    reason = "a build script fails by panicking, with the message cargo shows"
)]
fn oauth_client() {
    println!("cargo:rerun-if-env-changed=SUPERMONKEY_OAUTH_CLIENT");
    let path = std::env::var_os("SUPERMONKEY_OAUTH_CLIENT").map_or_else(
        || PathBuf::from("../google-oauth-client.json"),
        PathBuf::from,
    );
    println!("cargo:rerun-if-changed={}", path.display());

    let (id, secret) = match std::fs::read_to_string(&path) {
        Ok(text) => {
            let json: serde_json::Value = serde_json::from_str(&text)
                .unwrap_or_else(|e| panic!("{} is not valid JSON: {e}", path.display()));
            let client = json.get("installed").unwrap_or_else(|| {
                panic!(
                    "{} has no \"installed\" client. Download the JSON of a Desktop OAuth client.",
                    path.display()
                )
            });
            let field = |k: &str| {
                client
                    .get(k)
                    .and_then(|v| v.as_str())
                    .unwrap_or_else(|| panic!("{} has no {k}", path.display()))
                    .to_owned()
            };
            (field("client_id"), field("client_secret"))
        }
        // Tests and CI run against the fake Gmail server and need no real client.
        Err(_) if std::env::var_os("PROFILE").as_deref() != Some(OsStr::new("release")) => {
            println!(
                "cargo:warning=No OAuth client at {}; real Google sign-in is disabled in this build",
                path.display()
            );
            (String::new(), String::new())
        }
        Err(e) => panic!(
            "Missing OAuth client {}: {e}. Put the Desktop client JSON from Google Cloud there, or set SUPERMONKEY_OAUTH_CLIENT.",
            path.display()
        ),
    };
    println!("cargo:rustc-env=GOOGLE_CLIENT_ID={id}");
    println!("cargo:rustc-env=GOOGLE_CLIENT_SECRET={secret}");

    // Quota budget, units per minute per account (see SPEC Sync). Projects that
    // used the Gmail API before May 2026 keep a higher quota and can raise it.
    println!("cargo:rerun-if-env-changed=SUPERMONKEY_QUOTA_PER_MIN");
    let quota = match std::env::var_os("SUPERMONKEY_QUOTA_PER_MIN") {
        None => "5800".to_owned(),
        Some(value) => value.into_string().unwrap_or_else(|value| {
            panic!(
                "SUPERMONKEY_QUOTA_PER_MIN is not a number: {}",
                value.display()
            )
        }),
    };
    println!("cargo:rustc-env=SUPERMONKEY_QUOTA_PER_MIN={quota}");
}

fn main() {
    oauth_client();
    tauri_build::build();
}
