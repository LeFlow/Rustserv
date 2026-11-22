//! Analyse statique de compatibilité :
//! - **PHPCompatibility** (règles PHP_CodeSniffer) : fonctions, syntaxes et
//!   extensions supprimées ou dépréciées entre deux versions de PHP ;
//! - **PHPStan** : analyse de types, avec la version de PHP cible (`phpVersion`).
//!
//! Les deux rapports JSON sont convertis dans un format commun ([`Rapport`]).

use super::tools::{phpcs_bin, phpstan_bin};
use crate::catalog::{version, InstalledPackage};
use crate::error::{Error, Result};
use crate::paths::{slash, Paths};
use crate::platform;
use serde::Serialize;
use serde_json::Value;
use std::path::Path;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Gravite {
    Erreur,
    Avertissement,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Message {
    pub ligne: u64,
    pub gravite: Gravite,
    pub message: String,
    /// Règle (PHPCompatibility) ou identifiant (PHPStan), si fourni.
    pub regle: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Fichier {
    /// Chemin relatif au projet, avec des `/`.
    pub chemin: String,
    pub messages: Vec<Message>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Rapport {
    pub outil: String,
    pub cible: String,
    pub erreurs: u64,
    pub avertissements: u64,
    pub fichiers: Vec<Fichier>,
    /// Erreurs non rattachées à un fichier (configuration, autoload…).
    pub general: Vec<String>,
    pub duree_ms: u64,
}

fn relatif(projet: &Path, chemin: &str) -> String {
    let p = Path::new(chemin);
    slash(p.strip_prefix(projet).unwrap_or(p)).trim_start_matches('/').to_string()
}

/// Convertit le rapport JSON de PHP_CodeSniffer.
pub fn parse_phpcs(projet: &Path, json: &str, cible: &str) -> Result<Rapport> {
    let v: Value = serde_json::from_str(json).map_err(|e| Error::CompatEchec { outil: "PHPCompatibility".into(), detail: e.to_string() })?;
    let mut fichiers: Vec<Fichier> = v["files"]
        .as_object()
        .map(|o| {
            o.iter()
                .filter_map(|(chemin, f)| {
                    let messages: Vec<Message> = f["messages"]
                        .as_array()?
                        .iter()
                        .map(|m| Message {
                            ligne: m["line"].as_u64().unwrap_or(0),
                            gravite: if m["type"] == "ERROR" { Gravite::Erreur } else { Gravite::Avertissement },
                            message: m["message"].as_str().unwrap_or("").to_string(),
                            regle: m["source"].as_str().map(String::from),
                        })
                        .collect();
                    (!messages.is_empty()).then(|| Fichier { chemin: relatif(projet, chemin), messages })
                })
                .collect()
        })
        .unwrap_or_default();
    fichiers.sort_by(|a, b| a.chemin.cmp(&b.chemin));
    Ok(Rapport {
        outil: "PHPCompatibility".into(),
        cible: cible.into(),
        erreurs: v["totals"]["errors"].as_u64().unwrap_or(0),
        avertissements: v["totals"]["warnings"].as_u64().unwrap_or(0),
        fichiers,
        general: vec![],
        duree_ms: 0,
    })
}

/// Convertit le rapport JSON de PHPStan.
pub fn parse_phpstan(projet: &Path, json: &str, cible: &str) -> Result<Rapport> {
    let v: Value = serde_json::from_str(json).map_err(|e| Error::CompatEchec { outil: "PHPStan".into(), detail: e.to_string() })?;
    let mut fichiers: Vec<Fichier> = v["files"]
        .as_object()
        .map(|o| {
            o.iter()
                .map(|(chemin, f)| Fichier {
                    chemin: relatif(projet, chemin),
                    messages: f["messages"]
                        .as_array()
                        .map(|a| {
                            a.iter()
                                .map(|m| Message {
                                    ligne: m["line"].as_u64().unwrap_or(0),
                                    gravite: Gravite::Erreur,
                                    message: m["message"].as_str().unwrap_or("").to_string(),
                                    regle: m["identifier"].as_str().map(String::from),
                                })
                                .collect()
                        })
                        .unwrap_or_default(),
                })
                .filter(|f| !f.messages.is_empty())
                .collect()
        })
        .unwrap_or_default();
    fichiers.sort_by(|a, b| a.chemin.cmp(&b.chemin));
    let general: Vec<String> = v["errors"].as_array().map(|a| a.iter().filter_map(|e| e.as_str().map(String::from)).collect()).unwrap_or_default();
    Ok(Rapport {
        outil: "PHPStan".into(),
        cible: cible.into(),
        erreurs: v["totals"]["file_errors"].as_u64().unwrap_or(0) + general.len() as u64,
        avertissements: 0,
        fichiers,
        general,
        duree_ms: 0,
    })
}

/// Le JSON commence au premier `{` (certains outils affichent des avertissements avant).
fn extraire_json(sortie: &str) -> Option<&str> {
    let debut = sortie.find('{')?;
    let fin = sortie.rfind('}')?;
    (fin > debut).then(|| &sortie[debut..=fin])
}

async fn run_php(php: &InstalledPackage, ini: &Path, args: Vec<String>, cwd: &Path, outil: &str) -> Result<(Option<i32>, String, String)> {
    let mut cmd = tokio::process::Command::new(php.path.join(platform::exe_name("php")));
    cmd.arg("-c")
        .arg(ini)
        .args(&args)
        .current_dir(cwd)
        .env("PHPRC", ini.parent().unwrap_or(ini))
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    cmd.creation_flags(platform::CREATE_NO_WINDOW);
    let out = tokio::time::timeout(Duration::from_secs(900), cmd.output())
        .await
        .map_err(|_| Error::CompatEchec { outil: outil.into(), detail: "délai de 15 minutes dépassé".into() })?
        .map_err(|e| Error::CompatEchec { outil: outil.into(), detail: e.to_string() })?;
    Ok((out.status.code(), String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned()))
}

fn sans_json(outil: &str, stdout: &str, stderr: &str) -> Error {
    let t = format!("{stdout}\n{stderr}");
    let lignes: Vec<&str> = t.lines().filter(|l| !l.trim().is_empty()).collect();
    Error::CompatEchec { outil: outil.into(), detail: lignes[lignes.len().saturating_sub(15)..].join("\n") }
}

/// PHPCompatibility : `min`/`max` = branches (« 7.4 », « 8.5 ») ; `max` vide = toutes les suivantes.
pub async fn phpcompatibility(paths: &Paths, php: &InstalledPackage, ini: &Path, projet: &Path, min: &str, max: Option<&str>) -> Result<Rapport> {
    let cible = match max {
        Some(m) if m != min => format!("{min}-{m}"),
        Some(_) => min.to_string(),
        None => format!("{min}-"),
    };
    let debut = std::time::Instant::now();
    let args = vec![
        phpcs_bin(paths).display().to_string(),
        "--standard=PHPCompatibility".into(),
        "--runtime-set".into(),
        "testVersion".into(),
        cible.clone(),
        "--report=json".into(),
        "--extensions=php,inc".into(),
        "--ignore=*/vendor/*,*/node_modules/*,*/storage/*,*/var/cache/*".into(),
        "-q".into(),
        projet.display().to_string(),
    ];
    let (_, stdout, stderr) = run_php(php, ini, args, projet, "PHPCompatibility").await?;
    // Code de sortie 1/2 = problèmes trouvés : normal. Seule l'absence de JSON est une erreur.
    let json = extraire_json(&stdout).ok_or_else(|| sans_json("PHPCompatibility", &stdout, &stderr))?;
    let mut r = parse_phpcs(projet, json, &cible)?;
    r.duree_ms = debut.elapsed().as_millis() as u64;
    Ok(r)
}

/// Configuration temporaire de PHPStan pour un projet.
pub fn phpstan_neon(paths: &Paths, projet: &Path, niveau: u8, php_cible: &str) -> String {
    let (n, _) = version::parse(php_cible);
    let id = n.first().copied().unwrap_or(8) * 10000 + n.get(1).copied().unwrap_or(0) * 100;
    let p = slash(projet);
    let mut s = format!("parameters:\n\tlevel: {}\n\tphpVersion: {id}\n\tpaths:\n\t\t- '{p}'\n", niveau.min(10));
    let exclus: Vec<&str> = ["vendor", "node_modules", "storage", "var"].into_iter().filter(|d| projet.join(d).is_dir()).collect();
    if !exclus.is_empty() {
        s.push_str("\texcludePaths:\n\t\tanalyse:\n");
        for d in &exclus {
            s.push_str(&format!("\t\t\t- '{p}/{d}'\n"));
        }
    }
    // Les classes des dépendances sont connues sans être analysées.
    if projet.join("vendor").is_dir() {
        s.push_str(&format!("\tscanDirectories:\n\t\t- '{p}/vendor'\n"));
    }
    s.push_str(&format!("\ttmpDir: '{}'\n", slash(&paths.tmp().join("phpstan"))));
    s
}

/// PHPStan au niveau donné (0 à 10), pour la version de PHP cible (« 8.3 »).
pub async fn phpstan(paths: &Paths, php: &InstalledPackage, ini: &Path, projet: &Path, niveau: u8, php_cible: &str) -> Result<Rapport> {
    let conf = paths.tmp().join("phpstan").join("rustserv.neon");
    std::fs::create_dir_all(conf.parent().unwrap()).map_err(|e| Error::fichier(&conf, e))?;
    std::fs::write(&conf, phpstan_neon(paths, projet, niveau, php_cible)).map_err(|e| Error::fichier(&conf, e))?;
    let debut = std::time::Instant::now();
    let args = vec![
        phpstan_bin(paths).display().to_string(),
        "analyse".into(),
        "-c".into(),
        conf.display().to_string(),
        "--error-format=json".into(),
        "--no-progress".into(),
        "--no-ansi".into(),
        "--memory-limit=1G".into(),
    ];
    let (_, stdout, stderr) = run_php(php, ini, args, projet, "PHPStan").await?;
    let json = extraire_json(&stdout).ok_or_else(|| sans_json("PHPStan", &stdout, &stderr))?;
    let mut r = parse_phpstan(projet, json, &format!("PHP {php_cible}, niveau {niveau}"))?;
    r.duree_ms = debut.elapsed().as_millis() as u64;
    Ok(r)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rapport_phpcs() {
        let projet = Path::new(r"C:\www\demo");
        let json = r#"{"totals":{"errors":1,"warnings":1,"fixable":0},"files":{
            "C:\\www\\demo\\a.php":{"errors":1,"warnings":1,"messages":[
                {"message":"Function each() is removed since PHP 8.0","source":"PHPCompatibility.FunctionUse.RemovedFunctions.eachRemoved","severity":5,"fixable":false,"type":"ERROR","line":3,"column":5},
                {"message":"utf8_encode() is deprecated since PHP 8.2","source":"PHPCompatibility.FunctionUse.RemovedFunctions.utf8_encodeDeprecated","severity":5,"type":"WARNING","line":7,"column":1}]},
            "C:\\www\\demo\\b.php":{"errors":0,"warnings":0,"messages":[]}}}"#;
        let r = parse_phpcs(projet, json, "7.4-8.5").unwrap();
        assert_eq!((r.erreurs, r.avertissements), (1, 1));
        assert_eq!(r.fichiers.len(), 1, "fichier sans message ignoré");
        assert_eq!(r.fichiers[0].chemin, "a.php");
        assert_eq!(r.fichiers[0].messages[1].gravite, Gravite::Avertissement);
    }

    #[test]
    fn rapport_phpstan() {
        let projet = Path::new("C:/www/demo");
        let json = r#"{"totals":{"errors":1,"file_errors":2},"files":{"C:/www/demo/src/A.php":{"errors":2,"messages":[
            {"message":"Call to undefined function each().","line":4,"ignorable":true,"identifier":"function.notFound"},
            {"message":"Undefined variable: $x","line":9,"ignorable":true}]}},"errors":["Autoload error"]}"#;
        let r = parse_phpstan(projet, json, "PHP 8.3, niveau 5").unwrap();
        assert_eq!(r.erreurs, 3);
        assert_eq!(r.fichiers[0].chemin, "src/A.php");
        assert_eq!(r.fichiers[0].messages[0].regle.as_deref(), Some("function.notFound"));
        assert_eq!(r.general, ["Autoload error"]);
    }

    #[test]
    fn json_apres_un_avertissement() {
        assert_eq!(extraire_json("Warning: truc\n{\"a\":{}}\n"), Some("{\"a\":{}}"));
        assert_eq!(extraire_json("rien"), None);
    }

    #[test]
    fn configuration_phpstan() {
        let t = tempfile::tempdir().unwrap();
        let paths = Paths::new(t.path().join("root"));
        let projet = t.path().join("site");
        std::fs::create_dir_all(projet.join("vendor")).unwrap();
        let neon = phpstan_neon(&paths, &projet, 6, "8.3");
        assert!(neon.contains("level: 6") && neon.contains("phpVersion: 80300"));
        assert!(neon.contains("/vendor'") && neon.contains("scanDirectories"));
        assert!(!neon.contains("node_modules"), "dossier absent non exclu");
        assert!(phpstan_neon(&paths, &projet, 99, "7.4").contains("level: 10\n\tphpVersion: 70400"));
    }
}
