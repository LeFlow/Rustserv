//! Préparation d'une version de PHP installée : php.ini initial et ports du pool.

use super::ini::{quote, IniFile};
use crate::catalog::version;
use crate::error::{Error, Result};
use crate::paths::{slash, Paths};
use crate::settings::Settings;
use std::path::{Path, PathBuf};

/// Extensions activées par défaut (si la DLL existe pour cette version).
/// L'ordre compte : mbstring avant exif, mysqlnd est intégré au cœur depuis PHP 7.
const EXTENSIONS_PAR_DEFAUT: &[&str] = &[
    "curl", "fileinfo", "gd", "intl", "mbstring", "exif", "mysqli", "openssl", "pdo_mysql", "pdo_sqlite", "sqlite3",
    "sodium", "zip",
];

/// `etc/php/<version>/php.ini`
pub fn php_ini_path(paths: &Paths, version: &str) -> PathBuf {
    paths.etc().join("php").join(version).join("php.ini")
}

/// Ports du pool php-cgi : PHP X.Y → 9XY0, 9XY1… (8.3 → 9830-9833, 7.4 → 9740-9743).
pub fn pool_ports(php_version: &str, size: u8) -> Vec<u16> {
    let (n, _) = version::parse(php_version);
    let major = n.first().copied().unwrap_or(0) as u16;
    let minor = n.get(1).copied().unwrap_or(0) as u16;
    let base = 9000 + major * 100 + minor.min(9) * 10;
    (0..size.min(10) as u16).map(|i| base + i).collect()
}

/// Crée `etc/php/<version>/php.ini` à partir de `php.ini-development` s'il n'existe pas.
/// Ne modifie **jamais** un php.ini existant (l'utilisateur a pu le personnaliser).
pub fn ensure_php_ini(paths: &Paths, php_dir: &Path, version: &str, settings: &Settings) -> Result<PathBuf> {
    // Dossiers temporaires de PHP (sessions, envois de fichiers) : recréés à **chaque** fois,
    // car tmp/ peut être vidé ou absent (copie de la version portable sans tmp/, nettoyage).
    // Sans eux : « session_start(): open(...) failed » (Adminer, WordPress…).
    let tmp = paths.tmp().join("php");
    for d in ["sessions", "uploads"] {
        let dir = tmp.join(d);
        std::fs::create_dir_all(&dir).map_err(|e| Error::fichier(&dir, e))?;
    }
    let dest = php_ini_path(paths, version);
    if dest.exists() {
        return Ok(dest);
    }
    let source = ["php.ini-development", "php.ini-production"].iter().map(|f| php_dir.join(f)).find(|p| p.is_file());
    let text = match &source {
        Some(p) => std::fs::read_to_string(p).map_err(|e| Error::fichier(p, e))?,
        None => String::new(),
    };
    let mut ini = IniFile::parse(&text);
    configure(&mut ini, paths, php_dir, version, settings);

    let dir = dest.parent().expect("chemin php.ini sans parent");
    std::fs::create_dir_all(dir).map_err(|e| Error::fichier(dir, e))?;
    std::fs::write(&dest, ini.to_text()).map_err(|e| Error::fichier(&dest, e))?;
    tracing::info!(version, fichier = %dest.display(), "php.ini créé");
    Ok(dest)
}

