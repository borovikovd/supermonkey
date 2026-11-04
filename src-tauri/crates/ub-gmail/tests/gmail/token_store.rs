//! The refresh-token file: only the user can read it, even over a stale
//! temporary file, and accounts keep each other's tokens.

use std::os::unix::fs::PermissionsExt as _;

use ub_gmail::{RefreshToken, TokenFile, TokenStore as _};

fn mode(path: &std::path::Path) -> u32 {
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

#[test]
fn the_token_file_is_owner_only_and_keeps_every_account() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tokens.json");
    // A temporary file left by a crash, readable by everyone.
    let stale = path.with_extension("json.tmp");
    std::fs::write(&stale, b"{}").unwrap();
    std::fs::set_permissions(&stale, std::fs::Permissions::from_mode(0o644)).unwrap();

    let store = TokenFile::new(path.clone());
    store
        .store("a@example.com", &RefreshToken::new("1//a"))
        .unwrap();
    assert_eq!(mode(&path), 0o600, "the stale file's mode carried over");
    assert!(!stale.exists());
    store
        .store("b@example.com", &RefreshToken::new("1//b"))
        .unwrap();
    assert_eq!(mode(&path), 0o600);

    let reopened = TokenFile::new(path.clone());
    assert_eq!(
        reopened.load("a@example.com").unwrap(),
        Some(RefreshToken::new("1//a"))
    );
    reopened.delete("a@example.com").unwrap();
    let left: Vec<String> = reopened
        .entries()
        .unwrap()
        .into_iter()
        .map(|(email, _)| email)
        .collect();
    assert_eq!(left, vec!["b@example.com".to_owned()]);
    assert_eq!(reopened.load("a@example.com").unwrap(), None);
}
