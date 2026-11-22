//! Outils d'analyse installés par Composer dans `<racine>/tools/` (portable) :
//! PHP_CodeSniffer + PHPCompatibility, PHPStan.
//!
//! ⚠ PHPCompatibility : la dernière version stable (9.3.5, 2019) ne connaît pas
//! PHP 8. Seule la branche 10 (en alpha fin 2026) couvre PHP 8.x : on l'utilise
//! explicitement (`^10.0@alpha`).

use crate::catalog::InstalledPackage;
use crate::error::{Error, Result};
use crate::paths::Paths;
use crate::platform;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::time::Duration;

const COMPOSER_JSON: &str = r#"{
    "name": "rustserv/outils",
    "description": "Outils d'analyse gérés par Rustserv (ne pas modifier)",
    "require": {
        "squizlabs/php_codesniffer": "^4.0",
        "phpcompatibility/php-compatibility": "^10.0@alpha",
        "phpstan/phpstan": "^2.1",
        "dealerdirect/phpcodesniffer-composer-installer": "^1.0"
    },
    "config": {
        "allow-plugins": { "dealerdirect/phpcodesniffer-composer-installer": true },
        "sort-packages": true
    }
}
"#;

pub fn tools_dir(paths: &Paths) -> PathBuf {
    paths.root().join("tools")
}

pub fn phpcs_bin(paths: &Paths) -> PathBuf {
    tools_dir(paths).join("vendor/squizlabs/php_codesniffer/bin/phpcs")
}

pub fn phpstan_bin(paths: &Paths) -> PathBuf {
    tools_dir(paths).join("vendor/phpstan/phpstan/phpstan")
}

#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct ToolsStatus {
    pub installes: bool,
    pub phpcs: Option<String>,
    pub phpcompatibility: Option<String>,
    pub phpstan: Option<String>,
}

/// Versions installées, lues dans `vendor/composer/installed.json`.
pub fn status(paths: &Paths) -> ToolsStatus {
    let f = tools_dir(paths).join("vendor/composer/installed.json");
    let Ok(txt) = std::fs::read_to_string(f) else { return ToolsStatus::default() };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&txt) else { return ToolsStatus::default() };
    // Composer 2 : { "packages": [...] } ; Composer 1 : [...]
    let liste = v.get("packages").unwrap_or(&v).as_array().cloned().unwrap_or_default();
    let version = |nom: &str| {
        liste.iter().find(|p| p["name"] == nom).and_then(|p| p["version"].as_str()).map(|s| s.trim_start_matches('v').to_string())
    };
    let s = ToolsStatus {
        installes: false,
        phpcs: version("squizlabs/php_codesniffer"),
        phpcompatibility: version("phpcompatibility/php-compatibility"),
        phpstan: version("phpstan/phpstan"),
    };
    ToolsStatus { installes: s.phpcs.is_some() && s.phpcompatibility.is_some() && s.phpstan.is_some() && phpcs_bin(paths).is_file(), ..s }
}

/// Installe ou met à jour les outils avec Composer (5 min max).
/// `COMPOSER_HOME` est placé dans le dossier portable (aucune trace dans le profil Windows).
pub async fn install(paths: &Paths, php: &InstalledPackage, ini: &Path, composer: &InstalledPackage) -> Result<ToolsStatus> {
    let dir = tools_dir(paths);
    std::fs::create_dir_all(&dir).map_err(|e| Error::fichier(&dir, e))?;
    let cj = dir.join("composer.json");
    std::fs::write(&cj, COMPOSER_JSON).map_err(|e| Error::fichier(&cj, e))?;
    let phar = composer.path.join("composer.phar");
    let home = paths.tmp().join("composer");

    let mut cmd = tokio::process::Command::new(php.path.join(platform::exe_name("php")));
    cmd.arg("-c")
        .arg(ini)
        .arg(&phar)
        .args(["update", "--no-interaction", "--no-progress", "--no-ansi", "--prefer-dist"])
        .current_dir(&dir)
        .env("COMPOSER_HOME", &home)
        .env("PATH", format!("{};{}", php.path.display(), std::env::var("PATH").unwrap_or_default()))
        .env("PHPRC", ini.parent().unwrap_or(ini))
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    cmd.creation_flags(platform::CREATE_NO_WINDOW);
    tracing::info!(php = %php.version, "installation des outils d'analyse (Composer)");
    let out = tokio::time::timeout(Duration::from_secs(300), cmd.output())
        .await
        .map_err(|_| Error::CompatEchec { outil: "Composer".into(), detail: "délai de 5 minutes dépassé".into() })?
        .map_err(|e| Error::CompatEchec { outil: "Composer".into(), detail: e.to_string() })?;
    if !out.status.success() {
        let detail = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        let lignes: Vec<&str> = detail.lines().filter(|l| !l.trim().is_empty()).collect();
        return Err(Error::CompatEchec { outil: "Composer".into(), detail: lignes[lignes.len().saturating_sub(15)..].join("\n") });
    }
    let s = status(paths);
    if !s.installes {
        return Err(Error::CompatEchec { outil: "Composer".into(), detail: "outils absents après installation".into() });
    }
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn composer_json_valide() {
        let v: serde_json::Value = serde_json::from_str(COMPOSER_JSON).unwrap();
        assert_eq!(v["require"]["phpcompatibility/php-compatibility"], "^10.0@alpha");
    }

    #[test]
    fn lecture_des_versions() {
        let t = tempfile::tempdir().unwrap();
        let paths = Paths::new(t.path());
        assert!(!status(&paths).installes);
        let d = tools_dir(&paths).join("vendor/composer");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(
            d.join("installed.json"),
            r#"{"packages":[{"name":"squizlabs/php_codesniffer","version":"4.0.4"},{"name":"phpcompatibility/php-compatibility","version":"10.0.0-alpha3"},{"name":"phpstan/phpstan","version":"2.3.7"}]}"#,
        )
        .unwrap();
        let s = status(&paths);
        assert_eq!(s.phpcompatibility.as_deref(), Some("10.0.0-alpha3"));
        assert!(!s.installes, "binaire phpcs absent");
        std::fs::create_dir_all(phpcs_bin(&paths).parent().unwrap()).unwrap();
        std::fs::write(phpcs_bin(&paths), "").unwrap();
        assert!(status(&paths).installes);
    }
}
