use tauri::menu::{Menu, MenuItem, MenuItemKind, PredefinedMenuItem, Submenu};
use tauri::{AppHandle, Runtime};

pub const QUIT_ID: &str = "supermonkey.quit";

pub fn build<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<Menu<R>> {
    let menu = Menu::default(app)?;
    for item in menu.items()? {
        if let MenuItemKind::Submenu(sub) = item {
            replace_predefined_quit(app, &sub)?;
        }
    }
    Ok(menu)
}

fn replace_predefined_quit<R: Runtime>(
    app: &AppHandle<R>,
    submenu: &Submenu<R>,
) -> tauri::Result<()> {
    let items = submenu.items()?;
    for (idx, item) in items.into_iter().enumerate() {
        match item {
            MenuItemKind::Predefined(pre) if is_quit_item(&pre) => {
                let label = pre
                    .text()
                    .ok()
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| "Quit Supermonkey".to_owned());
                submenu.remove(&pre)?;
                let custom = MenuItem::with_id(app, QUIT_ID, label, true, Some("CmdOrCtrl+Q"))?;
                submenu.insert(&custom, idx)?;
            }
            MenuItemKind::Submenu(nested) => {
                replace_predefined_quit(app, &nested)?;
            }
            MenuItemKind::MenuItem(_)
            | MenuItemKind::Predefined(_)
            | MenuItemKind::Check(_)
            | MenuItemKind::Icon(_) => {}
        }
    }
    Ok(())
}

fn is_quit_item<R: Runtime>(item: &PredefinedMenuItem<R>) -> bool {
    item.text().is_ok_and(|text| {
        let lower = text.trim().to_ascii_lowercase();
        lower == "quit" || lower.starts_with("quit ")
    })
}
