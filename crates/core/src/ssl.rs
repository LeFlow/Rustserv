//! HTTPS local, à la manière de mkcert :
//! - une **autorité de certification (CA) locale** créée une fois (`ssl/ca/`), installée
//!   dans le magasin « Autorités racines de confiance » de l'utilisateur Windows
//!   (Chrome, Edge et Firefox récents l'utilisent) ;
//! - un **certificat par site** (`ssl/sites/<host>.pem`) signé par cette CA, valable pour
//!   `site.test` et `*.site.test`, régénéré automatiquement avant expiration.
//!
//! La clé privée de la CA ne quitte jamais `ssl/ca/` : quiconque la possède peut créer
//! des certificats acceptés par ce PC. Elle est donc propre à chaque installation.

use crate::error::{Error, Result};
use crate::paths::Paths;
use rcgen::{
    BasicConstraints, CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair, KeyUsagePurpose,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use time::{Duration, OffsetDateTime};

/// Durée de vie de la CA (10 ans) et des certificats de site (825 jours, limite Apple/Chrome).
const CA_JOURS: i64 = 3650;
const SITE_JOURS: i64 = 825;
/// Un certificat est renouvelé s'il expire dans moins de 30 jours.
const RENOUVELLEMENT_JOURS: i64 = 30;

fn ssl_err(e: impl std::fmt::Display) -> Error {
    Error::Ssl { detail: e.to_string() }
}

/// Informations sur la CA (fichier `ssl/ca/ca.json`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CaInfo {
    pub nom: String,
    pub cree_le: i64,
    pub expire_le: i64,
    /// Empreinte SHA256 du certificat (DER), en hexadécimal.
    pub empreinte: String,
}

/// CA chargée en mémoire.
pub struct Ca {
    pub info: CaInfo,
    pub der: Vec<u8>,
    key: KeyPair,
}

fn ca_dir(paths: &Paths) -> PathBuf {
    paths.ssl().join("ca")
}

/// Chemin du certificat de la CA (PEM), à importer manuellement si besoin (ex. Firefox ancien).
pub fn ca_cert_path(paths: &Paths) -> PathBuf {
    ca_dir(paths).join("rootCA.pem")
}

fn dn_ca(nom: &str) -> DistinguishedName {
    let mut dn = DistinguishedName::new();
    dn.push(DnType::OrganizationName, "Rustserv (developpement local)");
    dn.push(DnType::CommonName, nom);
    dn
}

/// Paramètres de la CA (reconstruits à l'identique pour signer les sites).
fn ca_params(info: &CaInfo) -> Result<CertificateParams> {
    let mut p = CertificateParams::new(Vec::<String>::new()).map_err(ssl_err)?;
    p.distinguished_name = dn_ca(&info.nom);
    p.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
    p.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign, KeyUsagePurpose::DigitalSignature];
    p.not_before = OffsetDateTime::from_unix_timestamp(info.cree_le).map_err(ssl_err)? - Duration::days(1);
    p.not_after = OffsetDateTime::from_unix_timestamp(info.expire_le).map_err(ssl_err)?;
    Ok(p)
}

fn write(p: &Path, content: impl AsRef<[u8]>) -> Result<()> {
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d).map_err(|e| Error::fichier(d, e))?;
    }
    std::fs::write(p, content).map_err(|e| Error::fichier(p, e))
}

fn read(p: &Path) -> Result<String> {
    std::fs::read_to_string(p).map_err(|e| Error::fichier(p, e))
}

/// Charge la CA si elle existe.
pub fn load_ca(paths: &Paths) -> Result<Option<Ca>> {
    let dir = ca_dir(paths);
    let info_path = dir.join("ca.json");
    if !info_path.is_file() {
        return Ok(None);
    }
    let info: CaInfo = serde_json::from_str(&read(&info_path)?).map_err(ssl_err)?;
    let key = KeyPair::from_pem(&read(&dir.join("rootCA-key.pem"))?).map_err(ssl_err)?;
    let der = std::fs::read(dir.join("rootCA.der")).map_err(|e| Error::fichier(dir.join("rootCA.der"), e))?;
    Ok(Some(Ca { info, der, key }))
}

/// Crée la CA si elle n'existe pas encore (sinon renvoie l'existante).
pub fn ensure_ca(paths: &Paths) -> Result<Ca> {
    if let Some(ca) = load_ca(paths)? {
        return Ok(ca);
    }
    let maintenant = OffsetDateTime::now_utc();
    let machine = std::env::var("COMPUTERNAME").unwrap_or_else(|_| "PC".into());
    let info0 = CaInfo {
        // Date dans le nom : deux CA successives restent distinguables dans Windows.
        nom: format!("Rustserv CA locale - {machine} - {}", maintenant.date()),
        cree_le: maintenant.unix_timestamp(),
        expire_le: (maintenant + Duration::days(CA_JOURS)).unix_timestamp(),
        empreinte: String::new(),
    };
    let key = KeyPair::generate().map_err(ssl_err)?;
    let cert = ca_params(&info0)?.self_signed(&key).map_err(ssl_err)?;
    let der = cert.der().to_vec();
    let info = CaInfo { empreinte: hex::encode(Sha256::digest(&der)), ..info0 };

    let dir = ca_dir(paths);
    write(&dir.join("rootCA-key.pem"), key.serialize_pem())?;
    write(&dir.join("rootCA.pem"), cert.pem())?;
    write(&dir.join("rootCA.der"), &der)?;
    write(&dir.join("ca.json"), serde_json::to_string_pretty(&info).map_err(ssl_err)?)?;
    tracing::info!(nom = %info.nom, "autorité de certification locale créée");
    Ok(Ca { info, der, key })
}

