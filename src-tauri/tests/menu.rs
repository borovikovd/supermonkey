//! The menu bar the app runs with (SPEC Window): Tauri's standard menus,
//! except Quit. Tauri's predefined Quit sends macOS's `terminate:`, which
//! ends the app without asking the page to close its composer and commit
//! (SPEC UI state, Actions), so the app menu has the app's own Quit in its
//! place, on `Cmd+Q`.
//!
//! No test harness: macOS makes menus only on the main thread, and only
//! `main` runs there.

use supermonkey_lib::{QUIT_MENU_ITEM, app_menu};
use tauri::menu::{Menu, MenuItemKind};
use tauri::test::{MockRuntime, mock_app};

/// Each menu as `title >`, then each of its items as `title > text`, with
/// `#id` for an item of the app's own rather than a predefined one.
fn items(within: &str, kinds: &[MenuItemKind<MockRuntime>], out: &mut Vec<String>) {
    for kind in kinds {
        match kind {
            MenuItemKind::Submenu(sub) => {
                let title = sub.text().unwrap_or_default();
                let path = if within.is_empty() {
                    title
                } else {
                    format!("{within} > {title}")
                };
                out.push(format!("{path} >"));
                items(&path, &sub.items().unwrap_or_default(), out);
            }
            MenuItemKind::Predefined(item) => {
                out.push(format!("{within} > {}", item.text().unwrap_or_default()));
            }
            MenuItemKind::MenuItem(item) => out.push(format!(
                "{within} > {} #{}",
                item.text().unwrap_or_default(),
                item.id().0
            )),
            MenuItemKind::Check(item) => out.push(format!("{within} > {} #check", item.id().0)),
            MenuItemKind::Icon(item) => out.push(format!("{within} > {} #icon", item.id().0)),
        }
    }
}

fn listed(menu: &Menu<MockRuntime>) -> Vec<String> {
    let mut out = Vec::new();
    items("", &menu.items().unwrap_or_default(), &mut out);
    out
}

fn main() {
    let app = mock_app();
    let name = &app.handle().package_info().name;
    let standard = Menu::default(app.handle()).map(|m| listed(&m));
    let ours = app_menu(app.handle()).map(|m| listed(&m));
    let (Ok(standard), Ok(ours)) = (standard, ours) else {
        panic!("couldn't build the menus");
    };

    // Tauri's menus, with its predefined Quit swapped for the app's own.
    let predefined_quit = format!("{name} > Quit ");
    assert!(
        standard.iter().any(|i| i.starts_with(&predefined_quit)),
        "Tauri's standard app menu has a predefined Quit: {standard:#?}",
    );
    let expected: Vec<String> = standard
        .iter()
        .map(|item| {
            if item.starts_with(&predefined_quit) {
                format!("{name} > Quit {name} #{QUIT_MENU_ITEM}")
            } else {
                item.clone()
            }
        })
        .collect();
    assert_eq!(ours, expected, "the app's menu bar");
}