/// Réglages adaptés au développement local et au mode portable.
fn configure(ini: &mut IniFile, paths: &Paths, php_dir: &Path, version: &str, settings: &Settings) {
    let tmp = paths.tmp().join("php");
    let ext_dir = php_dir.join("ext");
    ini.set("extension_dir", &quote(&slash(&ext_dir)));
    ini.set("error_log", &quote(&slash(&paths.logs().join(format!("php-{version}-error.log")))));
    ini.set("date.timezone", &settings.timezone);
    ini.set("memory_limit", "512M");
    ini.set("max_execution_time", "120");
    ini.set("upload_max_filesize", "256M");
    ini.set("post_max_size", "256M");
    ini.set("session.save_path", &quote(&slash(&tmp.join("sessions"))));
    ini.set("upload_tmp_dir", &quote(&slash(&tmp.join("uploads"))));
    ini.set("sys_temp_dir", &quote(&slash(&tmp)));
    // php-cgi derrière nginx/Apache en FastCGI : pas de REDIRECT_STATUS.
    ini.set("cgi.force_redirect", "0");
    ini.set("cgi.fix_pathinfo", "1");

    for name in EXTENSIONS_PAR_DEFAUT {
        // PHP 7.4 nomme encore gd « gd2 ».
        let candidates: &[&str] = if *name == "gd" { &["gd", "gd2"] } else { std::slice::from_ref(name) };
        if let Some(c) = candidates.iter().find(|c| ext_dir.join(format!("php_{c}.dll")).is_file()) {
            ini.enable_extension(c, false);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ports_par_version() {
        assert_eq!(pool_ports("8.3.37", 4), vec![9830, 9831, 9832, 9833]);
        assert_eq!(pool_ports("7.4.33", 2), vec![9740, 9741]);
        assert_eq!(pool_ports("8.6.0", 1), vec![9860]);
        assert_eq!(pool_ports("8.3.1", 50).len(), 10, "pool borné à 10");
    }

    #[test]
    fn dossiers_temporaires_recrees_si_tmp_vide() {
        let t = tempfile::tempdir().unwrap();
        let paths = Paths::new(t.path().join("root"));
        paths.ensure_layout().unwrap();
        let php = t.path().join("php-8.3.37");
        std::fs::create_dir_all(&php).unwrap();
        ensure_php_ini(&paths, &php, "8.3.37", &Settings::default()).unwrap();
        // Copie portable sans tmp/ : le php.ini existe déjà, les dossiers non.
        std::fs::remove_dir_all(paths.tmp()).unwrap();
        ensure_php_ini(&paths, &php, "8.3.37", &Settings::default()).unwrap();
        assert!(paths.tmp().join("php").join("sessions").is_dir());
        assert!(paths.tmp().join("php").join("uploads").is_dir());
    }

    #[test]
    fn cree_le_php_ini_sans_ecraser() {
        let t = tempfile::tempdir().unwrap();
        let paths = Paths::new(t.path().join("root"));
        paths.ensure_layout().unwrap();
        let php = t.path().join("php-8.3.37");
        std::fs::create_dir_all(php.join("ext")).unwrap();
        for e in ["curl", "mbstring", "exif", "gd"] {
            std::fs::write(php.join("ext").join(format!("php_{e}.dll")), "").unwrap();
        }
        std::fs::write(
            php.join("php.ini-development"),
            "[PHP]\r\nmemory_limit = 128M\r\n;extension_dir = \"ext\"\r\n;extension=curl\r\n;extension=gd\r\n;extension=intl\r\n;extension=mbstring\r\n;extension=exif\r\n",
        )
        .unwrap();

        let f = ensure_php_ini(&paths, &php, "8.3.37", &Settings::default()).unwrap();
        let ini = IniFile::parse(&std::fs::read_to_string(&f).unwrap());
        assert_eq!(ini.get("memory_limit").as_deref(), Some("512M"));
        assert!(ini.get("extension_dir").unwrap().ends_with("php-8.3.37/ext"));
        assert_eq!(ini.get("date.timezone").as_deref(), Some("Europe/Paris"));
        for e in ["curl", "gd", "mbstring", "exif"] {
            assert!(ini.is_extension_enabled(e), "{e} devrait être activée");
        }
        assert!(!ini.is_extension_enabled("intl"), "DLL absente : extension non activée");
        // mbstring reste avant exif (ordre du fichier d'origine).
        let pos = |n: &str| ini.lines().iter().position(|l| l == &format!("extension={n}")).unwrap();
        assert!(pos("mbstring") < pos("exif"));

        // Un php.ini existant n'est jamais écrasé.
        std::fs::write(&f, "perso = 1\n").unwrap();
        ensure_php_ini(&paths, &php, "8.3.37", &Settings::default()).unwrap();
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "perso = 1\n");
    }
}
