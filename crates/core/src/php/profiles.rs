//! Profils prédéfinis (Laravel, WordPress, Symfony) : un jeu d'extensions et
//! de réglages à appliquer en un clic sur une version de PHP.
//!
//! Un profil **ajoute** (n'active ni ne désactive jamais rien d'autre). On calcule
//! d'abord un [`ProfilePlan`] (affiché à l'utilisateur) puis on l'applique.

use super::extensions::ExtContext;
use super::ini::IniFile;
use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Profile {
    #[serde(default)]
    pub id: String,
    pub nom: String,
    pub description: String,
    pub extensions: Vec<String>,
    #[serde(default)]
    pub pecl: Vec<String>,
    #[serde(default)]
    pub ini: BTreeMap<String, String>,
}

/// Tous les profils intégrés, dans l'ordre d'affichage.
pub fn all() -> Vec<Profile> {
    let map: BTreeMap<String, Profile> =
        serde_json::from_str(include_str!("profiles.json")).expect("profiles.json intégré invalide");
    let mut v: Vec<Profile> = map.into_iter().map(|(id, mut p)| {
        p.id = id;
        p
    }).collect();
    let ordre = ["laravel", "symfony", "wordpress"];
    v.sort_by_key(|p| ordre.iter().position(|o| *o == p.id).unwrap_or(99));
    v
}

pub fn get(id: &str) -> Result<Profile> {
    all().into_iter().find(|p| p.id == id).ok_or_else(|| Error::ProfilInconnu { profil: id.into() })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct IniChange {
    pub cle: String,
    pub avant: Option<String>,
    pub apres: String,
}

/// Ce que l'application du profil va changer.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ProfilePlan {
    pub profil: String,
    /// Extensions déjà présentes (activées ou intégrées).
    pub deja_ok: Vec<String>,
    /// Extensions disponibles à activer.
    pub a_activer: Vec<String>,
    /// Extensions PECL à télécharger puis activer.
    pub a_telecharger: Vec<String>,
    /// Réglages php.ini modifiés.
    pub reglages: Vec<IniChange>,
    /// Extensions introuvables pour cette version (ni intégrées, ni en DLL, ni PECL).
    pub indisponibles: Vec<String>,
}

impl ProfilePlan {
    pub fn is_empty(&self) -> bool {
        self.a_activer.is_empty() && self.a_telecharger.is_empty() && self.reglages.is_empty()
    }
}

/// Calcule le plan d'un profil pour une version de PHP.
/// `actives` : extensions actuellement chargées (avec nom normalisé).
pub fn plan(profile: &Profile, ini: &IniFile, ctx: &ExtContext, actives: &BTreeSet<String>) -> ProfilePlan {
    let mut p = ProfilePlan { profil: profile.id.clone(), ..Default::default() };
    for ext in &profile.extensions {
        // Cas particulier : OPcache intégré (PHP ≥ 8.5) mais désactivé par opcache.enable=0.
        let opcache_coupe = ext == "opcache" && ctx.builtin.contains("opcache") && !actives.contains("opcache");
        if (ctx.builtin.contains(ext) || actives.contains(ext)) && !opcache_coupe {
            p.deja_ok.push(ext.clone());
        } else if ctx.dll_for(ext).is_some() || opcache_coupe {
            p.a_activer.push(ext.clone());
        } else {
            p.indisponibles.push(ext.clone());
        }
    }
    for ext in &profile.pecl {
        if actives.contains(ext) {
            p.deja_ok.push(ext.clone());
        } else if ctx.dlls.contains(ext) {
            p.a_activer.push(ext.clone());
        } else {
            p.a_telecharger.push(ext.clone());
        }
    }
    for (cle, val) in &profile.ini {
        let avant = ini.get(cle);
        if avant.as_deref() != Some(val.as_str()) {
            p.reglages.push(IniChange { cle: cle.clone(), avant, apres: val.clone() });
        }
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profils_integres_valides() {
        let v = all();
        assert_eq!(v.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(), ["laravel", "symfony", "wordpress"]);
        assert!(get("drupal").is_err());
    }

    #[test]
    fn calcul_du_plan() {
        let prof = get("wordpress").unwrap();
        let ini = IniFile::parse("max_input_vars = 1000\nupload_max_filesize = 256M\n");
        let ctx = ExtContext {
            builtin: ["dom", "hash", "xml", "opcache"].iter().map(|s| s.to_string()).collect(),
            dlls: ["curl", "exif", "fileinfo", "intl", "mbstring", "mysqli", "openssl", "zip", "gd"].iter().map(|s| s.to_string()).collect(),
        };
        let actives: BTreeSet<String> = ["curl", "mbstring", "dom", "hash", "xml", "opcache"].iter().map(|s| s.to_string()).collect();
        let p = plan(&prof, &ini, &ctx, &actives);
        assert_eq!(p.a_activer, ["exif", "fileinfo", "intl", "mysqli", "openssl", "zip", "gd"]);
        assert_eq!(p.a_telecharger, ["imagick"]);
        assert!(p.deja_ok.contains(&"opcache".to_string()));
        assert!(p.indisponibles.is_empty());
        let cles: Vec<_> = p.reglages.iter().map(|r| r.cle.as_str()).collect();
        assert_eq!(cles, ["max_execution_time", "max_input_vars", "post_max_size"], "upload_max_filesize déjà bon");
        assert_eq!(p.reglages[1].avant.as_deref(), Some("1000"));
    }
}
