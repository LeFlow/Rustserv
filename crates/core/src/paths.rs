//! Résolution du dossier portable et de ses sous-dossiers.
//!
//! Tout Rustserv vit dans **un seul dossier** (la « racine ») :
//!
//! ```text
//! <racine>/
//! ├── bin/     binaires téléchargés (php-8.3.15, nginx-1.27.x, mariadb-…)
//! ├── etc/     configurations générées (nginx, apache, php/<version>/php.ini, my.ini)
//! ├── data/    bases de données
//! ├── www/     projets (un dossier = un site <nom>.test)
//! ├── ssl/     autorité de certification locale + certificats
//! ├── logs/    journaux de l'app et des services
//! ├── tmp/     téléchargements en cours, extractions, profil WebView2
//! └── catalog.json
//! ```
//!
//! Ordre de détection de la racine :
//! 1. variable d'environnement `RUSTSERV_ROOT` (utile pour les tests) ;
//! 2. en build de développement : `<dépôt>/portable` ;
//! 3. sinon : le dossier qui contient l'exécutable (mode portable réel).

use crate::error::{Error, Result};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Paths {
    root: PathBuf,
}

impl Paths {
    /// Crée un `Paths` à partir d'une racine explicite.
    /// Le chemin est rendu absolu et nettoyé des `..` : php-cgi refuse les
    /// SCRIPT_FILENAME contenant `..` (« No input file specified »).
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let root: PathBuf = root.into();
        let abs = if root.is_absolute() {
            root
        } else {
            std::env::current_dir().map(|d| d.join(&root)).unwrap_or(root)
        };
        Self { root: normalize(&abs) }
    }

    /// Détecte automatiquement la racine (voir doc du module).
    pub fn detect() -> Result<Self> {
        if let Some(r) = std::env::var_os("RUSTSERV_ROOT") {
            return Ok(Self::new(r));
        }
        if cfg!(debug_assertions) {
            // CARGO_MANIFEST_DIR = <dépôt>/crates/core au moment de la compilation.
            let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
            let root = repo.join("portable");
            return Ok(Self::new(normalize(&root)));
        }
        let exe = std::env::current_exe()?;
        let dir = exe
            .parent()
            .ok_or_else(|| Error::Interne("exécutable sans dossier parent".into()))?;
        Ok(Self::new(dir))
    }

    pub fn root(&self) -> &Path { &self.root }
    pub fn bin(&self) -> PathBuf { self.root.join("bin") }
    pub fn etc(&self) -> PathBuf { self.root.join("etc") }
    pub fn data(&self) -> PathBuf { self.root.join("data") }
    pub fn www(&self) -> PathBuf { self.root.join("www") }
    pub fn ssl(&self) -> PathBuf { self.root.join("ssl") }
    pub fn logs(&self) -> PathBuf { self.root.join("logs") }
    pub fn tmp(&self) -> PathBuf { self.root.join("tmp") }
    pub fn catalog_file(&self) -> PathBuf { self.root.join("catalog.json") }
    /// Réglages utilisateur de l'application (ports, services actifs…).
    pub fn settings_file(&self) -> PathBuf { self.etc().join("rustserv.json") }

    /// Crée l'arborescence si elle n'existe pas encore (idempotent).
    pub fn ensure_layout(&self) -> Result<()> {
        for dir in [self.bin(), self.etc(), self.data(), self.www(), self.ssl(), self.logs(), self.tmp()] {
            std::fs::create_dir_all(&dir).map_err(|e| Error::fichier(&dir, e))?;
        }
        Ok(())
    }
}

/// Chemin avec des `/` : format attendu par nginx, Apache et php.ini sous Windows
/// (évite les problèmes d'échappement des `\`).
pub fn slash(p: &Path) -> String {
    p.display().to_string().replace('\\', "/")
}

/// Supprime les composants `..` sans exiger que le chemin existe
/// (contrairement à `canonicalize`, qui ajoute en plus un préfixe `\\?\` gênant).
fn normalize(p: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::ParentDir => { out.pop(); }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cree_l_arborescence() {
        let tmp = tempfile::tempdir().unwrap();
        let p = Paths::new(tmp.path());
        p.ensure_layout().unwrap();
        for d in ["bin", "etc", "data", "www", "ssl", "logs", "tmp"] {
            assert!(tmp.path().join(d).is_dir(), "{d} manquant");
        }
        // Idempotent : un second appel ne doit pas échouer.
        p.ensure_layout().unwrap();
    }

    #[test]
    fn normalise_les_points() {
        let n = normalize(Path::new(r"C:\a\b\..\..\c"));
        assert_eq!(n, PathBuf::from(r"C:\c"));
    }
}
