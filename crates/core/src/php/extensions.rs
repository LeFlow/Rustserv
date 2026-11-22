//! Extensions PHP : état (active, inactive, intégrée, en erreur) et activation
//! avec gestion des dépendances et de l'ordre de chargement.
//!
//! Sources de vérité combinées :
//! - `ext/php_*.dll` : extensions disponibles ;
//! - le php.ini : extensions demandées (`extension=` / `zend_extension=`) ;
//! - `php -n -m` : modules **intégrés** au binaire (non désactivables) ;
//! - `php -c php.ini -m` : modules réellement chargés (détecte les échecs).

use super::check::{builtin_modules, loaded_modules_lenient, module_to_extension};
use super::ini::IniFile;
use super::pecl;
use super::setup::php_ini_path;
use crate::error::{Error, Result};
use crate::paths::Paths;
use serde::Serialize;
use std::collections::BTreeSet;
use std::path::Path;

/// Extensions à charger avec `zend_extension=`.
pub const ZEND_EXTENSIONS: &[&str] = &["opcache", "xdebug"];

/// Dépendances : (extension, extensions requises **avant** elle).
pub const DEPENDANCES: &[(&str, &[&str])] = &[
    ("exif", &["mbstring"]),
    ("mysqli", &["mysqlnd"]),
    ("pdo_mysql", &["pdo", "mysqlnd"]),
    ("pdo_sqlite", &["pdo"]),
    ("pdo_pgsql", &["pdo"]),
    ("pdo_odbc", &["pdo"]),
    ("pdo_firebird", &["pdo"]),
    ("xsl", &["dom", "libxml"]),
    ("soap", &["libxml"]),
];

/// Extensions qui nécessitent un logiciel tiers installé sur la machine.
pub const LIBS_EXTERNES: &[(&str, &str)] = &[
    ("oci8_19", "Oracle Instant Client 19"),
    ("oci8_21", "Oracle Instant Client 21"),
    ("oci8_23", "Oracle Instant Client 23"),
    ("pdo_oci", "Oracle Instant Client"),
    ("pdo_firebird", "client Firebird (fbclient.dll)"),
    ("snmp", "bibliothèque Net-SNMP"),
];

/// Courte description en français des extensions courantes.
pub fn description(name: &str) -> Option<&'static str> {
    Some(match name {
        "apcu" => "Cache mémoire pour les données de l'application",
        "bcmath" => "Calculs sur grands nombres en précision arbitraire",
        "bz2" => "Compression bzip2",
        "curl" => "Requêtes HTTP vers d'autres serveurs (API…)",
        "exif" => "Lecture des métadonnées des photos",
        "ffi" => "Appel de bibliothèques C depuis PHP",
        "fileinfo" => "Détection du type des fichiers (MIME)",
        "ftp" => "Client FTP",
        "gd" | "gd2" => "Manipulation d'images (redimensionnement…)",
        "gettext" => "Traductions au format gettext",
        "gmp" => "Calculs sur grands entiers",
        "igbinary" => "Sérialisation binaire compacte",
        "imagick" => "Manipulation d'images avancée (ImageMagick)",
        "imap" => "Lecture des boîtes mail IMAP/POP3",
        "intl" => "Internationalisation (dates, nombres, langues)",
        "ldap" => "Annuaire LDAP / Active Directory",
        "mbstring" => "Chaînes multi-octets (UTF-8) — indispensable",
        "mongodb" => "Base de données MongoDB",
        "mysqli" => "Base de données MySQL/MariaDB (API mysqli)",
        "odbc" => "Bases de données via ODBC",
        "opcache" => "Cache du code compilé : accélère fortement PHP",
        "openssl" => "Chiffrement et connexions sécurisées (HTTPS)",
        "pcov" => "Couverture de code rapide pour les tests",
        "pdo_mysql" => "Base de données MySQL/MariaDB (PDO)",
        "pdo_pgsql" => "Base de données PostgreSQL (PDO)",
        "pdo_sqlite" => "Base de données SQLite (PDO)",
        "pgsql" => "Base de données PostgreSQL",
        "redis" => "Client pour le serveur de cache Redis",
        "shmop" => "Mémoire partagée",
        "soap" => "Services web SOAP",
        "sockets" => "Sockets réseau bas niveau",
        "sodium" => "Cryptographie moderne (libsodium)",
        "sqlite3" => "Base de données SQLite",
        "tidy" => "Nettoyage de HTML",
        "xdebug" => "Débogage pas à pas et profilage",
        "xsl" => "Transformations XSLT",
        "yaml" => "Lecture/écriture YAML",
        "zip" => "Archives ZIP",
        _ => return None,
    })
}

