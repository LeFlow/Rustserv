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

/// Installe (ou met à jour) les outils d'analyse ; installe Composer s'il manque.
pub async fn install_tools(paths: &Paths, settings: &Settings, installer: &Installer, catalog: &Catalog) -> Result<ToolsStatus> {
    let composer = match installer.installed()?.into_iter().filter(|p| p.package == "composer").max_by(|a, b| version::cmp(&a.version, &b.version)) {
        Some(c) => c,
        None => {
            let r = catalog.latest("composer", None).ok_or_else(|| Error::PaquetInconnu { id: "composer".into() })?;
            installer.install("composer", r, &AtomicBool::new(false), &|_| {}).await?
        }
    };
    let (php, ini) = php_outils(paths, settings, installer)?;
    tools::install(paths, &php, &ini, &composer).await
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
    Ok(runner::run_all(&phps, &spec, &paths.tmp().join("compat")).await)
}

fn outils_prets(paths: &Paths) -> Result<()> {
    if tools::status(paths).installes {
        Ok(())
    } else {
        Err(Error::CompatEchec { outil: "Rustserv".into(), detail: "les outils d'analyse ne sont pas installés".into() })
    }
}

pub async fn phpcompatibility(paths: &Paths, settings: &Settings, installer: &Installer, site: &str, min: &str, max: Option<&str>) -> Result<Rapport> {
    outils_prets(paths)?;
    let projet = site_dir(paths, settings, site)?;
    let (php, ini) = php_outils(paths, settings, installer)?;
    analysis::phpcompatibility(paths, &php, &ini, &projet, min, max).await
}

pub async fn phpstan(paths: &Paths, settings: &Settings, installer: &Installer, site: &str, niveau: u8, cible: &str) -> Result<Rapport> {
    outils_prets(paths)?;
    let projet = site_dir(paths, settings, site)?;
    let (php, ini) = php_outils(paths, settings, installer)?;
    analysis::phpstan(paths, &php, &ini, &projet, niveau, cible).await
}
