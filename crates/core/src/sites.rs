//! Détection des sites : chaque dossier de `www/` devient `<nom>.<tld>`.
//!
//! Racine web : `public/` si elle existe (Laravel, Symfony 4+), sinon `web/`
//! (Symfony 2/3), sinon le dossier lui-même (WordPress, PHP simple).

use crate::error::Result;
use crate::paths::Paths;
use serde::Serialize;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Site {
    /// Nom du dossier (`monsite`).
    pub name: String,
    /// Domaine (`monsite.test`).
    pub host: String,
    /// Dossier du projet.
    pub dir: PathBuf,
    /// Racine web servie.
    pub root: PathBuf,
}

/// Un nom de dossier est utilisable comme domaine (lettres, chiffres, tirets).
pub fn valid_name(n: &str) -> bool {
    !n.is_empty()
        && n.len() <= 63
        && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        && !n.starts_with('-')
        && !n.ends_with('-')
}

/// Liste les sites de `www/`, triés par nom. Les dossiers au nom invalide
/// (espaces, accents, `_`…) sont ignorés : ils restent accessibles via http://localhost/<dossier>/.
pub fn discover(paths: &Paths, tld: &str) -> Result<Vec<Site>> {
    let mut sites = Vec::new();
    let Ok(entries) = std::fs::read_dir(paths.www()) else { return Ok(sites) };
    for e in entries.flatten() {
        let dir = e.path();
        if !dir.is_dir() {
            continue;
        }
        let raw = e.file_name().to_string_lossy().into_owned();
        if !valid_name(&raw) {
            continue;
        }
        let name = raw.to_ascii_lowercase();
        let root = ["public", "web"].iter().map(|d| dir.join(d)).find(|p| p.is_dir()).unwrap_or_else(|| dir.clone());
        sites.push(Site { host: format!("{name}.{tld}"), name, dir, root });
    }
    sites.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(sites)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detecte_les_sites() {
        let t = tempfile::tempdir().unwrap();
        let p = Paths::new(t.path());
        p.ensure_layout().unwrap();
        for d in ["blog", "Laravel-App/public", "mon site", "_prive", "sf3/web"] {
            std::fs::create_dir_all(p.www().join(d)).unwrap();
        }
        std::fs::write(p.www().join("fichier.php"), "").unwrap();
        let s = discover(&p, "test").unwrap();
        let noms: Vec<_> = s.iter().map(|s| s.host.as_str()).collect();
        assert_eq!(noms, ["blog.test", "laravel-app.test", "sf3.test"]);
        assert!(s[1].root.ends_with("public"));
        assert!(s[2].root.ends_with("web"));
        assert_eq!(s[0].root, s[0].dir);
    }
}
