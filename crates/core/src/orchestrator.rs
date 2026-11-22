//! Pilotage de la pile par **groupes** (ce que voit l'utilisateur) :
//! - `web`     : nginx ou Apache selon les réglages (démarre PHP avant lui) ;
//! - `php`     : tous les processus php-cgi des versions utilisées ;
//! - `mariadb` : le serveur de bases de données.
//!
//! Les opérations lourdes (génération et validation des configs, initialisation
//! de MariaDB) sont exécutées hors du thread asynchrone.

use crate::catalog::InstalledPackage;
use crate::error::{Error, Result};
use crate::paths::Paths;
use crate::php::pool_ports;
use crate::process::{ServiceSpec, ServiceState, StatusEvent, Supervisor};
use crate::services::{php_service_id, Stack, APACHE, MARIADB, NGINX};
use crate::settings::{Settings, WebServer};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Group {
    Web,
    Php,
    Mariadb,
}

impl Group {
    pub const ALL: [Group; 3] = [Group::Web, Group::Php, Group::Mariadb];
}

/// Vue d'un groupe pour l'interface.
#[derive(Debug, Clone, Serialize)]
pub struct GroupView {
    pub group: Group,
    /// Logiciel concerné : `nginx`, `apache`, `php`, `mariadb`.
    pub software: String,
    pub version: Option<String>,
    pub installed: bool,
    pub ports: Vec<u16>,
    /// État agrégé du groupe.
    pub state: ServiceState,
    /// États individuels (utile pour le pool PHP).
    pub members: Vec<StatusEvent>,
    /// Fichier journal principal du groupe.
    pub log_file: String,
}

pub struct Orchestrator {
    paths: Paths,
    supervisor: Arc<Supervisor>,
}

impl Orchestrator {
    pub fn new(paths: Paths, supervisor: Arc<Supervisor>) -> Self {
        Self { paths, supervisor }
    }

    pub fn supervisor(&self) -> &Arc<Supervisor> {
        &self.supervisor
    }

    /// Construit des specs dans un thread dédié (appels bloquants : `nginx -t`, init MariaDB…).
    async fn build<F>(&self, settings: &Settings, installed: &[InstalledPackage], f: F) -> Result<Vec<ServiceSpec>>
    where
        F: FnOnce(&Stack) -> Result<Vec<ServiceSpec>> + Send + 'static,
    {
        let (paths, settings, installed) = (self.paths.clone(), settings.clone(), installed.to_vec());
        tokio::task::spawn_blocking(move || f(&Stack::new(&paths, &settings, &installed)))
            .await
            .map_err(|e| Error::Interne(e.to_string()))?
    }

    async fn start_specs(&self, specs: Vec<ServiceSpec>) -> Result<()> {
        for s in specs {
            if !self.supervisor.is_running(&s.id) {
                self.supervisor.start(s).await?;
            }
        }
        Ok(())
    }

    /// Démarre un groupe (et ses dépendances : PHP avant le serveur web).
    pub async fn start(&self, group: Group, settings: &Settings, installed: &[InstalledPackage]) -> Result<()> {
        match group {
            Group::Php => {
                let specs = self
                    .build(settings, installed, |st| {
                        let mut v = Vec::new();
                        for php in st.php_needed() {
                            v.extend(st.php_specs(php)?);
                        }
                        Ok(v)
                    })
                    .await?;
                self.start_specs(specs).await
            }
            Group::Web => {
                // Le serveur « non choisi » ne doit pas occuper le port.
                let autre = match settings.web_server {
                    WebServer::Nginx => APACHE,
                    WebServer::Apache => NGINX,
                };
                self.supervisor.stop(autre).await?;
                Box::pin(self.start(Group::Php, settings, installed)).await?;
                let spec = self.build(settings, installed, |st| Ok(vec![st.web_server()?])).await?;
                self.start_specs(spec).await
            }
            Group::Mariadb => {
                let spec = self.build(settings, installed, |st| Ok(vec![st.mariadb()?])).await?;
                self.start_specs(spec).await
            }
        }
    }

    /// Arrête un groupe. Arrêter PHP arrête aussi le serveur web (qui en dépend).
    pub async fn stop(&self, group: Group) -> Result<()> {
        let ids: Vec<String> = match group {
            Group::Web => vec![NGINX.into(), APACHE.into()],
            Group::Php => {
                self.supervisor.stop(NGINX).await?;
                self.supervisor.stop(APACHE).await?;
                self.supervisor.states().into_iter().map(|s| s.id).filter(|id| id.starts_with("php-")).collect()
            }
            Group::Mariadb => vec![MARIADB.into()],
        };
        futures_util::future::join_all(ids.iter().map(|id| self.supervisor.stop(id))).await;
        Ok(())
    }

    /// Démarre tout : MariaDB, puis PHP, puis le serveur web.
    /// Continue même si un groupe échoue et renvoie la première erreur.
    pub async fn start_all(&self, settings: &Settings, installed: &[InstalledPackage]) -> Result<()> {
        let mut premiere = None;
        for g in [Group::Mariadb, Group::Web] {
            let pkg = if g == Group::Mariadb { MARIADB } else { web_software(settings) };
            if !installed.iter().any(|p| p.package == pkg) {
                continue; // non installé : on l'ignore silencieusement pour « tout démarrer »
            }
            if let Err(e) = self.start(g, settings, installed).await {
                premiere.get_or_insert(e);
            }
        }
        premiere.map_or(Ok(()), Err)
    }

