//! Commandes appelables depuis l'interface (`invoke("nom", {...})`).
//!
//! Convention : chaque commande renvoie `Result<T, rustserv_core::Error>`.
//! L'erreur est sérialisée en `{ code, params, detail }` et traduite côté UI.

use crate::state::{job_key, AppState};
use rustserv_core::catalog::{version, Build, Category, InstalledPackage};
use rustserv_core::orchestrator::{Group, GroupView};
use rustserv_core::process::{logs, ServiceState};
use rustserv_core::{Error, Result, Settings};
use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, State};

/// Nom de l'événement de progression d'installation.
pub const EVT_INSTALL: &str = "install-progress";
/// Nom de l'événement de changement d'état d'un service.
pub const EVT_SERVICE: &str = "service-status";

#[derive(Serialize)]
pub struct AppInfo {
    version: &'static str,
    racine: String,
}

/// Informations générales affichées dans l'interface.
#[tauri::command]
pub fn app_info(state: State<'_, AppState>) -> Result<AppInfo> {
    Ok(AppInfo { version: env!("CARGO_PKG_VERSION"), racine: state.paths.root().display().to_string() })
}

// ---------------------------------------------------------------- catalogue

#[derive(Serialize)]
pub struct CatalogView {
    updated: String,
    packages: Vec<PackageView>,
    /// Erreur de chargement de catalog.json (repli sur le catalogue intégré).
    warning: Option<serde_json::Value>,
}

#[derive(Serialize)]
pub struct PackageView {
    id: String,
    name: String,
    category: Category,
    releases: Vec<ReleaseView>,
}

#[derive(Serialize)]
pub struct ReleaseView {
    version: String,
    branch: String,
    build: Option<Build>,
    installed: bool,
    installing: bool,
}

/// Catalogue + état d'installation de chaque version.
#[tauri::command]
pub fn catalog_list(state: State<'_, AppState>) -> Result<CatalogView> {
    let cat = state.catalog.read().map_err(|e| Error::Interne(e.to_string()))?;
    let jobs = state.jobs.lock().map_err(|e| Error::Interne(e.to_string()))?;
    let packages = cat
        .packages
        .iter()
        .map(|(id, p)| {
            let mut releases: Vec<_> = p
                .releases
                .iter()
                .map(|r| ReleaseView {
                    version: r.version.clone(),
                    branch: r.branch(),
                    build: r.build.clone(),
                    installed: state.installer.is_installed(id, &r.version),
                    installing: jobs.contains_key(&job_key(id, &r.version)),
                })
                .collect();
            releases.sort_by(|a, b| version::cmp(&b.version, &a.version));
            PackageView { id: id.clone(), name: p.name.clone(), category: p.category, releases }
        })
        .collect();
    Ok(CatalogView { updated: cat.updated.clone(), packages, warning: state.catalog_warning.clone() })
}

/// Lance l'installation ; la progression arrive via l'événement `install-progress`.
#[tauri::command]
pub async fn catalog_install(
    app: AppHandle,
    state: State<'_, AppState>,
    package: String,
    version: String,
) -> Result<InstalledPackage> {
    let release = {
        let cat = state.catalog.read().map_err(|e| Error::Interne(e.to_string()))?;
        cat.release(&package, &version)?.clone()
    };
    let key = job_key(&package, &version);
    let cancel = Arc::new(AtomicBool::new(false));
    {
        let mut jobs = state.jobs.lock().map_err(|e| Error::Interne(e.to_string()))?;
        if jobs.contains_key(&key) {
            return Err(Error::DejaInstalle { id: package, version });
        }
        jobs.insert(key.clone(), cancel.clone());
    }
    let res = state
        .installer
        .install(&package, &release, &cancel, &|p| {
            let _ = app.emit(EVT_INSTALL, &p);
        })
        .await;
    if let Ok(mut jobs) = state.jobs.lock() {
        jobs.remove(&key);
    }
    if let Err(e) = &res {
        tracing::error!(%package, %version, erreur = %e, "installation échouée");
    }
    res
}

/// Demande l'annulation d'une installation en cours.
#[tauri::command]
pub fn catalog_cancel(state: State<'_, AppState>, package: String, version: String) -> Result<()> {
    if let Some(flag) = state.jobs.lock().map_err(|e| Error::Interne(e.to_string()))?.get(&job_key(&package, &version)) {
        flag.store(true, Ordering::Relaxed);
    }
    Ok(())
}

