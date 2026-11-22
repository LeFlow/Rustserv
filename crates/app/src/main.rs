//! Point d'entrée de l'application Tauri.
//!
//! Cette couche reste volontairement fine : elle crée la fenêtre et l'icône
//! de notification, puis expose les fonctions du noyau (`rustserv-core`) à
//! l'interface sous forme de « commandes » Tauri.

// Pas de console noire derrière la fenêtre en version finale.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod compat_commands;
mod ext_commands;
mod php_commands;
mod site_commands;
mod state;
mod tray;

use rustserv_core::Paths;
use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};

fn main() {
    let paths = Paths::detect().expect("impossible de déterminer le dossier de Rustserv");
    paths.ensure_layout().expect("impossible de créer l'arborescence de Rustserv");
    let _log_guard = init_logs(&paths);
    tracing::info!(racine = %paths.root().display(), "démarrage de Rustserv");

    // TODO : afficher une boîte de dialogue native si l'état ne peut pas être créé
    // (ex. catalog.json corrompu) au lieu d'un simple message en console.
    let state = state::AppState::new(paths.clone()).expect("impossible d'initialiser Rustserv");

    tauri::Builder::default()
        .manage(state)
        .invoke_handler(tauri::generate_handler![
            commands::app_info,
            commands::catalog_list,
            commands::catalog_install,
            commands::catalog_cancel,
            commands::catalog_uninstall,
            commands::installed_list,
            commands::services_overview,
            commands::service_start,
            commands::service_stop,
            commands::service_restart,
            commands::services_start_all,
            commands::services_stop_all,
            commands::service_log,
            commands::settings_get,
            commands::settings_save,
            commands::services_restart_pending,
            commands::services_restart_all,
            php_commands::php_overview,
            php_commands::php_set_default,
            php_commands::php_pin,
            php_commands::php_check_updates,
            php_commands::php_upgrade,
            php_commands::sites_list,
            php_commands::site_set_php,
            ext_commands::php_extensions,
            ext_commands::php_extension_set,
            ext_commands::php_pecl_list,
            ext_commands::php_pecl_install,
            ext_commands::php_profiles,
            ext_commands::php_profile_plan,
            ext_commands::php_profile_apply,
            ext_commands::open_path,
            site_commands::hosts_status,
            site_commands::hosts_sync,
            site_commands::sites_refresh,
            site_commands::ssl_status,
            site_commands::ssl_enable,
            site_commands::ssl_disable,
            site_commands::terminal_open,
            site_commands::terminal_write,
            site_commands::terminal_resize,
            site_commands::terminal_close,
            compat_commands::compat_tools_status,
            compat_commands::compat_tools_install,
            compat_commands::compat_run,
            compat_commands::compat_phpcs,
            compat_commands::compat_phpstan,
        ])
        .plugin(tauri_plugin_opener::init())
        .setup(move |app| {
            // Fenêtre créée en code pour stocker le profil WebView2 dans le
            // dossier portable (sinon il irait dans %LOCALAPPDATA%).
            WebviewWindowBuilder::new(app, "main", WebviewUrl::default())
                .title("Rustserv")
                .inner_size(1100.0, 720.0)
                .min_inner_size(820.0, 540.0)
                .data_directory(paths.tmp().join("webview"))
                .build()?;
            tray::create(app.handle())?;
            forward_service_events(app.handle());
            Ok(())
        })
        .on_window_event(|window, event| {
            // Fermer la fenêtre la masque : l'app continue dans la zone de notification.
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .run(tauri::generate_context!())
        .expect("erreur au lancement de Rustserv");
}

/// Journaux : fichier `logs/rustserv.log` (rotation quotidienne) + console en dev.
fn init_logs(paths: &Paths) -> tracing_appender::non_blocking::WorkerGuard {
    use tracing_subscriber::{fmt, prelude::*, EnvFilter};
    let file = tracing_appender::rolling::daily(paths.logs(), "rustserv.log");
    let (writer, guard) = tracing_appender::non_blocking(file);
    let filter = EnvFilter::try_from_env("RUSTSERV_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer().with_writer(writer).with_ansi(false))
        .with(fmt::layer().with_writer(std::io::stderr))
        .init();
    guard
}

/// Relaie les changements d'état des services vers l'interface (`service-status`).
fn forward_service_events(app: &tauri::AppHandle) {
    use tauri::Emitter;
    let mut rx = app.state::<state::AppState>().supervisor().subscribe();
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(ev) => {
                    let _ = app.emit(commands::EVT_SERVICE, &ev);
                }
                // Événements perdus (UI trop lente) : on continue, l'UI peut relire l'état complet.
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => break,
            }
        }
    });
}

/// Arrête proprement tous les services (important pour MariaDB) puis quitte.
pub(crate) fn quit(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        let sup = app.state::<state::AppState>().supervisor().clone();
        tracing::info!("arrêt de tous les services avant de quitter");
        app.state::<state::AppState>().terminals.close_all();
        sup.stop_all().await;
        app.exit(0);
    });
}

/// Ré-affiche et met au premier plan la fenêtre principale.
pub(crate) fn show_main(app: &tauri::AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
}
