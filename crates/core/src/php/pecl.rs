//! Extensions PECL (xdebug, redis, apcu, imagick…) depuis les builds Windows
//! officiels de php.net :
//! `https://downloads.php.net/~windows/pecl/releases/<ext>/<version>/php_<ext>-<version>-<branche>-nts-<compilateur>-x64.zip`
//!
//! Le compilateur et le mode NTS sont imposés par la branche de PHP : il suffit
//! donc de chercher l'archive `-<branche>-nts-…-x64.zip` de la version la plus
//! récente de l'extension qui en propose une.
//!
//! php.net ne publie pas de SHA256 pour ces archives : l'empreinte est calculée
//! au téléchargement (HTTPS uniquement) et enregistrée dans
//! `etc/php/<version>/pecl.json`, puis `php -m` vérifie le chargement.
//!
//! Installation (procédure officielle) : `php_<ext>.dll` → `ext/`, les autres DLL
//! de l'archive (dépendances, ex. ImageMagick) → dossier de PHP.

use super::setup::php_ini_path;
use crate::catalog::{download_unverified, extract::extract_zip, version, InstallPhase, InstallProgress};
use crate::error::{Error, Result};
use crate::paths::Paths;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

pub const BASE: &str = "https://downloads.php.net/~windows/pecl/releases/";

/// Extensions PECL proposées dans l'interface.
pub const PECL: &[&str] = &["xdebug", "apcu", "redis", "igbinary", "imagick", "mongodb", "pcov", "yaml"];

/// Nombre de versions récentes de l'extension examinées pour trouver un build compatible.
const VERSIONS_ESSAYEES: usize = 6;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PeclBuild {
    pub extension: String,
    pub version: String,
    pub url: String,
}

/// Trace d'une extension installée par Rustserv.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeclRecord {
    pub version: String,
    pub url: String,
    pub sha256: String,
    /// Fichiers ajoutés, relatifs au dossier de PHP (`ext/php_redis.dll`…).
    pub files: Vec<String>,
}

fn records_path(paths: &Paths, php_version: &str) -> PathBuf {
    php_ini_path(paths, php_version).with_file_name("pecl.json")
}

