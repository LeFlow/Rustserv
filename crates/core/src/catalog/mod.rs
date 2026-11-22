//! Catalogue des binaires téléchargeables (`catalog.json`).
//!
//! Le catalogue décrit, pour chaque paquet (nginx, apache, mariadb, php…),
//! les versions disponibles avec leur URL, leurs miroirs et leur **SHA256
//! obligatoire**. Une copie de référence est embarquée dans l'exécutable
//! (`catalog/catalog.json` du dépôt) et recopiée dans la racine portable au
//! premier lancement ; l'utilisateur peut ensuite la modifier.
//!
//! Sous-modules :
//! - [`version`]  : comparaison de versions ;
//! - `download`   : téléchargement + vérification SHA256 ;
//! - `extract`    : extraction zip sécurisée ;
//! - [`install`]  : orchestration (bin/<paquet>/<paquet>-<version>).

mod download;
pub(crate) mod extract;
pub mod install;
pub mod php_remote;
pub mod version;

pub use download::{download_unverified, download_verified, sha256_file};
pub use install::{InstallPhase, InstallProgress, InstalledPackage, Installer};

use crate::error::{Error, Result};
use crate::paths::Paths;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::path::Path;

/// Catalogue de référence, embarqué à la compilation.
pub const EMBEDDED: &str = include_str!("../../../../catalog/catalog.json");

