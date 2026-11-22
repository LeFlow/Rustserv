//! Pilotage de la pile par **groupes** (ce que voit l'utilisateur) :
//! - `web`     : nginx ou Apache selon les réglages (démarre PHP avant lui) ;
//! - `php`     : tous les processus php-cgi des versions utilisées ;
//! - `mariadb` : le serveur de bases de données.
//!
//! Les opérations lourdes (génération et validation des configs, initialisation
//! de MariaDB) sont exécutées hors du thread asynchrone.

use crate::catalog::{version, InstallProgress, InstalledPackage, Installer, Release};
use crate::error::{Error, Result};
use crate::paths::Paths;
use crate::php::{self, pool_ports, MigrationReport};
use std::sync::atomic::AtomicBool;
use crate::process::{ServiceSpec, ServiceState, StatusEvent, Supervisor};
use crate::services::{Stack, APACHE, MARIADB, NGINX};
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
    /// Instances (MariaDB multi-versions) : id de service, version, port.
    pub instances: Vec<Instance>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Instance {
    pub id: String,
    pub version: String,
    pub port: u16,
    /// Instance par défaut (port des réglages).
    pub defaut: bool,
}

/// Le service appartient-il à MariaDB (« mariadb » ou « mariadb-11.4 ») ?
fn est_mariadb(id: &str) -> bool {
    id == MARIADB || id.starts_with("mariadb-")
}