/// Supprime la CA et tous les certificats de site (à faire après l'avoir retirée de Windows).
pub fn delete_all(paths: &Paths) -> Result<()> {
    let ssl = paths.ssl();
    for d in ["ca", "sites"] {
        let p = ssl.join(d);
        if p.exists() {
            std::fs::remove_dir_all(&p).map_err(|e| Error::fichier(&p, e))?;
        }
    }
    Ok(())
}

/// Métadonnées d'un certificat de site (`<host>.json`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct SiteMeta {
    noms: Vec<String>,
    expire_le: i64,
    /// Empreinte de la CA qui l'a signé (une nouvelle CA invalide les anciens certificats).
    ca: String,
}

/// Chemins du certificat et de la clé d'un site.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SiteCert {
    pub cert: PathBuf,
    pub key: PathBuf,
}

/// Noms couverts par le certificat d'un hôte.
fn noms_pour(host: &str) -> Vec<String> {
    if host == "localhost" {
        vec!["localhost".into(), "127.0.0.1".into()]
    } else {
        vec![host.to_string(), format!("*.{host}")]
    }
}

/// Certificat d'un hôte (`monsite.test` ou `localhost`), créé ou renouvelé si nécessaire.
pub fn ensure_site_cert(paths: &Paths, ca: &Ca, host: &str) -> Result<SiteCert> {
    let dir = paths.ssl().join("sites");
    let out = SiteCert { cert: dir.join(format!("{host}.pem")), key: dir.join(format!("{host}-key.pem")) };
    let meta_path = dir.join(format!("{host}.json"));
    let noms = noms_pour(host);
    let limite = (OffsetDateTime::now_utc() + Duration::days(RENOUVELLEMENT_JOURS)).unix_timestamp();

    if out.cert.is_file() && out.key.is_file() {
        if let Ok(meta) = read(&meta_path).and_then(|t| serde_json::from_str::<SiteMeta>(&t).map_err(ssl_err)) {
            if meta.noms == noms && meta.ca == ca.info.empreinte && meta.expire_le > limite {
                return Ok(out);
            }
        }
    }

    let maintenant = OffsetDateTime::now_utc();
    let mut p = CertificateParams::new(noms.clone()).map_err(ssl_err)?;
    p.distinguished_name = {
        let mut dn = DistinguishedName::new();
        dn.push(DnType::OrganizationName, "Rustserv (developpement local)");
        dn.push(DnType::CommonName, host);
        dn
    };
    p.is_ca = IsCa::NoCa;
    p.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    p.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    p.use_authority_key_identifier_extension = true;
    p.not_before = maintenant - Duration::days(1);
    p.not_after = maintenant + Duration::days(SITE_JOURS);

    let key = KeyPair::generate().map_err(ssl_err)?;
    let issuer = Issuer::new(ca_params(&ca.info)?, &ca.key);
    let cert = p.signed_by(&key, &issuer).map_err(ssl_err)?;
    write(&out.cert, cert.pem())?;
    write(&out.key, key.serialize_pem())?;
    let meta = SiteMeta { noms, expire_le: p.not_after.unix_timestamp(), ca: ca.info.empreinte.clone() };
    write(&meta_path, serde_json::to_string_pretty(&meta).map_err(ssl_err)?)?;
    tracing::info!(host, "certificat HTTPS créé");
    Ok(out)
}

/// État du HTTPS, pour l'interface.
#[derive(Debug, Clone, Serialize)]
pub struct SslStatus {
    pub ca: Option<CaInfo>,
    /// La CA est approuvée par Windows (magasin de l'utilisateur).
    pub installee: bool,
    pub ca_pem: String,
}

pub fn status(paths: &Paths) -> Result<SslStatus> {
    let ca = load_ca(paths)?;
    Ok(SslStatus {
        installee: ca.as_ref().is_some_and(|c| crate::platform::root_cert_installed(&c.der)),
        ca: ca.map(|c| c.info),
        ca_pem: ca_cert_path(paths).display().to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ca_puis_certificats_de_site() {
        let t = tempfile::tempdir().unwrap();
        let paths = Paths::new(t.path());
        paths.ensure_layout().unwrap();
        assert!(load_ca(&paths).unwrap().is_none());

        let ca = ensure_ca(&paths).unwrap();
        assert!(ca.info.nom.starts_with("Rustserv CA locale"));
        assert_eq!(ca.info.empreinte.len(), 64);
        // Rechargement : même CA.
        let ca2 = ensure_ca(&paths).unwrap();
        assert_eq!(ca2.info, ca.info);
        assert_eq!(ca2.der, ca.der);

        let c = ensure_site_cert(&paths, &ca2, "demo.test").unwrap();
        let pem = std::fs::read_to_string(&c.cert).unwrap();
        assert!(pem.starts_with("-----BEGIN CERTIFICATE-----"));
        assert!(std::fs::read_to_string(&c.key).unwrap().contains("PRIVATE KEY"));
        // Pas de régénération inutile.
        let avant = std::fs::metadata(&c.cert).unwrap().modified().unwrap();
        ensure_site_cert(&paths, &ca2, "demo.test").unwrap();
        assert_eq!(std::fs::metadata(&c.cert).unwrap().modified().unwrap(), avant);

        // Une nouvelle CA invalide les anciens certificats.
        delete_all(&paths).unwrap();
        assert!(load_ca(&paths).unwrap().is_none());
        let ca3 = ensure_ca(&paths).unwrap();
        assert_ne!(ca3.info.empreinte, ca.info.empreinte);
        ensure_site_cert(&paths, &ca3, "localhost").unwrap();
    }
}
