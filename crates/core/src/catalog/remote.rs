//! Recherche de nouvelles versions sur les sites officiels, pour tous les paquets.
//!
//! | Paquet   | Source                                   | Empreinte SHA256                 |
//! |----------|------------------------------------------|----------------------------------|
//! | php      | windows.php.net/…/releases.json          | publiée                          |
//! | mariadb  | downloads.mariadb.org/rest-api           | publiée                          |
//! | apache   | apachelounge.com/download                | publiée (fichier .txt)           |
//! | composer | getcomposer.org/versions                 | publiée (.sha256sum)             |
//! | adminer  | GitHub (vrana/adminer)                   | publiée (digest GitHub)          |
//! | wp-cli   | GitHub (wp-cli/wp-cli)                   | publiée (digest GitHub)          |
//! | nginx    | nginx.org/en/download.html               | **calculée** au téléchargement   |
//!
//! Chaque source est indépendante : une source indisponible n'empêche pas les autres.

use super::{php_remote, version, Release};
use crate::error::{Error, Result};
use serde_json::Value;
use sha2::{Digest, Sha256};

/// Résultat d'une recherche : versions trouvées et sources en échec.
#[derive(Debug, Default)]
pub struct Trouvailles {
    pub releases: Vec<(String, Release)>,
    pub erreurs: Vec<(String, String)>,
}

fn release(version: &str, url: String, sha256: String) -> Release {
    Release {
        version: version.into(),
        branch: None,
        build: None,
        url,
        mirrors: vec![],
        sha256: sha256.to_ascii_lowercase(),
        subdir: None,
        vcruntime_min: None,
        file: None,
        exclude: Vec::new(),
    }
}

async fn texte(client: &reqwest::Client, url: &str) -> Result<String> {
    let err = |e: reqwest::Error| Error::TelechargementEchoue { url: url.into(), detail: e.to_string() };
    let r = client.get(url).header("User-Agent", "Mozilla/5.0 (Rustserv)").send().await.map_err(err)?;
    if !r.status().is_success() {
        return Err(Error::HttpStatut { url: url.into(), status: r.status().as_u16() });
    }
    r.text().await.map_err(err)
}

async fn json(client: &reqwest::Client, url: &str) -> Result<Value> {
    serde_json::from_str(&texte(client, url).await?).map_err(|e| Error::JsonInvalide { fichier: url.into(), detail: e.to_string() })
}

/// Versions stables de nginx (mineure paire) listées sur la page de téléchargement.
pub fn parse_nginx(html: &str) -> Vec<String> {
    let mut v: Vec<String> = html
        .split("nginx-")
        .skip(1)
        .filter_map(|s| s.split(".zip").next())
        .filter(|s| s.chars().all(|c| c.is_ascii_digit() || c == '.') && s.matches('.').count() == 2)
        .filter(|s| s.split('.').nth(1).and_then(|m| m.parse::<u32>().ok()).is_some_and(|m| m % 2 == 0))
        .map(String::from)
        .collect();
    v.sort_by(|a, b| version::cmp(b, a));
    v.dedup();
    v
}

async fn nginx(client: &reqwest::Client) -> Result<Vec<Release>> {
    let html = texte(client, "https://nginx.org/en/download.html").await?;
    let mut out = Vec::new();
    // nginx ne publie pas de SHA256 : empreinte calculée sur l'archive officielle (HTTPS).
    for v in parse_nginx(&html).into_iter().take(1) {
        let url = format!("https://nginx.org/download/nginx-{v}.zip");
        let err = |e: reqwest::Error| Error::TelechargementEchoue { url: url.clone(), detail: e.to_string() };
        let octets = client.get(&url).send().await.map_err(err)?.bytes().await.map_err(err)?;
        out.push(release(&v, url.clone(), hex::encode(Sha256::digest(&octets))));
    }
    Ok(out)
}

