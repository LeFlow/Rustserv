//! Opérations complètes sur les extensions PHP, pour l'interface :
//! modification transactionnelle du php.ini + redémarrage des processus php-cgi
//! de la version concernée (uniquement s'ils tournaient).

use crate::catalog::{version, InstallProgress, InstalledPackage, Installer};
use crate::error::{Error, Result};
use crate::orchestrator::{Group, Orchestrator};
use crate::php::extensions::{self, ExtContext, ExtensionInfo};
use crate::php::pecl::{self, PeclRecord};
use crate::php::profiles::{self, ProfilePlan};
use crate::php::{ensure_php_ini, ini::IniFile, transaction, PhpCheck};
use crate::process::ServiceState;
use crate::settings::Settings;
use serde::Serialize;
use std::collections::BTreeSet;
use std::sync::atomic::AtomicBool;

/// Résultat de l'application d'un profil.
#[derive(Debug, Clone, Serialize)]
pub struct ProfileResult {
    pub plan: ProfilePlan,
    /// Extensions PECL qui n'ont pas pu être installées (nom, message d'erreur sérialisé).
    pub echecs_pecl: Vec<(String, serde_json::Value)>,
}

impl Orchestrator {
    fn php_pkg(&self, installer: &Installer, php_version: &str) -> Result<InstalledPackage> {
        installer
            .installed()?
            .into_iter()
            .find(|p| p.package == "php" && p.version == php_version)
            .ok_or_else(|| Error::NonInstalle { id: "php".into(), version: php_version.into() })
    }

    /// Redémarre les processus php-cgi d'une version **s'ils tournaient**.
    pub async fn reload_php_version(&self, installer: &Installer, settings: &Settings, php_version: &str) -> Result<()> {
        let prefix = format!("php-{php_version}#");
        let tournait = self
            .supervisor
            .states()
            .iter()
            .any(|s| s.id.starts_with(&prefix) && !matches!(s.state, ServiceState::Arrete | ServiceState::Erreur { .. }));
        if tournait {
            self.stop_php_version(php_version).await?;
            self.start(Group::Php, settings, &installer.installed()?).await?;
        }
        Ok(())
    }

    /// État des extensions d'une version (crée le php.ini au besoin).
    pub async fn extensions(&self, installer: &Installer, settings: &Settings, php_version: &str) -> Result<Vec<ExtensionInfo>> {
        let php = self.php_pkg(installer, php_version)?;
        ensure_php_ini(&self.paths, &php.path, &php.version, settings)?;
        let paths = self.paths.clone();
        tokio::task::spawn_blocking(move || extensions::scan(&paths, &php.path, &php.version))
            .await
            .map_err(|e| Error::Interne(e.to_string()))?
    }

    /// Active ou désactive une extension (avec dépendances), puis recharge PHP.
    pub async fn set_extension(
        &self,
        installer: &Installer,
        settings: &Settings,
        php_version: &str,
        name: &str,
        enabled: bool,
    ) -> Result<PhpCheck> {
        let php = self.php_pkg(installer, php_version)?;
        ensure_php_ini(&self.paths, &php.path, &php.version, settings)?;
        let (paths, name2) = (self.paths.clone(), name.to_string());
        let check = tokio::task::spawn_blocking(move || {
            let ctx = ExtContext::load(&php.path, &php.version)?;
            transaction::modify(&paths, &php.path, &php.version, |ini| {
                if enabled {
                    extensions::enable(ini, &ctx, &php.version, &name2).map(|_| ())
                } else {
                    extensions::disable(ini, &ctx, &name2).map(|_| ())
                }
            })
        })
        .await
        .map_err(|e| Error::Interne(e.to_string()))??;
        tracing::info!(php_version, extension = name, enabled, "extension modifiée");
        self.reload_php_version(installer, settings, php_version).await?;
        Ok(check)
    }

    /// Télécharge, installe et active une extension PECL. Retour arrière complet
    /// (fichiers + php.ini) si PHP refuse de la charger.
    #[allow(clippy::too_many_arguments)]
    pub async fn install_pecl(
        &self,
        installer: &Installer,
        settings: &Settings,
        php_version: &str,
        ext: &str,
        cancel: &AtomicBool,
        on_progress: &(dyn Fn(InstallProgress) + Send + Sync),
    ) -> Result<PeclRecord> {
        let record = self.install_pecl_files(installer, settings, php_version, ext, cancel, on_progress).await?;
        let php = self.php_pkg(installer, php_version)?;
        let (paths, ext2) = (self.paths.clone(), ext.to_ascii_lowercase());
        let res = tokio::task::spawn_blocking(move || {
            let ctx = ExtContext::load(&php.path, &php.version)?;
            transaction::modify(&paths, &php.path, &php.version, |ini| extensions::enable(ini, &ctx, &php.version, &ext2).map(|_| ()))
                .inspect_err(|_| {
                    let _ = pecl::remove_files(&paths, &php.path, &php.version, &ext2);
                })
        })
        .await
        .map_err(|e| Error::Interne(e.to_string()))?;
        res?;
        self.reload_php_version(installer, settings, php_version).await?;
        Ok(record)
    }

