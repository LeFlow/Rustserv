//! Superviseur de services.
//!
//! Chaque service démarré est suivi par une tâche asynchrone qui :
//! - attend la fin du processus ou une demande d'arrêt ;
//! - en cas d'arrêt inattendu, le relance (au plus `MAX_REDEMARRAGES` fois par
//!   minute, avec un délai croissant 1 s, 2 s, 4 s…) ;
//! - publie chaque changement d'état sur un canal (`subscribe()`), relayé à l'UI.
//!
//! Tous les processus sont placés dans un Job Object Windows : si Rustserv
//! se termine brutalement, Windows les arrête tous automatiquement.

use super::{logs, ports};
use crate::error::{Error, Result};
use crate::platform::{self, JobObject};
use serde::Serialize;
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::process::Child;
use tokio::sync::{broadcast, oneshot};
use tokio::task::JoinHandle;

/// Nombre maximal de redémarrages automatiques sur une fenêtre d'une minute.
const MAX_REDEMARRAGES: usize = 5;
/// Lignes de journal jointes aux messages d'erreur.
const LIGNES_JOURNAL: usize = 12;

/// Comment arrêter proprement un service.
#[derive(Debug, Clone)]
pub enum StopStrategy {
    /// Arrêt immédiat (processus sans état, ex. php-cgi).
    Kill,
    /// Événement Windows nommé ; `{pid}` est remplacé par le PID.
    /// Ex. `MySQLShutdown{pid}` (MariaDB), `ap{pid}_shutdown` (Apache).
    NamedEvent(String),
    /// Commande à exécuter (ex. `nginx -s quit`).
    Command { program: PathBuf, args: Vec<String> },
}

/// Description complète d'un service à lancer.
#[derive(Debug, Clone)]
pub struct ServiceSpec {
    /// Identifiant unique (`nginx`, `mariadb`, `php-8.3-0`…).
    pub id: String,
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub env: Vec<(String, String)>,
    /// Ports vérifiés avant démarrage puis attendus en écoute (« service prêt »).
    pub ports: Vec<u16>,
    pub stop: StopStrategy,
    /// Délai laissé à l'arrêt propre avant arrêt forcé.
    pub stop_timeout: Duration,
    /// Délai maximal pour que les ports passent en écoute.
    pub start_timeout: Duration,
    /// Fichier recevant stdout + stderr.
    pub log_file: PathBuf,
    /// Redémarrer automatiquement en cas d'arrêt inattendu.
    pub restart: bool,
}

impl ServiceSpec {
    /// Spécification minimale ; les autres champs ont des valeurs par défaut raisonnables.
    pub fn new(id: impl Into<String>, program: impl Into<PathBuf>, log_file: impl Into<PathBuf>) -> Self {
        let program = program.into();
        let cwd = program.parent().map(PathBuf::from).unwrap_or_default();
        Self {
            id: id.into(),
            program,
            args: vec![],
            cwd,
            env: vec![],
            ports: vec![],
            stop: StopStrategy::Kill,
            stop_timeout: Duration::from_secs(10),
            start_timeout: Duration::from_secs(15),
            log_file: log_file.into(),
            restart: true,
        }
    }
}

/// État d'un service, tel qu'affiché dans l'interface.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "etat", rename_all = "snake_case")]
pub enum ServiceState {
    Arrete,
    Demarrage,
    EnCours { pid: u32, depuis: u64 },
    Arret,
    /// Dernière erreur sérialisée `{code, params, detail}`.
    Erreur { erreur: serde_json::Value },
}

/// Changement d'état publié aux abonnés.
#[derive(Debug, Clone, Serialize)]
pub struct StatusEvent {
    pub id: String,
    pub state: ServiceState,
}

struct Entry {
    state: ServiceState,
    stop_tx: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<()>>,
}

pub struct Supervisor {
    job: JobObject,
    entries: Mutex<HashMap<String, Entry>>,
    tx: broadcast::Sender<StatusEvent>,
}

impl Supervisor {
    pub fn new() -> Result<Arc<Self>> {
        let (tx, _) = broadcast::channel(256);
        Ok(Arc::new(Self { job: JobObject::new()?, entries: Mutex::new(HashMap::new()), tx }))
    }

    /// S'abonner aux changements d'état.
    pub fn subscribe(&self) -> broadcast::Receiver<StatusEvent> {
        self.tx.subscribe()
    }

    /// État actuel d'un service (`Arrete` s'il n'a jamais été lancé).
    pub fn state(&self, id: &str) -> ServiceState {
        self.entries.lock().unwrap().get(id).map(|e| e.state.clone()).unwrap_or(ServiceState::Arrete)
    }

    /// États de tous les services connus.
    pub fn states(&self) -> Vec<StatusEvent> {
        let map = self.entries.lock().unwrap();
        let mut v: Vec<_> = map.iter().map(|(id, e)| StatusEvent { id: id.clone(), state: e.state.clone() }).collect();
        v.sort_by(|a, b| a.id.cmp(&b.id));
        v
    }