/// Extensions PECL installées pour une version de PHP.
pub fn records(paths: &Paths, php_version: &str) -> BTreeMap<String, PeclRecord> {
    std::fs::read_to_string(records_path(paths, php_version))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

fn save_records(paths: &Paths, php_version: &str, r: &BTreeMap<String, PeclRecord>) -> Result<()> {
    let f = records_path(paths, php_version);
    let json = serde_json::to_string_pretty(r).map_err(|e| Error::Interne(e.to_string()))?;
    std::fs::write(&f, json).map_err(|e| Error::fichier(&f, e))
}

/// Liens `href` d'une page d'index de répertoire.
pub fn parse_links(html: &str) -> Vec<String> {
    html.split("href=\"")
        .skip(1)
        .filter_map(|s| s.split('"').next())
        .filter(|s| !s.contains('?'))
        .map(|s| s.trim_end_matches('/').rsplit('/').next().unwrap_or("").to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Versions **stables** (sans RC/beta), de la plus récente à la plus ancienne.
pub fn stable_versions(links: &[String]) -> Vec<String> {
    let mut v: Vec<String> = links
        .iter()
        .filter(|l| l.chars().next().is_some_and(|c| c.is_ascii_digit()) && l.chars().all(|c| c.is_ascii_digit() || c == '.'))
        .cloned()
        .collect();
    v.sort_by(|a, b| version::cmp(b, a));
    v.dedup();
    v
}

/// Nom de l'archive compatible avec une branche de PHP dans une liste de fichiers.
pub fn match_build(files: &[String], ext: &str, ext_version: &str, php_branch: &str) -> Option<String> {
    let prefix = format!("php_{ext}-{ext_version}-{php_branch}-nts-").to_ascii_lowercase();
    files.iter().find(|f| {
        let f = f.to_ascii_lowercase();
        f.starts_with(&prefix) && f.ends_with("-x64.zip")
    }).cloned()
}

async fn get_text(client: &reqwest::Client, url: &str) -> Result<String> {
    let err = |e: reqwest::Error| Error::TelechargementEchoue { url: url.into(), detail: e.to_string() };
    let r = client.get(url).send().await.map_err(err)?;
    if !r.status().is_success() {
        return Err(Error::HttpStatut { url: url.into(), status: r.status().as_u16() });
    }
    r.text().await.map_err(err)
}

/// Cherche la version la plus récente de `ext` qui propose un build pour `php_branch`.
pub async fn find_build(client: &reqwest::Client, ext: &str, php_branch: &str) -> Result<PeclBuild> {
    let ext = ext.to_ascii_lowercase();
    let index = get_text(client, &format!("{BASE}{ext}/")).await?;
    for v in stable_versions(&parse_links(&index)).into_iter().take(VERSIONS_ESSAYEES) {
        let dir = format!("{BASE}{ext}/{v}/");
        let Ok(page) = get_text(client, &dir).await else { continue };
        if let Some(f) = match_build(&parse_links(&page), &ext, &v, php_branch) {
            return Ok(PeclBuild { extension: ext, version: v, url: format!("{dir}{f}") });
        }
    }
    Err(Error::PeclIntrouvable { extension: ext, branche: php_branch.into() })
}

/// Télécharge et copie les fichiers d'une extension PECL dans le dossier de PHP.
/// N'active **pas** l'extension (voir `Orchestrator::install_pecl`).
/// Renvoie l'enregistrement (déjà sauvegardé dans pecl.json).
pub async fn install_files(
    client: &reqwest::Client,
    paths: &Paths,
    php_dir: &Path,
    php_version: &str,
    build: &PeclBuild,
    cancel: &AtomicBool,
    on_progress: &(dyn Fn(InstallProgress) + Send + Sync),
) -> Result<PeclRecord> {
    let emit = |phase, done, total| {
        on_progress(InstallProgress { package: build.extension.clone(), version: build.version.clone(), phase, done, total })
    };
    let dl_dir = paths.tmp().join("downloads");
    std::fs::create_dir_all(&dl_dir).map_err(|e| Error::fichier(&dl_dir, e))?;
    let file_name = build.url.rsplit('/').next().unwrap_or("pecl.zip").to_string();
    let archive = dl_dir.join(&file_name);
    let sha256 = download_unverified(client, &build.url, &archive, cancel, &|d, t| emit(InstallPhase::Telechargement, d, t)).await?;
    tracing::info!(extension = %build.extension, version = %build.version, %sha256, "archive PECL téléchargée");

    let work = paths.tmp().join("extract").join(format!("pecl-{}-{php_version}", build.extension));
    let _ = std::fs::remove_dir_all(&work);
    emit(InstallPhase::Extraction, 0, None);
    let (a2, w2) = (archive.clone(), work.clone());
    tokio::task::spawn_blocking(move || extract_zip(&a2, &w2, |_, _| {})).await.map_err(|e| Error::Interne(e.to_string()))??;
    let res = copy_files(&work, php_dir, &build.extension, &file_name);
    let _ = std::fs::remove_dir_all(&work);
    let _ = std::fs::remove_file(&archive);
    let files = res?;

    let record = PeclRecord { version: build.version.clone(), url: build.url.clone(), sha256, files };
    let mut all = records(paths, php_version);
    all.insert(build.extension.clone(), record.clone());
    save_records(paths, php_version, &all)?;
    emit(InstallPhase::Termine, 1, Some(1));
    Ok(record)
}

/// Copie `php_<ext>.dll` dans `ext/` (en remplaçant une version précédente) et les
/// autres DLL de la racine de l'archive dans le dossier de PHP (sans jamais écraser
/// une DLL officielle de PHP).
fn copy_files(extracted: &Path, php_dir: &Path, ext: &str, label: &str) -> Result<Vec<String>> {
    let dll_name = format!("php_{ext}.dll");
    let entries: Vec<_> = std::fs::read_dir(extracted).map_err(|e| Error::fichier(extracted, e))?.flatten().collect();
    // Vérification avant toute copie : pas d'installation partielle.
    if !entries.iter().any(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case(&dll_name)) {
        return Err(Error::ArchiveInvalide { fichier: label.into(), detail: format!("{dll_name} absent de l'archive") });
    }
    let mut copies = Vec::new();
    for e in entries {
        let p = e.path();
        let name = e.file_name().to_string_lossy().into_owned();
        if !p.is_file() || !name.to_ascii_lowercase().ends_with(".dll") {
            continue;
        }
        if name.eq_ignore_ascii_case(&dll_name) {
            let dest = php_dir.join("ext").join(&dll_name);
            std::fs::copy(&p, &dest).map_err(|er| Error::fichier(&dest, er))?;
            copies.push(format!("ext/{dll_name}"));
        } else {
            let dest = php_dir.join(&name);
            if !dest.exists() {
                std::fs::copy(&p, &dest).map_err(|er| Error::fichier(&dest, er))?;
                copies.push(name);
            }
        }
    }
    copies.sort();
    Ok(copies)
}

/// Supprime les fichiers installés pour une extension (retour arrière / désinstallation).
pub fn remove_files(paths: &Paths, php_dir: &Path, php_version: &str, ext: &str) -> Result<()> {
    let mut all = records(paths, php_version);
    if let Some(r) = all.remove(ext) {
        for f in &r.files {
            let _ = std::fs::remove_file(php_dir.join(f));
        }
        save_records(paths, php_version, &all)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const INDEX: &str = r#"<a href="/~windows/pecl/releases/">..</a><a href="6.0.2/">6.0.2/</a>
        <a href="6.3.0/">6.3.0/</a><a href="6.1.0RC1/">6.1.0RC1/</a><a href="?C=N;O=D">Name</a><a href="logs/">logs/</a>"#;

    #[test]
    fn versions_stables_triees() {
        assert_eq!(stable_versions(&parse_links(INDEX)), ["6.3.0", "6.0.2"]);
    }

    #[test]
    fn choix_du_build() {
        let files: Vec<String> = [
            "php_redis-6.3.0-8.3-ts-vs16-x64.zip",
            "php_redis-6.3.0-8.3-nts-vs16-x86.zip",
            "php_redis-6.3.0-8.3-nts-vs16-x64.zip",
            "php_redis-6.3.0-8.4-nts-vs17-x64.zip",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert_eq!(match_build(&files, "redis", "6.3.0", "8.3").unwrap(), "php_redis-6.3.0-8.3-nts-vs16-x64.zip");
        assert_eq!(match_build(&files, "redis", "6.3.0", "8.4").unwrap(), "php_redis-6.3.0-8.4-nts-vs17-x64.zip");
        assert!(match_build(&files, "redis", "6.3.0", "8.6").is_none());
    }

    #[test]
    fn copie_et_suppression() {
        let t = tempfile::tempdir().unwrap();
        let paths = Paths::new(t.path().join("root"));
        paths.ensure_layout().unwrap();
        let ex = t.path().join("x");
        let php = t.path().join("php");
        std::fs::create_dir_all(&ex).unwrap();
        std::fs::create_dir_all(php.join("ext")).unwrap();
        std::fs::write(ex.join("php_imagick.dll"), "ext").unwrap();
        std::fs::write(ex.join("CORE_RL_zlib_.dll"), "dep").unwrap();
        std::fs::write(ex.join("libssl-3-x64.dll"), "copie").unwrap();
        std::fs::write(ex.join("README.md"), "").unwrap();
        std::fs::write(php.join("libssl-3-x64.dll"), "officielle").unwrap();

        let files = copy_files(&ex, &php, "imagick", "a.zip").unwrap();
        assert_eq!(files, ["CORE_RL_zlib_.dll", "ext/php_imagick.dll"]);
        assert_eq!(std::fs::read_to_string(php.join("libssl-3-x64.dll")).unwrap(), "officielle", "DLL de PHP jamais écrasée");

        std::fs::create_dir_all(php_ini_path(&paths, "8.3.37").parent().unwrap()).unwrap();
        let mut all = BTreeMap::new();
        all.insert("imagick".to_string(), PeclRecord { version: "3.8.1".into(), url: String::new(), sha256: String::new(), files });
        save_records(&paths, "8.3.37", &all).unwrap();
        remove_files(&paths, &php, "8.3.37", "imagick").unwrap();
        assert!(!php.join("ext").join("php_imagick.dll").exists() && !php.join("CORE_RL_zlib_.dll").exists());
        assert!(records(&paths, "8.3.37").is_empty());

        assert_eq!(copy_files(&ex, &php, "redis", "a.zip").unwrap_err().code(), "installation.archive");
        assert!(!php.join("CORE_RL_zlib_.dll").exists(), "aucune copie partielle en cas d'erreur");
    }
}