    /// Partie « fichiers » de l'installation PECL (sans activation).
    async fn install_pecl_files(
        &self,
        installer: &Installer,
        settings: &Settings,
        php_version: &str,
        ext: &str,
        cancel: &AtomicBool,
        on_progress: &(dyn Fn(InstallProgress) + Send + Sync),
    ) -> Result<PeclRecord> {
        let php = self.php_pkg(installer, php_version)?;
        ensure_php_ini(&self.paths, &php.path, &php.version, settings)?;
        let build = pecl::find_build(installer.client(), ext, &version::branch(php_version)).await?;
        pecl::install_files(installer.client(), &self.paths, &php.path, &php.version, &build, cancel, on_progress).await
    }

    /// Plan d'un profil pour une version (rien n'est modifié).
    pub async fn profile_plan(&self, installer: &Installer, settings: &Settings, php_version: &str, profil: &str) -> Result<ProfilePlan> {
        let prof = profiles::get(profil)?;
        let php = self.php_pkg(installer, php_version)?;
        let ini_path = ensure_php_ini(&self.paths, &php.path, &php.version, settings)?;
        tokio::task::spawn_blocking(move || {
            let ctx = ExtContext::load(&php.path, &php.version)?;
            let ini = IniFile::parse(&std::fs::read_to_string(&ini_path).map_err(|e| Error::fichier(&ini_path, e))?);
            let (loaded, _) = crate::php::check::loaded_modules_lenient(&php.path, &php.version, &ini_path)?;
            let actives: BTreeSet<String> = loaded.iter().map(|m| crate::php::check::module_to_extension(m)).collect();
            Ok(profiles::plan(&prof, &ini, &ctx, &actives))
        })
        .await
        .map_err(|e| Error::Interne(e.to_string()))?
    }

    /// Applique un profil : téléchargements PECL (les échecs sont signalés sans tout
    /// bloquer), puis **une seule** transaction php.ini pour tout le reste.
    pub async fn apply_profile(
        &self,
        installer: &Installer,
        settings: &Settings,
        php_version: &str,
        profil: &str,
        cancel: &AtomicBool,
        on_progress: &(dyn Fn(InstallProgress) + Send + Sync),
    ) -> Result<ProfileResult> {
        let plan = self.profile_plan(installer, settings, php_version, profil).await?;
        let mut echecs_pecl = Vec::new();
        let mut installees = Vec::new();
        for ext in &plan.a_telecharger {
            match self.install_pecl_files(installer, settings, php_version, ext, cancel, on_progress).await {
                Ok(_) => installees.push(ext.clone()),
                Err(Error::Annule) => return Err(Error::Annule),
                Err(e) => {
                    tracing::warn!(extension = %ext, erreur = %e, "extension PECL du profil non installée");
                    echecs_pecl.push((ext.clone(), serde_json::to_value(&e).unwrap_or_default()));
                }
            }
        }

        let php = self.php_pkg(installer, php_version)?;
        let paths = self.paths.clone();
        let (plan2, installees2) = (plan.clone(), installees.clone());
        let res = tokio::task::spawn_blocking(move || {
            let ctx = ExtContext::load(&php.path, &php.version)?;
            transaction::modify(&paths, &php.path, &php.version, |ini| {
                for ext in plan2.a_activer.iter().chain(installees2.iter()) {
                    extensions::enable(ini, &ctx, &php.version, ext)?;
                }
                for r in &plan2.reglages {
                    ini.set(&r.cle, &r.apres);
                }
                Ok(())
            })
            .inspect_err(|_| {
                for ext in &installees2 {
                    let _ = pecl::remove_files(&paths, &php.path, &php.version, ext);
                }
            })
        })
        .await
        .map_err(|e| Error::Interne(e.to_string()))?;
        res?;
        self.reload_php_version(installer, settings, php_version).await?;
        tracing::info!(php_version, profil, "profil appliqué");
        Ok(ProfileResult { plan, echecs_pecl })
    }
}
