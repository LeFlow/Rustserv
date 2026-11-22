//! Erreurs typées de Rustserv.
//!
//! Règle d'or : le Rust ne produit **jamais** le texte affiché à l'utilisateur.
//! Chaque erreur expose un `code` stable (ex. `port.occupe`) et des `params`
//! (ex. `{ "port": 80 }`). L'interface traduit grâce à `ui/src/i18n/fr.json`.
//! Le message `Display` ci-dessous ne sert qu'aux journaux (logs) techniques.

use serde::Serialize;
use serde_json::{json, Value};
use std::path::PathBuf;

/// Alias pratique utilisé dans tout le noyau.
pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Un port nécessaire est déjà utilisé par un autre programme.
    #[error("port {port} déjà utilisé (processus : {process:?}, pid : {pid:?})")]
    PortOccupe { port: u16, process: Option<String>, pid: Option<u32> },

    /// Port réservé par Windows (plages exclues par Hyper-V/WSL/Docker) : bind refusé.
    #[error("port {port} réservé par Windows")]
    PortReserve { port: u16 },

    /// Le service s'est arrêté pendant son démarrage, ou n'a pas ouvert ses ports à temps.
    #[error("démarrage de {service} échoué : {detail}")]
    ServiceDemarrage { service: String, detail: String, journal: String },

    /// Le service s'est arrêté tout seul et n'a pas pu être relancé.
    #[error("{service} s'est arrêté (code {code:?})")]
    ServicePlantage { service: String, code: Option<i32>, journal: String },

    #[error("{service} est déjà démarré")]
    ServiceDejaDemarre { service: String },

    #[error("service inconnu : {service}")]
    ServiceInconnu { service: String },

    /// Exécutable absent (paquet non installé ou supprimé).
    #[error("exécutable introuvable : {chemin:?}")]
    ExecutableIntrouvable { chemin: PathBuf },

    /// Aucune version installée d'un paquet nécessaire.
    #[error("aucune version de {id} installée")]
    AucuneVersion { id: String },

    /// La configuration générée est refusée par le serveur (`nginx -t`, `httpd -t`).
    #[error("configuration de {service} invalide : {detail}")]
    ConfigInvalide { service: String, detail: String },

    /// PHP ne démarre pas proprement avec ce php.ini (extension introuvable…).
    #[error("PHP {version} invalide : {detail}")]
    PhpInvalide { version: String, detail: String },

    /// Extension absente de cette version de PHP (pas de DLL dans ext/).
    #[error("extension {extension} indisponible pour PHP {version}")]
    ExtensionInconnue { extension: String, version: String },

    /// Extension compilée dans PHP : impossible de la désactiver.
    #[error("extension {extension} intégrée à PHP")]
    ExtensionIntegree { extension: String },

    /// D'autres extensions actives dépendent de celle-ci.
    #[error("extension {extension} requise par {par}")]
    ExtensionRequise { extension: String, par: String },

    /// Aucun build PECL compatible avec cette branche de PHP.
    #[error("aucun build PECL de {extension} pour PHP {branche}")]
    PeclIntrouvable { extension: String, branche: String },

    #[error("profil inconnu : {profil}")]
    ProfilInconnu { profil: String },

    /// L'utilisateur a refusé la fenêtre de Windows (UAC ou confirmation de certificat).
    #[error("autorisation refusée pour : {operation}")]
    AdminRefuse { operation: String },

    /// Le helper n'a pas pu modifier le fichier hosts.
    #[error("échec de la mise à jour du fichier hosts : {detail}")]
    HostsEchec { detail: String },

    /// Erreur de génération ou d'installation de certificat.
    #[error("HTTPS : {detail}")]
    Ssl { detail: String },

    /// Programme de test introuvable (PHPUnit/Pest non installés dans le projet, script absent).
    #[error("{quoi} introuvable ({aide})")]
    CompatIntrouvable { quoi: String, aide: String },

    /// Un outil d'analyse a échoué ou renvoyé un résultat illisible.
    #[error("{outil} a échoué : {detail}")]
    CompatEchec { outil: String, detail: String },

    /// Échec d'une étape d'import depuis Laragon/WAMP.
    #[error("import : {detail}")]
    Migration { detail: String },

    /// Laragon ou WAMP tourne encore (ports et données occupés).
    #[error("{outil} est en marche")]
    SourceActive { outil: String },

    /// Réglages incohérents (ports en double…).
    #[error("réglages invalides : {detail}")]
    ReglagesInvalides { detail: String },

    /// Échec de la création du dossier de données MariaDB.
    #[error("initialisation de la base échouée : {detail}")]
    BaseInit { detail: String },

    /// Erreur dans un gabarit de configuration.
    #[error("gabarit {nom} invalide : {detail}")]
    Gabarit { nom: String, detail: String },

    /// Runtime Visual C++ absent ou trop ancien pour ce binaire.
    #[error("runtime Visual C++ {requis} requis (installé : {installe:?})")]
    VcRuntime { requis: String, installe: Option<String> },

    /// Erreur d'entrée/sortie sur un fichier ou dossier précis.
    #[error("erreur d'accès à {path:?} : {source}")]
    Fichier { path: PathBuf, #[source] source: std::io::Error },

    /// Erreur d'entrée/sortie générique (sans chemin connu).
    #[error("erreur d'entrée/sortie : {0}")]
    Io(#[from] std::io::Error),

    /// JSON invalide (catalogue, réglages…).
    #[error("JSON invalide dans {fichier} : {detail}")]
    JsonInvalide { fichier: String, detail: String },

    /// Opération nécessitant des droits administrateur.
    #[error("droits administrateur requis pour : {operation}")]
    AdminRequis { operation: String },

    /// Le catalogue ne respecte pas le format attendu.
    #[error("catalogue invalide : {problemes:?}")]
    CatalogueInvalide { problemes: Vec<String> },

    /// Paquet absent du catalogue.
    #[error("paquet inconnu : {id}")]
    PaquetInconnu { id: String },

    /// Version absente du catalogue pour ce paquet.
    #[error("version {version} inconnue pour {id}")]
    VersionInconnue { id: String, version: String },

    /// Version déjà présente dans bin/.
    #[error("{id} {version} est déjà installé")]
    DejaInstalle { id: String, version: String },

    /// Version non installée (désinstallation, démarrage…).
    #[error("{id} {version} n'est pas installé")]
    NonInstalle { id: String, version: String },

    /// Échec réseau (DNS, connexion, coupure…).
    #[error("téléchargement de {url} échoué : {detail}")]
    TelechargementEchoue { url: String, detail: String },

    /// Le serveur a répondu avec un code HTTP d'erreur (404, 503…).
    #[error("le serveur a répondu {status} pour {url}")]
    HttpStatut { url: String, status: u16 },

    /// Empreinte SHA256 différente de celle du catalogue : fichier corrompu ou falsifié.
    #[error("SHA256 invalide pour {fichier} : attendu {attendu}, obtenu {obtenu}")]
    HashInvalide { fichier: String, attendu: String, obtenu: String },

    /// Archive illisible ou contenant des chemins dangereux.
    #[error("archive {fichier} invalide : {detail}")]
    ArchiveInvalide { fichier: String, detail: String },

    /// Opération annulée par l'utilisateur.
    #[error("opération annulée")]
    Annule,

    /// Erreur interne inattendue (bug) : on garde le détail pour les logs.
    #[error("erreur interne : {0}")]
    Interne(String),
}

impl Error {
    /// Code stable, utilisé comme clé de traduction côté interface.
    pub fn code(&self) -> &'static str {
        match self {
            Error::PortOccupe { .. } => "port.occupe",
            Error::PortReserve { .. } => "port.reserve",
            Error::ServiceDemarrage { .. } => "service.demarrage",
            Error::ServicePlantage { .. } => "service.plantage",
            Error::ServiceDejaDemarre { .. } => "service.deja_demarre",
            Error::ServiceInconnu { .. } => "service.inconnu",
            Error::ExecutableIntrouvable { .. } => "service.executable",
            Error::VcRuntime { .. } => "installation.vcruntime",
            Error::AucuneVersion { .. } => "service.aucune_version",
            Error::ConfigInvalide { .. } => "config.invalide",
            Error::Gabarit { .. } => "config.gabarit",
            Error::BaseInit { .. } => "service.base_init",
            Error::ReglagesInvalides { .. } => "reglages.invalides",
            Error::PhpInvalide { .. } => "php.invalide",
            Error::ExtensionInconnue { .. } => "extension.inconnue",
            Error::ExtensionIntegree { .. } => "extension.integree",
            Error::ExtensionRequise { .. } => "extension.requise",
            Error::PeclIntrouvable { .. } => "extension.pecl_introuvable",
            Error::ProfilInconnu { .. } => "extension.profil_inconnu",
            Error::AdminRefuse { .. } => "admin.refuse",
            Error::HostsEchec { .. } => "hosts.echec",
            Error::Ssl { .. } => "ssl.erreur",
            Error::CompatIntrouvable { aide, .. } => match aide.as_str() {
                "phpunit" => "compat.phpunit_absent",
                "pest" => "compat.pest_absent",
                _ => "compat.introuvable",
            },
            Error::CompatEchec { .. } => "compat.echec",
            Error::Migration { .. } => "migration.echec",
            Error::SourceActive { .. } => "migration.source_active",
            Error::Fichier { .. } => "fichier.acces",
            Error::Io(_) => "io.generique",
            Error::JsonInvalide { .. } => "json.invalide",
            Error::AdminRequis { .. } => "admin.requis",
            Error::CatalogueInvalide { .. } => "catalogue.invalide",
            Error::PaquetInconnu { .. } => "catalogue.paquet_inconnu",
            Error::VersionInconnue { .. } => "catalogue.version_inconnue",
            Error::DejaInstalle { .. } => "installation.deja_installe",
            Error::NonInstalle { .. } => "installation.non_installe",
            Error::TelechargementEchoue { .. } => "telechargement.echec",
            Error::HttpStatut { .. } => "telechargement.http",
            Error::HashInvalide { .. } => "telechargement.hash",
            Error::ArchiveInvalide { .. } => "installation.archive",
            Error::Annule => "annule",
            Error::Interne(_) => "interne",
        }
    }

    /// Paramètres injectés dans le message traduit.
    pub fn params(&self) -> Value {
        match self {
            Error::PortOccupe { port, process, pid } => {
                json!({
                    "port": port,
                    "processus": process.clone().unwrap_or_else(|| "inconnu".into()),
                    "pid": pid.map(|p| p.to_string()).unwrap_or_else(|| "?".into()),
                })
            }
            Error::PortReserve { port } => json!({ "port": port }),
            Error::ServiceDemarrage { service, detail, journal } => {
                json!({ "service": service, "detail": detail, "journal": journal })
            }
            Error::ServicePlantage { service, code, journal } => json!({
                "service": service,
                "code": code.map(|c| c.to_string()).unwrap_or_else(|| "?".into()),
                "journal": journal,
            }),
            Error::ServiceDejaDemarre { service } | Error::ServiceInconnu { service } => json!({ "service": service }),
            Error::ExecutableIntrouvable { chemin } => json!({ "chemin": chemin.display().to_string() }),
            Error::AucuneVersion { id } => json!({ "id": id }),
            Error::ConfigInvalide { service, detail } => json!({ "service": service, "detail": detail }),
            Error::Gabarit { nom, detail } => json!({ "nom": nom, "detail": detail }),
            Error::BaseInit { detail } | Error::ReglagesInvalides { detail } => json!({ "detail": detail }),
            Error::PhpInvalide { version, detail } => json!({ "version": version, "detail": detail }),
            Error::ExtensionInconnue { extension, version } => json!({ "extension": extension, "version": version }),
            Error::ExtensionIntegree { extension } => json!({ "extension": extension }),
            Error::ExtensionRequise { extension, par } => json!({ "extension": extension, "par": par }),
            Error::PeclIntrouvable { extension, branche } => json!({ "extension": extension, "branche": branche }),
            Error::ProfilInconnu { profil } => json!({ "profil": profil }),
            Error::AdminRefuse { operation } => json!({ "operation": operation }),
            Error::HostsEchec { detail } | Error::Ssl { detail } => json!({ "detail": detail }),
            Error::CompatIntrouvable { quoi, aide } => json!({ "quoi": quoi, "aide": aide }),
            Error::CompatEchec { outil, detail } => json!({ "outil": outil, "detail": detail }),
            Error::Migration { detail } => json!({ "detail": detail }),
            Error::SourceActive { outil } => json!({ "source": if outil == "wamp" { "WAMP" } else { "Laragon" } }),
            Error::VcRuntime { requis, installe } => json!({
                "requis": requis,
                "installe": installe.clone().unwrap_or_else(|| "aucun".into()),
                "lien": "https://aka.ms/vs/17/release/vc_redist.x64.exe",
            }),
            Error::Fichier { path, source } => {
                json!({ "chemin": path.display().to_string(), "detail": source.to_string() })
            }
            Error::Io(e) => json!({ "detail": e.to_string() }),
            Error::JsonInvalide { fichier, detail } => json!({ "fichier": fichier, "detail": detail }),
            Error::AdminRequis { operation } => json!({ "operation": operation }),
            Error::CatalogueInvalide { problemes } => {
                json!({ "nombre": problemes.len(), "detail": problemes.join(" ; ") })
            }
            Error::PaquetInconnu { id } => json!({ "id": id }),
            Error::VersionInconnue { id, version }
            | Error::DejaInstalle { id, version }
            | Error::NonInstalle { id, version } => json!({ "id": id, "version": version }),
            Error::TelechargementEchoue { url, detail } => json!({ "url": url, "detail": detail }),
            Error::HttpStatut { url, status } => json!({ "url": url, "status": status }),
            Error::HashInvalide { fichier, attendu, obtenu } => {
                json!({ "fichier": fichier, "attendu": attendu, "obtenu": obtenu })
            }
            Error::ArchiveInvalide { fichier, detail } => json!({ "fichier": fichier, "detail": detail }),
            Error::Annule => json!({}),
            Error::Interne(d) => json!({ "detail": d }),
        }
    }

    /// Construit une erreur fichier en gardant le chemin (bien plus utile qu'un io::Error nu).
    pub fn fichier(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Error::Fichier { path: path.into(), source }
    }
}

