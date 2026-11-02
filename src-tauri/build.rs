use std::path::PathBuf;

/// Reads the Desktop OAuth client from `google-oauth-client.json` (or the
/// path in `UBERMENSCH_OAUTH_CLIENT`) and bakes its ID and secret into the
/// binary. Google treats a desktop client secret as not confidential.
fn oauth_client() {
    println!("cargo:rerun-if-env-changed=UBERMENSCH_OAUTH_CLIENT");
    let path = std::env::var("UBERMENSCH_OAUTH_CLIENT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("../google-oauth-client.json"));
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
                    .to_string()
            };
            (field("client_id"), field("client_secret"))
        }
        // Tests and CI run against the fake Gmail server and need no real client.
        Err(_) if std::env::var("PROFILE").as_deref() != Ok("release") => {
            println!(
                "cargo:warning=No OAuth client at {}; real Google sign-in is disabled in this build",
                path.display()
            );
            (String::new(), String::new())
        }
        Err(e) => panic!(
            "Missing OAuth client {}: {e}. Put the Desktop client JSON from Google Cloud there, or set UBERMENSCH_OAUTH_CLIENT.",
            path.display()
        ),
    };
    println!("cargo:rustc-env=GOOGLE_CLIENT_ID={id}");
    println!("cargo:rustc-env=GOOGLE_CLIENT_SECRET={secret}");

    // Quota budget, units per minute per account (see SPEC Sync). Projects that
    // used the Gmail API before May 2026 keep a higher quota and can raise it.
    println!("cargo:rerun-if-env-changed=UBERMENSCH_QUOTA_PER_MIN");
    let quota = std::env::var("UBERMENSCH_QUOTA_PER_MIN").unwrap_or_else(|_| "5800".into());
    println!("cargo:rustc-env=UBERMENSCH_QUOTA_PER_MIN={quota}");
}

fn main() {
    oauth_client();
    tauri_build::build()
}
