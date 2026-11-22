//! Commandes de l'assistant d'import depuis Laragon / WAMP.

use crate::state::AppState;
use rustserv_core::migration::detect::{self, Detection};
use rustserv_core::migration::run::{self, ImportItem, ResultatSite};
use rustserv_core::orchestrator::Group;
use rustserv_core::{ssl, Error, Result};
use tauri::{AppHandle, Emitter, State};

pub const EVT_MIGRATION: &str = "migration-progress";

/// Détecte Laragon et WAMP, leurs sites et leurs bases (lecture seule).
#[tauri::command]
pub fn migration_detect(state: State<'_, AppState>) -> Detection {
    detect::detect(&state.paths, &state.settings(), &detect::racines_standard())
}

/// Importe les sites choisis. Démarre MariaDB si besoin, enregistre le PHP de chaque
/// site, puis reconfigure le serveur web s'il tourne.
#[tauri::command]
pub async fn migration_run(app: AppHandle, state: State<'_, AppState>, items: Vec<ImportItem>) -> Result<Vec<ResultatSite>> {
    let settings = state.settings();
    let installed = state.installer.installed()?;
    if !state.orchestrator.running_groups().contains(&Group::Mariadb) {
        state.orchestrator.start(Group::Mariadb, &settings, &installed).await?;
    }
    // Instances MariaDB choisies pour les sites importés : démarrées avant l'import.
    let mut avec_choix = settings.clone();
    for i in &items {
        if let Some(b) = &i.mariadb {
            avec_choix.site_mariadb.insert(i.nom.clone(), b.clone());
        }
    }
    {
        let st = rustserv_core::services::Stack::new(&state.paths, &avec_choix, &installed);
        let mut vues = std::collections::HashSet::new();
        let pkgs: Vec<_> = items.iter().filter_map(|i| i.mariadb.as_ref()).filter_map(|b| st.mariadb_for_branch(b).cloned()).filter(|p| vues.insert(p.version.clone())).collect();
        for p in pkgs {
            state.orchestrator.ensure_mariadb(&avec_choix, &installed, &p).await?;
        }
    }
    let detection = detect::detect(&state.paths, &settings, &detect::racines_standard());
    let catalog = state.catalog.read().map_err(|e| Error::Interne(e.to_string()))?.clone();
    let https = ssl::status(&state.paths).map(|s| s.ca.is_some() && s.installee).unwrap_or(false);
    let progression = |p| {
        let _ = app.emit(EVT_MIGRATION, &p);
    };
    let (resultats, nouveaux) =
        run::importer(&state.paths, &settings, &state.installer, &catalog, &detection.sources, &items, https, &progression).await?;
    nouveaux.save(&state.paths)?;
    *state.settings.write().map_err(|e| Error::Interne(e.to_string()))? = nouveaux.clone();
    let installed = state.installer.installed()?;
    state.orchestrator.apply(&nouveaux, &installed).await?;
    Ok(resultats)
}

#[derive(serde::Serialize)]
pub struct AdressesWp {
    ancienne: Option<String>,
    nouvelle: String,
}

/// Fait pointer un site WordPress vers son domaine actuel (base + wp-config.php).
#[tauri::command]
pub async fn site_wp_fix(state: State<'_, AppState>, site: String) -> Result<AdressesWp> {
    let settings = state.settings();
    if !state.orchestrator.running_groups().contains(&Group::Mariadb) {
        let installed = state.installer.installed()?;
        state.orchestrator.start(Group::Mariadb, &settings, &installed).await?;
    }
    let catalog = state.catalog.read().map_err(|e| Error::Interne(e.to_string()))?.clone();
    let https = ssl::status(&state.paths).map(|s| s.ca.is_some() && s.installee).unwrap_or(false);
    let (ancienne, nouvelle) = run::adapter_site(&state.paths, &settings, &state.installer, &catalog, &site, https).await?;
    Ok(AdressesWp { ancienne, nouvelle })
}
