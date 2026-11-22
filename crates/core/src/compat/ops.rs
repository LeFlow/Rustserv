//! Opérations complètes pour l'interface : résolution du site et des versions
//! de PHP, installation de Composer si nécessaire, puis appel des outils.

use super::analysis::{self, Rapport};
use super::runner::{self, RunResult, RunSpec, TestKind};
use super::tools::{self, ToolsStatus};
use crate::catalog::{version, Catalog, InstalledPackage, Installer};
use crate::error::{Error, Result};
use crate::paths::Paths;
use crate::php::ensure_php_ini;
use crate::settings::Settings;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

/// Dossier d'un site de www/.
pub fn site_dir(paths: &Paths, settings: &Settings, site: &str) -> Result<PathBuf> {
    crate::sites::discover(paths, &settings.tld)?
        .into_iter()
        .find(|s| s.name == site)
        .map(|s| s.dir)
        .ok_or_else(|| Error::CompatIntrouvable { quoi: site.into(), aide: "script".into() })
}

/// PHP le plus récent installé (exécute les outils d'analyse).
fn php_outils(paths: &Paths, settings: &Settings, installer: &Installer) -> Result<(InstalledPackage, PathBuf)> {
    let php = installer
        .installed()?
        .into_iter()
        .filter(|p| p.package == "php")
        .max_by(|a, b| version::cmp(&a.version, &b.version))
        .ok_or_else(|| Error::AucuneVersion { id: "php".into() })?;
    let ini = ensure_php_ini(paths, &php.path, &php.version, settings)?;
    Ok((php, ini))
}

/// Composer installé (le plus récent), ou installé depuis le catalogue s'il manque.
async fn ensure_composer(installer: &Installer, catalog: &Catalog) -> Result<InstalledPackage> {
    if let Some(c) = installer.installed()?.into_iter().filter(|p| p.package == "composer").max_by(|a, b| version::cmp(&a.version, &b.version)) {
        return Ok(c);
    }
    // Repli sur le catalogue intégré si le catalogue local ne connaît pas Composer.
    let integre = Catalog::embedded()?;
    let r = catalog
        .latest("composer", None)
        .or_else(|| integre.latest("composer", None))
        .ok_or_else(|| Error::PaquetInconnu { id: "composer".into() })?;
    installer.install("composer", r, &AtomicBool::new(false), &|_| {}).await
}

/// Installe (ou met à jour) les outils d'analyse ; installe Composer s'il manque.
pub async fn install_tools(paths: &Paths, settings: &Settings, installer: &Installer, catalog: &Catalog) -> Result<ToolsStatus> {
    let composer = ensure_composer(installer, catalog).await?;
    let (php, ini) = php_outils(paths, settings, installer)?;
    tools::install(paths, &php, &ini, &composer).await
}