/// Lien de l'archive Apache 64 bits et compilateur (« VS18 ») dans la page d'Apache Lounge.
pub fn parse_apache(html: &str) -> Option<(String, String, String)> {
    let lien = html.split("href=\"").skip(1).filter_map(|s| s.split('"').next()).find(|l| l.contains("httpd-") && l.contains("-Win64-VS") && l.ends_with(".zip"))?;
    let fichier = lien.rsplit('/').next()?;
    let version = fichier.strip_prefix("httpd-")?.split('-').next()?.to_string();
    let vs = fichier.rsplit('-').next()?.trim_end_matches(".zip").to_string();
    let url = if lien.starts_with("http") { lien.to_string() } else { format!("https://www.apachelounge.com{lien}") };
    Some((version, url, vs))
}

/// SHA256 dans le fichier .txt d'Apache Lounge (« SHA256-Checksum for: … : » puis l'empreinte).
pub fn parse_sha_apache(txt: &str) -> Option<String> {
    let i = txt.find("SHA256-Checksum")?;
    txt[i..].lines().skip(1).map(str::trim).find(|l| l.len() == 64 && l.chars().all(|c| c.is_ascii_hexdigit())).map(|s| s.to_ascii_lowercase())
}

async fn apache(client: &reqwest::Client) -> Result<Vec<Release>> {
    let html = texte(client, "https://www.apachelounge.com/download/").await?;
    let (v, url, vs) = parse_apache(&html).ok_or_else(|| Error::Interne("archive Apache introuvable sur Apache Lounge".into()))?;
    let sha = parse_sha_apache(&texte(client, &format!("{url}.txt")).await?).ok_or_else(|| Error::Interne("SHA256 Apache introuvable".into()))?;
    let mut r = release(&v, url, sha);
    r.subdir = Some("Apache24".into());
    r.vcruntime_min = php_remote::vcruntime_for(&vs.to_ascii_lowercase()).map(Into::into);
    Ok(vec![r])
}

async fn mariadb(client: &reqwest::Client) -> Result<Vec<Release>> {
    let index = json(client, "https://downloads.mariadb.org/rest-api/mariadb/").await?;
    let branches: Vec<String> = index["major_releases"]
        .as_array()
        .map(|a| a.iter().filter(|m| m["release_status"] == "Stable").filter_map(|m| m["release_id"].as_str().map(String::from)).collect())
        .unwrap_or_default();
    let mut out = Vec::new();
    for b in branches.into_iter().take(4) {
        let Ok(j) = json(client, &format!("https://downloads.mariadb.org/rest-api/mariadb/{b}/latest/")).await else { continue };
        let Some((v, infos)) = j["releases"].as_object().and_then(|o| o.iter().next()) else { continue };
        let Some(f) = infos["files"].as_array().and_then(|a| a.iter().find(|f| f["file_name"].as_str().is_some_and(|n| n.ends_with("-winx64.zip")))) else { continue };
        let (Some(nom), Some(sha)) = (f["file_name"].as_str(), f["checksum"]["sha256sum"].as_str()) else { continue };
        let mut r = release(v, format!("https://archive.mariadb.org/mariadb-{v}/winx64-packages/{nom}"), sha.into());
        r.mirrors = f["file_download_url"].as_str().map(|u| vec![u.replace("http://", "https://")]).unwrap_or_default();
        r.branch = Some(b);
        out.push(r);
    }
    Ok(out)
}

async fn composer(client: &reqwest::Client) -> Result<Vec<Release>> {
    let j = json(client, "https://getcomposer.org/versions").await?;
    let v = j["stable"][0]["version"].as_str().ok_or_else(|| Error::Interne("version Composer introuvable".into()))?.to_string();
    let base = format!("https://getcomposer.org/download/{v}/composer.phar");
    let sha = texte(client, &format!("{base}.sha256sum")).await?.split_whitespace().next().unwrap_or("").to_string();
    let mut r = release(&v, base, sha);
    r.file = Some("composer.phar".into());
    Ok(vec![r])
}