/// Version du format de catalogue comprise par cette version de Rustserv.
pub const SCHEMA: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Catalog {
    pub schema: u32,
    #[serde(default)]
    pub updated: String,
    /// Paquets indexés par identifiant (`nginx`, `php`…), triés.
    pub packages: BTreeMap<String, Package>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Category {
    Web,
    Database,
    Php,
    Tool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Package {
    pub name: String,
    pub category: Category,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub homepage: Option<String>,
    pub releases: Vec<Release>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Release {
    pub version: String,
    /// Branche (8.3, 12.3…). Déduite de la version si absente.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// Caractéristiques de compilation (surtout pour PHP : NTS/TS, VS16/VS17, x64).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build: Option<Build>,
    pub url: String,
    /// URLs de secours essayées dans l'ordre si `url` échoue.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mirrors: Vec<String>,
    /// Empreinte SHA256 de l'archive (64 caractères hexadécimaux).
    pub sha256: String,
    /// Sous-dossier de l'archive à installer (ex. `Apache24`). Si absent :
    /// dossier unique de premier niveau retiré automatiquement.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subdir: Option<String>,
    /// Version minimale du runtime Visual C++ (vcruntime140.dll) requise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vcruntime_min: Option<String>,
    /// Téléchargement d'un **fichier unique** (pas une archive) enregistré sous ce nom,
    /// ex. `composer.phar`. Un `.phar` reçoit un lanceur `<nom>.bat`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Build {
    pub thread_safe: bool,
    pub compiler: String,
    pub arch: String,
}

impl Release {
    /// URL principale puis miroirs.
    pub fn urls(&self) -> Vec<&str> {
        std::iter::once(self.url.as_str()).chain(self.mirrors.iter().map(String::as_str)).collect()
    }

    /// Nom du fichier téléchargé (dernier segment de l'URL).
    pub fn file_name(&self) -> String {
        let last = self.url.rsplit('/').next().unwrap_or("archive.zip");
        let clean = last.split(['?', '#']).next().unwrap_or(last);
        if clean.is_empty() { "archive.zip".into() } else { clean.into() }
    }

    /// Branche explicite ou déduite (8.3.37 → 8.3).
    pub fn branch(&self) -> String {
        self.branch.clone().unwrap_or_else(|| version::branch(&self.version))
    }
}

impl Catalog {
    /// Analyse et valide un catalogue JSON. `source` sert aux messages d'erreur.
    pub fn parse(json: &str, source: &str) -> Result<Self> {
        let cat: Catalog = serde_json::from_str(json)
            .map_err(|e| Error::JsonInvalide { fichier: source.into(), detail: e.to_string() })?;
        let problemes = cat.validate();
        if !problemes.is_empty() {
            return Err(Error::CatalogueInvalide { problemes });
        }
        Ok(cat)
    }

    /// Catalogue embarqué dans l'exécutable.
    pub fn embedded() -> Result<Self> {
        Self::parse(EMBEDDED, "catalogue intégré")
    }

    /// Charge un catalogue depuis un fichier.
    pub fn load(path: &Path) -> Result<Self> {
        let txt = std::fs::read_to_string(path).map_err(|e| Error::fichier(path, e))?;
        Self::parse(&txt, &path.display().to_string())
    }

    /// Charge `catalog.json` de la racine portable, en le créant depuis la
    /// version embarquée s'il n'existe pas encore.
    pub fn load_or_init(paths: &Paths) -> Result<Self> {
        let file = paths.catalog_file();
        if !file.exists() {
            std::fs::write(&file, EMBEDDED).map_err(|e| Error::fichier(&file, e))?;
            tracing::info!(fichier = %file.display(), "catalogue initialisé depuis la version intégrée");
        }
        Self::load(&file)
    }

    /// Vérifie la cohérence du catalogue ; renvoie la liste des problèmes (vide = OK).
    pub fn validate(&self) -> Vec<String> {
        let mut p = Vec::new();
        if self.schema != SCHEMA {
            p.push(format!("format {} non pris en charge (attendu {SCHEMA})", self.schema));
        }
        for (id, pkg) in &self.packages {
            if id.is_empty() || !id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') {
                p.push(format!("identifiant de paquet invalide : « {id} »"));
            }
            let mut vus = HashSet::new();
            for r in &pkg.releases {
                let ctx = format!("{id} {}", r.version);
                if version::parse(&r.version).0.is_empty() {
                    p.push(format!("{ctx} : numéro de version illisible"));
                }
                if !vus.insert(r.version.as_str()) {
                    p.push(format!("{ctx} : version en double"));
                }
                if r.sha256.len() != 64 || !r.sha256.chars().all(|c| c.is_ascii_hexdigit()) {
                    p.push(format!("{ctx} : SHA256 absent ou mal formé"));
                }
                for u in r.urls() {
                    if !u.starts_with("https://") {
                        p.push(format!("{ctx} : URL non sécurisée (https obligatoire) : {u}"));
                    }
                }
                for (quoi, v) in [("sous-dossier", &r.subdir), ("nom de fichier", &r.file)] {
                    if let Some(s) = v {
                        if s.contains("..") || s.starts_with(['/', '\\']) || s.contains(':') || (quoi == "nom de fichier" && s.contains(['/', '\\'])) {
                            p.push(format!("{ctx} : {quoi} dangereux « {s} »"));
                        }
                    }
                }
            }
        }
        p
    }

    pub fn package(&self, id: &str) -> Result<&Package> {
        self.packages.get(id).ok_or_else(|| Error::PaquetInconnu { id: id.into() })
    }

    pub fn release(&self, id: &str, version: &str) -> Result<&Release> {
        self.package(id)?
            .releases
            .iter()
            .find(|r| r.version == version)
            .ok_or_else(|| Error::VersionInconnue { id: id.into(), version: version.into() })
    }

    /// Ajoute au paquet les versions absentes du catalogue (versions connues inchangées).
    /// Renvoie les versions ajoutées. Le catalogue reste valide (vérifié).
    pub fn merge(&mut self, id: &str, releases: Vec<Release>) -> Result<Vec<String>> {
        let pkg = self.packages.get_mut(id).ok_or_else(|| Error::PaquetInconnu { id: id.into() })?;
        let mut ajoutees = Vec::new();
        for r in releases {
            if !pkg.releases.iter().any(|x| x.version == r.version) {
                ajoutees.push(r.version.clone());
                pkg.releases.push(r);
            }
        }
        pkg.releases.sort_by(|a, b| version::cmp(&b.version, &a.version));
        let problemes = self.validate();
        if !problemes.is_empty() {
            return Err(Error::CatalogueInvalide { problemes });
        }
        Ok(ajoutees)
    }

    /// Enregistre le catalogue (JSON indenté).
    pub fn save(&self, path: &Path) -> Result<()> {
        let json = serde_json::to_string_pretty(self).map_err(|e| Error::Interne(e.to_string()))?;
        std::fs::write(path, json).map_err(|e| Error::fichier(path, e))
    }

    /// Version la plus récente d'un paquet, éventuellement limitée à une branche.
    pub fn latest(&self, id: &str, branch: Option<&str>) -> Option<&Release> {
        self.packages
            .get(id)?
            .releases
            .iter()
            .filter(|r| branch.is_none_or(|b| r.branch() == b))
            .max_by(|a, b| version::cmp(&a.version, &b.version))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINI: &str = r#"{
        "schema": 1,
        "packages": {
            "php": { "name": "PHP", "category": "php", "releases": [
                { "version": "8.3.9",  "url": "https://x/php-8.3.9.zip",  "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" },
                { "version": "8.3.15", "url": "https://x/php-8.3.15.zip?dl=1", "sha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb" },
                { "version": "7.4.33", "url": "https://x/php-7.4.33.zip", "sha256": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc" }
            ]}
        }
    }"#;

    #[test]
    fn catalogue_integre_valide() {
        let cat = Catalog::embedded().expect("le catalogue intégré doit être valide");
        for id in ["nginx", "apache", "mariadb", "php"] {
            assert!(cat.packages.contains_key(id), "{id} absent du catalogue intégré");
        }
        // Toutes les versions PHP du catalogue intégré doivent être NTS x64 (mode FastCGI).
        for r in &cat.package("php").unwrap().releases {
            let b = r.build.as_ref().expect("build PHP manquant");
            assert!(!b.thread_safe && b.arch == "x64", "PHP {} doit être NTS x64", r.version);
        }
    }

    #[test]
    fn derniere_version_par_branche() {
        let cat = Catalog::parse(MINI, "test").unwrap();
        assert_eq!(cat.latest("php", None).unwrap().version, "8.3.15");
        assert_eq!(cat.latest("php", Some("7.4")).unwrap().version, "7.4.33");
        assert!(cat.latest("php", Some("5.6")).is_none());
        assert!(cat.latest("inconnu", None).is_none());
    }

    #[test]
    fn recherche_et_erreurs() {
        let cat = Catalog::parse(MINI, "test").unwrap();
        assert!(cat.release("php", "8.3.9").is_ok());
        assert_eq!(cat.release("php", "9.9.9").unwrap_err().code(), "catalogue.version_inconnue");
        assert_eq!(cat.release("nodejs", "1").unwrap_err().code(), "catalogue.paquet_inconnu");
    }

    #[test]
    fn nom_de_fichier_sans_parametres() {
        let cat = Catalog::parse(MINI, "test").unwrap();
        assert_eq!(cat.release("php", "8.3.15").unwrap().file_name(), "php-8.3.15.zip");
    }

    #[test]
    fn rejette_sha_http_et_doublons() {
        let json = r#"{ "schema": 1, "packages": { "nginx": { "name": "N", "category": "web", "releases": [
            { "version": "1.0", "url": "http://insecure/n.zip", "sha256": "123" },
            { "version": "1.0", "url": "https://ok/n.zip", "sha256": "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd", "subdir": "../x" }
        ]}}}"#;
        let err = Catalog::parse(json, "test").unwrap_err();
        let Error::CatalogueInvalide { problemes } = err else { panic!("mauvaise erreur") };
        let tout = problemes.join("\n");
        assert!(tout.contains("SHA256"), "{tout}");
        assert!(tout.contains("https obligatoire"), "{tout}");
        assert!(tout.contains("en double"), "{tout}");
        assert!(tout.contains("dangereux"), "{tout}");
    }

    #[test]
    fn rejette_format_inconnu_et_json_casse() {
        assert_eq!(Catalog::parse(r#"{"schema":2,"packages":{}}"#, "t").unwrap_err().code(), "catalogue.invalide");
        assert_eq!(Catalog::parse("{ pas du json", "t").unwrap_err().code(), "json.invalide");
    }

    #[test]
    fn fusion_de_nouvelles_versions() {
        let mut cat = Catalog::parse(MINI, "test").unwrap();
        let mut nouvelle = cat.release("php", "8.3.15").unwrap().clone();
        nouvelle.version = "8.3.16".into();
        let connue = cat.release("php", "8.3.9").unwrap().clone();
        let ajout = cat.merge("php", vec![nouvelle.clone(), connue]).unwrap();
        assert_eq!(ajout, ["8.3.16"]);
        assert_eq!(cat.latest("php", Some("8.3")).unwrap().version, "8.3.16");
        // Une entrée invalide est refusée.
        nouvelle.version = "8.3.17".into();
        nouvelle.sha256 = "court".into();
        assert!(cat.merge("php", vec![nouvelle]).is_err());

        let t = tempfile::tempdir().unwrap();
        let f = t.path().join("c.json");
        Catalog::parse(MINI, "test").unwrap().save(&f).unwrap();
        assert!(Catalog::load(&f).is_ok(), "aller-retour fichier");
    }

    #[test]
    fn initialise_le_fichier_portable() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::new(tmp.path());
        let cat = Catalog::load_or_init(&paths).unwrap();
        assert!(paths.catalog_file().exists());
        assert!(!cat.packages.is_empty());
    }
}