pub struct Orchestrator {
    pub(crate) paths: Paths,
    pub(crate) supervisor: Arc<Supervisor>,
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
                let specs = self.build(settings, installed, |st| st.mariadb_specs()).await?;
                self.start_specs(specs).await
            }
        }
    }

    /// Démarre (si besoin) l'instance MariaDB d'une version précise et renvoie son port.
    pub async fn ensure_mariadb(&self, settings: &Settings, installed: &[InstalledPackage], pkg: &InstalledPackage) -> Result<u16> {
        let p = pkg.clone();
        let spec = self.build(settings, installed, move |st| Ok(vec![st.mariadb_spec(&p)?])).await?;
        let port = spec[0].ports[0];
        self.start_specs(spec).await?;
        Ok(port)
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
            Group::Mariadb => self.supervisor.states().into_iter().map(|s| s.id).filter(|id| est_mariadb(id)).collect(),
        };
        futures_util::future::join_all(ids.iter().map(|id| self.supervisor.stop(id))).await;
        Ok(())
    }

    /// Applique un changement de configuration PHP (version par défaut, PHP d'un site,
    /// mise à jour) **sans rien démarrer qui était arrêté** :
    /// - arrête les pools PHP devenus inutiles ;
    /// - démarre les pools nouvellement nécessaires ;
    /// - redémarre le serveur web pour qu'il prenne la nouvelle configuration.
    pub async fn apply(&self, settings: &Settings, installed: &[InstalledPackage]) -> Result<()> {
        let needed: std::collections::HashSet<String> =
            Stack::new(&self.paths, settings, installed).php_service_ids().into_iter().collect();
        let states = self.supervisor.states();
        let actif = |id: &str| states.iter().any(|s| s.id == id && !matches!(s.state, ServiceState::Arrete | ServiceState::Erreur { .. }));
        let php_actif = states.iter().any(|s| s.id.starts_with("php-") && actif(&s.id));
        let web_actif = actif(NGINX) || actif(APACHE);
        // MariaDB : instances inutiles arrêtées, nouvelles démarrées (si MariaDB tournait).
        if states.iter().any(|s| est_mariadb(&s.id) && actif(&s.id)) {
            let st = Stack::new(&self.paths, settings, installed);
            let voulues: std::collections::HashSet<String> = st.mariadb_needed().into_iter().map(|p| st.mariadb_instance(p)).collect();
            for s in states.iter().filter(|s| est_mariadb(&s.id) && !voulues.contains(&s.id)) {
                self.supervisor.stop(&s.id).await?;
            }
            self.start(Group::Mariadb, settings, installed).await?;
        }
        if !php_actif && !web_actif {
            return Ok(());
        }
        for s in states.iter().filter(|s| s.id.starts_with("php-") && !needed.contains(&s.id)) {
            self.supervisor.stop(&s.id).await?;
        }
        self.start(Group::Php, settings, installed).await?;
        if web_actif {
            self.supervisor.stop(NGINX).await?;
            self.supervisor.stop(APACHE).await?;
            self.start(Group::Web, settings, installed).await?;
        }
        Ok(())
    }

    /// Met à jour une branche de PHP vers `release` (ex. 8.3.12 → 8.3.15) :
    /// 1. installation de la nouvelle version (si absente) ;
    /// 2. reprise du php.ini et des DLL ajoutées à la main ;
    /// 3. validation `php -m` — en cas d'échec, **retour arrière complet** ;
    /// 4. bascule des services en marche ;
    /// 5. suppression de l'ancienne version si `keep_old` est faux.
    ///
    /// Renvoie le rapport et les réglages à enregistrer (épinglage retiré).
    #[allow(clippy::too_many_arguments)]
    pub async fn upgrade_php_branch(
        &self,
        installer: &Installer,
        settings: &Settings,
        branch: &str,
        release: &Release,
        keep_old: bool,
        cancel: &AtomicBool,
        on_progress: &(dyn Fn(InstallProgress) + Send + Sync),
    ) -> Result<(MigrationReport, Settings)> {
        let installed = installer.installed()?;
        let old = Stack::new(&self.paths, settings, &installed)
            .php_for_branch(branch)
            .cloned()
            .ok_or_else(|| Error::AucuneVersion { id: format!("php {branch}") })?;
        if version::cmp(&release.version, &old.version).is_le() {
            return Err(Error::DejaInstalle { id: "php".into(), version: old.version });
        }

        let deja = installer.is_installed("php", &release.version);
        if !deja {
            installer.install("php", release, cancel, on_progress).await?;
        }
        let new_dir = installer.install_dir("php", &release.version);
        let rollback = |ini_cree: bool| {
            if ini_cree {
                let _ = std::fs::remove_dir_all(php::php_ini_path(&self.paths, &release.version).parent().unwrap());
            }
            if !deja {
                let _ = installer.uninstall("php", &release.version);
            }
        };

        let ini_existait = php::php_ini_path(&self.paths, &release.version).exists();
        let etapes = (|| -> Result<(bool, Vec<String>, std::path::PathBuf)> {
            let repris = php::migrate_ini(&self.paths, &old.path, &old.version, &new_dir, &release.version)?;
            let dll = php::copy_extra_dlls(&old.path, &new_dir)?;
            let ini = php::ensure_php_ini(&self.paths, &new_dir, &release.version, settings)?;
            Ok((repris, dll, ini))
        })();
        let (ini_repris, dll_copiees, ini) = match etapes {
            Ok(v) => v,
            Err(e) => {
                rollback(!ini_existait);
                return Err(e);
            }
        };

        let (d, v, i) = (new_dir.clone(), release.version.clone(), ini.clone());
        let verif = tokio::task::spawn_blocking(move || php::check(&d, &v, &i)).await.map_err(|e| Error::Interne(e.to_string()))?;
        if let Err(e) = verif {
            tracing::error!(erreur = %e, "nouvelle version PHP invalide, retour arrière");
            rollback(!ini_existait);
            return Err(e);
        }

        let mut nouveaux = settings.clone();
        if nouveaux.php_pins.get(branch) == Some(&old.version) {
            nouveaux.php_pins.remove(branch);
        }
        let installed = installer.installed()?;
        self.apply(&nouveaux, &installed).await?;

        if !keep_old {
            self.stop_php_version(&old.version).await?;
            installer.uninstall("php", &old.version)?;
        }
        tracing::info!(branche = branch, de = %old.version, vers = %release.version, "PHP mis à jour");
        Ok((MigrationReport { ini_repris, dll_copiees }, nouveaux))
    }

    /// Arrête les processus d'une version précise de PHP (avant suppression ou mise à jour).
    pub async fn stop_php_version(&self, version: &str) -> Result<()> {
        let prefix = format!("php-{version}#");
        let ids: Vec<String> = self.supervisor.states().into_iter().map(|s| s.id).filter(|id| id.starts_with(&prefix)).collect();
        futures_util::future::join_all(ids.iter().map(|id| self.supervisor.stop(id))).await;
        Ok(())
    }

    /// Groupes actuellement actifs (en marche ou en transition).
    pub fn running_groups(&self) -> Vec<Group> {
        let states = self.supervisor.states();
        let actif = |pred: &dyn Fn(&str) -> bool| {
            states.iter().any(|s| pred(&s.id) && !matches!(s.state, ServiceState::Arrete | ServiceState::Erreur { .. }))
        };
        let mut g = Vec::new();
        if actif(&|id| est_mariadb(id)) {
            g.push(Group::Mariadb);
        }
        if actif(&|id| id.starts_with("php-")) {
            g.push(Group::Php);
        }
        if actif(&|id| id == NGINX || id == APACHE) {
            g.push(Group::Web);
        }
        g
    }

    /// Redémarre **uniquement ce qui tournait**, avec les réglages et configurations
    /// actuels (ports, serveur web, php.ini modifié à la main…). Ordre : arrêt complet,
    /// puis MariaDB, puis PHP + serveur web. Renvoie les groupes redémarrés ;
    /// continue même si un groupe échoue et renvoie la première erreur.
    pub async fn restart_running(&self, settings: &Settings, installed: &[InstalledPackage]) -> Result<Vec<Group>> {
        let groupes = self.running_groups();
        if groupes.is_empty() {
            return Ok(groupes);
        }
        tracing::info!(?groupes, "redémarrage des services en marche");
        self.supervisor.stop_all().await;
        let mut premiere = None;
        for g in [Group::Mariadb, Group::Web, Group::Php] {
            // Le web démarre PHP lui-même : PHP seul seulement s'il tournait sans serveur web.
            let lancer = groupes.contains(&g) && !(g == Group::Php && groupes.contains(&Group::Web));
            if lancer {
                if let Err(e) = self.start(g, settings, installed).await {
                    premiere.get_or_insert(e);
                }
            }
        }
        premiere.map_or(Ok(groupes), Err)
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
        let db_pkg = stack.default_mariadb().ok();
        let instances: Vec<Instance> = stack
            .mariadb_needed()
            .into_iter()
            .map(|p| Instance { id: stack.mariadb_instance(p), version: p.version.clone(), port: stack.mariadb_port(p), defaut: stack.mariadb_instance(p) == MARIADB })
            .collect();

        // Toutes les versions de PHP utilisées (défaut + sites).
        let php_needed = stack.php_needed();
        let php_ids = stack.php_service_ids();
        let php_versions = php_needed.iter().map(|p| p.version.clone()).collect::<Vec<_>>().join(", ");
        let php_ports: Vec<u16> = php_needed.iter().flat_map(|p| pool_ports(&p.version, settings.php_pool_size)).collect();

        vec![
            GroupView {
                group: Group::Web,
                software: web.into(),
                version: web_pkg.map(|p| p.version.clone()),
                installed: web_pkg.is_some(),
                ports: stack.web_ports().unwrap_or_else(|_| vec![settings.ports.http]),
                // Si l'autre serveur tourne encore (réglage changé), on l'affiche aussi.
                members: members_of(&|id| id == NGINX || id == APACHE),
                state: ServiceState::Arrete,
                log_file: logs.join(format!("{web}-error.log")).display().to_string(),
                instances: vec![],
            },
            GroupView {
                group: Group::Php,
                software: "php".into(),
                version: (!php_versions.is_empty()).then_some(php_versions),
                installed: php_pkg.is_some(),
                ports: php_ports,
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
                instances: vec![],
            },
            GroupView {
                group: Group::Mariadb,
                software: MARIADB.into(),
                version: (!instances.is_empty()).then(|| instances.iter().map(|i| i.version.clone()).collect::<Vec<_>>().join(", ")),
                installed: db_pkg.is_some(),
                ports: instances.iter().map(|i| i.port).collect(),
                members: {
                    let mut m = members_of(&|id| est_mariadb(id));
                    for i in &instances {
                        if !m.iter().any(|s| s.id == i.id) {
                            m.push(StatusEvent { id: i.id.clone(), state: ServiceState::Arrete });
                        }
                    }
                    m
                },
                state: ServiceState::Arrete,
                log_file: logs.join("mariadb-error.log").display().to_string(),
                instances,
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
