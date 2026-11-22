//! Domaines des sites dans le fichier hosts de Windows.
//!
//! L'application **lit** le fichier hosts (lecture autorisée sans droits) pour savoir
//! si une mise à jour est nécessaire, et la confie au helper lancé en administrateur
//! (une fenêtre UAC s'affiche) seulement quand l'utilisateur le demande.

use crate::error::{Error, Result};
use crate::paths::Paths;
use crate::platform::{self, Elevation};
use crate::settings::Settings;
use crate::sites;
use serde::Serialize;
use std::path::PathBuf;

pub use rustserv_helper::{hosts_path, valid_host, TLD_AUTORISES};

/// État des domaines dans le fichier hosts.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct HostsStatus {
    /// Domaines des sites, déjà déclarés.
    pub declares: Vec<String>,
    /// Domaines de sites à ajouter.
    pub manquants: Vec<String>,
    /// Domaines présents dans le bloc Rustserv mais plus utilisés (site supprimé).
    pub en_trop: Vec<String>,
}

impl HostsStatus {
    pub fn a_jour(&self) -> bool {
        self.manquants.is_empty() && self.en_trop.is_empty()
    }
}

/// Domaines voulus : un par site de www/ (sites aux noms invalides ignorés).
pub fn desired(paths: &Paths, settings: &Settings) -> Result<Vec<String>> {
    let mut v: Vec<String> = sites::discover(paths, &settings.tld)?.into_iter().map(|s| s.host).filter(|h| valid_host(h)).collect();
    v.sort();
    Ok(v)
}

/// Domaines du bloc Rustserv actuellement dans le fichier hosts.
pub fn current() -> Vec<String> {
    let bytes = std::fs::read(hosts_path()).unwrap_or_default();
    rustserv_helper::read_block(&String::from_utf8_lossy(&bytes))
}

/// Compare les domaines voulus et le fichier hosts.
pub fn status(paths: &Paths, settings: &Settings) -> Result<HostsStatus> {
    Ok(compare(&desired(paths, settings)?, &current()))
}

fn compare(voulus: &[String], actuels: &[String]) -> HostsStatus {
    HostsStatus {
        declares: voulus.iter().filter(|h| actuels.contains(h)).cloned().collect(),
        manquants: voulus.iter().filter(|h| !actuels.contains(h)).cloned().collect(),
        en_trop: actuels.iter().filter(|h| !voulus.contains(h)).cloned().collect(),
    }
}

/// Emplacement du helper : à côté de l'exécutable de Rustserv.
pub fn helper_path() -> Result<PathBuf> {
    let exe = std::env::current_exe()?;
    let p = exe.parent().map(|d| d.join(platform::exe_name("rustserv-helper"))).unwrap_or_default();
    if !p.is_file() {
        return Err(Error::ExecutableIntrouvable { chemin: p });
    }
    Ok(p)
}

/// Met le bloc Rustserv du fichier hosts en conformité avec les sites.
/// **Bloquant** : affiche la fenêtre UAC et attend la fin du helper.
pub fn sync(paths: &Paths, settings: &Settings, helper: &std::path::Path) -> Result<()> {
    let voulus = desired(paths, settings)?;
    let demande = paths.tmp().join("hosts-demande.txt");
    std::fs::write(&demande, voulus.join("\r\n")).map_err(|e| Error::fichier(&demande, e))?;
    let err_file = demande.with_extension("err");
    let _ = std::fs::remove_file(&err_file);

    let args = format!("hosts-sync \"{}\"", demande.display());
    match platform::run_elevated(helper, &args)? {
        Elevation::Refuse => Err(Error::AdminRefuse { operation: "mise à jour du fichier hosts".into() }),
        Elevation::Termine(0) => {
            tracing::info!(domaines = ?voulus, "fichier hosts mis à jour");
            Ok(())
        }
        Elevation::Termine(code) => {
            let detail = std::fs::read_to_string(&err_file).unwrap_or_else(|_| format!("code de sortie {code}"));
            Err(Error::HostsEchec { detail })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Chaque domaine proposé dans les Paramètres doit avoir sa description en français.
    #[test]
    fn chaque_domaine_est_decrit() {
        let fr: serde_json::Value = serde_json::from_str(include_str!("../../../ui/src/i18n/fr.json")).unwrap();
        for tld in TLD_AUTORISES {
            assert!(fr["parametres"]["tld_desc"][tld].is_string(), "description manquante : parametres.tld_desc.{tld}");
        }
        assert_eq!(TLD_AUTORISES[0], "test", "le premier domaine est affiché comme recommandé");
    }

    #[test]
    fn comparaison() {
        let s = compare(&["a.test".into(), "b.test".into()], &["b.test".into(), "vieux.test".into()]);
        assert_eq!(s.declares, ["b.test"]);
        assert_eq!(s.manquants, ["a.test"]);
        assert_eq!(s.en_trop, ["vieux.test"]);
        assert!(!s.a_jour());
        assert!(compare(&["a.test".into()], &["a.test".into()]).a_jour());
    }
}
