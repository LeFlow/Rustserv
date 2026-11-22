//! Icône de la zone de notification et son menu.
//!
//! Les libellés sont ici en français en dur : le menu natif ne passe pas par
//! l'i18n de l'interface web. Ils seront centralisés quand on ajoutera une
//! deuxième langue.

use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle,
};

pub fn create(app: &AppHandle) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "show", "Ouvrir Rustserv", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Arrêter les services et quitter", true, None::<&str>)?;
    let sep = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(app, &[&show, &sep, &quit])?;

    TrayIconBuilder::with_id("main")
        .icon(app.default_window_icon().cloned().expect("icône manquante"))
        .tooltip("Rustserv")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => crate::show_main(app),
            "quit" => crate::quit(app.clone()),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            // Clic gauche sur l'icône : ouvre la fenêtre.
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                crate::show_main(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}
