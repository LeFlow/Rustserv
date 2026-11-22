//! Exécution d'un script ou d'une suite de tests (PHPUnit, Pest) sur **plusieurs
//! versions de PHP en parallèle**, pour comparer les résultats.
//!
//! - Chaque version utilise son propre `php.exe` et son php.ini Rustserv.
//! - PHPUnit et Pest écrivent un rapport JUnit (`--log-junit`) : on en tire le
//!   nombre de tests, d'assertions, d'échecs…
//! - Pour un script, on compare les **sorties** entre versions (empreinte).

use crate::catalog::InstalledPackage;
use crate::error::{Error, Result};
use crate::platform;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

/// Taille maximale de sortie conservée par exécution (la fin est gardée).
const SORTIE_MAX: usize = 256 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TestKind {
    Phpunit,
    Pest,
    Script,
}

#[derive(Debug, Clone)]
pub struct RunSpec {
    pub project: PathBuf,
    pub kind: TestKind,
    /// Script à exécuter (relatif au projet) pour `TestKind::Script`.
    pub script: Option<String>,
    /// Arguments supplémentaires (filtre de tests…).
    pub args: Vec<String>,
    pub timeout: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Statut {
    Reussi,
    Echec,
    /// PHP n'a pas pu exécuter le programme (erreur fatale, dépendance manquante…).
    Erreur,
    DelaiDepasse,
    /// Interrompu à la demande de l'utilisateur.
    Annule,
}

/// Résumé tiré du rapport JUnit.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct JunitSummary {
    pub tests: u64,
    pub assertions: u64,
    pub echecs: u64,
    pub erreurs: u64,
    pub ignores: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct RunResult {
    pub version: String,
    pub statut: Statut,
    pub code: Option<i32>,
    pub duree_ms: u64,
    pub sortie: String,
    pub resume: Option<JunitSummary>,
    /// Empreinte de la sortie (comparaison entre versions pour un script).
    pub empreinte: String,
}

/// Programme à lancer avec `php` selon le type de tests.
pub fn entry_point(spec: &RunSpec) -> Result<PathBuf> {
    let p = &spec.project;
    let candidats: Vec<PathBuf> = match spec.kind {
        TestKind::Phpunit => vec![p.join("vendor/phpunit/phpunit/phpunit"), p.join("vendor/bin/phpunit")],
        TestKind::Pest => vec![p.join("vendor/pestphp/pest/bin/pest"), p.join("vendor/bin/pest")],
        TestKind::Script => {
            let s = spec.script.as_deref().unwrap_or("").trim();
            let rel = Path::new(s);
            // Le script doit rester dans le projet.
            if s.is_empty() || rel.is_absolute() || rel.components().any(|c| matches!(c, Component::ParentDir | Component::Prefix(_))) {
                return Err(Error::CompatIntrouvable { quoi: s.to_string(), aide: "script".into() });
            }
            vec![p.join(rel)]
        }
    };
    candidats.into_iter().find(|c| c.is_file()).ok_or_else(|| Error::CompatIntrouvable {
        quoi: match spec.kind {
            TestKind::Phpunit => "PHPUnit".into(),
            TestKind::Pest => "Pest".into(),
            TestKind::Script => spec.script.clone().unwrap_or_default(),
        },
        aide: match spec.kind {
            TestKind::Phpunit => "phpunit".into(),
            TestKind::Pest => "pest".into(),
            TestKind::Script => "script".into(),
        },
    })
}

/// Fichiers de configuration reconnus par PHPUnit/Pest.
const CONFIGS_PHPUNIT: &[&str] = &["phpunit.xml", "phpunit.xml.dist", "phpunit.dist.xml"];

/// Arguments par défaut : sans configuration PHPUnit ni argument, PHPUnit affiche
/// son aide au lieu de lancer les tests → on lui donne le dossier `tests/`.
pub fn default_args(spec: &RunSpec) -> Vec<String> {
    if spec.kind == TestKind::Script || !spec.args.is_empty() {
        return vec![];
    }
    let a_config = CONFIGS_PHPUNIT.iter().any(|c| spec.project.join(c).is_file());
    if !a_config && spec.project.join("tests").is_dir() {
        vec!["tests".into()]
    } else {
        vec![]
    }
}

/// Lit les attributs du premier `<testsuite …>` d'un rapport JUnit.
pub fn parse_junit(xml: &str) -> Option<JunitSummary> {
    let debut = xml.find("<testsuite ")?;
    let fin = xml[debut..].find('>')? + debut;
    let balise = &xml[debut..fin];
    let attr = |nom: &str| -> u64 {
        let motif = format!(" {nom}=\"");
        balise.find(&motif).and_then(|i| {
            let reste = &balise[i + motif.len()..];
            reste[..reste.find('"')?].parse().ok()
        }).unwrap_or(0)
    };
    Some(JunitSummary {
        tests: attr("tests"),
        assertions: attr("assertions"),
        echecs: attr("failures"),
        erreurs: attr("errors"),
        ignores: attr("skipped"),
    })
}

/// Retire les séquences de couleurs ANSI d'une sortie.
pub fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c == '\x1b' {
            if it.peek() == Some(&'[') {
                it.next();
                for c2 in it.by_ref() {
                    if c2.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            continue;
        }
        out.push(c);
    }
    out
}

fn garder_fin(s: String) -> String {
    if s.len() <= SORTIE_MAX {
        return s;
    }
    let mut debut = s.len() - SORTIE_MAX;
    while !s.is_char_boundary(debut) {
        debut += 1;
    }
    format!("[… début de la sortie tronqué …]\n{}", &s[debut..])
}

/// Exécute le programme avec une version de PHP.
pub async fn run_one(php: &InstalledPackage, ini: &Path, spec: &RunSpec, work_dir: &Path, cancel: &AtomicBool) -> RunResult {
    let debut = Instant::now();
    let resultat = |statut, code, sortie: String, resume| RunResult {
        version: php.version.clone(),
        statut,
        code,
        duree_ms: debut.elapsed().as_millis() as u64,
        empreinte: hex::encode(Sha256::digest(sortie.as_bytes()))[..16].to_string(),
        sortie: garder_fin(sortie),
        resume,
    };
    let entree = match entry_point(spec) {
        Ok(e) => e,
        Err(e) => return resultat(Statut::Erreur, None, e.to_string(), None),
    };
    let _ = std::fs::create_dir_all(work_dir);
    let junit = work_dir.join(format!("junit-{}.xml", php.version));
    let _ = std::fs::remove_file(&junit);

    let mut cmd = tokio::process::Command::new(php.path.join(platform::exe_name("php")));
    cmd.arg("-c").arg(ini).arg(&entree);
    match spec.kind {
        TestKind::Phpunit | TestKind::Pest => {
            cmd.arg("--colors=never").arg("--log-junit").arg(&junit);
        }
        TestKind::Script => {}
    }
    cmd.args(&spec.args)
        .args(default_args(spec))
        .current_dir(&spec.project)
        .env("PATH", format!("{};{}", php.path.display(), std::env::var("PATH").unwrap_or_default()))
        .env("PHPRC", ini.parent().unwrap_or(ini))
        .env("NO_COLOR", "1")
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    cmd.creation_flags(platform::CREATE_NO_WINDOW);

    let sortie = match super::ou_annuler(tokio::time::timeout(spec.timeout, cmd.output()), cancel).await {
        Err(_) => return resultat(Statut::Annule, None, "exécution annulée".into(), None),
        Ok(Err(_)) => return resultat(Statut::DelaiDepasse, None, format!("délai de {} s dépassé", spec.timeout.as_secs()), None),
        Ok(Ok(Err(e))) => return resultat(Statut::Erreur, None, e.to_string(), None),
        Ok(Ok(Ok(o))) => o,
    };
    let mut texte = strip_ansi(&String::from_utf8_lossy(&sortie.stdout));
    let err = strip_ansi(&String::from_utf8_lossy(&sortie.stderr));
    if !err.trim().is_empty() {
        texte.push_str("\n--- sortie d'erreur ---\n");
        texte.push_str(&err);
    }
    let resume = std::fs::read_to_string(&junit).ok().and_then(|x| parse_junit(&x));
    let code = sortie.status.code();
    let statut = match (code, &resume) {
        (Some(0), _) => Statut::Reussi,
        // Des tests ont tourné : ce sont des échecs de tests, pas une erreur d'exécution.
        (_, Some(r)) if r.tests > 0 => Statut::Echec,
        (Some(1), None) if spec.kind == TestKind::Script => Statut::Echec,
        _ => Statut::Erreur,
    };
    resultat(statut, code, texte, resume)
}

/// Exécute sur toutes les versions **en parallèle** ; résultats dans l'ordre des versions.
pub async fn run_all(phps: &[(InstalledPackage, PathBuf)], spec: &RunSpec, work_dir: &Path, cancel: &AtomicBool) -> Vec<RunResult> {
    futures_util::future::join_all(phps.iter().map(|(p, ini)| run_one(p, ini, spec, work_dir, cancel))).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lecture_junit() {
        let xml = r#"<?xml version="1.0"?><testsuites><testsuite name="T" tests="12" assertions="30" errors="1" failures="2" skipped="3" time="0.1"><testsuite name="sub" tests="1"/></testsuite></testsuites>"#;
        assert_eq!(parse_junit(xml).unwrap(), JunitSummary { tests: 12, assertions: 30, echecs: 2, erreurs: 1, ignores: 3 });
        assert!(parse_junit("<testsuites/>").is_none());
    }

    #[test]
    fn retire_les_couleurs() {
        assert_eq!(strip_ansi("\x1b[32mOK\x1b[0m (3 tests)"), "OK (3 tests)");
    }

    #[test]
    fn script_hors_du_projet_refuse() {
        let t = tempfile::tempdir().unwrap();
        std::fs::write(t.path().join("a.php"), "").unwrap();
        let spec = |s: &str| RunSpec { project: t.path().into(), kind: TestKind::Script, script: Some(s.into()), args: vec![], timeout: Duration::from_secs(1) };
        assert!(entry_point(&spec("a.php")).is_ok());
        for mauvais in ["../a.php", "C:\\Windows\\win.ini", "", "absent.php"] {
            assert_eq!(entry_point(&spec(mauvais)).unwrap_err().code(), "compat.introuvable", "{mauvais}");
        }
        let pu = RunSpec { kind: TestKind::Phpunit, ..spec("a.php") };
        assert_eq!(entry_point(&pu).unwrap_err().params()["aide"], "phpunit");
    }

    #[test]
    fn dossier_tests_par_defaut() {
        let t = tempfile::tempdir().unwrap();
        let spec = RunSpec { project: t.path().into(), kind: TestKind::Phpunit, script: None, args: vec![], timeout: Duration::from_secs(1) };
        assert!(default_args(&spec).is_empty(), "pas de dossier tests");
        std::fs::create_dir(t.path().join("tests")).unwrap();
        assert_eq!(default_args(&spec), ["tests"]);
        std::fs::write(t.path().join("phpunit.xml.dist"), "").unwrap();
        assert!(default_args(&spec).is_empty(), "la configuration du projet décide");
    }

    #[test]
    fn sortie_tronquee_par_le_debut() {
        let s = "é".repeat(SORTIE_MAX);
        let g = garder_fin(s);
        assert!(g.starts_with("[… début"));
        assert!(g.len() < SORTIE_MAX + 100);
    }
}
