//! Commandes de la phase 4 : domaines (.test), HTTPS et terminal intégré.

use crate::state::AppState;
use rustserv_core::hosts::{self, HostsStatus};
use rustserv_core::ssl::{self, SslStatus};
use rustserv_core::{platform, Error, Result};
use serde::Serialize;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, State};

pub const EVT_TERMINAL_OUTPUT: &str = "terminal-output";
pub const EVT_TERMINAL_EXIT: &str = "terminal-exit";

fn interne(e: impl std::fmt::Display) -> Error {
    Error::Interne(e.to_string())
}

/// Regénère les configurations et redémarre ce qui tourne (nouveau site, HTTPS…).
async fn appliquer(state: &AppState) -> Result<()> {
    let installed = state.installer.installed()?;
    state.orchestrator.apply(&state.settings(), &installed).await
}

// ---------------------------------------------------------------- domaines

#[tauri::command]
pub fn hosts_status(state: State<'_, AppState>) -> Result<HostsStatus> {
    hosts::status(&state.paths, &state.settings())
}

/// Met à jour le fichier hosts (fenêtre UAC de Windows).
#[tauri::command]
pub async fn hosts_sync(state: State<'_, AppState>) -> Result<HostsStatus> {
    let (paths, settings) = (state.paths.clone(), state.settings());
    let helper = hosts::helper_path()?;
    tauri::async_runtime::spawn_blocking(move || hosts::sync(&paths, &settings, &helper)).await.map_err(interne)??;
    hosts::status(&state.paths, &state.settings())
}

/// Prend en compte les sites ajoutés/supprimés dans www/ (configs + redémarrage du serveur web).
#[tauri::command]
pub async fn sites_refresh(state: State<'_, AppState>) -> Result<()> {
    appliquer(&state).await
}

// ---------------------------------------------------------------- HTTPS

#[tauri::command]
pub fn ssl_status(state: State<'_, AppState>) -> Result<SslStatus> {
    ssl::status(&state.paths)
}

/// Crée l'autorité locale (si besoin), la fait approuver par Windows (fenêtre de
/// confirmation) puis active le HTTPS sur les serveurs en marche.
#[tauri::command]
pub async fn ssl_enable(state: State<'_, AppState>) -> Result<SslStatus> {
    let paths = state.paths.clone();
    let approuvee = tauri::async_runtime::spawn_blocking(move || -> Result<bool> {
        let ca = ssl::ensure_ca(&paths)?;
        if platform::root_cert_installed(&ca.der) {
            return Ok(true);
        }
        platform::install_root_cert(&ca.der).map_err(|e| Error::Ssl { detail: e.to_string() })
    })
    .await
    .map_err(interne)??;
    if !approuvee {
        return Err(Error::AdminRefuse { operation: "approbation du certificat Rustserv".into() });
    }
    appliquer(&state).await?;
    ssl::status(&state.paths)
}

/// Retire l'autorité de Windows, supprime les certificats et désactive le HTTPS.
#[tauri::command]
pub async fn ssl_disable(state: State<'_, AppState>) -> Result<SslStatus> {
    let paths = state.paths.clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<()> {
        if let Some(ca) = ssl::load_ca(&paths)? {
            platform::remove_root_cert(&ca.der).map_err(|e| Error::Ssl { detail: e.to_string() })?;
        }
        ssl::delete_all(&paths)
    })
    .await
    .map_err(interne)??;
    appliquer(&state).await?;
    ssl::status(&state.paths)
}

// ---------------------------------------------------------------- terminal

#[derive(Serialize, Clone)]
struct TerminalOutput {
    id: u32,
    data: String,
}

#[derive(Serialize)]
pub struct TerminalInfo {
    id: u32,
    cwd: String,
    php: Option<String>,
}

#[tauri::command]
pub fn terminal_open(app: AppHandle, state: State<'_, AppState>, site: Option<String>, cols: u16, rows: u16) -> Result<TerminalInfo> {
    let settings = state.settings();
    let installed = state.installer.installed()?;
    let stack = rustserv_core::services::Stack::new(&state.paths, &settings, &installed);
    let env = stack.terminal_env(site.as_deref())?;
    // php.ini de la version utilisée (créé au besoin) pour que PHPRC soit valide.
    if let Some(php) = site.as_deref().and_then(|s| stack.site_php(s)).or_else(|| stack.default_php()) {
        rustserv_core::php::ensure_php_ini(&state.paths, &php.path, &php.version, &settings)?;
    }
    let (a1, a2) = (app.clone(), app.clone());
    let id = state.terminals.open(
        &env,
        cols.max(20),
        rows.max(5),
        Arc::new(move |id, data| {
            let _ = a1.emit(EVT_TERMINAL_OUTPUT, TerminalOutput { id, data });
        }),
        Arc::new(move |id| {
            let _ = a2.emit(EVT_TERMINAL_EXIT, id);
        }),
    )?;
    Ok(TerminalInfo { id, cwd: env.cwd.display().to_string(), php: env.php })
}

#[tauri::command]
pub fn terminal_write(state: State<'_, AppState>, id: u32, data: String) -> Result<()> {
    state.terminals.write(id, &data)
}

#[tauri::command]
pub fn terminal_resize(state: State<'_, AppState>, id: u32, cols: u16, rows: u16) -> Result<()> {
    state.terminals.resize(id, cols.max(20), rows.max(5))
}

#[tauri::command]
pub fn terminal_close(state: State<'_, AppState>, id: u32) {
    state.terminals.close(id);
}
