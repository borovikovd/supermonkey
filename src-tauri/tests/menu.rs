#![expect(
    clippy::expect_used,
    reason = "menu integration test fails via expect when standard menu setup breaks"
)]

use supermonkey_lib::shell::menu::{QUIT_ID, build};
use tauri::menu::{MenuItemKind, Submenu};

fn inspect_submenu<R: tauri::Runtime>(
    sub: &Submenu<R>,
    custom_quits: &mut usize,
    predefined_quits: &mut usize,
) {
    for item in sub.items().expect("read submenu items") {
        match item {
            MenuItemKind::MenuItem(m) => {
                if m.id() == QUIT_ID {
                    *custom_quits += 1;
                }
            }
            MenuItemKind::Predefined(pre) => {
                if let Ok(text) = pre.text() {
                    let lower = text.trim().to_ascii_lowercase();
                    if lower == "quit" || lower.starts_with("quit ") {
                        *predefined_quits += 1;
                    }
                }
            }
            MenuItemKind::Submenu(nested) => {
                inspect_submenu(&nested, custom_quits, predefined_quits);
            }
            MenuItemKind::Check(_) | MenuItemKind::Icon(_) => {}
        }
    }
}

fn main() {
    if std::env::args().any(|arg| arg == "--list") {
        return;
    }
    let app = tauri::test::mock_app();
    let menu = build(app.handle()).expect("build standard menu");
    let mut custom_quits = 0usize;
    let mut predefined_quits = 0usize;
    for item in menu.items().expect("read menu bar items") {
        if let MenuItemKind::Submenu(sub) = item {
            inspect_submenu(&sub, &mut custom_quits, &mut predefined_quits);
        }
    }
    assert_eq!(
        predefined_quits, 0,
        "predefined Quit item bypasses Session::request_quit"
    );
    assert_eq!(
        custom_quits, 1,
        "standard menu must contain exactly one app-owned Quit item"
    );
}