/// Installe PHPUnit ou Pest dans le projet (`composer require --dev`).
///
/// Composer s'exécute avec la **plus ancienne** des versions de PHP demandées : il
/// choisit ainsi la version du framework compatible avec toutes (ex. PHPUnit 9.6 si
/// PHP 7.4 fait partie des versions testées). Renvoie la version installée.
pub async fn install_test_framework(
    paths: &Paths,
    settings: &Settings,
    installer: &Installer,
    catalog: &Catalog,
    site: &str,
    kind: TestKind,
    versions: &[String],
) -> Result<String> {
    let paquet = match kind {
        TestKind::Phpunit => "phpunit/phpunit",
        TestKind::Pest => "pestphp/pest",
        TestKind::Script => return Err(Error::Interne("rien à installer pour un script".into())),
    };
    let projet = site_dir(paths, settings, site)?;
    let composer = ensure_composer(installer, catalog).await?;
    let installed = installer.installed()?;
    let php = installed
        .iter()
        .filter(|p| p.package == "php" && (versions.is_empty() || versions.contains(&p.version)))
        .min_by(|a, b| version::cmp(&a.version, &b.version))
        .cloned()
        .ok_or_else(|| Error::AucuneVersion { id: "php".into() })?;
    let ini = ensure_php_ini(paths, &php.path, &php.version, settings)?;
    let phar = composer.path.join("composer.phar").display().to_string();

    // Projet sans composer.json : « composer config » en exige un (« require » le créerait seul).
    let cj = projet.join("composer.json");
    if !cj.exists() {
        std::fs::write(&cj, "{}\n").map_err(|e| Error::fichier(&cj, e))?;
    }
    let mut etapes: Vec<Vec<String>> = Vec::new();
    if kind == TestKind::Pest {
        // Pest est un plugin Composer : il doit être autorisé explicitement (mode non interactif).
        etapes.push(vec![phar.clone(), "config".into(), "--no-plugins".into(), "allow-plugins.pestphp/pest-plugin".into(), "true".into()]);
    }
    etapes.push(vec![
        phar.clone(),
        "require".into(),
        "--dev".into(),
        paquet.into(),
        "--with-all-dependencies".into(),
        "--no-interaction".into(),
        "--no-progress".into(),
        "--no-ansi".into(),
    ]);
    tracing::info!(site, paquet, php = %php.version, "installation du framework de tests");
    for args in etapes {
        tools::run_composer(paths, &php, &ini, &projet, &args).await?;
    }
    tools::installed_version(&projet, paquet)
        .ok_or_else(|| Error::CompatEchec { outil: "Composer".into(), detail: format!("{paquet} absent après installation") })
}

/// Lance des tests ou un script sur les versions de PHP demandées (versions complètes).
pub async fn run_tests(
    paths: &Paths,
    settings: &Settings,
    installer: &Installer,
    site: &str,
    kind: TestKind,
    script: Option<String>,
    versions: &[String],
    cancel: &AtomicBool,
) -> Result<Vec<RunResult>> {
    let project = site_dir(paths, settings, site)?;
    let installed = installer.installed()?;
    let mut phps = Vec::new();
    for v in versions {
        let p = installed
            .iter()
            .find(|p| p.package == "php" && &p.version == v)
            .cloned()
            .ok_or_else(|| Error::NonInstalle { id: "php".into(), version: v.clone() })?;
        let ini = ensure_php_ini(paths, &p.path, &p.version, settings)?;
        phps.push((p, ini));
    }
    phps.sort_by(|a, b| version::cmp(&a.0.version, &b.0.version));
    let spec = RunSpec { project, kind, script, args: vec![], timeout: Duration::from_secs(600) };
    // Vérification immédiate (message clair plutôt qu'une erreur par version).
    runner::entry_point(&spec)?;
    let r = runner::run_all(&phps, &spec, &paths.tmp().join("compat"), cancel).await;
    if cancel.load(std::sync::atomic::Ordering::Relaxed) {
        return Err(Error::Annule);
    }
    Ok(r)
}

fn outils_prets(paths: &Paths) -> Result<()> {
    if tools::status(paths).installes {
        Ok(())
    } else {
        Err(Error::CompatEchec { outil: "Rustserv".into(), detail: "les outils d'analyse ne sont pas installés".into() })
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn phpcompatibility(paths: &Paths, settings: &Settings, installer: &Installer, site: &str, min: &str, max: Option<&str>, cancel: &AtomicBool) -> Result<Rapport> {
    outils_prets(paths)?;
    let projet = site_dir(paths, settings, site)?;
    let (php, ini) = php_outils(paths, settings, installer)?;
    analysis::phpcompatibility(paths, &php, &ini, &projet, min, max, cancel).await
}

#[allow(clippy::too_many_arguments)]
pub async fn phpstan(paths: &Paths, settings: &Settings, installer: &Installer, site: &str, niveau: u8, cible: &str, cancel: &AtomicBool) -> Result<Rapport> {
    outils_prets(paths)?;
    let projet = site_dir(paths, settings, site)?;
    let (php, ini) = php_outils(paths, settings, installer)?;
    analysis::phpstan(paths, &php, &ini, &projet, niveau, cible, cancel).await
}