/// Dernière version GitHub : (version, url, sha256) de l'actif choisi par `filtre`.
async fn github(client: &reqwest::Client, depot: &str, filtre: impl Fn(&str, &str) -> bool) -> Result<(String, String, String)> {
    let j = json(client, &format!("https://api.github.com/repos/{depot}/releases/latest")).await?;
    let v = j["tag_name"].as_str().unwrap_or("").trim_start_matches('v').to_string();
    let a = j["assets"]
        .as_array()
        .and_then(|a| a.iter().find(|x| x["name"].as_str().is_some_and(|n| filtre(n, &v))))
        .ok_or_else(|| Error::Interne(format!("fichier introuvable dans la version {v} de {depot}")))?;
    let sha = a["digest"].as_str().and_then(|d| d.strip_prefix("sha256:")).ok_or_else(|| Error::Interne(format!("{depot} : empreinte non publiée")))?;
    Ok((v, a["browser_download_url"].as_str().unwrap_or("").to_string(), sha.to_string()))
}

async fn adminer(client: &reqwest::Client) -> Result<Vec<Release>> {
    let (v, url, sha) = github(client, "vrana/adminer", |n, v| n == format!("adminer-{v}.php")).await?;
    let mut r = release(&v, url, sha);
    r.file = Some("adminer.php".into());
    Ok(vec![r])
}

async fn wp_cli(client: &reqwest::Client) -> Result<Vec<Release>> {
    let (v, url, sha) = github(client, "wp-cli/wp-cli", |n, v| n == format!("wp-cli-{v}.phar")).await?;
    let mut r = release(&v, url, sha);
    r.file = Some("wp.phar".into());
    Ok(vec![r])
}

/// Interroge toutes les sources (en parallèle).
pub async fn fetch_all(client: &reqwest::Client) -> Trouvailles {
    let (php, ng, ap, ma, co, ad, wp) = tokio::join!(
        php_remote::fetch(client),
        nginx(client),
        apache(client),
        mariadb(client),
        composer(client),
        adminer(client),
        wp_cli(client)
    );
    let mut t = Trouvailles::default();
    for (id, res) in [("php", php), ("nginx", ng), ("apache", ap), ("mariadb", ma), ("composer", co), ("adminer", ad), ("wp-cli", wp)] {
        match res {
            Ok(v) => t.releases.extend(v.into_iter().map(|r| (id.to_string(), r))),
            Err(e) => {
                tracing::warn!(paquet = id, erreur = %e, "recherche de mises à jour impossible");
                t.erreurs.push((id.to_string(), e.to_string()));
            }
        }
    }
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_nginx() {
        let html = r#"<a href="/download/nginx-1.31.9.zip">nginx/Windows-1.31.9</a> <a href="/download/nginx-1.30.7.zip">x</a> <a href="/download/nginx-1.28.2.zip">x</a>"#;
        assert_eq!(parse_nginx(html), ["1.30.7", "1.28.2"], "versions stables (mineure paire) seulement");
    }

    #[test]
    fn page_apache() {
        let html = r#"<a href="/download/VS18/binaries/httpd-2.4.72-261201-Win64-VS18.zip">x</a><a href="/download/VS18/binaries/httpd-2.4.72-261201-Win64-VS18.zip.asc">"#;
        let (v, url, vs) = parse_apache(html).unwrap();
        assert_eq!((v.as_str(), vs.as_str()), ("2.4.72", "VS18"));
        assert_eq!(url, "https://www.apachelounge.com/download/VS18/binaries/httpd-2.4.72-261201-Win64-VS18.zip");
        let txt = "SHA1-Checksum for: x:\nAAAA\n\nSHA256-Checksum for: httpd.zip:\n44961A929DFF16B767B3ADBCEA55760A942A00239B2891DC55806BE34029E1FC\n";
        assert_eq!(parse_sha_apache(txt).unwrap(), "44961a929dff16b767b3adbcea55760a942a00239b2891dc55806be34029e1fc");
    }
}
