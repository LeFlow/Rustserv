//! Assemblage de la pile : choisit les versions installées, génère les
//! configurations (gabarits), les valide (`nginx -t`, `httpd -t`) et produit
//! les [`ServiceSpec`] à confier au superviseur.
//!
//! Identifiants de service : `nginx`, `apache`, `mariadb`, `php-<version>#<n>`.

use crate::catalog::{version, InstalledPackage};
use crate::error::{Error, Result};
use crate::paths::{slash, Paths};
use crate::php::{self, pool_ports};
use crate::process::{ServiceSpec, StopStrategy};
use crate::settings::{Settings, WebServer};
use crate::{platform, sites, templates};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const NGINX: &str = "nginx";
pub const APACHE: &str = "apache";
pub const MARIADB: &str = "mariadb";

/// Identifiant du n-ième processus du pool d'une version de PHP.
pub fn php_service_id(version: &str, n: usize) -> String {
    format!("php-{version}#{n}")
}

/// Nom d'upstream nginx / balancer Apache pour une version de PHP.
pub fn upstream_name(version: &str) -> String {
    format!("php_{}", version.replace('.', "_"))
}

#[derive(Debug, Serialize)]
struct Upstream {
    name: String,
    ports: Vec<u16>,
}

#[derive(Debug, Serialize)]
struct SiteCtx {
    name: String,
    /// Nom réel du dossier (casse d'origine) pour l'accès http://localhost/<dossier>/.
    dir_name: String,
    dir: String,
    host: String,
    root: String,
    upstream: String,
    php_version: String,
    /// Certificat HTTPS (absent si le HTTPS n'est pas activé).
    cert: Option<CertCtx>,
}

#[derive(Debug, Serialize)]
struct CertCtx {
    cert: String,
    key: String,
}

impl From<crate::ssl::SiteCert> for CertCtx {
    fn from(c: crate::ssl::SiteCert) -> Self {
        Self { cert: slash(&c.cert), key: slash(&c.key) }
    }
}

/// Contexte commun aux gabarits nginx et Apache.
#[derive(Debug, Serialize)]
struct WebContext {
    server_dir: String,
    www: String,
    logs: String,
    tmp: String,
    http_port: u16,
    https_port: u16,
    upstreams: Vec<Upstream>,
    /// Upstream PHP du site par défaut (localhost) ; vide si PHP non installé.
    default_upstream: String,
    sites: Vec<SiteCtx>,
    /// Dossier du lanceur Adminer (etc/adminer) si Adminer est installé, sinon vide.
    adminer_dir: String,
    /// HTTPS actif (autorité locale créée).
    https: bool,
    localhost_cert: Option<CertCtx>,
}

pub struct Stack<'a> {
    pub paths: &'a Paths,
    pub settings: &'a Settings,
    pub installed: &'a [InstalledPackage],
}

impl<'a> Stack<'a> {
    pub fn new(paths: &'a Paths, settings: &'a Settings, installed: &'a [InstalledPackage]) -> Self {
        Self { paths, settings, installed }
    }

