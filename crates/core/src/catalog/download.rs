//! Téléchargement en streaming avec calcul du SHA256 à la volée.
//!
//! - Le fichier est écrit dans `<dest>.part`, puis renommé seulement si
//!   l'empreinte correspond : un fichier `dest` présent est donc toujours sain.
//! - Les URLs sont essayées dans l'ordre (principale puis miroirs).
//! - Un fichier déjà présent et valide n'est pas re-téléchargé (cache).

use crate::error::{Error, Result};
use futures_util::StreamExt;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::io::AsyncWriteExt;

/// Intervalle minimal (en octets) entre deux notifications de progression.
const PAS_PROGRESSION: u64 = 256 * 1024;

/// Callback de progression : (octets reçus, taille totale si connue).
pub type OnProgress<'a> = &'a (dyn Fn(u64, Option<u64>) + Send + Sync);

/// Télécharge la première URL qui fonctionne vers `dest` et vérifie le SHA256.
pub async fn download_verified(
    client: &reqwest::Client,
    urls: &[&str],
    dest: &Path,
    sha256: &str,
    cancel: &AtomicBool,
    on_progress: OnProgress<'_>,
) -> Result<()> {
    let attendu = sha256.to_ascii_lowercase();

    // Cache : fichier déjà téléchargé et intègre.
    if dest.exists() {
        if sha256_file(dest).await? == attendu {
            tracing::info!(fichier = %dest.display(), "archive déjà présente et valide, pas de téléchargement");
            return Ok(());
        }
        let _ = tokio::fs::remove_file(dest).await;
    }

    let mut derniere_erreur = Error::Interne("aucune URL fournie".into());
    for url in urls {
        match download_one(client, url, dest, Some(&attendu), cancel, on_progress).await {
            Ok(_) => return Ok(()),
            Err(Error::Annule) => return Err(Error::Annule),
            Err(e) => {
                tracing::warn!(%url, erreur = %e, "échec, essai de l'URL suivante");
                derniere_erreur = e;
            }
        }
    }
    Err(derniere_erreur)
}

/// Télécharge une URL **sans empreinte connue à l'avance** (extensions PECL : php.net
/// ne publie pas de SHA256). Seul HTTPS est accepté ; l'empreinte calculée est
/// renvoyée pour être enregistrée (traçabilité).
pub async fn download_unverified(
    client: &reqwest::Client,
    url: &str,
    dest: &Path,
    cancel: &AtomicBool,
    on_progress: OnProgress<'_>,
) -> Result<String> {
    if !url.starts_with("https://") {
        return Err(Error::TelechargementEchoue { url: url.into(), detail: "HTTPS obligatoire".into() });
    }
    download_one(client, url, dest, None, cancel, on_progress).await
}

/// Télécharge `url` vers `dest` ; vérifie l'empreinte si `attendu` est fourni.
/// Renvoie l'empreinte SHA256 calculée.
async fn download_one(
    client: &reqwest::Client,
    url: &str,
    dest: &Path,
    attendu: Option<&str>,
    cancel: &AtomicBool,
    on_progress: OnProgress<'_>,
) -> Result<String> {
    let reseau = |e: reqwest::Error| Error::TelechargementEchoue { url: url.into(), detail: e.to_string() };

    tracing::info!(%url, "téléchargement");
    let resp = client.get(url).send().await.map_err(reseau)?;
    let status = resp.status();
    if !status.is_success() {
        return Err(Error::HttpStatut { url: url.into(), status: status.as_u16() });
    }
    let total = resp.content_length();

    let part = part_path(dest);
    let mut file = tokio::fs::File::create(&part).await.map_err(|e| Error::fichier(&part, e))?;
    let mut hasher = Sha256::new();
    let mut recu: u64 = 0;
    let mut dernier_signal: u64 = 0;
    on_progress(0, total);

    let mut stream = resp.bytes_stream();
    let resultat: Result<()> = async {
        while let Some(chunk) = stream.next().await {
            if cancel.load(Ordering::Relaxed) {
                return Err(Error::Annule);
            }
            let chunk = chunk.map_err(reseau)?;
            hasher.update(&chunk);
            file.write_all(&chunk).await.map_err(|e| Error::fichier(&part, e))?;
            recu += chunk.len() as u64;
            if recu - dernier_signal >= PAS_PROGRESSION {
                dernier_signal = recu;
                on_progress(recu, total);
            }
        }
        file.flush().await.map_err(|e| Error::fichier(&part, e))?;
        Ok(())
    }
    .await;
    drop(file);

    if let Err(e) = resultat {
        let _ = tokio::fs::remove_file(&part).await;
        return Err(e);
    }
    on_progress(recu, total);

    let obtenu = hex::encode(hasher.finalize());
    if let Some(attendu) = attendu {
        if obtenu != attendu {
            let _ = tokio::fs::remove_file(&part).await;
            return Err(Error::HashInvalide { fichier: file_label(dest), attendu: attendu.into(), obtenu });
        }
    }
    let _ = tokio::fs::remove_file(dest).await;
    tokio::fs::rename(&part, dest).await.map_err(|e| Error::fichier(dest, e))?;
    Ok(obtenu)
}