    pub fn is_running(&self, id: &str) -> bool {
        matches!(self.state(id), ServiceState::EnCours { .. } | ServiceState::Demarrage)
    }

    fn set_state(&self, id: &str, state: ServiceState) {
        {
            let mut map = self.entries.lock().unwrap();
            let e = map.entry(id.to_string()).or_insert(Entry { state: ServiceState::Arrete, stop_tx: None, task: None });
            e.state = state.clone();
        }
        tracing::debug!(service = id, ?state, "état");
        let _ = self.tx.send(StatusEvent { id: id.to_string(), state });
    }

    /// Démarre un service et attend qu'il soit prêt (ports en écoute).
    pub async fn start(self: &Arc<Self>, spec: ServiceSpec) -> Result<()> {
        {
            let mut map = self.entries.lock().unwrap();
            if let Some(e) = map.get(&spec.id) {
                if e.task.is_some() {
                    return Err(Error::ServiceDejaDemarre { service: spec.id.clone() });
                }
            }
            // Réservation immédiate : empêche deux démarrages simultanés.
            map.insert(spec.id.clone(), Entry { state: ServiceState::Demarrage, stop_tx: None, task: None });
        }
        let _ = self.tx.send(StatusEvent { id: spec.id.clone(), state: ServiceState::Demarrage });

        match self.launch(&spec).await {
            Ok(child) => {
                let pid = child.id().unwrap_or(0);
                let (stop_tx, stop_rx) = oneshot::channel();
                let me = Arc::clone(self);
                let spec2 = spec.clone();
                let task = tokio::spawn(async move { me.monitor(spec2, child, stop_rx).await });
                {
                    let mut map = self.entries.lock().unwrap();
                    if let Some(e) = map.get_mut(&spec.id) {
                        e.stop_tx = Some(stop_tx);
                        e.task = Some(task);
                    }
                }
                self.set_state(&spec.id, ServiceState::EnCours { pid, depuis: now() });
                tracing::info!(service = %spec.id, pid, "démarré");
                Ok(())
            }
            Err(e) => {
                tracing::error!(service = %spec.id, erreur = %e, "démarrage échoué");
                self.entries.lock().unwrap().remove(&spec.id);
                self.set_state(&spec.id, ServiceState::Erreur { erreur: serde_json::to_value(&e).unwrap_or_default() });
                Err(e)
            }
        }
    }

    /// Arrête un service (sans erreur s'il n'est pas démarré).
    pub async fn stop(&self, id: &str) -> Result<()> {
        let (stop_tx, task) = {
            let mut map = self.entries.lock().unwrap();
            match map.get_mut(id) {
                Some(e) => (e.stop_tx.take(), e.task.take()),
                None => return Ok(()),
            }
        };
        if let Some(tx) = stop_tx {
            let _ = tx.send(());
        }
        if let Some(t) = task {
            let _ = t.await;
        }
        self.set_state(id, ServiceState::Arrete);
        Ok(())
    }

    /// Arrête tous les services en parallèle (fermeture de l'app).
    pub async fn stop_all(&self) {
        let ids: Vec<String> = self.entries.lock().unwrap().keys().cloned().collect();
        futures_util::future::join_all(ids.iter().map(|id| self.stop(id))).await;
    }

    /// Vérifie, lance le processus et attend qu'il soit prêt.
    async fn launch(&self, spec: &ServiceSpec) -> Result<Child> {
        if !spec.program.is_file() {
            return Err(Error::ExecutableIntrouvable { chemin: spec.program.clone() });
        }
        ports::ensure_all_free(&spec.ports)?;

        if let Some(dir) = spec.log_file.parent() {
            std::fs::create_dir_all(dir).map_err(|e| Error::fichier(dir, e))?;
        }
        let mut log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&spec.log_file)
            .map_err(|e| Error::fichier(&spec.log_file, e))?;
        {
            use std::io::Write;
            let _ = writeln!(log, "\n===== Rustserv : démarrage de {} ({}) =====", spec.id, now());
        }
        let log_err = log.try_clone().map_err(|e| Error::fichier(&spec.log_file, e))?;

        let mut cmd = tokio::process::Command::new(&spec.program);
        cmd.args(&spec.args)
            .current_dir(&spec.cwd)
            .envs(spec.env.iter().map(|(k, v)| (k, v)))
            .stdin(Stdio::null())
            .stdout(Stdio::from(log))
            .stderr(Stdio::from(log_err))
            .kill_on_drop(true);
        let mut child = platform::spawn_in_job(&self.job, &mut cmd)
            .map_err(|e| Error::ServiceDemarrage { service: spec.id.clone(), detail: e.to_string(), journal: String::new() })?;