    /// Version demandée si installée, sinon la plus récente installée.
    pub fn pick(&self, package: &str, wanted: Option<&str>) -> Result<&'a InstalledPackage> {
        let candidats = self.installed.iter().filter(|p| p.package == package);
        if let Some(w) = wanted {
            if let Some(p) = self.installed.iter().find(|p| p.package == package && p.version == w) {
                return Ok(p);
            }
            tracing::warn!(package, version = w, "version choisie absente, utilisation de la plus récente");
        }
        // MariaDB : chaque branche a son propre dossier de données. Installer une nouvelle
        // branche ne doit pas faire basculer silencieusement sur une base VIDE : on garde la
        // version la plus récente dont le dossier de données existe déjà.
        if package == MARIADB {
            let avec_donnees = self
                .installed
                .iter()
                .filter(|p| p.package == MARIADB && self.mariadb_datadir(p).join("mysql").is_dir())
                .max_by(|a, b| version::cmp(&a.version, &b.version));
            if let Some(p) = avec_donnees {
                return Ok(p);
            }
        }
        candidats
            .max_by(|a, b| version::cmp(&a.version, &b.version))
            .ok_or_else(|| Error::AucuneVersion { id: package.into() })
    }

    /// Versions de PHP installées.
    pub fn php_installed(&self) -> impl Iterator<Item = &'a InstalledPackage> + 'a {
        self.installed.iter().filter(|p| p.package == "php")
    }

    /// Version **active** d'une branche : la version épinglée si elle est installée,
    /// sinon la plus récente installée de la branche.
    /// Une seule version par branche tourne à la fois (les ports dépendent de la branche).
    pub fn php_for_branch(&self, branch: &str) -> Option<&'a InstalledPackage> {
        let dans_branche = || self.php_installed().filter(move |p| version::branch(&p.version) == branch);
        if let Some(pin) = self.settings.php_pins.get(branch) {
            if let Some(p) = dans_branche().find(|p| &p.version == pin) {
                return Some(p);
            }
        }
        dans_branche().max_by(|a, b| version::cmp(&a.version, &b.version))
    }

    /// Résout un choix « 8.3 » ou « 8.3.37 » vers la version active de la branche.
    pub fn resolve_php(&self, choix: &str) -> Option<&'a InstalledPackage> {
        self.php_for_branch(&version::branch(choix))
    }

    /// PHP par défaut (None si aucun PHP installé : le serveur web sert alors du statique).
    pub fn default_php(&self) -> Option<&'a InstalledPackage> {
        self.settings
            .versions
            .php
            .as_deref()
            .and_then(|c| self.resolve_php(c))
            .or_else(|| self.php_installed().max_by(|a, b| version::cmp(&a.version, &b.version)))
            .and_then(|p| self.php_for_branch(&version::branch(&p.version)))
    }

    /// PHP d'un site : son choix s'il est installé, sinon le PHP par défaut.
    pub fn site_php(&self, site: &str) -> Option<&'a InstalledPackage> {
        self.settings.site_php.get(site).and_then(|c| self.resolve_php(c)).or_else(|| self.default_php())
    }

    /// Versions de PHP nécessaires : la version par défaut + celles des sites existants.
    pub fn php_needed(&self) -> Vec<&'a InstalledPackage> {
        let mut v: Vec<&InstalledPackage> = self.default_php().into_iter().collect();
        if let Ok(sites) = sites::discover(self.paths, &self.settings.tld) {
            for s in sites {
                if let Some(p) = self.site_php(&s.name) {
                    if !v.iter().any(|x| x.version == p.version) {
                        v.push(p);
                    }
                }
            }
        }
        v.sort_by(|a, b| version::cmp(&b.version, &a.version));
        v
    }

    /// Identifiants de tous les processus php-cgi nécessaires.
    pub fn php_service_ids(&self) -> Vec<String> {
        self.php_needed()
            .iter()
            .flat_map(|p| (0..self.settings.php_pool_size as usize).map(move |n| php_service_id(&p.version, n)))
            .collect()
    }

    /// Specs du pool php-cgi d'une version (php.ini créé au besoin).
    pub fn php_specs(&self, php: &InstalledPackage) -> Result<Vec<ServiceSpec>> {
        let ini = php::ensure_php_ini(self.paths, &php.path, &php.version, self.settings)?;
        let exe = php.path.join(platform::exe_name("php-cgi"));
        // Le dossier PHP en tête du PATH : les DLL dépendantes (libssl, libsodium…) y sont.
        let path_env = format!("{};{}", php.path.display(), std::env::var("PATH").unwrap_or_default());
        Ok(pool_ports(&php.version, self.settings.php_pool_size)
            .into_iter()
            .enumerate()
            .map(|(n, port)| {
                let mut s = ServiceSpec::new(
                    php_service_id(&php.version, n),
                    &exe,
                    self.paths.logs().join(format!("php-{}-cgi.log", php.version)),
                );
                s.args = vec!["-b".into(), format!("127.0.0.1:{port}"), "-c".into(), ini.display().to_string()];
                s.env = vec![
                    // Par défaut php-cgi s'arrête après 500 requêtes : on désactive cette limite.
                    ("PHP_FCGI_MAX_REQUESTS".into(), "0".into()),
                    ("PHPRC".into(), ini.parent().unwrap().display().to_string()),
                    ("PATH".into(), path_env.clone()),
                ];
                s.ports = vec![port];
                s.stop = StopStrategy::Kill;
                s
            })
            .collect())
    }

    fn web_context(&self, server: &InstalledPackage) -> Result<WebContext> {
        let phps = self.php_needed();
        let upstreams = phps
            .iter()
            .map(|p| Upstream { name: upstream_name(&p.version), ports: pool_ports(&p.version, self.settings.php_pool_size) })
            .collect();
        let default = self.default_php();
        // HTTPS dès que l'autorité locale existe : certificats créés ou renouvelés au besoin.
        let ca = crate::ssl::load_ca(self.paths)?;
        let cert_pour = |host: &str| -> Result<Option<CertCtx>> {
            match &ca {
                Some(ca) => Ok(Some(crate::ssl::ensure_site_cert(self.paths, ca, host)?.into())),
                None => Ok(None),
            }
        };
        let localhost_cert = cert_pour("localhost")?;
        let sites = sites::discover(self.paths, &self.settings.tld)?
            .into_iter()
            .filter_map(|s| {
                let php = self.site_php(&s.name)?;
                let cert = match cert_pour(&s.host) {
                    Ok(c) => c,
                    Err(e) => return Some(Err(e)),
                };
                Some(Ok(SiteCtx {
                    cert,
                    dir_name: s.dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                    dir: slash(&s.dir),
                    name: s.name,
                    host: s.host,
                    root: slash(&s.root),
                    upstream: upstream_name(&php.version),
                    php_version: php.version.clone(),
                }))
            })
            .collect::<Result<Vec<_>>>()?;
        let adminer_dir = self.prepare_adminer()?.map(|d| slash(&d)).unwrap_or_default();
        Ok(WebContext {
            adminer_dir,
            https: localhost_cert.is_some(),
            localhost_cert,
            server_dir: slash(&server.path),
            www: slash(&self.paths.www()),
            logs: slash(&self.paths.logs()),
            tmp: slash(&self.paths.tmp()),
            http_port: self.settings.ports.http,
            https_port: self.settings.ports.https,
            upstreams,
            default_upstream: default.map(|p| upstream_name(&p.version)).unwrap_or_default(),
            sites,
        })
    }

    /// Génère `etc/adminer/index.php` (connexion automatique au MariaDB local) si
    /// Adminer est installé. Renvoie le dossier à servir sous /adminer/.
    pub fn prepare_adminer(&self) -> Result<Option<PathBuf>> {
        let Ok(pkg) = self.pick("adminer", None) else { return Ok(None) };
        let dir = self.paths.etc().join("adminer");
        let ctx = serde_json::json!({
            "port": self.settings.ports.mariadb,
            "adminer_php": slash(&pkg.path.join("adminer.php")),
        });
        write(&dir.join("index.php"), &templates::render(self.paths, templates::ADMINER_PHP, ctx)?)?;
        Ok(Some(dir))
    }

    /// Ports du serveur web : HTTP, plus HTTPS si l'autorité locale existe.
    pub fn web_ports(&self) -> Result<Vec<u16>> {
        let mut v = vec![self.settings.ports.http];
        if crate::ssl::load_ca(self.paths)?.is_some() {
            v.push(self.settings.ports.https);
        }
        Ok(v)
    }

    /// Serveur web choisi dans les réglages.
    pub fn web_server(&self) -> Result<ServiceSpec> {
        match self.settings.web_server {
            WebServer::Nginx => self.nginx(),
            WebServer::Apache => self.apache(),
        }
    }

    /// Génère `etc/nginx/nginx.conf`, le valide et renvoie la spec.
    pub fn nginx(&self) -> Result<ServiceSpec> {
        let pkg = self.pick("nginx", self.settings.versions.nginx.as_deref())?;
        let conf = self.paths.etc().join("nginx").join("nginx.conf");
        for d in ["client_body", "proxy", "fastcgi", "uwsgi", "scgi"] {
            mkdir(&self.paths.tmp().join("nginx").join(d))?;
        }
        mkdir(&self.paths.logs().join("sites"))?;
        write(&conf, &templates::render(self.paths, templates::NGINX_CONF, self.web_context(pkg)?)?)?;

        let exe = pkg.path.join(platform::exe_name("nginx"));
        // -p : préfixe = racine portable (nginx y cherche logs/ au démarrage).
        let base = vec!["-p".to_string(), format!("{}/", slash(self.paths.root())), "-c".into(), slash(&conf)];
        check_config(NGINX, &exe, &[base.clone(), vec!["-t".into()]].concat(), &pkg.path)?;

        let mut s = ServiceSpec::new(NGINX, &exe, self.paths.logs().join("nginx.log"));
        s.args = base.clone();
        s.cwd = pkg.path.clone();
        s.ports = self.web_ports()?;
        s.stop = StopStrategy::Command { program: exe, args: [base, vec!["-s".into(), "quit".into()]].concat() };
        s.stop_timeout = Duration::from_secs(5);
        Ok(s)
    }

    /// Génère `etc/apache/httpd.conf`, le valide et renvoie la spec.
    pub fn apache(&self) -> Result<ServiceSpec> {
        let pkg = self.pick("apache", self.settings.versions.apache.as_deref())?;
        let conf = self.paths.etc().join("apache").join("httpd.conf");
        mkdir(&self.paths.tmp().join("apache"))?;
        mkdir(&self.paths.logs().join("sites"))?;
        write(&conf, &templates::render(self.paths, templates::HTTPD_CONF, self.web_context(pkg)?)?)?;

        let exe = pkg.path.join("bin").join(platform::exe_name("httpd"));
        let base = vec!["-f".to_string(), slash(&conf)];
        check_config(APACHE, &exe, &[base.clone(), vec!["-t".into()]].concat(), &pkg.path)?;

        let mut s = ServiceSpec::new(APACHE, &exe, self.paths.logs().join("apache.log"));
        s.args = base;
        s.cwd = pkg.path.clone();
        s.ports = self.web_ports()?;
        // Apache (mode console Windows) surveille l'événement « ap<pid>_shutdown ».
        s.stop = StopStrategy::NamedEvent("ap{pid}_shutdown".into());
        Ok(s)
    }
}