#[tauri::command]
pub fn catalog_uninstall(state: State<'_, AppState>, package: String, version: String) -> Result<()> {
    // Refus si un service utilise cette version (Windows bloquerait la suppression à moitié).
    let actif = |s: &ServiceState| !matches!(s, ServiceState::Arrete | ServiceState::Erreur { .. });
    let en_cours = if package == "php" {
        // Plusieurs versions de PHP peuvent tourner : on regarde les processus de celle-ci.
        let prefix = format!("php-{version}#");
        state.supervisor().states().iter().any(|s| s.id.starts_with(&prefix) && actif(&s.state))
    } else {
        let installed = state.installer.installed()?;
        state
            .orchestrator
            .overview(&state.settings(), &installed)
            .into_iter()
            .any(|g| g.software == package && g.version.as_deref() == Some(version.as_str()) && actif(&g.state))
    };
    if en_cours {
        return Err(Error::ServiceDejaDemarre { service: format!("{package} {version}") });
    }
    state.installer.uninstall(&package, &version)
}

#[tauri::command]
pub fn installed_list(state: State<'_, AppState>) -> Result<Vec<InstalledPackage>> {
    state.installer.installed()
}

// ---------------------------------------------------------------- services

#[tauri::command]
pub fn services_overview(state: State<'_, AppState>) -> Result<Vec<GroupView>> {
    let installed = state.installer.installed()?;
    Ok(state.orchestrator.overview(&state.settings(), &installed))
}

#[tauri::command]
pub async fn service_start(state: State<'_, AppState>, group: Group) -> Result<()> {
    let installed = state.installer.installed()?;
    state.orchestrator.start(group, &state.settings(), &installed).await
}

#[tauri::command]
pub async fn service_stop(state: State<'_, AppState>, group: Group) -> Result<()> {
    state.orchestrator.stop(group).await
}

#[tauri::command]
pub async fn service_restart(state: State<'_, AppState>, group: Group) -> Result<()> {
    state.orchestrator.stop(group).await?;
    let installed = state.installer.installed()?;
    state.orchestrator.start(group, &state.settings(), &installed).await
}

#[tauri::command]
pub async fn services_start_all(state: State<'_, AppState>) -> Result<()> {
    let installed = state.installer.installed()?;
    state.orchestrator.start_all(&state.settings(), &installed).await
}

#[tauri::command]
pub async fn services_stop_all(state: State<'_, AppState>) -> Result<()> {
    state.supervisor().stop_all().await;
    Ok(())
}

/// Dernières lignes du journal d'un groupe.
#[tauri::command]
pub fn service_log(state: State<'_, AppState>, group: Group) -> Result<String> {
    let installed = state.installer.installed()?;
    let view = state.orchestrator.overview(&state.settings(), &installed);
    let file = view.iter().find(|g| g.group == group).map(|g| g.log_file.clone()).unwrap_or_default();
    Ok(logs::tail(std::path::Path::new(&file), 200))
}

// ---------------------------------------------------------------- réglages

#[derive(Serialize)]
pub struct SettingsView {
    settings: Settings,
    warning: Option<serde_json::Value>,
    /// Domaines de sites autorisés (réservés au réseau local), dans l'ordre recommandé.
    tlds: &'static [&'static str],
}

#[tauri::command]
pub fn settings_get(state: State<'_, AppState>) -> Result<SettingsView> {
    Ok(SettingsView {
        settings: state.settings(),
        warning: state.settings_warning.clone(),
        tlds: rustserv_core::hosts::TLD_AUTORISES,
    })
}

#[derive(Serialize)]
pub struct SaveResult {
    /// Le domaine des sites a changé : les domaines doivent être redéclarés dans Windows.
    domaine_change: bool,
}

/// Enregistre les réglages. Un changement de domaine est appliqué tout de suite
/// (configurations et certificats régénérés, serveur web redémarré s'il tournait) ;
/// les autres réglages s'appliquent au prochain (re)démarrage des services.
#[tauri::command]
pub async fn settings_save(state: State<'_, AppState>, settings: Settings) -> Result<SaveResult> {
    settings.validate()?;
    let ancien_tld = state.settings().tld;
    settings.save(&state.paths)?;
    let domaine_change = settings.tld != ancien_tld;
    *state.settings.write().map_err(|e| Error::Interne(e.to_string()))? = settings.clone();
    if domaine_change {
        tracing::info!(de = %ancien_tld, vers = %settings.tld, "domaine des sites modifié");
        let installed = state.installer.installed()?;
        state.orchestrator.apply(&settings, &installed).await?;
    }
    Ok(SaveResult { domaine_change })
}
