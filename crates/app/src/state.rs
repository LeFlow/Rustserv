//! État partagé de l'application, accessible depuis toutes les commandes.

use rustserv_core::catalog::{Catalog, Installer};
use rustserv_core::orchestrator::Orchestrator;
use rustserv_core::process::Supervisor;
use rustserv_core::{Paths, Result, Settings};
use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, RwLock};

pub struct AppState {
    pub paths: Paths,
    pub catalog: RwLock<Catalog>,
    pub installer: Installer,
    pub settings: RwLock<Settings>,
    pub orchestrator: Orchestrator,
    /// Installations en cours : clé « paquet@version » → drapeau d'annulation.
    pub jobs: Mutex<HashMap<String, Arc<AtomicBool>>>,
    /// Problèmes rencontrés au démarrage (catalog.json, rustserv.json illisibles…).
    pub catalog_warning: Option<serde_json::Value>,
    pub settings_warning: Option<serde_json::Value>,
    /// Dernière recherche de mises à jour PHP (session courante).
    pub last_php_check: Mutex<Option<u64>>,
}

impl AppState {
    pub fn new(paths: Paths) -> Result<Self> {
        // Un fichier corrompu ne doit pas empêcher l'app de démarrer :
        // on se replie sur les valeurs intégrées et on prévient l'utilisateur.
        let (catalog, catalog_warning) = match Catalog::load_or_init(&paths) {
            Ok(c) => (c, None),
            Err(e) => {
                tracing::error!(erreur = %e, "catalog.json illisible, utilisation du catalogue intégré");
                (Catalog::embedded()?, serde_json::to_value(&e).ok())
            }
        };
        let (settings, settings_warning) = match Settings::load(&paths) {
            Ok(s) => (s, None),
            Err(e) => {
                tracing::error!(erreur = %e, "rustserv.json illisible, réglages par défaut");
                (Settings::default(), serde_json::to_value(&e).ok())
            }
        };
        let installer = Installer::new(paths.clone())?;
        let orchestrator = Orchestrator::new(paths.clone(), Supervisor::new()?);
        Ok(Self {
            paths,
            catalog: RwLock::new(catalog),
            installer,
            settings: RwLock::new(settings),
            orchestrator,
            jobs: Mutex::new(HashMap::new()),
            catalog_warning,
            settings_warning,
            last_php_check: Mutex::new(None),
        })
    }

    pub fn supervisor(&self) -> &Arc<Supervisor> {
        self.orchestrator.supervisor()
    }

    /// Copie des réglages courants (on ne garde jamais le verrou pendant un `await`).
    pub fn settings(&self) -> Settings {
        self.settings.read().map(|s| s.clone()).unwrap_or_default()
    }
}

pub fn job_key(package: &str, version: &str) -> String {
    format!("{package}@{version}")
}