/// Nom du module chargé correspondant à une DLL quand ils diffèrent
/// (PHP 7.4 : `php_gd2.dll` → module `gd` ; Oracle : `php_oci8_19.dll` → `oci8`).
pub fn module_name(ext: &str) -> &str {
    match ext {
        "gd2" => "gd",
        e if e.starts_with("oci8_") => "oci8",
        e => e,
    }
}

pub fn is_zend(name: &str) -> bool {
    ZEND_EXTENSIONS.contains(&name)
}

pub fn deps_of(name: &str) -> &'static [&'static str] {
    DEPENDANCES.iter().find(|(n, _)| *n == name).map(|(_, d)| *d).unwrap_or(&[])
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtState {
    /// Activée et chargée.
    Active,
    /// Disponible, non activée.
    Inactive,
    /// Compilée dans PHP : toujours présente.
    Integree,
    /// Activée dans le php.ini mais ne se charge pas.
    Erreur,
}

#[derive(Debug, Clone, Serialize)]
pub struct ExtensionInfo {
    pub name: String,
    pub state: ExtState,
    pub zend: bool,
    pub description: Option<&'static str>,
    pub dependances: Vec<String>,
    /// Logiciel tiers nécessaire.
    pub requiert: Option<&'static str>,
    /// Message de PHP quand le chargement échoue.
    pub erreur: Option<String>,
    /// Installée depuis PECL par Rustserv (version).
    pub pecl: Option<String>,
}

/// Contexte d'une version de PHP pour les décisions d'activation.
#[derive(Debug, Clone, Default)]
pub struct ExtContext {
    /// Extensions intégrées au binaire.
    pub builtin: BTreeSet<String>,
    /// Extensions disponibles en DLL (`ext/php_<nom>.dll`).
    pub dlls: BTreeSet<String>,
}

impl ExtContext {
    /// Analyse le dossier `ext/` et `php -n -m`.
    pub fn load(php_dir: &Path, version: &str) -> Result<Self> {
        let builtin = builtin_modules(php_dir, version)?.iter().map(|m| module_to_extension(m)).collect();
        Ok(Self { builtin, dlls: list_dlls(php_dir) })
    }

    /// Nom de DLL à activer pour une extension demandée (« gd » → « gd2 » sur PHP 7.4).
    pub fn dll_for(&self, name: &str) -> Option<String> {
        if self.dlls.contains(name) {
            return Some(name.to_string());
        }
        self.dlls.iter().find(|d| module_name(d) == name).cloned()
    }

    /// OPcache est intégré à PHP depuis la 8.5 : on le pilote alors avec `opcache.enable`.
    fn opcache_integre(&self) -> bool {
        self.builtin.contains("opcache")
    }
}

/// Noms des extensions présentes dans `ext/` (`php_curl.dll` → `curl`).
pub fn list_dlls(php_dir: &Path) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    if let Ok(rd) = std::fs::read_dir(php_dir.join("ext")) {
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().to_ascii_lowercase();
            if let Some(name) = n.strip_prefix("php_").and_then(|s| s.strip_suffix(".dll")) {
                out.insert(name.to_string());
            }
        }
    }
    out
}

fn est_faux(v: &str) -> bool {
    matches!(v.to_ascii_lowercase().as_str(), "0" | "off" | "false" | "no")
}

