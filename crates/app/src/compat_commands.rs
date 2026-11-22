//! Commandes de la phase 5 : tests multi-versions et analyses de compatibilité.

use crate::state::AppState;
use rustserv_core::compat::analysis::Rapport;
use rustserv_core::compat::ops;
use rustserv_core::compat::runner::{RunResult, TestKind};
use rustserv_core::compat::tools::{self, ToolsStatus};
use rustserv_core::{Error, Result};
use tauri::State;

#[tauri::command]
pub fn compat_tools_status(state: State<'_, AppState>) -> ToolsStatus {
    tools::status(&state.paths)
}

/// Installe ou met à jour PHP_CodeSniffer + PHPCompatibility et PHPStan (Composer).
#[tauri::command]
pub async fn compat_tools_install(state: State<'_, AppState>) -> Result<ToolsStatus> {
    let catalog = state.catalog.read().map_err(|e| Error::Interne(e.to_string()))?.clone();
    ops::install_tools(&state.paths, &state.settings(), &state.installer, &catalog).await
}

/// Lance PHPUnit, Pest ou un script sur plusieurs versions de PHP en parallèle.
#[tauri::command]
pub async fn compat_run(
    state: State<'_, AppState>,
    site: String,
    kind: TestKind,
    script: Option<String>,
    versions: Vec<String>,
) -> Result<Vec<RunResult>> {
    ops::run_tests(&state.paths, &state.settings(), &state.installer, &site, kind, script, &versions).await
}

#[tauri::command]
pub async fn compat_phpcs(state: State<'_, AppState>, site: String, min: String, max: Option<String>) -> Result<Rapport> {
    ops::phpcompatibility(&state.paths, &state.settings(), &state.installer, &site, &min, max.as_deref()).await
}

#[tauri::command]
pub async fn compat_phpstan(state: State<'_, AppState>, site: String, niveau: u8, cible: String) -> Result<Rapport> {
    ops::phpstan(&state.paths, &state.settings(), &state.installer, &site, niveau, &cible).await
}