/// SHA256 d'un fichier existant (lecture dans un thread dédié, sans bloquer l'async).
pub async fn sha256_file(path: &Path) -> Result<String> {
    let p = path.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let mut f = std::fs::File::open(&p).map_err(|e| Error::fichier(&p, e))?;
        let mut h = Sha256::new();
        let mut buf = vec![0u8; 1024 * 1024];
        loop {
            let n = std::io::Read::read(&mut f, &mut buf).map_err(|e| Error::fichier(&p, e))?;
            if n == 0 {
                break;
            }
            h.update(&buf[..n]);
        }
        Ok(hex::encode(h.finalize()))
    })
    .await
    .map_err(|e| Error::Interne(e.to_string()))?
}

fn part_path(dest: &Path) -> PathBuf {
    let mut s = dest.as_os_str().to_owned();
    s.push(".part");
    PathBuf::from(s)
}

fn file_label(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use tokio::io::AsyncReadExt;
    use tokio::net::TcpListener;

    const CORPS: &[u8] = b"contenu de test pour rustserv";

    /// Mini serveur HTTP local : `/ok` renvoie CORPS, tout le reste 404.
    async fn serveur() -> String {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((mut s, _)) = l.accept().await else { break };
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 4096];
                    let n = s.read(&mut buf).await.unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]);
                    let resp = if req.starts_with("GET /ok ") {
                        let mut r = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", CORPS.len()).into_bytes();
                        r.extend_from_slice(CORPS);
                        r
                    } else {
                        b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec()
                    };
                    let _ = s.write_all(&resp).await;
                });
            }
        });
        format!("http://{addr}")
    }

    fn client() -> reqwest::Client {
        reqwest::Client::builder().no_proxy().build().unwrap()
    }

    fn sha_corps() -> String {
        hex::encode(Sha256::digest(CORPS))
    }

    #[tokio::test]
    async fn telecharge_et_verifie() {
        let base = serveur().await;
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("a.zip");
        let vu = Mutex::new(Vec::new());
        let cb = |r: u64, t: Option<u64>| vu.lock().unwrap().push((r, t));
        let url = format!("{base}/ok");
        download_verified(&client(), &[&url], &dest, &sha_corps(), &AtomicBool::new(false), &cb).await.unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), CORPS);
        assert!(!part_path(&dest).exists());
        let dernier = *vu.lock().unwrap().last().unwrap();
        assert_eq!(dernier, (CORPS.len() as u64, Some(CORPS.len() as u64)));
    }

    #[tokio::test]
    async fn hash_faux_supprime_le_fichier() {
        let base = serveur().await;
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("a.zip");
        let url = format!("{base}/ok");
        let faux = "0".repeat(64);
        let err = download_verified(&client(), &[&url], &dest, &faux, &AtomicBool::new(false), &|_, _| {}).await.unwrap_err();
        assert_eq!(err.code(), "telechargement.hash");
        assert!(!dest.exists() && !part_path(&dest).exists());
    }

    #[tokio::test]
    async fn bascule_sur_le_miroir() {
        let base = serveur().await;
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("a.zip");
        let (u404, uok) = (format!("{base}/absent"), format!("{base}/ok"));
        download_verified(&client(), &[&u404, &uok], &dest, &sha_corps(), &AtomicBool::new(false), &|_, _| {}).await.unwrap();
        assert!(dest.exists());
    }

    #[tokio::test]
    async fn erreur_http_si_tout_echoue() {
        let base = serveur().await;
        let tmp = tempfile::tempdir().unwrap();
        let url = format!("{base}/absent");
        let err = download_verified(&client(), &[&url], &tmp.path().join("a.zip"), &sha_corps(), &AtomicBool::new(false), &|_, _| {}).await.unwrap_err();
        assert_eq!(err.code(), "telechargement.http");
        assert_eq!(err.params()["status"], 404);
    }

    #[tokio::test]
    async fn annulation() {
        let base = serveur().await;
        let tmp = tempfile::tempdir().unwrap();
        let url = format!("{base}/ok");
        let err = download_verified(&client(), &[&url], &tmp.path().join("a.zip"), &sha_corps(), &AtomicBool::new(true), &|_, _| {}).await.unwrap_err();
        assert_eq!(err.code(), "annule");
    }

    #[tokio::test]
    async fn reutilise_le_cache() {
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("a.zip");
        std::fs::write(&dest, CORPS).unwrap();
        // URL invalide : si le cache n'était pas utilisé, l'appel échouerait.
        download_verified(&client(), &["http://127.0.0.1:1/x"], &dest, &sha_corps(), &AtomicBool::new(false), &|_, _| {}).await.unwrap();
    }
}