    /// Vue de chaque groupe.
    pub fn overview(&self, settings: &Settings, installed: &[InstalledPackage]) -> Vec<GroupView> {
        let stack = Stack::new(&self.paths, settings, installed);
        let all = self.supervisor.states();
        let members_of = |pred: &dyn Fn(&str) -> bool| -> Vec<StatusEvent> { all.iter().filter(|s| pred(&s.id)).cloned().collect() };
        let logs = self.paths.logs();

        let web = web_software(settings);
        let web_pkg = stack.pick(web, version_wanted(settings, web)).ok();
        let php_pkg = stack.default_php();
        let db_pkg = stack.pick(MARIADB, settings.versions.mariadb.as_deref()).ok();

        let php_ids: Vec<String> = php_pkg
            .map(|p| (0..settings.php_pool_size as usize).map(|n| php_service_id(&p.version, n)).collect())
            .unwrap_or_default();

        vec![
            GroupView {
                group: Group::Web,
                software: web.into(),
                version: web_pkg.map(|p| p.version.clone()),
                installed: web_pkg.is_some(),
                ports: vec![settings.ports.http],
                // Si l'autre serveur tourne encore (réglage changé), on l'affiche aussi.
                members: members_of(&|id| id == NGINX || id == APACHE),
                state: ServiceState::Arrete,
                log_file: logs.join(format!("{web}-error.log")).display().to_string(),
            },
            GroupView {
                group: Group::Php,
                software: "php".into(),
                version: php_pkg.map(|p| p.version.clone()),
                installed: php_pkg.is_some(),
                ports: php_pkg.map(|p| pool_ports(&p.version, settings.php_pool_size)).unwrap_or_default(),
                members: {
                    let mut m = members_of(&|id| id.starts_with("php-"));
                    // Membres attendus jamais démarrés : affichés « arrêtés ».
                    for id in &php_ids {
                        if !m.iter().any(|s| &s.id == id) {
                            m.push(StatusEvent { id: id.clone(), state: ServiceState::Arrete });
                        }
                    }
                    m
                },
                state: ServiceState::Arrete,
                log_file: php_pkg.map(|p| logs.join(format!("php-{}-error.log", p.version)).display().to_string()).unwrap_or_default(),
            },
            GroupView {
                group: Group::Mariadb,
                software: MARIADB.into(),
                version: db_pkg.map(|p| p.version.clone()),
                installed: db_pkg.is_some(),
                ports: vec![settings.ports.mariadb],
                members: members_of(&|id| id == MARIADB),
                state: ServiceState::Arrete,
                log_file: logs.join("mariadb-error.log").display().to_string(),
            },
        ]
        .into_iter()
        .map(|mut g| {
            g.state = aggregate(&g.members);
            g
        })
        .collect()
    }
}

fn web_software(settings: &Settings) -> &'static str {
    match settings.web_server {
        WebServer::Nginx => NGINX,
        WebServer::Apache => APACHE,
    }
}

fn version_wanted<'a>(settings: &'a Settings, software: &str) -> Option<&'a str> {
    match software {
        NGINX => settings.versions.nginx.as_deref(),
        APACHE => settings.versions.apache.as_deref(),
        _ => None,
    }
}

/// État global d'un groupe : une erreur l'emporte, puis les transitions,
/// puis « en cours » si au moins un membre tourne.
pub fn aggregate(members: &[StatusEvent]) -> ServiceState {
    if let Some(e) = members.iter().find(|m| matches!(m.state, ServiceState::Erreur { .. })) {
        return e.state.clone();
    }
    if members.iter().any(|m| matches!(m.state, ServiceState::Demarrage)) {
        return ServiceState::Demarrage;
    }
    if members.iter().any(|m| matches!(m.state, ServiceState::Arret)) {
        return ServiceState::Arret;
    }
    members
        .iter()
        .filter_map(|m| match m.state {
            ServiceState::EnCours { depuis, pid } => Some((depuis, pid)),
            _ => None,
        })
        .min()
        .map(|(depuis, pid)| ServiceState::EnCours { pid, depuis })
        .unwrap_or(ServiceState::Arrete)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(id: &str, state: ServiceState) -> StatusEvent {
        StatusEvent { id: id.into(), state }
    }

    #[test]
    fn agregation_des_etats() {
        assert_eq!(aggregate(&[]), ServiceState::Arrete);
        let run = |pid| ServiceState::EnCours { pid, depuis: pid as u64 };
        assert_eq!(aggregate(&[ev("a", run(2)), ev("b", run(1))]), run(1));
        assert_eq!(aggregate(&[ev("a", run(2)), ev("b", ServiceState::Demarrage)]), ServiceState::Demarrage);
        let err = ServiceState::Erreur { erreur: serde_json::json!({"code": "x"}) };
        assert_eq!(aggregate(&[ev("a", run(1)), ev("b", err.clone())]), err);
    }
}
