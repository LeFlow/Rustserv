//! Recherche de mises à jour du catalogue : à la demande (bouton) et automatiquement
//! au démarrage, au plus une fois toutes les 24 h. Résultat mémorisé dans
//! `etc/catalog-verification.json` (date + versions apparues).

use crate::state::AppState;
use rustserv_core::catalog::remote;
use rustserv_core::{Error, Paths, Result};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};

pub const EVT_CATALOGUE: &str = "catalog-updated";
/// Délai minimal entre deux vérifications automatiques.
const INTERVALLE: u64 = 24 * 3600;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Verification {
    pub verifie_le: u64,
    /// Versions ajoutées au catalogue (« php 8.7.0 »).
    pub ajouts: Vec<String>,
    /// Sources injoignables (paquet, message).
    pub erreurs: Vec<(String, String)>,
}

fn fichier(paths: &Paths) -> std::path::PathBuf {
    paths.etc().join("catalog-verification.json")
}

pub fn lire(paths: &Paths) -> Option<Verification> {
    std::fs::read_to_string(fichier(paths)).ok().and_then(|t| serde_json::from_str(&t).ok())
}

fn maintenant() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Interroge les sources officielles, complète le catalogue et l'enregistre.
pub async fn verifier(state: &AppState) -> Result<Verification> {
    let trouvailles = remote::fetch_all(state.installer.client()).await;
    let mut ajouts = Vec::new();
    {
        let mut cat = state.catalog.write().map_err(|e| Error::Interne(e.to_string()))?;
        let ids: Vec<String> = cat.packages.keys().cloned().collect();
        for id in ids {
            let releases: Vec<_> = trouvailles.releases.iter().filter(|(p, _)| p == &id).map(|(_, r)| r.clone()).collect();
            if releases.is_empty() {
                continue;
            }
            match cat.merge(&id, releases) {
                Ok(v) => ajouts.extend(v.into_iter().map(|v| format!("{id} {v}"))),
                Err(e) => tracing::warn!(paquet = %id, erreur = %e, "versions trouvées refusées"),
            }
        }
        if !ajouts.is_empty() {
            cat.save(&state.paths.catalog_file())?;
        }
    }
    let v = Verification { verifie_le: maintenant(), ajouts, erreurs: trouvailles.erreurs };
    let f = fichier(&state.paths);
    let json = serde_json::to_string_pretty(&v).map_err(|e| Error::Interne(e.to_string()))?;
    std::fs::write(&f, json).map_err(|e| Error::fichier(&f, e))?;
    tracing::info!(ajouts = ?v.ajouts, "recherche de mises à jour terminée");
    Ok(v)
}

/// Vérification automatique en arrière-plan si la dernière date de plus de 24 h.
pub fn au_demarrage(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        let recente = lire(&state.paths).is_some_and(|v| maintenant().saturating_sub(v.verifie_le) < INTERVALLE);
        if recente {
            return;
        }
        // Laisse l'interface démarrer avant d'utiliser le réseau.
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        match verifier(&state).await {
            Ok(v) => {
                let _ = app.emit(EVT_CATALOGUE, &v);
            }
            Err(e) => tracing::warn!(erreur = %e, "vérification automatique impossible"),
        }
    });
}
