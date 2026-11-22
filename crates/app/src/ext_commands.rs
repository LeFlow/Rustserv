//! Commandes de la phase 3 : extensions PHP, PECL et profils.

use crate::commands::EVT_INSTALL;
use crate::state::{job_key, AppState};
use rustserv_core::extensions_ops::ProfileResult;
use rustserv_core::php::extensions::{description, ExtensionInfo};
use rustserv_core::php::pecl::{self, PeclRecord};
use rustserv_core::php::profiles::{self, Profile, ProfilePlan};
use rustserv_core::{Error, Result};
use serde::Serialize;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, State};
use tauri_plugin_opener::OpenerExt;

#[tauri::command]
pub async fn php_extensions(state: State<'_, AppState>, version: String) -> Result<Vec<ExtensionInfo>> {
    state.orchestrator.extensions(&state.installer, &state.settings(), &version).await
}

#[tauri::command]
pub async fn php_extension_set(state: State<'_, AppState>, version: String, name: String, enabled: bool) -> Result<()> {
    state.orchestrator.set_extension(&state.installer, &state.settings(), &version, &name, enabled).await.map(|_| ())
}

#[derive(Serialize)]
pub struct PeclEntry {
    name: &'static str,
    description: Option<&'static str>,
}

/// Extensions PECL proposées.
#[tauri::command]
pub fn php_pecl_list() -> Vec<PeclEntry> {
    pecl::PECL.iter().map(|n| PeclEntry { name: n, description: description(n) }).collect()
}

/// Opération longue enregistrée (annulable) ; retirée automatiquement à la fin.
struct Job<'a> {
    state: &'a AppState,
    cle: String,
    cancel: Arc<AtomicBool>,
}

impl<'a> Job<'a> {
    fn start(state: &'a AppState, cle: String) -> Result<Self> {
        let cancel = Arc::new(AtomicBool::new(false));
        let mut jobs = state.jobs.lock().map_err(|e| Error::Interne(e.to_string()))?;
        if jobs.contains_key(&cle) {
            return Err(Error::Interne(format!("opération déjà en cours : {cle}")));
        }
        jobs.insert(cle.clone(), cancel.clone());
        Ok(Self { state, cle, cancel })
    }
}

impl Drop for Job<'_> {
    fn drop(&mut self) {
        if let Ok(mut jobs) = self.state.jobs.lock() {
            jobs.remove(&self.cle);
        }
    }
}

#[tauri::command]
pub async fn php_pecl_install(app: AppHandle, state: State<'_, AppState>, version: String, name: String) -> Result<PeclRecord> {
    let settings = state.settings();
    let job = Job::start(&state, job_key(&format!("pecl-{name}"), &version))?;
    let progression = |p| {
        let _ = app.emit(EVT_INSTALL, &p);
    };
    state.orchestrator.install_pecl(&state.installer, &settings, &version, &name, &job.cancel, &progression).await
}

#[tauri::command]
pub fn php_profiles() -> Vec<Profile> {
    profiles::all()
}

#[tauri::command]
pub async fn php_profile_plan(state: State<'_, AppState>, version: String, profile: String) -> Result<ProfilePlan> {
    state.orchestrator.profile_plan(&state.installer, &state.settings(), &version, &profile).await
}

#[tauri::command]
pub async fn php_profile_apply(app: AppHandle, state: State<'_, AppState>, version: String, profile: String) -> Result<ProfileResult> {
    let settings = state.settings();
    let job = Job::start(&state, job_key(&format!("profil-{profile}"), &version))?;
    let progression = |p| {
        let _ = app.emit(EVT_INSTALL, &p);
    };
    state.orchestrator.apply_profile(&state.installer, &settings, &version, &profile, &job.cancel, &progression).await
}

/// Ouvre un fichier ou dossier **du dossier Rustserv** avec l'application Windows associée
/// (ex. php.ini dans l'éditeur de texte).
#[tauri::command]
pub fn open_path(app: AppHandle, state: State<'_, AppState>, path: String) -> Result<()> {
    let p = std::path::PathBuf::from(&path);
    if !p.starts_with(state.paths.root()) {
        return Err(Error::Interne(format!("chemin hors du dossier Rustserv : {path}")));
    }
    app.opener().open_path(path, None::<&str>).map_err(|e| Error::Interne(e.to_string()))
}