/// État de toutes les extensions d'une version de PHP.
pub fn scan(paths: &Paths, php_dir: &Path, version: &str) -> Result<Vec<ExtensionInfo>> {
    let ctx = ExtContext::load(php_dir, version)?;
    let ini_path = php_ini_path(paths, version);
    let ini = IniFile::parse(&std::fs::read_to_string(&ini_path).map_err(|e| Error::fichier(&ini_path, e))?);
    let (loaded, warnings) = loaded_modules_lenient(php_dir, version, &ini_path)?;
    let loaded: BTreeSet<String> = loaded.iter().map(|m| module_to_extension(m)).collect();
    let enabled: BTreeSet<String> = ini.extensions().into_iter().filter(|e| e.active).map(|e| e.name).collect();
    let pecl_records = pecl::records(paths, version);

    let mut names: BTreeSet<String> = ctx.dlls.clone();
    names.extend(enabled.iter().cloned());
    names.extend(ctx.builtin.iter().cloned());

    Ok(names
        .into_iter()
        .map(|name| {
            let state = if name == "opcache" && ctx.opcache_integre() {
                if ini.get("opcache.enable").is_some_and(|v| est_faux(&v)) { ExtState::Inactive } else { ExtState::Active }
            } else if ctx.builtin.contains(&name) {
                ExtState::Integree
            } else if enabled.contains(&name) {
                if loaded.contains(module_name(&name)) { ExtState::Active } else { ExtState::Erreur }
            } else {
                ExtState::Inactive
            };
            let erreur = (state == ExtState::Erreur).then(|| {
                warnings
                    .iter()
                    .find(|w| w.to_ascii_lowercase().contains(&name))
                    .cloned()
                    .unwrap_or_else(|| "extension activée mais non chargée".into())
            });
            ExtensionInfo {
                zend: is_zend(&name),
                description: description(&name),
                dependances: deps_of(&name).iter().map(|s| s.to_string()).collect(),
                requiert: LIBS_EXTERNES.iter().find(|(n, _)| *n == name).map(|(_, l)| *l),
                pecl: pecl_records.get(&name).map(|r| r.version.clone()),
                erreur,
                state,
                name,
            }
        })
        .collect())
}

/// Active une extension (et ses dépendances) dans `ini`. Renvoie les extensions activées.
pub fn enable(ini: &mut IniFile, ctx: &ExtContext, version: &str, name: &str) -> Result<Vec<String>> {
    let name = name.to_ascii_lowercase();
    let mut activees = Vec::new();
    enable_rec(ini, ctx, version, &name, &mut activees, 0)?;
    Ok(activees)
}

fn enable_rec(ini: &mut IniFile, ctx: &ExtContext, version: &str, name: &str, out: &mut Vec<String>, prof: u8) -> Result<()> {
    if prof > 5 {
        return Err(Error::Interne(format!("dépendances circulaires pour {name}")));
    }
    if name == "opcache" && ctx.opcache_integre() {
        if ini.get("opcache.enable").is_some_and(|v| est_faux(&v)) {
            ini.set("opcache.enable", "1");
            out.push(name.into());
        }
        reglages_opcache_dev(ini);
        return Ok(());
    }
    if ctx.builtin.contains(name) {
        return Ok(());
    }
    let Some(dll) = ctx.dll_for(name) else {
        return Err(Error::ExtensionInconnue { extension: name.into(), version: version.into() });
    };
    let name = dll.as_str();
    for dep in deps_of(name) {
        enable_rec(ini, ctx, version, dep, out, prof + 1)?;
    }
    if ini.enable_extension(name, is_zend(name)) {
        out.push(name.into());
    }
    for dep in deps_of(name) {
        ini.ensure_loaded_before(dep, name);
    }
    if name == "opcache" {
        reglages_opcache_dev(ini);
    }
    Ok(())
}

/// En développement, le code modifié doit être pris en compte immédiatement.
fn reglages_opcache_dev(ini: &mut IniFile) {
    if !ini.is_set("opcache.revalidate_freq") {
        ini.set("opcache.revalidate_freq", "0");
    }
    if !ini.is_set("opcache.validate_timestamps") {
        ini.set("opcache.validate_timestamps", "1");
    }
}