impl<'a> Stack<'a> {
    /// Dossier de données MariaDB : un par branche (12.3, 11.4…), car une
    /// montée de version majeure nécessite une migration (mariadb-upgrade).
    pub fn mariadb_datadir(&self, pkg: &InstalledPackage) -> PathBuf {
        self.paths.data().join(format!("mariadb-{}", version::branch(&pkg.version)))
    }

    // ------------------------------------------------------------ MariaDB multi-versions
    //
    // Comme pour PHP, on raisonne par **branche** (« 11.4 ») : chaque branche a son dossier
    // de données (`data/mariadb-11.4`), son port et son service. L'instance **par défaut**
    // garde le port des réglages (3306) et le nom de service historique « mariadb ».

    /// MariaDB par défaut (celle qui contient les données si aucun choix explicite).
    pub fn default_mariadb(&self) -> Result<&'a InstalledPackage> {
        self.pick(MARIADB, self.settings.versions.mariadb.as_deref())
    }

    /// Version installée la plus récente d'une branche de MariaDB.
    pub fn mariadb_for_branch(&self, branch: &str) -> Option<&'a InstalledPackage> {
        self.installed
            .iter()
            .filter(|p| p.package == MARIADB && version::branch(&p.version) == branch)
            .max_by(|a, b| version::cmp(&a.version, &b.version))
    }

    /// MariaDB d'un site : son choix s'il est installé, sinon la version par défaut.
    pub fn site_mariadb(&self, site: &str) -> Result<&'a InstalledPackage> {
        match self.settings.site_mariadb.get(site).and_then(|b| self.mariadb_for_branch(&version::branch(b))) {
            Some(p) => Ok(p),
            None => self.default_mariadb(),
        }
    }

    fn est_defaut(&self, pkg: &InstalledPackage) -> bool {
        self.default_mariadb().is_ok_and(|d| version::branch(&d.version) == version::branch(&pkg.version))
    }

    /// Port d'une instance : celui des réglages pour la version par défaut, sinon
    /// « 3 » + majeure + mineure sur deux chiffres (11.4 → 31104, 10.11 → 31011).
    pub fn mariadb_port(&self, pkg: &InstalledPackage) -> u16 {
        if self.est_defaut(pkg) {
            return self.settings.ports.mariadb;
        }
        let (n, _) = version::parse(&pkg.version);
        (30000 + n.first().copied().unwrap_or(0) * 100 + n.get(1).copied().unwrap_or(0).min(99)) as u16
    }

    /// Nom de l'instance (service, journaux, dossier temporaire) : « mariadb » ou « mariadb-11.4 ».
    pub fn mariadb_instance(&self, pkg: &InstalledPackage) -> String {
        if self.est_defaut(pkg) { MARIADB.to_string() } else { format!("{MARIADB}-{}", version::branch(&pkg.version)) }
    }

    /// Versions de MariaDB nécessaires : la version par défaut + celles des sites existants.
    pub fn mariadb_needed(&self) -> Vec<&'a InstalledPackage> {
        let mut v: Vec<&InstalledPackage> = self.default_mariadb().into_iter().collect();
        if let Ok(sites) = sites::discover(self.paths, &self.settings.tld) {
            for s in sites {
                if let Ok(p) = self.site_mariadb(&s.name) {
                    if !v.iter().any(|x| version::branch(&x.version) == version::branch(&p.version)) {
                        v.push(p);
                    }
                }
            }
        }
        v
    }

    /// Client `mariadb.exe` d'une instance.
    pub fn mariadb_client(&self, pkg: &InstalledPackage) -> Result<PathBuf> {
        let bin = pkg.path.join("bin");
        ["mariadb", "mysql"]
            .iter()
            .map(|n| bin.join(platform::exe_name(n)))
            .find(|p| p.is_file())
            .ok_or_else(|| Error::ExecutableIntrouvable { chemin: bin.join(platform::exe_name("mariadb")) })
    }

    /// Spec de la MariaDB par défaut.
    pub fn mariadb(&self) -> Result<ServiceSpec> {
        self.mariadb_spec(self.default_mariadb()?)
    }

    /// Specs de toutes les instances nécessaires.
    pub fn mariadb_specs(&self) -> Result<Vec<ServiceSpec>> {
        self.mariadb_needed().into_iter().map(|p| self.mariadb_spec(p)).collect()
    }

    /// Génère `etc/mariadb/my.ini` (défaut) ou `etc/mariadb/my-<branche>.ini`, initialise le
    /// dossier de données au premier lancement (`mariadb-install-db`) et renvoie la spec.
    pub fn mariadb_spec(&self, pkg: &InstalledPackage) -> Result<ServiceSpec> {
        let bin = pkg.path.join("bin");
        let datadir = self.mariadb_datadir(pkg);
        let instance = self.mariadb_instance(pkg);
        let port = self.mariadb_port(pkg);
        let conf = if instance == MARIADB {
            self.paths.etc().join("mariadb").join("my.ini")
        } else {
            self.paths.etc().join("mariadb").join(format!("my-{}.ini", version::branch(&pkg.version)))
        };
        mkdir(&self.paths.tmp().join(&instance))?;

        // Exécutable serveur : « mariadbd » (récent) ou « mysqld » (anciennes versions).
        let server = ["mariadbd", "mysqld"]
            .iter()
            .map(|n| bin.join(platform::exe_name(n)))
            .find(|p| p.is_file())
            .ok_or_else(|| Error::ExecutableIntrouvable { chemin: bin.join(platform::exe_name("mariadbd")) })?;

        if !datadir.join("mysql").is_dir() {
            self.mariadb_init(pkg, &bin, &datadir, port)?;
        }

        let ctx = serde_json::json!({
            "instance": instance,
            "server_dir": slash(&pkg.path),
            "datadir": slash(&datadir),
            "port": port,
            "logs": slash(&self.paths.logs()),
            "tmp": slash(&self.paths.tmp()),
        });
        write(&conf, &templates::render(self.paths, templates::MY_INI, ctx)?)?;

        let mut s = ServiceSpec::new(&instance, &server, self.paths.logs().join(format!("{instance}.log")));
        // --defaults-file doit être le premier argument.
        s.args = vec![format!("--defaults-file={}", slash(&conf))];
        s.cwd = pkg.path.clone();
        s.ports = vec![port];
        // mariadbd (Windows) surveille l'événement « MySQLShutdown<pid> » : arrêt propre,
        // indispensable pour ne pas abîmer les bases.
        s.stop = StopStrategy::NamedEvent("MySQLShutdown{pid}".into());
        s.stop_timeout = Duration::from_secs(30);
        s.start_timeout = Duration::from_secs(60);
        Ok(s)
    }

    /// Crée le dossier de données (utilisateur root sans mot de passe, accès local uniquement).
    fn mariadb_init(&self, pkg: &InstalledPackage, bin: &Path, datadir: &Path, port: u16) -> Result<()> {
        let exe = ["mariadb-install-db", "mysql_install_db"]
            .iter()
            .map(|n| bin.join(platform::exe_name(n)))
            .find(|p| p.is_file())
            .ok_or_else(|| Error::ExecutableIntrouvable { chemin: bin.join(platform::exe_name("mariadb-install-db")) })?;
        tracing::info!(datadir = %datadir.display(), "initialisation du dossier de données MariaDB");
        if let Some(parent) = datadir.parent() {
            mkdir(parent)?;
        }
        let args = vec![format!("--datadir={}", datadir.display()), format!("--port={port}")];
        if let Err(e) = check_config(MARIADB, &exe, &args, &pkg.path) {
            // Un dossier à moitié créé bloquerait les essais suivants.
            let _ = std::fs::remove_dir_all(datadir);
            return Err(match e {
                Error::ConfigInvalide { detail, .. } => Error::BaseInit { detail },
                autre => autre,
            });
        }
        Ok(())
    }
}

