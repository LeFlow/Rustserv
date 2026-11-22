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
    let (ok, stdout, stderr) = run_m(php_dir, version, Some(ini))?;
    parse(version, ok, &stdout, &stderr)
}

/// Modules **intégrés** au binaire (chargés sans aucun php.ini : `php -n -m`).
pub fn builtin_modules(php_dir: &Path, version: &str) -> Result<Vec<String>> {
    let (ok, stdout, stderr) = run_m(php_dir, version, None)?;
    Ok(parse(version, ok, &stdout, &stderr)?.modules)
}

/// État réel, **sans échouer** : modules chargés + lignes d'avertissement.
pub fn loaded_modules_lenient(php_dir: &Path, version: &str, ini: &Path) -> Result<(Vec<String>, Vec<String>)> {
    let (_, stdout, stderr) = run_m(php_dir, version, Some(ini))?;
    let warnings = stdout.lines().chain(stderr.lines()).map(str::trim).filter(|l| is_warning(l)).map(String::from).collect();
    Ok((module_lines(&stdout), warnings))
}

/// Nom de module tel qu'affiché par `php -m` → nom d'extension (« Zend OPcache » → « opcache »).
pub fn module_to_extension(module: &str) -> String {
    match module {
        "zend opcache" => "opcache".into(),
        m => m.replace(' ', "_"),
    }
}

fn is_warning(l: &str) -> bool {
    let l = l.to_ascii_lowercase();
    l.contains("startup:") || l.starts_with("php warning") || l.starts_with("warning:") || l.contains("fatal error")
}

fn module_lines(stdout: &str) -> Vec<String> {
    stdout
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('[') && !is_warning(l))
        .map(str::to_ascii_lowercase)
        .collect()
}

/// Exécute `php [-n | -c ini] -m` ; renvoie (succès, stdout, stderr).
fn run_m(php_dir: &Path, version: &str, ini: Option<&Path>) -> Result<(bool, String, String)> {
    let exe = php_dir.join(platform::exe_name("php"));
    if !exe.is_file() {
        return Err(Error::ExecutableIntrouvable { chemin: exe });
    }
    let mut cmd = std::process::Command::new(&exe);
    match ini {
        Some(i) => cmd.arg("-c").arg(i),
        None => cmd.arg("-n"),
    };
    cmd.arg("-m").current_dir(php_dir);
    // Les DLL dépendantes (libssl…) sont dans le dossier de PHP.
    let path = format!("{};{}", php_dir.display(), std::env::var("PATH").unwrap_or_default());
    cmd.env("PATH", path);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(platform::CREATE_NO_WINDOW);
    }
    let out = cmd.output().map_err(|e| Error::PhpInvalide { version: version.into(), detail: e.to_string() })?;
    Ok((out.status.success(), String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned()))
}

/// Analyse la sortie de `php -m` (séparé pour être testable sans PHP).
pub fn parse(version: &str, success: bool, stdout: &str, stderr: &str) -> Result<PhpCheck> {
    let problemes: Vec<&str> = stdout.lines().chain(stderr.lines()).map(str::trim).filter(|l| is_warning(l)).collect();
    if !success || !problemes.is_empty() {
        let detail = if problemes.is_empty() { stderr.trim().to_string() } else { problemes.join("\n") };
        return Err(Error::PhpInvalide { version: version.into(), detail });
    }
    Ok(PhpCheck { modules: module_lines(stdout) })
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
