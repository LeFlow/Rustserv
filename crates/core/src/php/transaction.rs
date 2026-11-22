//! Modification **transactionnelle** d'un php.ini :
//! 1. sauvegarde horodatée dans `etc/php/<version>/backups/` (10 dernières gardées) ;
//! 2. application des changements ;
//! 3. validation `php -m` ;
//! 4. en cas d'échec : restauration du fichier d'origine et erreur explicite.

use super::check::{check, PhpCheck};
use super::ini::IniFile;
use super::setup::php_ini_path;
use crate::error::{Error, Result};
use crate::paths::Paths;
use std::path::{Path, PathBuf};

/// Nombre de sauvegardes conservées par version.
const SAUVEGARDES_MAX: usize = 10;

/// Dossier des sauvegardes d'une version.
pub fn backups_dir(paths: &Paths, version: &str) -> PathBuf {
    php_ini_path(paths, version).parent().expect("php.ini sans parent").join("backups")
}

/// Copie le php.ini dans le dossier de sauvegardes ; renvoie le chemin de la copie.
pub fn backup(paths: &Paths, version: &str) -> Result<PathBuf> {
    let src = php_ini_path(paths, version);
    let dir = backups_dir(paths, version);
    std::fs::create_dir_all(&dir).map_err(|e| Error::fichier(&dir, e))?;
    let ts = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    let dest = dir.join(format!("php.ini.{ts}"));
    std::fs::copy(&src, &dest).map_err(|e| Error::fichier(&dest, e))?;
    purge(&dir);
    Ok(dest)
}

/// Supprime les sauvegardes les plus anciennes au-delà de `SAUVEGARDES_MAX`.
fn purge(dir: &Path) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut files: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.is_file()).collect();
    // Le nom contient l'horodatage en millisecondes : l'ordre alphabétique suffit à longueur égale.
    files.sort_by_key(|p| (p.as_os_str().len(), p.clone()));
    let trop = files.len().saturating_sub(SAUVEGARDES_MAX);
    for f in files.into_iter().take(trop) {
        let _ = std::fs::remove_file(f);
    }
}

/// Applique `modifier` au php.ini de `version`, valide avec `php -m`, et restaure
/// l'original en cas d'échec. Ne fait rien (pas de sauvegarde) si rien n'a changé.
pub fn modify(
    paths: &Paths,
    php_dir: &Path,
    version: &str,
    modifier: impl FnOnce(&mut IniFile) -> Result<()>,
) -> Result<PhpCheck> {
    let ini_path = php_ini_path(paths, version);
    let original = std::fs::read_to_string(&ini_path).map_err(|e| Error::fichier(&ini_path, e))?;
    let mut ini = IniFile::parse(&original);
    modifier(&mut ini)?;
    let nouveau = ini.to_text();
    if nouveau == original {
        return check(php_dir, version, &ini_path);
    }
    backup(paths, version)?;
    std::fs::write(&ini_path, &nouveau).map_err(|e| Error::fichier(&ini_path, e))?;
    match check(php_dir, version, &ini_path) {
        Ok(c) => Ok(c),
        Err(e) => {
            tracing::warn!(version, erreur = %e, "php.ini refusé par PHP, restauration");
            std::fs::write(&ini_path, &original).map_err(|er| Error::fichier(&ini_path, er))?;
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn purge_les_vieilles_sauvegardes() {
        let t = tempfile::tempdir().unwrap();
        for i in 0..15u64 {
            std::fs::write(t.path().join(format!("php.ini.{}", 1_700_000_000_000 + i)), "").unwrap();
        }
        purge(t.path());
        let mut restants: Vec<_> = std::fs::read_dir(t.path()).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
        restants.sort();
        assert_eq!(restants.len(), SAUVEGARDES_MAX);
        assert_eq!(restants[0], "php.ini.1700000000005", "les plus anciennes sont supprimées");
    }
}