fn mkdir(p: &Path) -> Result<()> {
    std::fs::create_dir_all(p).map_err(|e| Error::fichier(p, e))
}

fn write(p: &Path, content: &str) -> Result<()> {
    if let Some(d) = p.parent() {
        mkdir(d)?;
    }
    std::fs::write(p, content).map_err(|e| Error::fichier(p, e))
}

/// Lance la vérification de configuration du serveur (`-t`) et renvoie sa sortie en cas d'échec.
fn check_config(service: &str, exe: &Path, args: &[String], cwd: &PathBuf) -> Result<()> {
    if !exe.is_file() {
        return Err(Error::ExecutableIntrouvable { chemin: exe.to_path_buf() });
    }
    let mut cmd = std::process::Command::new(exe);
    cmd.args(args).current_dir(cwd);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(platform::CREATE_NO_WINDOW);
    }
    let out = cmd.output().map_err(|e| Error::ServiceDemarrage { service: service.into(), detail: e.to_string(), journal: String::new() })?;
    if out.status.success() {
        return Ok(());
    }
    let detail = format!("{}{}", String::from_utf8_lossy(&out.stderr), String::from_utf8_lossy(&out.stdout)).trim().to_string();
    Err(Error::ConfigInvalide { service: service.into(), detail })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pkg(package: &str, version: &str, root: &Path) -> InstalledPackage {
        InstalledPackage {
            package: package.into(),
            version: version.into(),
            sha256: String::new(),
            installed_at: 0,
            path: root.join("bin").join(package).join(format!("{package}-{version}")),
        }
    }

    #[test]
    fn choisit_la_version() {
        let t = tempfile::tempdir().unwrap();
        let paths = Paths::new(t.path());
        let installed = vec![pkg("php", "8.3.9", t.path()), pkg("php", "8.3.15", t.path()), pkg("nginx", "1.30.7", t.path())];
        let mut settings = Settings::default();
        let st = Stack::new(&paths, &settings, &installed);
        assert_eq!(st.pick("php", None).unwrap().version, "8.3.15");
        assert_eq!(st.pick("php", Some("8.3.9")).unwrap().version, "8.3.9");
        assert_eq!(st.pick("php", Some("5.6.0")).unwrap().version, "8.3.15", "repli sur la plus récente");
        assert_eq!(st.pick("mariadb", None).unwrap_err().code(), "service.aucune_version");
        // Le choix « 8.3.9 » désigne la branche 8.3 → version la plus récente…
        settings.versions.php = Some("8.3.9".into());
        assert_eq!(Stack::new(&paths, &settings, &installed).default_php().unwrap().version, "8.3.15");
        // … sauf si une version est épinglée pour la branche.
        settings.php_pins.insert("8.3".into(), "8.3.9".into());
        assert_eq!(Stack::new(&paths, &settings, &installed).default_php().unwrap().version, "8.3.9");
    }

    #[test]
    fn mariadb_par_site_ports_et_instances() {
        let t = tempfile::tempdir().unwrap();
        let paths = Paths::new(t.path());
        paths.ensure_layout().unwrap();
        for s in ["prod", "neuf"] {
            std::fs::create_dir_all(paths.www().join(s)).unwrap();
        }
        let installed = vec![pkg("mariadb", "12.3.4", t.path()), pkg("mariadb", "11.4.14", t.path()), pkg("mariadb", "10.11.15", t.path())];
        let mut settings = Settings::default();
        settings.versions.mariadb = Some("12.3.4".into());
        settings.site_mariadb.insert("prod".into(), "11.4".into());
        settings.site_mariadb.insert("disparu".into(), "10.11".into());
        let st = Stack::new(&paths, &settings, &installed);

        assert_eq!(st.site_mariadb("prod").unwrap().version, "11.4.14");
        assert_eq!(st.site_mariadb("neuf").unwrap().version, "12.3.4");
        let needed: Vec<_> = st.mariadb_needed().iter().map(|p| p.version.clone()).collect();
        assert_eq!(needed, ["12.3.4", "11.4.14"], "10.11 n'est utilisée par aucun site existant");
        assert_eq!(st.mariadb_port(&installed[0]), 3306);
        assert_eq!(st.mariadb_port(&installed[1]), 31104);
        assert_eq!(st.mariadb_port(&installed[2]), 31011);
        assert_eq!(st.mariadb_instance(&installed[0]), "mariadb");
        assert_eq!(st.mariadb_instance(&installed[1]), "mariadb-11.4");
    }

    #[test]
    fn mariadb_reste_sur_la_version_qui_a_les_donnees() {
        let t = tempfile::tempdir().unwrap();
        let paths = Paths::new(t.path());
        let installed = vec![pkg("mariadb", "12.3.4", t.path()), pkg("mariadb", "13.0.2", t.path())];
        let settings = Settings::default();
        let st = Stack::new(&paths, &settings, &installed);
        assert_eq!(st.pick(MARIADB, None).unwrap().version, "13.0.2", "aucune donnée : la plus récente");
        std::fs::create_dir_all(paths.data().join("mariadb-12.3").join("mysql")).unwrap();
        assert_eq!(st.pick(MARIADB, None).unwrap().version, "12.3.4", "pas de bascule vers une base vide");
        assert_eq!(st.pick(MARIADB, Some("13.0.2")).unwrap().version, "13.0.2", "choix explicite respecté");
    }

    #[test]
    fn php_par_site_et_versions_necessaires() {
        let t = tempfile::tempdir().unwrap();
        let paths = Paths::new(t.path());
        paths.ensure_layout().unwrap();
        for s in ["ancien", "moderne", "autre"] {
            std::fs::create_dir_all(paths.www().join(s)).unwrap();
        }
        let installed = vec![pkg("php", "7.4.33", t.path()), pkg("php", "8.3.37", t.path()), pkg("php", "8.2.36", t.path())];
        let mut settings = Settings::default();
        settings.versions.php = Some("8.3".into());
        settings.site_php.insert("ancien".into(), "7.4".into());
        settings.site_php.insert("autre".into(), "5.6".into()); // non installée → défaut
        settings.site_php.insert("supprime".into(), "8.2".into()); // site absent → ignoré
        let st = Stack::new(&paths, &settings, &installed);

        assert_eq!(st.site_php("ancien").unwrap().version, "7.4.33");
        assert_eq!(st.site_php("moderne").unwrap().version, "8.3.37");
        assert_eq!(st.site_php("autre").unwrap().version, "8.3.37");
        let needed: Vec<_> = st.php_needed().iter().map(|p| p.version.clone()).collect();
        assert_eq!(needed, ["8.3.37", "7.4.33"], "8.2 n'est utilisé par aucun site existant");
        assert_eq!(st.php_service_ids().len(), 8);

        // Le gabarit route chaque site (domaine et /dossier/) vers son PHP.
        let n = templates::render(&paths, templates::NGINX_CONF, st.web_context(&pkg("nginx", "1.30.7", t.path())).unwrap()).unwrap();
        assert!(n.contains("upstream php_7_4_33 {") && n.contains("server 127.0.0.1:9740;"));
        assert!(n.contains("location ^~ /ancien/ {"));
        let bloc_ancien = &n[n.find("location ^~ /ancien/").unwrap()..];
        assert!(bloc_ancien[..bloc_ancien.find("location ^~ /autre/").unwrap_or(bloc_ancien.len())].contains("fastcgi_pass php_7_4_33;"));
    }

    #[test]
    fn gabarits_nginx_et_apache_complets() {
        let t = tempfile::tempdir().unwrap();
        let paths = Paths::new(t.path());
        paths.ensure_layout().unwrap();
        std::fs::create_dir_all(paths.www().join("blog")).unwrap();
        let installed = vec![pkg("php", "8.3.37", t.path()), pkg("nginx", "1.30.7", t.path())];
        let settings = Settings::default();
        let st = Stack::new(&paths, &settings, &installed);
        let ctx = st.web_context(&installed[1]).unwrap();

        let n = templates::render(&paths, templates::NGINX_CONF, &ctx).unwrap();
        assert!(n.contains("upstream php_8_3_37 {"), "{n}");
        assert!(n.contains("server 127.0.0.1:9830;") && n.contains("server 127.0.0.1:9833;"));
        assert!(n.contains("server_name blog.test *.blog.test;"));
        assert!(n.contains("fastcgi_pass php_8_3_37;"));
        assert!(!n.contains(":\\"), "chemin Windows avec antislash dans nginx.conf");

        let a = templates::render(&paths, templates::HTTPD_CONF, &ctx).unwrap();
        // Le « / » final est indispensable avec SetHandler (sinon « No protocol handler »).
        assert!(a.contains("<Proxy \"balancer://php_8_3_37/\">"));
        assert!(a.contains("SetHandler \"proxy:balancer://php_8_3_37/\""));
        assert!(a.contains("BalancerMember \"fcgi://127.0.0.1:9831\""));
        assert!(a.contains("ServerName blog.test"));
    }
}