/// Forme sérialisée envoyée à l'interface : `{ code, params, detail }`.
#[derive(Debug, Serialize)]
pub struct ErrorPayload {
    pub code: &'static str,
    pub params: Value,
    /// Message technique (anglais/français brut) pour la section « détails ».
    pub detail: String,
}

impl From<&Error> for ErrorPayload {
    fn from(e: &Error) -> Self {
        ErrorPayload { code: e.code(), params: e.params(), detail: e.to_string() }
    }
}

impl Serialize for Error {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        ErrorPayload::from(self).serialize(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Une instance de chaque variante : le `match` exhaustif de `code()` oblige
    /// à compléter cette liste quand on ajoute une variante.
    fn toutes_les_variantes() -> Vec<Error> {
        let s = String::new;
        vec![
            Error::PortOccupe { port: 1, process: None, pid: None },
            Error::PortReserve { port: 1 },
            Error::ServiceDemarrage { service: s(), detail: s(), journal: s() },
            Error::ServicePlantage { service: s(), code: None, journal: s() },
            Error::ServiceDejaDemarre { service: s() },
            Error::ServiceInconnu { service: s() },
            Error::ExecutableIntrouvable { chemin: PathBuf::new() },
            Error::VcRuntime { requis: s(), installe: None },
            Error::AucuneVersion { id: s() },
            Error::ConfigInvalide { service: s(), detail: s() },
            Error::Gabarit { nom: s(), detail: s() },
            Error::BaseInit { detail: s() },
            Error::ReglagesInvalides { detail: s() },
            Error::PhpInvalide { version: s(), detail: s() },
            Error::ExtensionInconnue { extension: s(), version: s() },
            Error::ExtensionIntegree { extension: s() },
            Error::ExtensionRequise { extension: s(), par: s() },
            Error::PeclIntrouvable { extension: s(), branche: s() },
            Error::ProfilInconnu { profil: s() },
            Error::AdminRefuse { operation: s() },
            Error::HostsEchec { detail: s() },
            Error::Ssl { detail: s() },
            Error::CompatIntrouvable { quoi: s(), aide: "script".into() },
            Error::CompatIntrouvable { quoi: s(), aide: "phpunit".into() },
            Error::CompatIntrouvable { quoi: s(), aide: "pest".into() },
            Error::CompatEchec { outil: s(), detail: s() },
            Error::Migration { detail: s() },
            Error::SourceActive { outil: s() },
            Error::fichier("x", std::io::Error::other("x")),
            Error::Io(std::io::Error::other("x")),
            Error::JsonInvalide { fichier: s(), detail: s() },
            Error::AdminRequis { operation: s() },
            Error::CatalogueInvalide { problemes: vec![] },
            Error::PaquetInconnu { id: s() },
            Error::VersionInconnue { id: s(), version: s() },
            Error::DejaInstalle { id: s(), version: s() },
            Error::NonInstalle { id: s(), version: s() },
            Error::TelechargementEchoue { url: s(), detail: s() },
            Error::HttpStatut { url: s(), status: 404 },
            Error::HashInvalide { fichier: s(), attendu: s(), obtenu: s() },
            Error::ArchiveInvalide { fichier: s(), detail: s() },
            Error::Annule,
            Error::Interne(s()),
        ]
    }

    /// Chaque code d'erreur doit avoir sa traduction dans ui/src/i18n/fr.json,
    /// et chaque paramètre utilisé dans le texte doit être fourni par `params()`.
    #[test]
    fn toutes_les_erreurs_sont_traduites() {
        let fr: Value = serde_json::from_str(include_str!("../../../ui/src/i18n/fr.json")).unwrap();
        for e in toutes_les_variantes() {
            let mut node = &fr["erreurs"];
            for part in e.code().split('.') {
                node = &node[part];
            }
            let texte = node.as_str().unwrap_or_else(|| panic!("traduction manquante : erreurs.{}", e.code()));
            for cap in texte.split('{').skip(1) {
                let nom = cap.split('}').next().unwrap();
                assert!(e.params().get(nom).is_some(), "erreurs.{} utilise {{{nom}}} absent de params()", e.code());
            }
        }
    }

    #[test]
    fn serialise_code_et_params() {
        let e = Error::PortOccupe { port: 80, process: Some("httpd.exe".into()), pid: Some(42) };
        let v = serde_json::to_value(&e).unwrap();
        assert_eq!(v["code"], "port.occupe");
        assert_eq!(v["params"]["port"], 80);
        assert_eq!(v["params"]["processus"], "httpd.exe");
    }
}
