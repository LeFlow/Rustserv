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

/// Active `pgsql` et `pdo_pgsql` dans le PHP d'un site (ou le PHP par défaut, pour Adminer)
/// si elles ne le sont pas déjà. Un échec est seulement journalisé : il ne doit pas
/// empêcher l'action principale (changement de base, ouverture d'Adminer).
pub async fn activer_pgsql(
    state: &AppState,
    settings: &rustserv_core::Settings,
    installed: &[rustserv_core::catalog::InstalledPackage],
    site: Option<&str>,
) {
    use rustserv_core::php::extensions::ExtState;
    let st = rustserv_core::services::Stack::new(&state.paths, settings, installed);
    let php = match site {
        Some(s) => st.site_php(s),
        None => st.default_php(),
    };
    let Some(version) = php.map(|p| p.version.clone()) else { return };
    let etat = match state.orchestrator.extensions(&state.installer, settings, &version).await {
        Ok(v) => v,
        Err(e) => return tracing::warn!(erreur = %e, "extensions PHP illisibles"),
    };
    for ext in ["pgsql", "pdo_pgsql"] {
        if etat.iter().any(|x| x.name == ext && matches!(x.state, ExtState::Inactive)) {
            match state.orchestrator.set_extension(&state.installer, settings, &version, ext, true).await {
                Ok(_) => tracing::info!(php = %version, extension = ext, "extension PostgreSQL activée"),
                Err(e) => tracing::warn!(php = %version, extension = ext, erreur = %e, "activation impossible"),
            }
        }
    }
}

#[derive(Serialize)]
pub struct PlanPhp {
    /// Versions complètes actuelle et visée (« 8.5.13 »).
    de: Option<String>,
    vers: String,
    /// Extensions actives avec la version actuelle, présentes mais inactives dans la nouvelle.
    a_activer: Vec<String>,
    /// Extensions actives avec la version actuelle, absentes de la nouvelle ; `pecl` = installable.
    absentes: Vec<AbsentePhp>,
}

#[derive(Serialize)]
pub struct AbsentePhp {
    nom: String,
    pecl: bool,
}

/// Prépare le changement de PHP d'un site (`branche` None = PHP par défaut) : compare les
/// extensions actives de la version actuelle avec celles de la version visée.
#[tauri::command]
pub async fn site_php_plan(state: State<'_, AppState>, site: String, branche: Option<String>) -> Result<PlanPhp> {
    use rustserv_core::php::extensions::ExtState;
    let settings = state.settings();
    let installed = state.installer.installed()?;
    let (de, vers) = {
        let st = rustserv_core::services::Stack::new(&state.paths, &settings, &installed);
        let de = st.site_php(&site).map(|p| p.version.clone());
        let vers = match &branche {
            Some(b) => st.resolve_php(b),
            None => st.default_php(),
        }
        .map(|p| p.version.clone())
        .ok_or_else(|| Error::AucuneVersion { id: format!("php {}", branche.clone().unwrap_or_default()) })?;
        (de, vers)
    };
    let mut plan = PlanPhp { de: de.clone(), vers: vers.clone(), a_activer: vec![], absentes: vec![] };
    let Some(de) = de.filter(|d| *d != vers) else { return Ok(plan) };
    let actuelles = state.orchestrator.extensions(&state.installer, &settings, &de).await?;
    let cibles = state.orchestrator.extensions(&state.installer, &settings, &vers).await?;
    for ext in actuelles.iter().filter(|e| matches!(e.state, ExtState::Active)) {
        match cibles.iter().find(|c| c.name == ext.name).map(|c| &c.state) {
            Some(ExtState::Active | ExtState::Integree) => {}
            Some(_) => plan.a_activer.push(ext.name.clone()),
            None => plan.absentes.push(AbsentePhp { pecl: pecl::PECL.contains(&ext.name.as_str()), nom: ext.name.clone() }),
        }
    }
    Ok(plan)
}
