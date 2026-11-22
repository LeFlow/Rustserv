//! Réglages utilisateur (`etc/rustserv.json`).
//!
//! Tous les champs ont une valeur par défaut : un fichier absent, partiel ou
//! venant d'une ancienne version reste lisible.

use crate::error::{Error, Result};
use crate::paths::Paths;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum WebServer {
    #[default]
    Nginx,
    Apache,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct Ports {
    pub http: u16,
    pub https: u16,
    pub mariadb: u16,
}

impl Default for Ports {
    fn default() -> Self {
        Self { http: 80, https: 443, mariadb: 3306 }
    }
}

/// Versions choisies ; `None` = la plus récente installée.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(default)]
pub struct Versions {
    pub nginx: Option<String>,
    pub apache: Option<String>,
    pub mariadb: Option<String>,
    /// **Branche** de PHP par défaut des sites (ex. « 8.3 »). Une version complète
    /// (« 8.3.37 », anciens réglages) est acceptée et ramenée à sa branche.
    pub php: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct Settings {
    pub web_server: WebServer,
    pub ports: Ports,
    pub versions: Versions,
    /// Nombre de processus php-cgi par version (1 à 10).
    pub php_pool_size: u8,
    /// Domaine des sites (`monsite.test`).
    pub tld: String,
    /// Fuseau horaire PHP par défaut (date.timezone).
    pub timezone: String,
    /// Branche de PHP choisie par site (nom du site → « 7.4 »). Absent = PHP par défaut.
    pub site_php: BTreeMap<String, String>,
    /// Version épinglée par branche (« 8.3 » → « 8.3.36 »). Absent = la plus récente installée.
    pub php_pins: BTreeMap<String, String>,
    /// Branche de MariaDB choisie par site (nom du site → « 11.4 »). Absent = MariaDB par défaut.
    pub site_mariadb: BTreeMap<String, String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            web_server: WebServer::Nginx,
            ports: Ports::default(),
            versions: Versions::default(),
            php_pool_size: 4,
            tld: "test".into(),
            timezone: "Europe/Paris".into(),
            site_php: BTreeMap::new(),
            php_pins: BTreeMap::new(),
            site_mariadb: BTreeMap::new(),
        }
    }
}

impl Settings {
    /// Charge les réglages (valeurs par défaut si le fichier est absent).
    pub fn load(paths: &Paths) -> Result<Self> {
        let f = paths.settings_file();
        if !f.exists() {
            return Ok(Self::default());
        }
        let txt = std::fs::read_to_string(&f).map_err(|e| Error::fichier(&f, e))?;
        let mut s: Settings = serde_json::from_str(&txt)
            .map_err(|e| Error::JsonInvalide { fichier: f.display().to_string(), detail: e.to_string() })?;
        s.php_pool_size = s.php_pool_size.clamp(1, 10);
        Ok(s)
    }

    /// Vérifie la cohérence des réglages avant enregistrement.
    pub fn validate(&self) -> Result<()> {
        let invalide = |d: String| Err(Error::ReglagesInvalides { detail: d });
        let p = &self.ports;
        for (nom, port) in [("HTTP", p.http), ("HTTPS", p.https), ("MariaDB", p.mariadb)] {
            if port == 0 {
                return invalide(format!("le port {nom} ne peut pas être 0"));
            }
            if (9000..=9999).contains(&port) {
                return invalide(format!("le port {nom} ({port}) est dans la plage 9000-9999 réservée aux processus PHP"));
            }
        }
        if p.http == p.https || p.http == p.mariadb || p.https == p.mariadb {
            return invalide("les ports HTTP, HTTPS et MariaDB doivent être différents".into());
        }
        if !(1..=10).contains(&self.php_pool_size) {
            return invalide("le nombre de processus PHP doit être compris entre 1 et 10".into());
        }
        // Uniquement des domaines réservés au local : un vrai domaine (.dev, .com…)
        // rendrait des sites Internet inaccessibles et .dev impose en plus le HTTPS.
        if !crate::hosts::TLD_AUTORISES.contains(&self.tld.as_str()) {
            return invalide(format!(
                "le domaine « .{} » n'est pas autorisé ; choisissez parmi : .{}",
                self.tld,
                crate::hosts::TLD_AUTORISES.join(", .")
            ));
        }
        if self.timezone.trim().is_empty() {
            return invalide("le fuseau horaire est obligatoire".into());
        }
        Ok(())
    }

    pub fn save(&self, paths: &Paths) -> Result<()> {
        let f = paths.settings_file();
        let json = serde_json::to_string_pretty(self).map_err(|e| Error::Interne(e.to_string()))?;
        std::fs::write(&f, json).map_err(|e| Error::fichier(&f, e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defauts_et_fichier_partiel() {
        let t = tempfile::tempdir().unwrap();
        let p = Paths::new(t.path());
        p.ensure_layout().unwrap();
        assert_eq!(Settings::load(&p).unwrap(), Settings::default());

        std::fs::write(p.settings_file(), r#"{ "web_server": "apache", "ports": { "http": 8080 }, "php_pool_size": 99 }"#).unwrap();
        let s = Settings::load(&p).unwrap();
        assert_eq!(s.web_server, WebServer::Apache);
        assert_eq!(s.ports.http, 8080);
        assert_eq!(s.ports.mariadb, 3306, "les champs absents gardent leur défaut");
        assert_eq!(s.php_pool_size, 10, "taille de pool bornée");

        s.save(&p).unwrap();
        assert_eq!(Settings::load(&p).unwrap(), s);
    }

    #[test]
    fn validation() {
        assert!(Settings::default().validate().is_ok());
        let mut s = Settings::default();
        s.ports.mariadb = 80;
        assert_eq!(s.validate().unwrap_err().code(), "reglages.invalides");
        let mut s = Settings::default();
        s.ports.http = 9830;
        assert!(s.validate().is_err(), "conflit avec le pool PHP");
        let mut s = Settings::default();
        s.tld = "mon tld".into();
        assert!(s.validate().is_err());
        s.tld = "dev".into();
        assert!(s.validate().is_err(), ".dev est un vrai domaine");
        s.tld = "localhost".into();
        assert!(s.validate().is_ok());
    }
}