/// Désactive une extension. Refuse si une autre extension active en dépend.
pub fn disable(ini: &mut IniFile, ctx: &ExtContext, name: &str) -> Result<bool> {
    let name = name.to_ascii_lowercase();
    if name == "opcache" && ctx.opcache_integre() {
        ini.set("opcache.enable", "0");
        return Ok(true);
    }
    if ctx.builtin.contains(&name) {
        return Err(Error::ExtensionIntegree { extension: name });
    }
    let dependants: Vec<String> = ini
        .extensions()
        .into_iter()
        .filter(|e| e.active && deps_of(&e.name).contains(&name.as_str()))
        .map(|e| e.name)
        .collect();
    if !dependants.is_empty() {
        return Err(Error::ExtensionRequise { extension: name, par: dependants.join(", ") });
    }
    Ok(ini.disable_extension(&name))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(builtin: &[&str], dlls: &[&str]) -> ExtContext {
        ExtContext {
            builtin: builtin.iter().map(|s| s.to_string()).collect(),
            dlls: dlls.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn active_avec_dependances_et_ordre() {
        // exif est listée avant mbstring dans ce php.ini : l'ordre doit être corrigé.
        let mut ini = IniFile::parse(";extension=exif\n;extension=curl\n;extension=mbstring\n");
        let c = ctx(&["pdo", "mysqlnd", "core"], &["exif", "curl", "mbstring", "pdo_mysql"]);
        let act = enable(&mut ini, &c, "8.3.37", "exif").unwrap();
        assert_eq!(act, ["mbstring", "exif"]);
        let l = ini.lines();
        let pos = |n: &str| l.iter().position(|x| x == &format!("extension={n}")).unwrap();
        assert!(pos("mbstring") < pos("exif"), "{l:?}");

        // pdo et mysqlnd sont intégrées : seule pdo_mysql est ajoutée.
        assert_eq!(enable(&mut ini, &c, "8.3.37", "pdo_mysql").unwrap(), ["pdo_mysql"]);
        // Extension sans DLL.
        assert_eq!(enable(&mut ini, &c, "8.3.37", "redis").unwrap_err().code(), "extension.inconnue");
        // Intégrée : rien à faire.
        assert!(enable(&mut ini, &c, "8.3.37", "core").unwrap().is_empty());
        // PHP 7.4 : « gd » demandé → DLL « gd2 » activée.
        let mut ini = IniFile::parse(";extension=gd2\n");
        let c74 = ctx(&[], &["gd2"]);
        assert_eq!(enable(&mut ini, &c74, "7.4.33", "gd").unwrap(), ["gd2"]);
        assert!(ini.is_extension_enabled("gd2"));
    }

    #[test]
    fn noms_de_modules() {
        assert_eq!(module_name("gd2"), "gd");
        assert_eq!(module_name("oci8_19"), "oci8");
        assert_eq!(module_name("curl"), "curl");
    }

    #[test]
    fn desactivation_protegee() {
        let mut ini = IniFile::parse("extension=mbstring\nextension=exif\n");
        let c = ctx(&["core"], &["exif", "mbstring"]);
        let e = disable(&mut ini, &c, "mbstring").unwrap_err();
        assert_eq!(e.code(), "extension.requise");
        assert_eq!(e.params()["par"], "exif");
        assert!(disable(&mut ini, &c, "exif").unwrap());
        assert!(disable(&mut ini, &c, "mbstring").unwrap());
        assert_eq!(disable(&mut ini, &c, "core").unwrap_err().code(), "extension.integree");
    }

    #[test]
    fn opcache_zend_et_reglages_dev() {
        let mut ini = IniFile::parse(";zend_extension=opcache\n");
        let c = ctx(&[], &["opcache"]);
        enable(&mut ini, &c, "8.3.37", "opcache").unwrap();
        assert!(ini.lines().contains(&"zend_extension=opcache".to_string()));
        assert_eq!(ini.get("opcache.revalidate_freq").as_deref(), Some("0"));

        // PHP 8.5+ : OPcache intégré, piloté par opcache.enable.
        let mut ini = IniFile::parse("");
        let c = ctx(&["opcache"], &[]);
        assert!(disable(&mut ini, &c, "opcache").unwrap());
        assert_eq!(ini.get("opcache.enable").as_deref(), Some("0"));
        assert_eq!(enable(&mut ini, &c, "8.5.13", "opcache").unwrap(), ["opcache"]);
        assert_eq!(ini.get("opcache.enable").as_deref(), Some("1"));
    }
}
