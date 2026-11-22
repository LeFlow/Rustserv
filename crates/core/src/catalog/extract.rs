//! Extraction sécurisée des archives zip.
//!
//! Protection « zip slip » : toute entrée dont le chemin sortirait du dossier
//! de destination (`..`, chemin absolu) fait échouer l'extraction.

use crate::error::{Error, Result};
use std::fs;
use std::path::{Path, PathBuf};

/// Extrait `archive` dans `out`. `on_progress(fichiers_faits, total)`.
pub fn extract_zip(archive: &Path, out: &Path, mut on_progress: impl FnMut(u64, u64)) -> Result<()> {
    let label = archive.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let invalide = |d: String| Error::ArchiveInvalide { fichier: label.clone(), detail: d };

    let f = fs::File::open(archive).map_err(|e| Error::fichier(archive, e))?;
    let mut zip = zip::ZipArchive::new(f).map_err(|e| invalide(e.to_string()))?;
    let total = zip.len() as u64;
    fs::create_dir_all(out).map_err(|e| Error::fichier(out, e))?;

    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).map_err(|e| invalide(e.to_string()))?;
        let nom = entry.name().map(|n| n.into_owned()).unwrap_or_else(|_| "?".into());
        let rel = entry
            .enclosed_name()
            .ok_or_else(|| invalide(format!("chemin dangereux refusé : {nom}")))?;
        let cible = out.join(rel);
        if entry.is_dir() {
            fs::create_dir_all(&cible).map_err(|e| Error::fichier(&cible, e))?;
        } else {
            if let Some(parent) = cible.parent() {
                fs::create_dir_all(parent).map_err(|e| Error::fichier(parent, e))?;
            }
            let mut w = fs::File::create(&cible).map_err(|e| Error::fichier(&cible, e))?;
            std::io::copy(&mut entry, &mut w).map_err(|e| invalide(format!("{nom} : {e}")))?;
        }
        if i % 50 == 0 || i as u64 + 1 == total {
            on_progress(i as u64 + 1, total);
        }
    }
    Ok(())
}

/// Dossier à installer réellement après extraction :
/// - `subdir` s'il est indiqué dans le catalogue (ex. `Apache24`) ;
/// - sinon, si l'archive ne contient qu'un seul dossier de premier niveau, celui-ci ;
/// - sinon, le dossier d'extraction lui-même (cas de PHP).
pub fn content_root(extracted: &Path, subdir: Option<&str>, label: &str) -> Result<PathBuf> {
    if let Some(s) = subdir {
        let p = extracted.join(s);
        return if p.is_dir() {
            Ok(p)
        } else {
            Err(Error::ArchiveInvalide { fichier: label.into(), detail: format!("dossier « {s} » introuvable dans l'archive") })
        };
    }
    let entries: Vec<_> = fs::read_dir(extracted)
        .map_err(|e| Error::fichier(extracted, e))?
        .filter_map(|e| e.ok())
        .collect();
    match entries.as_slice() {
        [seule] if seule.path().is_dir() => Ok(seule.path()),
        [] => Err(Error::ArchiveInvalide { fichier: label.into(), detail: "archive vide".into() }),
        _ => Ok(extracted.to_path_buf()),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::io::Write;
    use zip::write::SimpleFileOptions;

    /// Crée un zip de test avec les entrées (chemin, contenu) données.
    pub fn make_zip(path: &Path, entries: &[(&str, &str)]) {
        let f = fs::File::create(path).unwrap();
        let mut z = zip::ZipWriter::new(f);
        for (name, content) in entries {
            if name.ends_with('/') {
                z.add_directory(*name, SimpleFileOptions::default()).unwrap();
            } else {
                z.start_file(*name, SimpleFileOptions::default()).unwrap();
                z.write_all(content.as_bytes()).unwrap();
            }
        }
        z.finish().unwrap();
    }

    #[test]
    fn retire_le_dossier_unique() {
        let t = tempfile::tempdir().unwrap();
        let a = t.path().join("n.zip");
        make_zip(&a, &[("nginx-1.0/", ""), ("nginx-1.0/nginx.exe", "MZ"), ("nginx-1.0/conf/nginx.conf", "x")]);
        let out = t.path().join("out");
        extract_zip(&a, &out, |_, _| {}).unwrap();
        let root = content_root(&out, None, "n.zip").unwrap();
        assert!(root.join("nginx.exe").is_file());
        assert!(root.join("conf/nginx.conf").is_file());
    }

    #[test]
    fn archive_a_plat_garde_la_racine() {
        let t = tempfile::tempdir().unwrap();
        let a = t.path().join("php.zip");
        make_zip(&a, &[("php.exe", "MZ"), ("php-cgi.exe", "MZ"), ("ext/php_curl.dll", "MZ")]);
        let out = t.path().join("out");
        extract_zip(&a, &out, |_, _| {}).unwrap();
        assert_eq!(content_root(&out, None, "php.zip").unwrap(), out);
    }

    #[test]
    fn sous_dossier_explicite() {
        let t = tempfile::tempdir().unwrap();
        let a = t.path().join("httpd.zip");
        make_zip(&a, &[("ReadMe.txt", "x"), ("Apache24/bin/httpd.exe", "MZ")]);
        let out = t.path().join("out");
        extract_zip(&a, &out, |_, _| {}).unwrap();
        assert!(content_root(&out, Some("Apache24"), "httpd.zip").unwrap().join("bin/httpd.exe").is_file());
        assert_eq!(content_root(&out, Some("Absent"), "httpd.zip").unwrap_err().code(), "installation.archive");
    }

    #[test]
    fn refuse_le_zip_slip() {
        let t = tempfile::tempdir().unwrap();
        let a = t.path().join("evil.zip");
        make_zip(&a, &[("../../evil.txt", "pwned")]);
        let err = extract_zip(&a, &t.path().join("out"), |_, _| {}).unwrap_err();
        assert_eq!(err.code(), "installation.archive");
        assert!(!t.path().join("evil.txt").exists());
    }

    #[test]
    fn fichier_non_zip() {
        let t = tempfile::tempdir().unwrap();
        let a = t.path().join("faux.zip");
        fs::write(&a, "ceci n'est pas un zip").unwrap();
        assert_eq!(extract_zip(&a, &t.path().join("o"), |_, _| {}).unwrap_err().code(), "installation.archive");
    }
}
