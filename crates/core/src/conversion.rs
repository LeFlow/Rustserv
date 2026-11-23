//! Conversion d'une base entre familles de moteurs (MariaDB / MySQL ↔ PostgreSQL), données
//! comprises. Le travail est fait par un script PHP/PDO embarqué (`scripts/convertir_base.php`),
//! lancé avec le PHP installé le plus récent **sans son php.ini** (`-n`) : seules les extensions
//! PDO nécessaires sont chargées, les réglages de l'utilisateur n'interviennent pas.

use crate::catalog::{version, InstalledPackage};
use crate::db::Moteur;
use crate::error::{Error, Result};
use crate::paths::Paths;
use crate::platform;
use serde::{Deserialize, Serialize};
use std::process::Stdio;
use std::time::Duration;

const SCRIPT: &str = include_str!("scripts/convertir_base.php");

/// Une base dans une instance locale qui tourne.
#[derive(Debug, Clone, Serialize)]
pub struct Connexion {
    pub moteur: Moteur,
    pub port: u16,
    pub base: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TableConvertie {
    pub nom: String,
    pub lignes: u64,
}

/// Point d'attention (code traduit par l'interface : `conversion.avert.<code>`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Avertissement {
    pub code: String,
    pub params: serde_json::Value,
}

/// Objets qui ne se convertissent pas automatiquement (à reprendre à la main).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct NonConvertis {
    #[serde(default)]
    pub vues: Vec<String>,
    #[serde(default)]
    pub procedures: Vec<String>,
    #[serde(default)]
    pub declencheurs: Vec<String>,
    #[serde(default)]
    pub evenements: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RapportConversion {
    pub tables: Vec<TableConvertie>,
    pub avertissements: Vec<Avertissement>,
    pub non_convertis: NonConvertis,
    pub secondes: f64,
}

impl RapportConversion {
    pub fn lignes(&self) -> u64 {
        self.tables.iter().map(|t| t.lignes).sum()
    }
}

/// PHP le plus récent installé, en version 8 ou plus (le script utilise la syntaxe PHP 8).
pub fn php_pour_conversion(installed: &[InstalledPackage]) -> Result<&InstalledPackage> {
    installed
        .iter()
        .filter(|p| p.package == "php" && version::cmp(&p.version, "8.0").is_ge())
        .max_by(|a, b| version::cmp(&a.version, &b.version))
        .ok_or_else(|| Error::AucuneVersion { id: "php 8".into() })
}

/// Convertit `source` vers `cible` (base déjà créée et **vide** dans la cible).
pub async fn convertir(paths: &Paths, php: &InstalledPackage, source: &Connexion, cible: &Connexion) -> Result<RapportConversion> {
    let travail = paths.tmp().join("conversion");
    std::fs::create_dir_all(&travail).map_err(|e| Error::fichier(&travail, e))?;
    let script = travail.join("convertir_base.php");
    std::fs::write(&script, SCRIPT).map_err(|e| Error::fichier(&script, e))?;
    let config = travail.join(format!("{}-{}.json", source.base, std::process::id()));
    let json = serde_json::json!({ "source": source, "cible": cible });
    std::fs::write(&config, json.to_string()).map_err(|e| Error::fichier(&config, e))?;

    let ext = php.path.join("ext");
    let mut cmd = tokio::process::Command::new(php.path.join(platform::exe_name("php")));
    cmd.arg("-n")
        .arg(format!("-dextension_dir={}", ext.display()))
        .args(["-dextension=pdo_mysql", "-dextension=pdo_pgsql", "-dmemory_limit=1G", "-ddate.timezone=UTC"])
        .arg(&script)
        .arg(&config)
        // libpq.dll (pdo_pgsql) se trouve dans le dossier de PHP.
        .env("PATH", format!("{};{}", php.path.display(), std::env::var("PATH").unwrap_or_default()))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    cmd.creation_flags(platform::CREATE_NO_WINDOW);
    let sortie = tokio::time::timeout(Duration::from_secs(3 * 3600), cmd.output())
        .await
        .map_err(|_| Error::ConversionBase { detail: "délai dépassé (3 h)".into() })??;
    let _ = std::fs::remove_file(&config);
    let stdout = String::from_utf8_lossy(&sortie.stdout);
    let stderr = String::from_utf8_lossy(&sortie.stderr);
    // Dernière ligne JSON de la sortie (des avertissements PHP peuvent précéder).
    let json: Option<serde_json::Value> = stdout.lines().rev().find_map(|l| serde_json::from_str(l.trim()).ok());
    let Some(json) = json else {
        return Err(Error::ConversionBase { detail: format!("{}\n{}", stdout.trim(), stderr.trim()).trim().to_string() });
    };
    match json["erreur"].as_str() {
        Some("cible_non_vide") => Err(Error::BaseCibleNonVide { base: cible.base.clone(), moteur: cible.moteur.nom().into() }),
        Some(_) => Err(Error::ConversionBase { detail: json["detail"].as_str().unwrap_or_default().to_string() }),
        None => {
            let r: RapportConversion = serde_json::from_value(json).map_err(|e| Error::ConversionBase { detail: e.to_string() })?;
            tracing::info!(base = %source.base, de = ?source.moteur, vers = ?cible.moteur, tables = r.tables.len(), lignes = r.lignes(), "base convertie");
            Ok(r)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rapport_lisible() {
        let j = r#"{"tables":[{"nom":"users","lignes":3},{"nom":"posts","lignes":10}],
            "avertissements":[{"code":"dates_zero","params":{"table":"posts","colonne":"d","n":2}}],
            "non_convertis":{"vues":["v"],"procedures":[],"declencheurs":[],"evenements":[]},"secondes":0.4}"#;
        let r: RapportConversion = serde_json::from_str(j).unwrap();
        assert_eq!(r.lignes(), 13);
        assert_eq!(r.non_convertis.vues, ["v"]);
    }

    #[test]
    fn php_8_minimum() {
        let p = |v: &str| InstalledPackage { package: "php".into(), version: v.into(), sha256: String::new(), installed_at: 0, path: v.into() };
        assert_eq!(php_pour_conversion(&[p("7.4.33"), p("8.3.37"), p("8.5.13")]).unwrap().version, "8.5.13");
        assert!(php_pour_conversion(&[p("7.4.33")]).is_err());
    }
}
