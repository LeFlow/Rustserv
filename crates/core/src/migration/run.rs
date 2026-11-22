//! Exécution d'un import Laragon/WAMP → Rustserv.
//!
//! Garanties :
//! - les fichiers et données d'origine ne sont **jamais modifiés** : les sites sont copiés,
//!   et les bases sont exportées depuis une **copie** des données MySQL, démarrée
//!   temporairement sur un port libre avec le MySQL de l'outil d'origine ;
//! - un site en erreur n'interrompt pas les autres.

use super::detect::{wp_define, DbServer, Source, SourceKind, TypeSite};
use crate::catalog::{version, Catalog, InstalledPackage, Installer};
use crate::error::{Error, Result};
use crate::paths::Paths;
use crate::platform;
use crate::services::Stack;
use crate::settings::Settings;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

#[derive(Debug, Clone, Deserialize)]
pub struct ImportItem {
    pub source: SourceKind,
    pub dossier: PathBuf,
    /// Nom du site dans Rustserv (dossier www/<nom>).
    pub nom: String,
    pub type_site: TypeSite,
    pub base: Option<String>,
    /// Branche de PHP du site (« 8.1 ») ; None = PHP par défaut.
    pub php: Option<String>,
    /// WordPress : convertir les adresses vers <nom>.<tld>.
    pub convertir_url: bool,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Etape {
    Preparation,
    Export,
    Copie,
    Import,
    Configuration,
    Adresses,
    Termine,
    Erreur,
}

#[derive(Debug, Clone, Serialize)]
pub struct Progression {
    pub site: String,
    pub etape: Etape,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ResultatSite {
    pub nom: String,
    pub ok: bool,
    pub erreur: Option<serde_json::Value>,
    pub base: Option<String>,
    pub ancienne_url: Option<String>,
    pub nouvelle_url: Option<String>,
}

type OnProgress<'a> = &'a (dyn Fn(Progression) + Send + Sync);

fn echec(detail: impl Into<String>) -> Error {
    Error::Migration { detail: detail.into() }
}

/// Détail d'une erreur sans le préfixe « import : » (évite les messages en double).
fn detail_de(e: &Error) -> String {
    match e {
        Error::Migration { detail } => detail.clone(),
        autre => autre.to_string(),
    }
}

// ---------------------------------------------------------------- fichiers

/// Copie récursive d'un dossier (les liens symboliques sont ignorés).
pub fn copy_dir(src: &Path, dst: &Path, filtre: &dyn Fn(&str) -> bool) -> Result<u64> {
    std::fs::create_dir_all(dst).map_err(|e| Error::fichier(dst, e))?;
    let mut n = 0;
    for e in std::fs::read_dir(src).map_err(|e| Error::fichier(src, e))?.flatten() {
        let ft = e.file_type().map_err(|er| Error::fichier(e.path(), er))?;
        let nom = e.file_name().to_string_lossy().into_owned();
        let cible = dst.join(&nom);
        if ft.is_dir() {
            n += copy_dir(&e.path(), &cible, filtre)?;
        } else if ft.is_file() && filtre(&nom) {
            std::fs::copy(e.path(), &cible).map_err(|er| Error::fichier(&cible, er))?;
            n += 1;
        }
    }
    Ok(n)
}

/// Fichiers inutiles pour un export SQL (journaux binaires : souvent plusieurs Go).
fn fichier_de_donnees_utile(nom: &str) -> bool {
    let n = nom.to_ascii_lowercase();
    !(n.starts_with("binlog.") || n.starts_with("mysql-bin.") || n.contains("-bin.0") || n.ends_with(".err"))
}

// ---------------------------------------------------------------- MySQL d'origine (copie temporaire)

struct ServeurTemporaire {
    child: tokio::process::Child,
    /// MySQL 8+ sous Windows lance un processus « moniteur » qui démarre le vrai serveur :
    /// arrêter le premier laisserait le second orphelin. Le Job arrête toute l'arborescence.
    job: Option<platform::JobObject>,
    port: u16,
    copie: PathBuf,
}

impl ServeurTemporaire {
    async fn demarrer(s: &DbServer, travail: &Path) -> Result<Self> {
        let copie = travail.join("donnees");
        let _ = std::fs::remove_dir_all(&copie);
        let (src, dst) = (s.data.clone(), copie.clone());
        tokio::task::spawn_blocking(move || copy_dir(&src, &dst, &fichier_de_donnees_utile)).await.map_err(|e| echec(e.to_string()))??;
        let port = std::net::TcpListener::bind("127.0.0.1:0")?.local_addr()?.port();
        let exe = ["mariadbd.exe", "mysqld.exe"].iter().map(|n| s.bin.join(n)).find(|p| p.is_file()).ok_or_else(|| echec(format!("serveur introuvable dans {}", s.bin.display())))?;
        let mut args = vec![
            "--no-defaults".to_string(),
            format!("--datadir={}", copie.display()),
            format!("--port={port}"),
            "--bind-address=127.0.0.1".into(),
            "--console".into(),
        ];
        if s.mariadb {
            args.push("--skip-log-bin".into());
        } else {
            // MySQL 8+ : pas de X Protocol (port 33060) ni de journal binaire.
            args.push("--mysqlx=OFF".into());
            args.push("--disable-log-bin".into());
        }
        // Mot de passe root inconnu (ex. WAMP) : vidé au démarrage, sur la COPIE uniquement.
        let init = travail.join("init.sql");
        std::fs::write(
            &init,
            "ALTER USER IF EXISTS 'root'@'localhost' IDENTIFIED BY '';
ALTER USER IF EXISTS 'root'@'127.0.0.1' IDENTIFIED BY '';
ALTER USER IF EXISTS 'root'@'::1' IDENTIFIED BY '';
",
        )?;
        args.push(format!("--init-file={}", init.display()));
        let log = std::fs::File::create(travail.join("serveur.log"))?;
        let mut cmd = tokio::process::Command::new(&exe);
        cmd.args(&args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::from(log)).kill_on_drop(true);
        let job = platform::JobObject::new()?;
        let mut child = platform::spawn_in_job(&job, &mut cmd)?;
        let mut job = Some(job);
        // Jusqu'à 5 min : sur de gros volumes, l'initialisation d'InnoDB peut être longue.
        for _ in 0..600 {
            if crate::process::ports::is_listening(port) {
                return Ok(Self { child, job, port, copie });
            }
            if let Ok(Some(st)) = child.try_wait() {
                let journal = crate::process::logs::tail(&travail.join("serveur.log"), 8);
                return Err(echec(format!("{} n'a pas démarré (code {:?}) :\n{journal}", s.label, st.code())));
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        let _ = child.kill().await;
        drop(job.take());
        Err(echec(format!("{} : délai de démarrage dépassé", s.label)))
    }

    async fn arreter(mut self) {
        // Fermer le Job tue le serveur et le processus moniteur.
        drop(self.job.take());
        let _ = self.child.kill().await;
        let _ = self.child.wait().await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        let _ = std::fs::remove_dir_all(&self.copie);
    }
}

async fn executer(exe: &Path, args: &[String], stdin: Option<&Path>, delai: u64) -> Result<(bool, String, String)> {
    let mut cmd = tokio::process::Command::new(exe);
    cmd.args(args).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    match stdin {
        Some(f) => cmd.stdin(Stdio::from(std::fs::File::open(f).map_err(|e| Error::fichier(f, e))?)),
        None => cmd.stdin(Stdio::null()),
    };
    cmd.creation_flags(platform::CREATE_NO_WINDOW);
    let out = tokio::time::timeout(Duration::from_secs(delai), cmd.output())
        .await
        .map_err(|_| echec(format!("{} : délai dépassé", exe.display())))??;
    Ok((out.status.success(), String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned()))
}

async fn exporter(s: &DbServer, port: u16, base: &str, fichier: &Path) -> Result<()> {
    let exe = ["mariadb-dump.exe", "mysqldump.exe"].iter().map(|n| s.bin.join(n)).find(|p| p.is_file()).ok_or_else(|| echec("mysqldump introuvable"))?;
    let mut args = vec![
        "--host=127.0.0.1".to_string(),
        format!("--port={port}"),
        "--user=root".into(),
        "--routines".into(),
        "--triggers".into(),
        "--events".into(),
        "--single-transaction".into(),
        "--default-character-set=utf8mb4".into(),
        "--no-tablespaces".into(),
        format!("--result-file={}", fichier.display()),
    ];
    if !s.mariadb {
        args.push("--set-gtid-purged=OFF".into());
    }
    args.push(base.into());
    let (ok, _, err) = executer(&exe, &args, None, 1800).await?;
    if !ok {
        return Err(echec(format!("export de « {base} » : {}", err.trim())));
    }
    Ok(())
}

// ---------------------------------------------------------------- MariaDB de Rustserv

struct Cible {
    client: PathBuf,
    port: u16,
}

impl Cible {
    fn base_args(&self) -> Vec<String> {
        vec!["--host=127.0.0.1".into(), format!("--port={}", self.port), "--user=root".into(), "--default-character-set=utf8mb4".into()]
    }

    async fn sql(&self, requete: &str) -> Result<String> {
        let mut a = self.base_args();
        a.extend(["-N".into(), "-B".into(), "-e".into(), requete.into()]);
        let (ok, out, err) = executer(&self.client, &a, None, 120).await?;
        if !ok {
            return Err(echec(err.trim().to_string()));
        }
        Ok(out)
    }

    async fn bases(&self) -> Result<Vec<String>> {
        Ok(self.sql("SHOW DATABASES").await?.lines().map(|l| l.trim().to_string()).collect())
    }

    /// Importe un export SQL dans `base` (créée). Repli automatique si MariaDB ne connaît
    /// pas une collation propre à MySQL 8 (utf8mb4_0900_*).
    async fn importer(&self, base: &str, fichier: &Path) -> Result<()> {
        for essai in 0..2 {
            self.sql(&format!("DROP DATABASE IF EXISTS `{base}`; CREATE DATABASE `{base}` CHARACTER SET utf8mb4 COLLATE utf8mb4_unicode_ci")).await?;
            let mut a = self.base_args();
            a.push(base.into());
            let (ok, _, err) = executer(&self.client, &a, Some(fichier), 3600).await?;
            if ok {
                return Ok(());
            }
            if essai == 0 && err.contains("collation") && err.contains("0900") {
                tracing::warn!(base, "collation MySQL 8 inconnue, conversion de l'export");
                let txt = std::fs::read_to_string(fichier).map_err(|e| Error::fichier(fichier, e))?;
                let txt = convertir_collations(&txt);
                std::fs::write(fichier, txt).map_err(|e| Error::fichier(fichier, e))?;
                continue;
            }
            return Err(echec(format!("import de « {base} » : {}", err.trim())));
        }
        unreachable!()
    }
}

/// Remplace les collations MySQL 8 par leurs équivalents MariaDB.
pub fn convertir_collations(sql: &str) -> String {
    sql.replace("utf8mb4_0900_ai_ci", "utf8mb4_unicode_520_ci")
        .replace("utf8mb4_0900_as_ci", "utf8mb4_unicode_520_ci")
        .replace("utf8mb4_0900_as_cs", "utf8mb4_bin")
        .replace("utf8mb4_0900_bin", "utf8mb4_bin")
}

/// Nom de base libre dans Rustserv (« imprimerie », sinon « imprimerie_2 »…).
fn base_libre(voulu: &str, existantes: &[String]) -> String {
    if !existantes.iter().any(|b| b == voulu) {
        return voulu.to_string();
    }
    (2..).map(|k| format!("{voulu}_{k}")).find(|n| !existantes.contains(n)).unwrap()
}

// ---------------------------------------------------------------- configuration des sites

/// Remplace la valeur d'un `define('CLE', '…')` (les guillemets d'origine sont conservés).
pub fn set_wp_define(texte: &str, cle: &str, valeur: &str) -> String {
    let Some(i) = texte.find(&format!("'{cle}'")).or_else(|| texte.find(&format!("\"{cle}\""))) else { return texte.to_string() };
    let apres_cle = i + cle.len() + 2;
    let Some(v) = texte[apres_cle..].find(',') else { return texte.to_string() };
    let debut_val = apres_cle + v + 1;
    let decalage = texte[debut_val..].len() - texte[debut_val..].trim_start().len();
    let q_pos = debut_val + decalage;
    let Some(q) = texte[q_pos..].chars().next().filter(|c| *c == '\'' || *c == '"') else { return texte.to_string() };
    let Some(fin) = texte[q_pos + 1..].find(q) else { return texte.to_string() };
    format!("{}{q}{}{q}{}", &texte[..q_pos], valeur.replace(q, &format!("\\{q}")), &texte[q_pos + 1 + fin + 1..])
}

/// Remplace (ou ajoute) `CLE=valeur` dans un .env.
pub fn set_env(texte: &str, cle: &str, valeur: &str) -> String {
    let mut trouve = false;
    let eol = if texte.contains("\r\n") { "\r\n" } else { "\n" };
    let mut lignes: Vec<String> = texte
        .lines()
        .map(|l| {
            if l.trim_start().starts_with(&format!("{cle}=")) {
                trouve = true;
                format!("{cle}={valeur}")
            } else {
                l.to_string()
            }
        })
        .collect();
    if !trouve {
        lignes.push(format!("{cle}={valeur}"));
    }
    lignes.join(eol) + eol
}

fn table_prefix(wp_config: &str) -> String {
    wp_config
        .find("$table_prefix")
        .and_then(|i| {
            let r = &wp_config[i..];
            let q = r.find(['\'', '"'])?;
            let qc = r[q..].chars().next()?;
            let fin = r[q + 1..].find(qc)?;
            Some(r[q + 1..q + 1 + fin].to_string())
        })
        .unwrap_or_else(|| "wp_".into())
}

/// Corrige un .htaccess WordPress installé dans un sous-dossier (« RewriteBase /site/ » → « / »).
pub fn corriger_htaccess(texte: &str, ancien_chemin: &str) -> String {
    if ancien_chemin.is_empty() || ancien_chemin == "/" {
        return texte.to_string();
    }
    let p = format!("/{}/", ancien_chemin.trim_matches('/'));
    texte.replace(&format!("RewriteBase {p}"), "RewriteBase /").replace(&format!(" {p}index.php"), " /index.php")
}

/// Sépare « http://localhost/site » en (« localhost/site », « site »).
fn hote_et_chemin(url: &str) -> (String, String) {
    let sans = url.split("://").nth(1).unwrap_or(url).trim_end_matches('/');
    let chemin = sans.split_once('/').map(|(_, c)| c.to_string()).unwrap_or_default();
    (sans.to_string(), chemin)
}

// ---------------------------------------------------------------- orchestration

/// Paquet installé le plus récent (ou installé depuis le catalogue).
async fn outil(installer: &Installer, catalog: &Catalog, id: &str) -> Result<InstalledPackage> {
    if let Some(p) = installer.installed()?.into_iter().filter(|p| p.package == id).max_by(|a, b| version::cmp(&a.version, &b.version)) {
        return Ok(p);
    }
    let integre = Catalog::embedded()?;
    let r = catalog.latest(id, None).or_else(|| integre.latest(id, None)).ok_or_else(|| Error::PaquetInconnu { id: id.into() })?;
    installer.install(id, r, &AtomicBool::new(false), &|_| {}).await
}

/// Refuse l'import si Laragon/WAMP tourne encore (ports pris, données en cours d'écriture).
pub fn verifier_arrete(sources: &[Source]) -> Result<()> {
    for port in [80u16, 443, 3306, 3307] {
        if let Some(o) = platform::port_owner(port) {
            if let Some(chemin) = &o.path {
                if let Some(s) = sources.iter().find(|s| chemin.starts_with(&s.root)) {
                    return Err(Error::SourceActive { outil: s.kind.suffixe().into() });
                }
            }
        }
    }
    Ok(())
}

/// Importe les sites demandés. MariaDB de Rustserv doit être démarré.
/// Renvoie un résultat par site et les réglages à enregistrer (PHP par site).
#[allow(clippy::too_many_arguments)]
pub async fn importer(
    paths: &Paths,
    settings: &Settings,
    installer: &Installer,
    catalog: &Catalog,
    sources: &[Source],
    items: &[ImportItem],
    https: bool,
    on: OnProgress<'_>,
) -> Result<(Vec<ResultatSite>, Settings)> {
    verifier_arrete(sources)?;
    let travail = paths.tmp().join("migration");
    std::fs::create_dir_all(&travail).map_err(|e| Error::fichier(&travail, e))?;
    let installed = installer.installed()?;
    let stack = Stack::new(paths, settings, &installed);
    let mariadb = stack.pick("mariadb", settings.versions.mariadb.as_deref())?;
    let client = ["mariadb.exe", "mysql.exe"].iter().map(|n| mariadb.path.join("bin").join(n)).find(|p| p.is_file()).ok_or_else(|| echec("client MariaDB introuvable"))?;
    let cible = Cible { client, port: settings.ports.mariadb };
    let mut prises = cible.bases().await?;

    // 1. Exports, serveur par serveur (une seule copie temporaire par serveur).
    let mut exports: Vec<(String, PathBuf)> = Vec::new();
    let mut erreurs_export: Vec<(String, String)> = Vec::new();
    for src in sources {
        for s in &src.serveurs {
            let a_exporter: Vec<&ImportItem> = items.iter().filter(|i| i.source == src.kind && i.base.as_ref().is_some_and(|b| s.bases.contains(b))).collect();
            if a_exporter.is_empty() {
                continue;
            }
            for i in &a_exporter {
                on(Progression { site: i.nom.clone(), etape: Etape::Preparation, detail: s.label.clone() });
            }
            let dossier = travail.join(format!("{}-{}", src.kind.suffixe(), s.label.replace(' ', "-")));
            match ServeurTemporaire::demarrer(s, &dossier).await {
                Ok(serveur) => {
                    for i in a_exporter {
                        let base = i.base.clone().unwrap();
                        on(Progression { site: i.nom.clone(), etape: Etape::Export, detail: base.clone() });
                        let f = travail.join(format!("{}.sql", i.nom));
                        match exporter(s, serveur.port, &base, &f).await {
                            Ok(()) => exports.push((i.nom.clone(), f)),
                            Err(e) => erreurs_export.push((i.nom.clone(), detail_de(&e))),
                        }
                    }
                    serveur.arreter().await;
                }
                Err(e) => {
                    for i in a_exporter {
                        erreurs_export.push((i.nom.clone(), detail_de(&e)));
                    }
                }
            }
        }
    }

    // 2. Sites un par un.
    let mut nouveaux = settings.clone();
    let mut resultats = Vec::new();
    for item in items {
        let r = importer_site(paths, &nouveaux, installer, catalog, &cible, &mut prises, item, &exports, &erreurs_export, https, on).await;
        match r {
            Ok(mut res) => {
                if let Some(b) = &item.php {
                    if stack.php_for_branch(b).is_some() {
                        nouveaux.site_php.insert(item.nom.clone(), b.clone());
                    }
                }
                on(Progression { site: item.nom.clone(), etape: Etape::Termine, detail: String::new() });
                res.ok = true;
                resultats.push(res);
            }
            Err(e) => {
                tracing::error!(site = %item.nom, erreur = %e, "import échoué");
                on(Progression { site: item.nom.clone(), etape: Etape::Erreur, detail: e.to_string() });
                resultats.push(ResultatSite { nom: item.nom.clone(), ok: false, erreur: serde_json::to_value(&e).ok(), base: None, ancienne_url: None, nouvelle_url: None });
            }
        }
    }
    for (_, f) in &exports {
        let _ = std::fs::remove_file(f);
    }
    Ok((resultats, nouveaux))
}

#[allow(clippy::too_many_arguments)]
async fn importer_site(
    paths: &Paths,
    settings: &Settings,
    installer: &Installer,
    catalog: &Catalog,
    cible: &Cible,
    prises: &mut Vec<String>,
    item: &ImportItem,
    exports: &[(String, PathBuf)],
    erreurs_export: &[(String, String)],
    https: bool,
    on: OnProgress<'_>,
) -> Result<ResultatSite> {
    let etape = |e, d: String| on(Progression { site: item.nom.clone(), etape: e, detail: d });
    if !crate::sites::valid_name(&item.nom) {
        return Err(echec(format!("nom de site invalide : « {} »", item.nom)));
    }
    if let Some((_, d)) = erreurs_export.iter().find(|(n, _)| n == &item.nom) {
        return Err(echec(d.clone()));
    }
    let dest = paths.www().join(&item.nom);
    if dest.exists() {
        return Err(echec(format!("le dossier www\\{} existe déjà", item.nom)));
    }

    // Fichiers
    etape(Etape::Copie, item.dossier.display().to_string());
    let (src, dst) = (item.dossier.clone(), dest.clone());
    let n = tokio::task::spawn_blocking(move || copy_dir(&src, &dst, &|_| true)).await.map_err(|e| echec(e.to_string()))??;
    tracing::info!(site = %item.nom, fichiers = n, "fichiers copiés");

    // Base
    let mut res = ResultatSite { nom: item.nom.clone(), ok: false, erreur: None, base: None, ancienne_url: None, nouvelle_url: None };
    if let Some((_, dump)) = exports.iter().find(|(n, _)| n == &item.nom) {
        let base = base_libre(item.base.as_deref().unwrap_or(&item.nom), prises);
        etape(Etape::Import, base.clone());
        cible.importer(&base, dump).await?;
        prises.push(base.clone());
        res.base = Some(base);
    }

    // Configuration
    let host = if cible.port == 3306 { "127.0.0.1".to_string() } else { format!("127.0.0.1:{}", cible.port) };
    let domaine = format!("{}.{}", item.nom, settings.tld);
    let nouvelle_url = format!("{}://{domaine}", if https { "https" } else { "http" });
    match item.type_site {
        TypeSite::Wordpress => {
            etape(Etape::Configuration, "wp-config.php".into());
            let f = dest.join("wp-config.php");
            let mut t = std::fs::read_to_string(&f).map_err(|e| Error::fichier(&f, e))?;
            std::fs::write(dest.join("wp-config.php.avant-rustserv"), &t).map_err(|e| Error::fichier(&f, e))?;
            if let Some(b) = &res.base {
                t = set_wp_define(&t, "DB_NAME", b);
            }
            t = set_wp_define(&t, "DB_USER", "root");
            t = set_wp_define(&t, "DB_PASSWORD", "");
            t = set_wp_define(&t, "DB_HOST", &host);
            std::fs::write(&f, &t).map_err(|e| Error::fichier(&f, e))?;

            if item.convertir_url && res.base.is_some() {
                etape(Etape::Adresses, nouvelle_url.clone());
                res.ancienne_url = adapter_adresses(paths, settings, installer, catalog, cible, &dest, item.php.as_deref(), &nouvelle_url).await?;
                res.nouvelle_url = Some(nouvelle_url);
            }
        }
        TypeSite::Laravel => {
            etape(Etape::Configuration, ".env".into());
            let f = dest.join(".env");
            if let Ok(mut t) = std::fs::read_to_string(&f) {
                std::fs::write(dest.join(".env.avant-rustserv"), &t).map_err(|e| Error::fichier(&f, e))?;
                for (k, v) in [
                    ("DB_HOST", "127.0.0.1".to_string()),
                    ("DB_PORT", cible.port.to_string()),
                    ("DB_USERNAME", "root".into()),
                    ("DB_PASSWORD", String::new()),
                    ("APP_URL", nouvelle_url.clone()),
                ] {
                    t = set_env(&t, k, &v);
                }
                if let Some(b) = &res.base {
                    t = set_env(&t, "DB_DATABASE", b);
                }
                std::fs::write(&f, t).map_err(|e| Error::fichier(&f, e))?;
                res.nouvelle_url = Some(nouvelle_url);
            }
        }
        TypeSite::Autre => {}
    }
    Ok(res)
}

/// Fait pointer un site WordPress vers `nouvelle` (ex. https://monsite.test) :
/// - adresses en base (WP-CLI, données sérialisées comprises), depuis l'adresse en base
///   **et** celle des constantes `WP_HOME`/`WP_SITEURL` si elles diffèrent ;
/// - constantes `WP_HOME`/`WP_SITEURL` de wp-config.php (elles priment sur la base) ;
/// - .htaccess d'une ancienne installation en sous-dossier.
/// Renvoie l'ancienne adresse (None si déjà à jour).
#[allow(clippy::too_many_arguments)]
async fn adapter_adresses(
    paths: &Paths,
    settings: &Settings,
    installer: &Installer,
    catalog: &Catalog,
    cible: &Cible,
    dest: &Path,
    php_branche: Option<&str>,
    nouvelle: &str,
) -> Result<Option<String>> {
    let f = dest.join("wp-config.php");
    let mut t = std::fs::read_to_string(&f).map_err(|e| Error::fichier(&f, e))?;
    let base = wp_define(&t, "DB_NAME").ok_or_else(|| echec("DB_NAME absent de wp-config.php"))?;
    let prefix = table_prefix(&t);
    let en_base = cible
        .sql(&format!("SELECT option_value FROM `{base}`.`{prefix}options` WHERE option_name='siteurl'"))
        .await?
        .trim()
        .to_string();
    let constante = wp_define(&t, "WP_HOME");
    let mut anciennes: Vec<String> = [Some(en_base.clone()), constante.clone()]
        .into_iter()
        .flatten()
        .map(|u| u.trim_end_matches('/').to_string())
        .filter(|u| !u.is_empty() && u != nouvelle)
        .collect();
    anciennes.dedup();
    if anciennes.is_empty() {
        return Ok(None);
    }
    // Constantes d'abord : WordPress lit l'« ancienne » valeur de siteurl via WP_SITEURL ;
    // sinon `option update` ne modifie aucune ligne (base déjà convertie) et échoue.
    for cle in ["WP_HOME", "WP_SITEURL"] {
        if wp_define(&t, cle).is_some() {
            t = set_wp_define(&t, cle, nouvelle);
        }
    }
    std::fs::write(&f, &t).map_err(|e| Error::fichier(&f, e))?;
    for ancienne in &anciennes {
        convertir_wordpress(paths, settings, installer, catalog, php_branche, dest, ancienne, nouvelle).await?;
        let (_, chemin) = hote_et_chemin(ancienne);
        let ht = dest.join(".htaccess");
        if let Ok(h) = std::fs::read_to_string(&ht) {
            let _ = std::fs::write(&ht, corriger_htaccess(&h, &chemin));
        }
    }
    // siteurl/home fixés directement en SQL (via WP-CLI, la constante WP_SITEURL fausse la comparaison).
    let v = nouvelle.replace('\\', "\\\\").replace('\'', "\\'");
    cible.sql(&format!("UPDATE `{base}`.`{prefix}options` SET option_value='{v}' WHERE option_name IN ('siteurl','home')")).await?;
    Ok(anciennes.into_iter().next())
}

/// Adapte les adresses d'un site WordPress **déjà présent** dans Rustserv au domaine actuel
/// (après un import ou un changement de domaine). MariaDB doit être démarré.
/// Renvoie (ancienne adresse si changée, nouvelle adresse).
pub async fn adapter_site(
    paths: &Paths,
    settings: &Settings,
    installer: &Installer,
    catalog: &Catalog,
    site: &str,
    https: bool,
) -> Result<(Option<String>, String)> {
    let dest = paths.www().join(site);
    if !dest.join("wp-config.php").is_file() {
        return Err(echec(format!("« {site} » n'est pas un site WordPress")));
    }
    let installed = installer.installed()?;
    let stack = Stack::new(paths, settings, &installed);
    let mariadb = stack.pick("mariadb", settings.versions.mariadb.as_deref())?;
    let client = ["mariadb.exe", "mysql.exe"].iter().map(|n| mariadb.path.join("bin").join(n)).find(|p| p.is_file()).ok_or_else(|| echec("client MariaDB introuvable"))?;
    let cible = Cible { client, port: settings.ports.mariadb };
    let nouvelle = format!("{}://{site}.{}", if https { "https" } else { "http" }, settings.tld);
    let php = stack.site_php(site).map(|p| version::branch(&p.version));
    let ancienne = adapter_adresses(paths, settings, installer, catalog, &cible, &dest, php.as_deref(), &nouvelle).await?;
    Ok((ancienne, nouvelle))
}

/// Remplace les adresses avec WP-CLI (gère les données sérialisées).
#[allow(clippy::too_many_arguments)]
async fn convertir_wordpress(
    paths: &Paths,
    settings: &Settings,
    installer: &Installer,
    catalog: &Catalog,
    php_branche: Option<&str>,
    dest: &Path,
    ancienne: &str,
    nouvelle: &str,
) -> Result<()> {
    let wp = outil(installer, catalog, "wp-cli").await?;
    let installed = installer.installed()?;
    let stack = Stack::new(paths, settings, &installed);
    let php = php_branche.and_then(|b| stack.php_for_branch(b)).or_else(|| stack.default_php()).ok_or_else(|| Error::AucuneVersion { id: "php".into() })?;
    let ini = crate::php::ensure_php_ini(paths, &php.path, &php.version, settings)?;
    let base_args = |extra: &[&str]| -> Vec<String> {
        let mut a = vec!["-c".into(), ini.display().to_string(), wp.path.join("wp.phar").display().to_string(), format!("--path={}", dest.display())];
        a.extend(extra.iter().map(|s| s.to_string()));
        a.extend(["--skip-plugins".into(), "--skip-themes".into(), "--quiet".into()]);
        a
    };
    let exe = php.path.join(platform::exe_name("php"));
    // Remplacement sans le protocole (« //localhost/site ») : couvre http et https.
    let (ancien_hote, _) = hote_et_chemin(ancienne);
    let (nouvel_hote, _) = hote_et_chemin(nouvelle);
    let paires = [
        (format!("//{ancien_hote}"), format!("//{nouvel_hote}")),
        (format!("\\/\\/{}", ancien_hote.replace('/', "\\/")), format!("\\/\\/{nouvel_hote}")),
    ];
    for (a, b) in &paires {
        let args = base_args(&["search-replace", a, b, "--all-tables-with-prefix", "--skip-columns=guid", "--precise"]);
        let (ok, _, err) = executer(&exe, &args, None, 1800).await?;
        if !ok {
            return Err(echec(format!("WP-CLI : {}", err.trim())));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edition_wp_config() {
        let t = "define( 'DB_NAME', 'ancienne' );\ndefine(\"DB_HOST\", \"localhost\");\ndefine('DB_PASSWORD','secret');";
        let t = set_wp_define(t, "DB_NAME", "nouvelle");
        let t = set_wp_define(&t, "DB_HOST", "127.0.0.1:3307");
        let t = set_wp_define(&t, "DB_PASSWORD", "");
        assert_eq!(t, "define( 'DB_NAME', 'nouvelle' );\ndefine(\"DB_HOST\", \"127.0.0.1:3307\");\ndefine('DB_PASSWORD','');");
        assert_eq!(table_prefix("$table_prefix  = 'wpx_';"), "wpx_");
        assert_eq!(table_prefix("rien"), "wp_");
    }

    #[test]
    fn edition_env() {
        let t = set_env("APP_URL=http://localhost\r\nDB_HOST=mysql\r\n", "DB_HOST", "127.0.0.1");
        assert_eq!(t, "APP_URL=http://localhost\r\nDB_HOST=127.0.0.1\r\n");
        assert!(set_env(&t, "DB_PORT", "3306").ends_with("DB_PORT=3306\r\n"));
    }

    #[test]
    fn adresses_et_htaccess() {
        assert_eq!(hote_et_chemin("http://localhost/imprimerie/"), ("localhost/imprimerie".into(), "imprimerie".into()));
        assert_eq!(hote_et_chemin("https://agence-web.test"), ("agence-web.test".into(), String::new()));
        let h = "RewriteBase /imprimerie/\nRewriteRule . /imprimerie/index.php [L]\n";
        assert_eq!(corriger_htaccess(h, "imprimerie"), "RewriteBase /\nRewriteRule . /index.php [L]\n");
        assert_eq!(corriger_htaccess(h, ""), h);
    }

    #[test]
    fn bases_et_collations() {
        assert_eq!(base_libre("imp", &["imp".into(), "imp_2".into()]), "imp_3");
        assert_eq!(base_libre("neuve", &[]), "neuve");
        assert!(convertir_collations("COLLATE=utf8mb4_0900_ai_ci").contains("utf8mb4_unicode_520_ci"));
        assert!(!fichier_de_donnees_utile("binlog.000042") && fichier_de_donnees_utile("ibdata1") && fichier_de_donnees_utile("ib_logfile0"));
    }
}
