//! Dossier Rustserv **déplaçable** (portable).
//!
//! Certains fichiers contiennent le chemin complet du dossier Rustserv : surtout les
//! php.ini (`extension_dir`, `error_log`, sessions…), créés une seule fois et jamais
//! régénérés. Si le dossier est déplacé (`C:\Users\…\portable` → `C:\Rustserv`, clé USB…),
//! PHP ne trouverait plus ses extensions.
//!
//! Au démarrage, Rustserv compare son dossier actuel à celui mémorisé dans
//! `etc/racine.txt` ; s'ils diffèrent, l'ancien chemin est remplacé par le nouveau dans
//! les fichiers de configuration (`etc/**`, `data/*/my.ini`), sous ses deux formes
//! (`C:\…` et `C:/…`).

use crate::error::{Error, Result};
use crate::paths::{slash, Paths};
use std::path::{Path, PathBuf};

/// Extensions des fichiers de configuration à corriger.
const EXTENSIONS: &[&str] = &["ini", "conf", "php", "neon", "json"];

fn marque(paths: &Paths) -> PathBuf {
    paths.etc().join("racine.txt")
}

fn fichiers(dossier: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dossier) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            fichiers(&p, out);
        } else if p.extension().and_then(|x| x.to_str()).is_some_and(|x| EXTENSIONS.contains(&x.to_ascii_lowercase().as_str()))
            // Sauvegardes horodatées des php.ini (« php.ini.1795320050758 ») : à corriger aussi,
            // sinon une restauration réintroduirait l'ancien chemin.
            || p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("php.ini."))
        {
            out.push(p);
        }
    }
}

/// Corrige les chemins si le dossier a été déplacé. Renvoie les fichiers modifiés.
/// À appeler au démarrage, avant toute lecture de configuration.
pub fn relocate(paths: &Paths) -> Result<Vec<PathBuf>> {
    let actuelle = paths.root().display().to_string();
    let f = marque(paths);
    let ancienne = std::fs::read_to_string(&f).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    let mut modifies = Vec::new();
    if let Some(ancienne) = ancienne.filter(|a| !a.eq_ignore_ascii_case(&actuelle)) {
        let remplacements = [
            (ancienne.replace('/', "\\"), actuelle.replace('/', "\\")),
            (slash(Path::new(&ancienne)), slash(paths.root())),
        ];
        let mut cibles = Vec::new();
        fichiers(&paths.etc(), &mut cibles);
        if let Ok(rd) = std::fs::read_dir(paths.data()) {
            cibles.extend(rd.flatten().map(|e| e.path().join("my.ini")).filter(|p| p.is_file()));
        }
        for c in cibles {
            let Ok(texte) = std::fs::read_to_string(&c) else { continue };
            let mut nouveau = texte.clone();
            for (a, b) in &remplacements {
                nouveau = nouveau.replace(a.as_str(), b);
            }
            if nouveau != texte {
                std::fs::write(&c, nouveau).map_err(|e| Error::fichier(&c, e))?;
                modifies.push(c);
            }
        }
        tracing::info!(de = %ancienne, vers = %actuelle, fichiers = modifies.len(), "dossier Rustserv déplacé : chemins corrigés");
    }
    std::fs::write(&f, &actuelle).map_err(|e| Error::fichier(&f, e))?;
    Ok(modifies)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deplacement_du_dossier() {
        let t = tempfile::tempdir().unwrap();
        let ancien = Paths::new(t.path().join("ancien"));
        ancien.ensure_layout().unwrap();
        let r = slash(ancien.root());
        let ini = ancien.etc().join("php").join("8.3.37").join("php.ini");
        std::fs::create_dir_all(ini.parent().unwrap()).unwrap();
        std::fs::write(&ini, format!("extension_dir = \"{r}/bin/php/php-8.3.37/ext\"\r\nerror_log = \"{}\\logs\\php.log\"\r\n", ancien.root().display())).unwrap();
        std::fs::create_dir_all(ini.parent().unwrap().join("backups")).unwrap();
        std::fs::write(ini.parent().unwrap().join("backups").join("php.ini.1795320050758"), format!("extension_dir = \"{r}/ext\"")).unwrap();
        std::fs::create_dir_all(ancien.data().join("mariadb-12.3")).unwrap();
        std::fs::write(ancien.data().join("mariadb-12.3").join("my.ini"), format!("datadir={r}/data/mariadb-12.3\n")).unwrap();
        // Premier démarrage : rien à corriger, la racine est mémorisée.
        assert!(relocate(&ancien).unwrap().is_empty());

        // Le dossier est déplacé.
        let nouveau_dir = t.path().join("Rustserv");
        std::fs::rename(ancien.root(), &nouveau_dir).unwrap();
        let nouveau = Paths::new(&nouveau_dir);
        let modifies = relocate(&nouveau).unwrap();
        assert_eq!(modifies.len(), 3, "php.ini, sa sauvegarde et my.ini");
        let txt = std::fs::read_to_string(nouveau.etc().join("php").join("8.3.37").join("php.ini")).unwrap();
        let n = slash(nouveau.root());
        assert!(txt.contains(&format!("{n}/bin/php/php-8.3.37/ext")), "{txt}");
        assert!(txt.contains(&format!("{}\\logs\\php.log", nouveau.root().display())), "{txt}");
        assert!(!txt.contains("ancien"), "{txt}");
        // Démarrage suivant : plus rien à faire.
        assert!(relocate(&nouveau).unwrap().is_empty());
    }
}
