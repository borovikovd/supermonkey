//! The menu bar (SPEC Window): Tauri's standard menus, except Quit.
//!
//! Tauri's predefined Quit sends macOS's `terminate:`, which ends the app
//! at once: the shell sees only `RunEvent::Exit`, too late to ask the page
//! to close its composer and commit (SPEC UI state, Actions). The app
//! menu's Quit is the app's own item instead, and the shell runs its quit
//! on it (`request_quit`).

use tauri::menu::{
    AboutMetadata, HELP_SUBMENU_ID, Menu, MenuBuilder, MenuItemBuilder, SubmenuBuilder,
    WINDOW_SUBMENU_ID,
};
use tauri::{AppHandle, Runtime};

/// The id of the app menu's Quit item.
pub const QUIT_MENU_ITEM: &str = "quit";

/// The menu bar: `Menu::default`'s menus and items, with the app's own Quit
/// in place of the predefined one (`tests/menu.rs` holds it to that).
pub fn app_menu<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<Menu<R>> {
    let package = app.package_info();
    let name = &package.name;
    let config = app.config();
    let about = AboutMetadata {
        name: Some(name.clone()),
        version: Some(package.version.to_string()),
        copyright: config.bundle.copyright.clone(),
        authors: config.bundle.publisher.clone().map(|p| vec![p]),
        ..AboutMetadata::default()
    };
    let quit = MenuItemBuilder::with_id(QUIT_MENU_ITEM, format!("Quit {name}"))
        .accelerator("CmdOrCtrl+Q")
        .build(app)?;
    let app_submenu = SubmenuBuilder::new(app, name)
        .about(Some(about))
        .separator()
        .services()
        .separator()
        .hide()
        .hide_others()
        .separator()
        .item(&quit)
        .build()?;
    let file = SubmenuBuilder::new(app, "File").close_window().build()?;
    let edit = SubmenuBuilder::new(app, "Edit")
        .undo()
        .redo()
        .separator()
        .cut()
        .copy()
        .paste()
        .select_all()
        .build()?;
    let view = SubmenuBuilder::new(app, "View").fullscreen().build()?;
    let window = SubmenuBuilder::with_id(app, WINDOW_SUBMENU_ID, "Window")
        .minimize()
        .maximize()
        .separator()
        .close_window()
        .build()?;
    let help = SubmenuBuilder::with_id(app, HELP_SUBMENU_ID, "Help").build()?;
    MenuBuilder::new(app)
        .items(&[&app_submenu, &file, &edit, &view, &window, &help])
        .build()
}
