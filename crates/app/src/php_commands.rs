//! Commandes des pages **PHP** et **Sites** (phase 2 : PHP multi-versions).

use crate::commands::EVT_INSTALL;
use crate::state::{job_key, AppState};
use rustserv_core::catalog::{php_remote, version};
use rustserv_core::php::{self, MigrationReport};
use rustserv_core::process::ServiceState;
use rustserv_core::services::Stack;
use rustserv_core::{sites, Error, Result, Settings};
use serde::Serialize;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, State};

#[derive(Serialize)]
pub struct PhpVersionView {
    version: String,
    branch: String,
    /// Version utilisée pour sa branche (une seule par branche).
    active: bool,
    /// Branche par défaut des sites.
    default: bool,
    /// Version épinglée (empêche la bascule automatique vers la plus récente).
    pinned: bool,
    running: bool,
    ports: Vec<u16>,
    /// Version plus récente disponible dans le catalogue pour cette branche.
    update: Option<String>,
    /// Sites qui utilisent cette version.
    sites: Vec<String>,
    ini: String,
}

#[derive(Serialize)]
pub struct PhpView {
    versions: Vec<PhpVersionView>,
    /// Dernière recherche de mises à jour (horodatage Unix), si faite pendant cette session.
    last_check: Option<u64>,
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn lock_err<T>(e: std::sync::PoisonError<T>) -> Error {
    Error::Interne(e.to_string())
}

#[tauri::command]
pub fn php_overview(state: State<'_, AppState>) -> Result<PhpView> {
    let settings = state.settings();
    let installed = state.installer.installed()?;
    let st = Stack::new(&state.paths, &settings, &installed);
    let cat = state.catalog.read().map_err(lock_err)?;
    let sup = state.supervisor();
    let default_version = st.default_php().map(|p| p.version.clone());
    let liste_sites = sites::discover(&state.paths, &settings.tld)?;

    let versions = st
        .php_installed()
        .map(|p| {
            let branch = version::branch(&p.version);
            let active = st.php_for_branch(&branch).is_some_and(|a| a.version == p.version);
            let prefix = format!("php-{}#", p.version);
            let running = sup.states().iter().any(|s| s.id.starts_with(&prefix) && matches!(s.state, ServiceState::EnCours { .. }));
            let update = cat
                .latest("php", Some(&branch))
                .filter(|r| version::cmp(&r.version, &p.version).is_gt())
                // Rien à proposer si la version la plus récente est déjà installée.
                .filter(|r| !installed.iter().any(|i| i.package == "php" && i.version == r.version))
                .map(|r| r.version.clone());
            let sites_v = if active {
                liste_sites.iter().filter(|s| st.site_php(&s.name).is_some_and(|x| x.version == p.version)).map(|s| s.name.clone()).collect()
            } else {
                vec![]
            };
            PhpVersionView {
                version: p.version.clone(),
                branch: branch.clone(),
                active,
                default: default_version.as_deref() == Some(p.version.as_str()),
                pinned: settings.php_pins.get(&branch) == Some(&p.version),
                running,
                ports: php::pool_ports(&p.version, settings.php_pool_size),
                update,
                sites: sites_v,
                ini: php::php_ini_path(&state.paths, &p.version).display().to_string(),
            }
        })
        .collect::<Vec<_>>();
    let mut versions = versions;
    versions.sort_by(|a, b| version::cmp(&b.version, &a.version));
    Ok(PhpView { versions, last_check: *state.last_php_check.lock().map_err(lock_err)? })
}

/// Modifie les réglages, les enregistre puis reconfigure les services en marche.
async fn update_settings(state: &AppState, f: impl FnOnce(&mut Settings)) -> Result<()> {
    let settings = {
        let mut s = state.settings.write().map_err(lock_err)?;
        f(&mut s);
        s.validate()?;
        s.save(&state.paths)?;
        s.clone()
    };
    let installed = state.installer.installed()?;
    state.orchestrator.apply(&settings, &installed).await
}

/// Choisit la branche de PHP par défaut (« 8.3 »).
#[tauri::command]
pub async fn php_set_default(state: State<'_, AppState>, branch: String) -> Result<()> {
    update_settings(&state, |s| s.versions.php = Some(branch)).await
}

/// Épingle une version dans sa branche, ou retire l'épinglage (`version` = null).
#[tauri::command]
pub async fn php_pin(state: State<'_, AppState>, branch: String, version: Option<String>) -> Result<()> {
    update_settings(&state, |s| match version {
        Some(v) => {
            s.php_pins.insert(branch, v);
        }
        None => {
            s.php_pins.remove(&branch);
        }
    })
    .await
}

/// Interroge windows.php.net et ajoute les nouvelles versions au catalogue.
/// Renvoie les versions ajoutées.
#[tauri::command]
pub async fn php_check_updates(state: State<'_, AppState>) -> Result<Vec<String>> {
    let releases = php_remote::fetch(state.installer.client()).await?;
    let ajoutees = {
        let mut cat = state.catalog.write().map_err(lock_err)?;
        let a = cat.merge("php", releases)?;
        if !a.is_empty() {
            cat.save(&state.paths.catalog_file())?;
        }
        a
    };
    *state.last_php_check.lock().map_err(lock_err)? = Some(now());
    tracing::info!(?ajoutees, "recherche de mises à jour PHP");
    Ok(ajoutees)
}

/// Met à jour une branche vers la dernière version du catalogue :
/// installation, reprise du php.ini et des DLL ajoutées, validation (`php -m`),
/// bascule des services. En cas d'échec de validation, retour à l'ancienne version.
#[tauri::command]
pub async fn php_upgrade(app: AppHandle, state: State<'_, AppState>, branch: String, keep_old: bool) -> Result<MigrationReport> {
    let release = {
        let cat = state.catalog.read().map_err(lock_err)?;
        cat.latest("php", Some(&branch)).cloned().ok_or_else(|| Error::VersionInconnue { id: "php".into(), version: branch.clone() })?
    };
    let key = job_key("php", &release.version);
    let cancel = Arc::new(AtomicBool::new(false));
    state.jobs.lock().map_err(lock_err)?.insert(key.clone(), cancel.clone());
    let progression = |p| {
        let _ = app.emit(EVT_INSTALL, &p);
    };
    let res = state
        .orchestrator
        .upgrade_php_branch(&state.installer, &state.settings(), &branch, &release, keep_old, &cancel, &progression)
        .await;
    state.jobs.lock().map_err(lock_err)?.remove(&key);
    let (rapport, nouveaux) = res?;
    // Enregistrement des réglages (épinglage de l'ancienne version retiré).
    nouveaux.save(&state.paths)?;
    *state.settings.write().map_err(lock_err)? = nouveaux;
    Ok(rapport)
}

// ---------------------------------------------------------------- sites

#[derive(Serialize)]
pub struct SiteView {
    name: String,
    host: String,
    dir: String,
    root: String,
    /// Branche choisie pour ce site (null = PHP par défaut).
    php_choice: Option<String>,
    /// Version réellement utilisée.
    php_version: Option<String>,
    /// Le choix pointe vers une branche qui n'est plus installée.
    php_missing: bool,
    /// Adresse utilisable tout de suite (avant la gestion des domaines .test).
    local_url: String,
    /// Site WordPress (wp-config.php présent).
    wordpress: bool,
    /// Base choisie (« mysql:8.4 », null = par défaut), moteur et version réellement utilisés.
    db_choice: Option<String>,
    db_moteur: Option<rustserv_core::db::Moteur>,
    db_version: Option<String>,
    /// Base réellement utilisée, au format d'un choix (« mariadb:12.3 »).
    db_actuel: Option<String>,
    /// Le site n'a pas de choix : il suit la base par défaut.
    db_defaut: bool,
    /// Type de site reconnu (WordPress, Laravel…) et sa version.
    type_site: Option<rustserv_core::site_type::TypeSite>,
}

#[tauri::command]
pub fn sites_list(state: State<'_, AppState>) -> Result<Vec<SiteView>> {
    let settings = state.settings();
    let installed = state.installer.installed()?;
    let st = Stack::new(&state.paths, &settings, &installed);
    let port = settings.ports.http;
    let base = if port == 80 { "http://localhost".to_string() } else { format!("http://localhost:{port}") };
    Ok(sites::discover(&state.paths, &settings.tld)?
        .into_iter()
        .map(|s| {
            let choice = settings.site_php.get(&s.name).cloned();
            let missing = choice.as_deref().is_some_and(|c| st.resolve_php(c).is_none());
            let dir_name = s.dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            SiteView {
                php_version: st.site_php(&s.name).map(|p| p.version.clone()),
                local_url: format!("{base}/{dir_name}/"),
                host: s.host,
                dir: s.dir.display().to_string(),
                root: s.root.display().to_string(),
                wordpress: s.dir.join("wp-config.php").is_file(),
                db_actuel: st.site_db(&s.name).ok().map(|p| {
                    let m = Stack::moteur_de(p);
                    rustserv_core::db::ChoixBase { moteur: m, branche: m.branche(&p.version) }.texte()
                }),
                db_defaut: settings.choix_base(&s.name).is_none(),
                type_site: rustserv_core::site_type::detecter(&s.dir),
                db_choice: settings.choix_base(&s.name).map(|c| c.texte()),
                db_moteur: st.site_db(&s.name).ok().map(Stack::moteur_de),
                db_version: st.site_db(&s.name).ok().map(|p| p.version.clone()),
                php_choice: choice,
                php_missing: missing,
                name: s.name,
            }
        })
        .collect())
}

/// Choisit la branche de PHP d'un site (null = PHP par défaut).
#[tauri::command]
pub async fn site_set_php(state: State<'_, AppState>, site: String, branch: Option<String>) -> Result<()> {
    update_settings(&state, |s| match branch {
        Some(b) => {
            s.site_php.insert(site, b);
        }
        None => {
            s.site_php.remove(&site);
        }
    })
    .await
}
