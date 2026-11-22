//! Mise à jour d'une branche de PHP (ex. 8.3.12 → 8.3.15).
//!
//! Au sein d'une même branche, l'ABI des extensions est identique : on peut
//! donc reprendre **tel quel** le php.ini de l'ancienne version, en remplaçant
//! seulement les chemins qui pointent vers l'ancien dossier, et recopier les
//! DLL ajoutées à la main (extensions PECL, dépendances comme ImageMagick).

use super::ini::IniFile;
use super::setup::php_ini_path;
use crate::error::{Error, Result};
use crate::paths::{slash, Paths};
use std::path::Path;

/// Rapport de migration, affiché à l'utilisateur.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct MigrationReport {
    /// Le php.ini de l'ancienne version a été repris.
    pub ini_repris: bool,
    /// DLL recopiées (chemins relatifs au dossier PHP, ex. `ext/php_xdebug.dll`).
    pub dll_copiees: Vec<String>,
}

/// Reprend `etc/php/<ancienne>/php.ini` pour la nouvelle version.
/// Ne fait rien si la nouvelle version a déjà son php.ini.
pub fn migrate_ini(paths: &Paths, old_dir: &Path, old_version: &str, new_dir: &Path, new_version: &str) -> Result<bool> {
    let src = php_ini_path(paths, old_version);
    let dest = php_ini_path(paths, new_version);
    if dest.exists() || !src.is_file() {
        return Ok(false);
    }
    let text = std::fs::read_to_string(&src).map_err(|e| Error::fichier(&src, e))?;
    // Remplacement des chemins : dossier d'installation (avec / et \) et noms de journaux.
    let mut out = text;
    for (a, b) in [
        (slash(old_dir), slash(new_dir)),
        (old_dir.display().to_string(), new_dir.display().to_string()),
        (format!("php-{old_version}-"), format!("php-{new_version}-")),
    ] {
        out = out.replace(&a, &b);
    }
    // Vérification de cohérence : l'ini doit rester lisible.
    let _ = IniFile::parse(&out);
    let dir = dest.parent().expect("php.ini sans dossier parent");
    std::fs::create_dir_all(dir).map_err(|e| Error::fichier(dir, e))?;
    std::fs::write(&dest, out).map_err(|e| Error::fichier(&dest, e))?;
    // Journal des extensions PECL : mêmes DLL recopiées (copy_extra_dlls), même branche.
    let pecl_src = src.with_file_name("pecl.json");
    if pecl_src.is_file() {
        let pecl_dest = dest.with_file_name("pecl.json");
        std::fs::copy(&pecl_src, &pecl_dest).map_err(|e| Error::fichier(&pecl_dest, e))?;
    }
    tracing::info!(old_version, new_version, "php.ini repris");
    Ok(true)
}

/// Recopie les DLL présentes dans l'ancienne version mais absentes de la nouvelle
/// (dossier racine et `ext/`). Les DLL officielles de la nouvelle version ne sont jamais écrasées.
pub fn copy_extra_dlls(old_dir: &Path, new_dir: &Path) -> Result<Vec<String>> {
    let mut copiees = Vec::new();
    for sous in ["", "ext"] {
        let (src, dst) = (old_dir.join(sous), new_dir.join(sous));
        let Ok(entries) = std::fs::read_dir(&src) else { continue };
        for e in entries.flatten() {
            let p = e.path();
            let is_dll = p.extension().is_some_and(|x| x.eq_ignore_ascii_case("dll"));
            if !is_dll || !p.is_file() {
                continue;
            }
            let cible = dst.join(e.file_name());
            if cible.exists() {
                continue;
            }
            std::fs::create_dir_all(&dst).map_err(|er| Error::fichier(&dst, er))?;
            std::fs::copy(&p, &cible).map_err(|er| Error::fichier(&cible, er))?;
            let rel = if sous.is_empty() { e.file_name().to_string_lossy().into_owned() } else { format!("{sous}/{}", e.file_name().to_string_lossy()) };
            copiees.push(rel);
        }
    }
    copiees.sort();
    Ok(copiees)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reprend_le_php_ini_et_les_dll() {
        let t = tempfile::tempdir().unwrap();
        let paths = Paths::new(t.path().join("root"));
        paths.ensure_layout().unwrap();
        let old = paths.bin().join("php").join("php-8.3.12");
        let new = paths.bin().join("php").join("php-8.3.15");
        for d in [&old, &new] {
            std::fs::create_dir_all(d.join("ext")).unwrap();
            std::fs::write(d.join("ext").join("php_curl.dll"), "officielle").unwrap();
            std::fs::write(d.join("php8ts.dll"), "officielle").unwrap();
        }
        // Ajouts manuels dans l'ancienne version.
        std::fs::write(old.join("ext").join("php_xdebug.dll"), "pecl").unwrap();
        std::fs::write(old.join("CORE_RL_MagickCore_.dll"), "imagick").unwrap();
        std::fs::write(old.join("ext").join("php_curl.dll"), "ancienne").unwrap();

        let ini_old = php_ini_path(&paths, "8.3.12");
        std::fs::create_dir_all(ini_old.parent().unwrap()).unwrap();
        std::fs::write(
            &ini_old,
            format!(
                "memory_limit = 777M\r\nextension_dir = \"{}/ext\"\r\nerror_log = \"{}/php-8.3.12-error.log\"\r\nzend_extension=\"{}\\ext\\php_xdebug.dll\"\r\n",
                slash(&old),
                slash(&paths.logs()),
                old.display()
            ),
        )
        .unwrap();

        std::fs::write(ini_old.with_file_name("pecl.json"), "{}").unwrap();
        assert!(migrate_ini(&paths, &old, "8.3.12", &new, "8.3.15").unwrap());
        assert!(php_ini_path(&paths, "8.3.15").with_file_name("pecl.json").is_file(), "journal PECL repris");
        let ini = IniFile::parse(&std::fs::read_to_string(php_ini_path(&paths, "8.3.15")).unwrap());
        assert_eq!(ini.get("memory_limit").as_deref(), Some("777M"), "réglage personnalisé conservé");
        assert_eq!(ini.get("extension_dir").unwrap(), format!("{}/ext", slash(&new)));
        assert!(ini.get("error_log").unwrap().ends_with("php-8.3.15-error.log"));
        assert!(ini.get("zend_extension").unwrap().contains("php-8.3.15"));
        assert!(!migrate_ini(&paths, &old, "8.3.12", &new, "8.3.15").unwrap(), "jamais d'écrasement");

        let c = copy_extra_dlls(&old, &new).unwrap();
        assert_eq!(c, ["CORE_RL_MagickCore_.dll", "ext/php_xdebug.dll"]);
        assert_eq!(std::fs::read_to_string(new.join("ext").join("php_curl.dll")).unwrap(), "officielle");
    }
}
