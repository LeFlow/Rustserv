//! Détection des installations Laragon / WAMP, de leurs sites et de leurs bases.
//! **Lecture seule** : rien n'est modifié chez Laragon ou WAMP.

use crate::paths::Paths;
use crate::sites;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SourceKind {
    Laragon,
    Wamp,
}

impl SourceKind {
    pub fn suffixe(self) -> &'static str {
        match self {
            SourceKind::Laragon => "laragon",
            SourceKind::Wamp => "wamp",
        }
    }
}

/// Un serveur de bases de l'outil d'origine (une version de MySQL/MariaDB et ses données).
#[derive(Debug, Clone, Serialize)]
pub struct DbServer {
    /// « MySQL 8.4.3 », « MariaDB 11.5.2 ».
    pub label: String,
    /// Dossier contenant mysqld.exe / mysqldump.exe.
    pub bin: PathBuf,
    pub data: PathBuf,
    pub mariadb: bool,
    /// Bases utilisateur (noms réels, décodés).
    pub bases: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Source {
    pub kind: SourceKind,
    pub root: PathBuf,
    pub serveurs: Vec<DbServer>,
    /// Branche de PHP utilisée par l'outil (« 8.1 »), si connue.
    pub php: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TypeSite {
    Wordpress,
    Laravel,
    Autre,
}

#[derive(Debug, Clone, Serialize)]
pub struct SiteDetecte {
    pub source: SourceKind,
    /// Nom d'origine du dossier.
    pub nom: String,
    pub dossier: PathBuf,
    pub type_site: TypeSite,
    /// Base utilisée par le site (lue dans wp-config.php ou .env).
    pub base: Option<String>,
    /// La base a été trouvée dans un serveur de l'outil d'origine.
    pub base_trouvee: bool,
    /// Nom proposé dans Rustserv (valide, sans conflit).
    pub nom_propose: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Detection {
    pub sources: Vec<Source>,
    pub sites: Vec<SiteDetecte>,
    /// Sites déjà présents dans Rustserv (pour vérifier les noms choisis).
    pub existants: Vec<String>,
}

/// Décode un nom de dossier MySQL (« agence@002dweb » → « agence-web »).
pub fn decode_nom_base(s: &str) -> String {
    let mut out = String::new();
    let mut i = 0;
    let b = s.as_bytes();
    while i < b.len() {
        if b[i] == b'@' && i + 5 <= b.len() {
            if let Some(c) = u32::from_str_radix(&s[i + 1..i + 5], 16).ok().and_then(char::from_u32) {
                out.push(c);
                i += 5;
                continue;
            }
        }
        out.push(b[i] as char);
        i += 1;
    }
    out
}

/// Nom de site valide pour Rustserv : minuscules, chiffres, tirets (« imp - Copie » → « imp-copie »).
pub fn slugify(s: &str) -> String {
    let mut out = String::new();
    for c in s.to_lowercase().chars() {
        let c = match c {
            'à' | 'â' | 'ä' | 'á' => 'a',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'î' | 'ï' | 'í' => 'i',
            'ô' | 'ö' | 'ó' => 'o',
            'ù' | 'û' | 'ü' | 'ú' => 'u',
            'ç' => 'c',
            c if c.is_ascii_alphanumeric() => c,
            _ => '-',
        };
        if c == '-' && (out.is_empty() || out.ends_with('-')) {
            continue;
        }
        out.push(c);
    }
    let out = out.trim_end_matches('-').to_string();
    if out.is_empty() { "site".into() } else { out.chars().take(63).collect() }
}

fn bases_dans(data: &Path) -> Vec<String> {
    let systeme = ["mysql", "performance_schema", "sys", "information_schema"];
    let mut v: Vec<String> = std::fs::read_dir(data)
        .map(|rd| {
            rd.flatten()
                .filter(|e| e.path().is_dir())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| !n.starts_with('#') && !systeme.contains(&n.as_str()))
                .map(|n| decode_nom_base(&n))
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

fn sous_dossiers(p: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(p).map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect()).unwrap_or_default();
    v.sort();
    v
}

fn nom(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

/// Version « 8.4.3 » extraite d'un nom de dossier (« mysql-8.4.3-winx64 », « mysql9.1.0 »).
fn version_dans(n: &str) -> String {
    let debut = n.find(|c: char| c.is_ascii_digit()).unwrap_or(n.len());
    n[debut..].chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect::<String>().trim_end_matches('.').to_string()
}

fn serveur(bin_racine: &Path, data: &Path) -> Option<DbServer> {
    let bin = bin_racine.join("bin");
    let mariadb = nom(bin_racine).to_lowercase().contains("mariadb");
    if !bin.join("mysqld.exe").is_file() && !bin.join("mariadbd.exe").is_file() {
        return None;
    }
    let bases = bases_dans(data);
    Some(DbServer {
        label: format!("{} {}", if mariadb { "MariaDB" } else { "MySQL" }, version_dans(&nom(bin_racine))),
        bin,
        data: data.to_path_buf(),
        mariadb,
        bases,
    })
}

/// Laragon : `bin/mysql/mysql-X…`, données dans `data/mysql-<branche>` (« mysql-8 » = 8.0).
fn laragon(root: &Path) -> Source {
    let bins = sous_dossiers(&root.join("bin").join("mysql"));
    let mut serveurs = Vec::new();
    for data in sous_dossiers(&root.join("data")) {
        let n = nom(&data);
        let Some(branche) = n.strip_prefix("mysql-") else { continue };
        let branche = if branche.contains('.') { branche.to_string() } else { format!("{branche}.0") };
        if let Some(b) = bins.iter().find(|b| version_dans(&nom(b)).starts_with(&format!("{branche}."))) {
            if let Some(s) = serveur(b, &data) {
                if !s.bases.is_empty() {
                    serveurs.push(s);
                }
            }
        }
    }
    let php = std::fs::read_to_string(root.join("usr").join("laragon.ini")).ok().and_then(|t| {
        let mut dans_php = false;
        for l in t.lines() {
            let l = l.trim();
            if l.starts_with('[') {
                dans_php = l == "[php]";
            } else if dans_php {
                if let Some(v) = l.strip_prefix("Version=") {
                    return Some(crate::catalog::version::branch(&version_dans(v)));
                }
            }
        }
        None
    });
    Source { kind: SourceKind::Laragon, root: root.into(), serveurs, php }
}

/// WAMP : `bin/mysql/mysqlX/data`, `bin/mariadb/mariadbX/data`, PHP dans wampmanager.conf.
fn wamp(root: &Path) -> Source {
    let mut serveurs = Vec::new();
    for moteur in ["mysql", "mariadb"] {
        for b in sous_dossiers(&root.join("bin").join(moteur)) {
            if let Some(s) = serveur(&b, &b.join("data")) {
                if !s.bases.is_empty() {
                    serveurs.push(s);
                }
            }
        }
    }
    let php = std::fs::read_to_string(root.join("wampmanager.conf"))
        .ok()
        .and_then(|t| t.lines().find(|l| l.trim_start().starts_with("phpVersion")).map(String::from))
        .and_then(|l| l.split('"').nth(1).map(|v| crate::catalog::version::branch(v)));
    Source { kind: SourceKind::Wamp, root: root.into(), serveurs, php }
}

/// Valeur d'un `define('CLE', 'valeur')` de wp-config.php.
pub fn wp_define(texte: &str, cle: &str) -> Option<String> {
    let i = texte.find(&format!("'{cle}'")).or_else(|| texte.find(&format!("\"{cle}\"")))?;
    let reste = &texte[i + cle.len() + 2..];
    let virgule = reste.find(',')?;
    let apres = reste[virgule + 1..].trim_start();
    let q = apres.chars().next()?;
    if q != '\'' && q != '"' {
        return None;
    }
    let fin = apres[1..].find(q)?;
    Some(apres[1..1 + fin].to_string())
}

/// Valeur `CLE=valeur` d'un fichier .env.
pub fn env_value(texte: &str, cle: &str) -> Option<String> {
    texte.lines().find_map(|l| {
        let l = l.trim();
        let v = l.strip_prefix(cle)?.strip_prefix('=')?;
        Some(v.trim().trim_matches('"').trim_matches('\'').to_string())
    })
}

fn analyser_site(dossier: &Path) -> (TypeSite, Option<String>) {
    if let Ok(t) = std::fs::read_to_string(dossier.join("wp-config.php")) {
        return (TypeSite::Wordpress, wp_define(&t, "DB_NAME"));
    }
    if dossier.join("artisan").is_file() {
        let base = std::fs::read_to_string(dossier.join(".env")).ok().and_then(|t| env_value(&t, "DB_DATABASE"));
        return (TypeSite::Laravel, base);
    }
    (TypeSite::Autre, None)
}

/// Dossiers de www/ propres à WAMP, à ne pas importer.
const IGNORES: &[&str] = &["wamplangues", "wampthemes", "dup-installer"];

/// Détecte Laragon et WAMP aux emplacements standards (ou ceux fournis).
pub fn detect(paths: &Paths, settings: &crate::Settings, racines: &[(SourceKind, PathBuf)]) -> Detection {
    let existants: Vec<String> = sites::discover(paths, &settings.tld).map(|v| v.into_iter().map(|s| s.name).collect()).unwrap_or_default();
    let mut pris: HashSet<String> = existants.iter().cloned().collect();
    let mut sources = Vec::new();
    let mut sites_detectes = Vec::new();
    for (kind, root) in racines {
        if !root.join("www").is_dir() {
            continue;
        }
        let src = match kind {
            SourceKind::Laragon => laragon(root),
            SourceKind::Wamp => wamp(root),
        };
        for dossier in sous_dossiers(&root.join("www")) {
            let n = nom(&dossier);
            if IGNORES.contains(&n.as_str()) || n.starts_with('.') {
                continue;
            }
            let (type_site, base) = analyser_site(&dossier);
            let base_trouvee = base.as_ref().is_some_and(|b| src.serveurs.iter().any(|s| s.bases.contains(b)));
            let mut propose = slugify(&n);
            if pris.contains(&propose) {
                propose = format!("{propose}-{}", kind.suffixe());
            }
            let mut k = 2;
            let base_nom = propose.clone();
            while pris.contains(&propose) {
                propose = format!("{base_nom}-{k}");
                k += 1;
            }
            pris.insert(propose.clone());
            sites_detectes.push(SiteDetecte { source: *kind, nom: n, dossier, type_site, base, base_trouvee, nom_propose: propose });
        }
        sources.push(src);
    }
    Detection { sources, sites: sites_detectes, existants }
}

/// Emplacements standards de Laragon et WAMP.
pub fn racines_standard() -> Vec<(SourceKind, PathBuf)> {
    vec![
        (SourceKind::Laragon, PathBuf::from(r"C:\laragon")),
        (SourceKind::Wamp, PathBuf::from(r"C:\wamp64")),
        (SourceKind::Wamp, PathBuf::from(r"C:\wamp")),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noms() {
        assert_eq!(decode_nom_base("agence@002dweb"), "agence-web");
        assert_eq!(decode_nom_base("tuto@002dlaravel"), "tuto-laravel");
        assert_eq!(slugify("imp - Copie"), "imp-copie");
        assert_eq!(slugify("Été_2026!"), "ete-2026");
        assert_eq!(version_dans("mysql-8.4.3-winx64"), "8.4.3");
        assert_eq!(version_dans("mysql9.1.0"), "9.1.0");
    }

    #[test]
    fn lecture_des_configurations() {
        let wp = "<?php\ndefine( 'DB_NAME', 'agence-web' );\ndefine('DB_USER', \"root\");\ndefine( 'DB_PASSWORD', '' );\n$table_prefix = 'wp_';";
        assert_eq!(wp_define(wp, "DB_NAME").as_deref(), Some("agence-web"));
        assert_eq!(wp_define(wp, "DB_USER").as_deref(), Some("root"));
        assert_eq!(wp_define(wp, "DB_PASSWORD").as_deref(), Some(""));
        assert_eq!(env_value("APP_NAME=x\nDB_DATABASE=\"shop\"\n", "DB_DATABASE").as_deref(), Some("shop"));
    }

    #[test]
    fn detection_simulee() {
        let t = tempfile::tempdir().unwrap();
        let paths = Paths::new(t.path().join("rustserv"));
        paths.ensure_layout().unwrap();
        std::fs::create_dir_all(paths.www().join("imprimerie")).unwrap();
        let lar = t.path().join("laragon");
        std::fs::create_dir_all(lar.join("bin/mysql/mysql-8.4.3-winx64/bin")).unwrap();
        std::fs::write(lar.join("bin/mysql/mysql-8.4.3-winx64/bin/mysqld.exe"), "").unwrap();
        std::fs::create_dir_all(lar.join("data/mysql-8.4/imprimerie")).unwrap();
        std::fs::create_dir_all(lar.join("data/mysql-8.4/#innodb_temp")).unwrap();
        std::fs::create_dir_all(lar.join("www/imprimerie")).unwrap();
        std::fs::write(lar.join("www/imprimerie/wp-config.php"), "define('DB_NAME', 'imprimerie');").unwrap();
        std::fs::create_dir_all(lar.join("www/imp - Copie")).unwrap();
        std::fs::create_dir_all(lar.join("usr")).unwrap();
        std::fs::write(lar.join("usr/laragon.ini"), "[php]\nVersion=php-8.1.10-Win32-vs16-x64\n").unwrap();

        let d = detect(&paths, &crate::Settings::default(), &[(SourceKind::Laragon, lar.clone())]);
        assert_eq!(d.sources[0].php.as_deref(), Some("8.1"));
        assert_eq!(d.sources[0].serveurs[0].label, "MySQL 8.4.3");
        assert_eq!(d.sources[0].serveurs[0].bases, ["imprimerie"]);
        let imp = d.sites.iter().find(|s| s.nom == "imprimerie").unwrap();
        assert_eq!((imp.type_site, imp.base.as_deref(), imp.base_trouvee), (TypeSite::Wordpress, Some("imprimerie"), true));
        assert_eq!(imp.nom_propose, "imprimerie-laragon", "conflit avec un site Rustserv existant");
        assert!(d.sites.iter().any(|s| s.nom_propose == "imp-copie"));
    }
}
