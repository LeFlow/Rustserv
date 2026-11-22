//! Validation d'une installation PHP + php.ini avec `php -m`.
//!
//! PHP démarre même quand une extension ne se charge pas : il affiche
//! seulement un avertissement (« Unable to load dynamic library… »). On
//! considère donc comme une erreur tout avertissement de démarrage.

use crate::error::{Error, Result};
use crate::platform;
use std::path::Path;

/// Résultat d'une validation réussie.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhpCheck {
    /// Modules chargés, en minuscules (`core`, `curl`, `zend opcache`…).
    pub modules: Vec<String>,
}

/// Lance `php -c <ini> -m` et vérifie l'absence d'avertissement de démarrage.
pub fn check(php_dir: &Path, version: &str, ini: &Path) -> Result<PhpCheck> {
    let exe = php_dir.join(platform::exe_name("php"));
    if !exe.is_file() {
        return Err(Error::ExecutableIntrouvable { chemin: exe });
    }
    let mut cmd = std::process::Command::new(&exe);
    cmd.arg("-c").arg(ini).arg("-m").current_dir(php_dir);
    // Les DLL dépendantes (libssl…) sont dans le dossier de PHP.
    let path = format!("{};{}", php_dir.display(), std::env::var("PATH").unwrap_or_default());
    cmd.env("PATH", path);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(platform::CREATE_NO_WINDOW);
    }
    let out = cmd.output().map_err(|e| Error::PhpInvalide { version: version.into(), detail: e.to_string() })?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    parse(version, out.status.success(), &stdout, &stderr)
}

/// Analyse la sortie de `php -m` (séparé pour être testable sans PHP).
pub fn parse(version: &str, success: bool, stdout: &str, stderr: &str) -> Result<PhpCheck> {
    let problemes: Vec<&str> = stdout
        .lines()
        .chain(stderr.lines())
        .map(str::trim)
        .filter(|l| {
            let l = l.to_ascii_lowercase();
            l.contains("startup:") || l.starts_with("php warning") || l.starts_with("warning:") || l.contains("fatal error")
        })
        .collect();
    if !success || !problemes.is_empty() {
        let detail = if problemes.is_empty() { stderr.trim().to_string() } else { problemes.join("\n") };
        return Err(Error::PhpInvalide { version: version.into(), detail });
    }
    let modules = stdout
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('['))
        .map(str::to_ascii_lowercase)
        .collect();
    Ok(PhpCheck { modules })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sortie_correcte() {
        let out = "[PHP Modules]\r\nCore\r\ncurl\r\nmbstring\r\n\r\n[Zend Modules]\r\nZend OPcache\r\n";
        let c = parse("8.3.37", true, out, "").unwrap();
        assert_eq!(c.modules, ["core", "curl", "mbstring", "zend opcache"]);
    }

    #[test]
    fn extension_introuvable() {
        let err = "PHP Warning:  PHP Startup: Unable to load dynamic library 'redis' (tried: C:/php/ext/redis (Le module spécifié est introuvable))";
        let e = parse("8.3.37", true, "[PHP Modules]\nCore\n", err).unwrap_err();
        assert_eq!(e.code(), "php.invalide");
        assert!(e.params()["detail"].as_str().unwrap().contains("redis"));
    }

    #[test]
    fn echec_du_processus() {
        assert!(parse("8.3.37", false, "", "boom").is_err());
    }
}