        // Attente de disponibilité : tous les ports en écoute, et processus toujours vivant.
        let debut = Instant::now();
        let echec = |detail: String| Error::ServiceDemarrage {
            service: spec.id.clone(),
            detail,
            journal: logs::tail(&spec.log_file, LIGNES_JOURNAL),
        };
        loop {
            if let Ok(Some(status)) = child.try_wait() {
                return Err(echec(format!("le programme s'est arrêté immédiatement (code {})", fmt_code(status.code()))));
            }
            let prets = spec.ports.iter().all(|&p| ports::is_listening(p));
            if prets && (!spec.ports.is_empty() || debut.elapsed() >= Duration::from_millis(500)) {
                return Ok(child);
            }
            if debut.elapsed() > spec.start_timeout {
                let _ = child.kill().await;
                let attendus: Vec<String> = spec.ports.iter().map(|p| p.to_string()).collect();
                return Err(echec(format!("les ports {} ne sont pas ouverts après {} s", attendus.join(", "), spec.start_timeout.as_secs())));
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// Surveillance d'un service démarré jusqu'à son arrêt.
    async fn monitor(self: Arc<Self>, spec: ServiceSpec, mut child: Child, mut stop_rx: oneshot::Receiver<()>) {
        let mut historique: VecDeque<Instant> = VecDeque::new();
        loop {
            tokio::select! {
                _ = &mut stop_rx => {
                    self.set_state(&spec.id, ServiceState::Arret);
                    graceful_stop(&spec, &mut child).await;
                    tracing::info!(service = %spec.id, "arrêté");
                    return;
                }
                status = child.wait() => {
                    let code = status.ok().and_then(|s| s.code());
                    tracing::warn!(service = %spec.id, code = ?code, "arrêt inattendu");
                    historique.retain(|t| t.elapsed() < Duration::from_secs(60));
                    if !spec.restart || historique.len() >= MAX_REDEMARRAGES {
                        let e = Error::ServicePlantage { service: spec.id.clone(), code, journal: logs::tail(&spec.log_file, LIGNES_JOURNAL) };
                        self.finish_with_error(&spec.id, &e);
                        return;
                    }
                    historique.push_back(Instant::now());
                    let attente = Duration::from_secs(1 << (historique.len() - 1));
                    self.set_state(&spec.id, ServiceState::Demarrage);
                    tokio::select! {
                        _ = &mut stop_rx => { self.set_state(&spec.id, ServiceState::Arrete); return; }
                        _ = tokio::time::sleep(attente) => {}
                    }
                    match self.launch(&spec).await {
                        Ok(c) => {
                            child = c;
                            let pid = child.id().unwrap_or(0);
                            tracing::info!(service = %spec.id, pid, "relancé automatiquement");
                            self.set_state(&spec.id, ServiceState::EnCours { pid, depuis: now() });
                        }
                        Err(e) => {
                            self.finish_with_error(&spec.id, &e);
                            return;
                        }
                    }
                }
            }
        }
    }

    /// Termine la surveillance en erreur : l'entrée reste pour afficher l'erreur,
    /// mais sans tâche, ce qui permet un nouveau démarrage.
    fn finish_with_error(&self, id: &str, e: &Error) {
        tracing::error!(service = id, erreur = %e, "service en erreur");
        if let Some(entry) = self.entries.lock().unwrap().get_mut(id) {
            entry.stop_tx = None;
            entry.task = None;
        }
        self.set_state(id, ServiceState::Erreur { erreur: serde_json::to_value(e).unwrap_or_default() });
    }
}

/// Arrêt propre puis, si le délai est dépassé, arrêt forcé.
async fn graceful_stop(spec: &ServiceSpec, child: &mut Child) {
    let pid = child.id().unwrap_or(0);
    match &spec.stop {
        StopStrategy::Kill => {
            let _ = child.kill().await;
            return;
        }
        StopStrategy::NamedEvent(tpl) => {
            let name = tpl.replace("{pid}", &pid.to_string());
            if !platform::signal_named_event(&name) {
                tracing::warn!(service = %spec.id, evenement = %name, "événement d'arrêt introuvable, arrêt forcé");
                let _ = child.kill().await;
                return;
            }
        }
        StopStrategy::Command { program, args } => {
            let mut cmd = tokio::process::Command::new(program);
            cmd.args(args).current_dir(&spec.cwd).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
            cmd.creation_flags(platform::CREATE_NO_WINDOW);
            match cmd.status().await {
                Ok(s) if s.success() => {}
                other => {
                    tracing::warn!(service = %spec.id, resultat = ?other, "commande d'arrêt en échec, arrêt forcé");
                    let _ = child.kill().await;
                    return;
                }
            }
        }
    }
    if tokio::time::timeout(spec.stop_timeout, child.wait()).await.is_err() {
        tracing::warn!(service = %spec.id, "arrêt propre trop long, arrêt forcé");
        let _ = child.kill().await;
    }
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn fmt_code(c: Option<i32>) -> String {
    c.map(|c| c.to_string()).unwrap_or_else(|| "?".into())
}
