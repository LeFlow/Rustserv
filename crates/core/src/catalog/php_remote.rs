//! Détection des nouvelles versions de PHP via windows.php.net.
//!
//! `releases.json` liste, pour chaque branche maintenue, la dernière version et
//! ses archives (NTS/TS, compilateur, architecture) avec leur SHA256 :
//!
//! ```json
//! { "8.3": { "version": "8.3.37",
//!            "nts-vs16-x64": { "zip": { "path": "php-8.3.37-nts-Win32-vs16-x64.zip", "sha256": "…" } } } }
//! ```

use super::{Build, Release};
use crate::error::{Error, Result};
use serde_json::Value;

pub const RELEASES_URL: &str = "https://windows.php.net/downloads/releases/releases.json";
const BASE: &str = "https://windows.php.net/downloads/releases/";

/// Version minimale de vcruntime140.dll selon le compilateur.
pub fn vcruntime_for(compiler: &str) -> Option<&'static str> {
    match compiler {
        "vc15" => Some("14.10"),
        "vs16" => Some("14.20"),
        "vs17" => Some("14.30"),
        "vs18" => Some("14.50"),
        _ => None,
    }
}

/// Extrait les builds **NTS x64** (mode FastCGI) de `releases.json`.
pub fn parse_releases(json: &str) -> Result<Vec<Release>> {
    let v: Value = serde_json::from_str(json)
        .map_err(|e| Error::JsonInvalide { fichier: "releases.json".into(), detail: e.to_string() })?;
    let obj = v.as_object().ok_or_else(|| Error::JsonInvalide { fichier: "releases.json".into(), detail: "objet attendu".into() })?;
    let mut out = Vec::new();
    for (branch, b) in obj {
        let Some(version) = b.get("version").and_then(Value::as_str) else { continue };
        let Some(bo) = b.as_object() else { continue };
        // Clé du type « nts-vs16-x64 ».
        let Some((key, build)) = bo.iter().find(|(k, _)| k.starts_with("nts-") && k.ends_with("-x64")) else { continue };
        let compiler = key.trim_start_matches("nts-").trim_end_matches("-x64").to_string();
        let (Some(path), Some(sha)) = (
            build.pointer("/zip/path").and_then(Value::as_str),
            build.pointer("/zip/sha256").and_then(Value::as_str),
        ) else {
            continue;
        };
        out.push(Release {
            version: version.to_string(),
            branch: Some(branch.clone()),
            build: Some(Build { thread_safe: false, compiler: compiler.clone(), arch: "x64".into() }),
            url: format!("{BASE}{path}"),
            // Quand une version est remplacée, son archive part dans /archives/.
            mirrors: vec![format!("{BASE}archives/{path}")],
            sha256: sha.to_ascii_lowercase(),
            subdir: None,
            vcruntime_min: vcruntime_for(&compiler).map(Into::into),
            file: None,
        });
    }
    Ok(out)
}

/// Télécharge et analyse `releases.json`.
pub async fn fetch(client: &reqwest::Client) -> Result<Vec<Release>> {
    let err = |e: reqwest::Error| Error::TelechargementEchoue { url: RELEASES_URL.into(), detail: e.to_string() };
    let resp = client.get(RELEASES_URL).send().await.map_err(err)?;
    if !resp.status().is_success() {
        return Err(Error::HttpStatut { url: RELEASES_URL.into(), status: resp.status().as_u16() });
    }
    parse_releases(&resp.text().await.map_err(err)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXTRAIT: &str = r#"{
        "8.3": { "version": "8.3.38",
                 "source": { "path": "php-8.3.38-src.zip", "sha256": "x" },
                 "ts-vs16-x64":  { "zip": { "path": "php-8.3.38-Win32-vs16-x64.zip", "sha256": "AAAA" } },
                 "nts-vs16-x64": { "mtime": "2026-12-01", "zip": { "path": "php-8.3.38-nts-Win32-vs16-x64.zip", "sha256": "ABCDEF" } },
                 "nts-vs16-x86": { "zip": { "path": "php-8.3.38-nts-Win32-vs16-x86.zip", "sha256": "BBBB" } } },
        "8.6": { "version": "8.6.1",
                 "nts-vs18-x64": { "zip": { "path": "php-8.6.1-nts-Win32-vs18-x64.zip", "sha256": "cd" } } },
        "5.6": { "version": "5.6.40" }
    }"#;

    #[test]
    fn extrait_les_builds_nts_x64() {
        let r = parse_releases(EXTRAIT).unwrap();
        assert_eq!(r.len(), 2, "5.6 sans build NTS x64 ignorée");
        let p83 = r.iter().find(|r| r.version == "8.3.38").unwrap();
        assert_eq!(p83.url, "https://windows.php.net/downloads/releases/php-8.3.38-nts-Win32-vs16-x64.zip");
        assert_eq!(p83.mirrors[0], "https://windows.php.net/downloads/releases/archives/php-8.3.38-nts-Win32-vs16-x64.zip");
        assert_eq!(p83.sha256, "abcdef");
        assert_eq!(p83.branch.as_deref(), Some("8.3"));
        assert_eq!(p83.build.as_ref().unwrap().compiler, "vs16");
        assert_eq!(p83.vcruntime_min.as_deref(), Some("14.20"));
        let p86 = r.iter().find(|r| r.version == "8.6.1").unwrap();
        assert_eq!(p86.vcruntime_min.as_deref(), Some("14.50"));
    }
}
