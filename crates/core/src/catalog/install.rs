//! Installation / désinstallation des paquets du catalogue.
//!
//! Déroulé d'une installation :
//! 1. téléchargement dans `tmp/downloads/<fichier>` (+ vérif. SHA256) ;
//! 2. extraction dans `tmp/extract/<paquet>-<version>` ;
//! 3. **déplacement atomique** du contenu vers `bin/<paquet>/<paquet>-<version>` ;
//! 4. écriture d'un marqueur `.rustserv-install.json` dans ce dossier.
//!
//! En cas d'échec à n'importe quelle étape, rien n'apparaît dans `bin/` :
//! on ne laisse jamais une installation à moitié faite.

use super::download::download_verified;
use super::extract::{content_root, extract_zip};
use super::{version, Release};
use crate::error::{Error, Result};
use crate::paths::Paths;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

/// Nom du fichier marqueur écrit dans chaque dossier installé.
pub const MARKER: &str = ".rustserv-install.json";

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum InstallPhase {
    Telechargement,
    Extraction,
    Termine,
}

/// Événement de progression envoyé à l'interface.
#[derive(Debug, Clone, Serialize)]
pub struct InstallProgress {
    pub package: String,
    pub version: String,
    pub phase: InstallPhase,
    pub done: u64,
    pub total: Option<u64>,
}

/// Paquet présent dans `bin/`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InstalledPackage {
    pub package: String,
    pub version: String,
    pub sha256: String,
    /// Horodatage Unix (secondes) de l'installation.
    pub installed_at: u64,
    /// Chemin du dossier (non stocké dans le marqueur, recalculé à la lecture).
    #[serde(skip_deserializing, default)]
    pub path: PathBuf,
}

pub struct Installer {
    paths: Paths,
    client: reqwest::Client,
}

impl Installer {
    pub fn new(paths: Paths) -> Result<Self> {
        let client = reqwest::Client::builder()
            .user_agent(concat!("Rustserv/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(std::time::Duration::from_secs(15))
            .build()
            .map_err(|e| Error::Interne(e.to_string()))?;
        Ok(Self { paths, client })
    }

    /// Variante pour les tests : client HTTP fourni.
    #[cfg(test)]
    fn with_client(paths: Paths, client: reqwest::Client) -> Self {
        Self { paths, client }
    }

    /// Dossier d'installation : `bin/<paquet>/<paquet>-<version>`.
    pub fn install_dir(&self, package: &str, version: &str) -> PathBuf {
        self.paths.bin().join(package).join(format!("{package}-{version}"))
    }

    pub fn is_installed(&self, package: &str, version: &str) -> bool {
        self.install_dir(package, version).join(MARKER).is_file()
    }

    /// Télécharge, vérifie, extrait et installe une version.
    pub async fn install(
        &self,
        package: &str,
        release: &Release,
        cancel: &AtomicBool,
        on_progress: &(dyn Fn(InstallProgress) + Send + Sync),
    ) -> Result<InstalledPackage> {
        let dest = self.install_dir(package, &release.version);
        if dest.exists() {
            return Err(Error::DejaInstalle { id: package.into(), version: release.version.clone() });
        }
        check_vcruntime(release)?;
        let emit = |phase, done, total| {
            on_progress(InstallProgress { package: package.into(), version: release.version.clone(), phase, done, total })
        };

        // 1. Téléchargement
        let dl_dir = self.paths.tmp().join("downloads");
        tokio::fs::create_dir_all(&dl_dir).await.map_err(|e| Error::fichier(&dl_dir, e))?;
        let archive = dl_dir.join(release.file_name());
        let urls = release.urls();
        download_verified(&self.client, &urls, &archive, &release.sha256, cancel, &|d, t| {
            emit(InstallPhase::Telechargement, d, t)
        })
        .await?;

        // 2. Extraction (opération bloquante → thread dédié)
        let work = self.paths.tmp().join("extract").join(format!("{package}-{}", release.version));
        let _menage = Menage(work.clone());
        if work.exists() {
            let _ = std::fs::remove_dir_all(&work);
        }
        let (archive2, work2, subdir) = (archive.clone(), work.clone(), release.subdir.clone());
        let label = release.file_name();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<(u64, u64)>();
        let tache = tokio::task::spawn_blocking(move || -> Result<PathBuf> {
            extract_zip(&archive2, &work2, |d, t| {
                let _ = tx.send((d, t));
            })?;
            content_root(&work2, subdir.as_deref(), &label)
        });
        while let Some((d, t)) = rx.recv().await {
            emit(InstallPhase::Extraction, d, Some(t));
        }
        let racine = tache.await.map_err(|e| Error::Interne(e.to_string()))??;

        // 3. Déplacement atomique vers bin/
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|e| Error::fichier(parent, e))?;
        }
        std::fs::rename(&racine, &dest).map_err(|e| Error::fichier(&dest, e))?;

        // 4. Marqueur
        let installe = InstalledPackage {
            package: package.into(),
            version: release.version.clone(),
            sha256: release.sha256.to_ascii_lowercase(),
            installed_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            path: dest.clone(),
        };
        let marker = dest.join(MARKER);
        let json = serde_json::to_string_pretty(&installe).map_err(|e| Error::Interne(e.to_string()))?;
        std::fs::write(&marker, json).map_err(|e| Error::fichier(&marker, e))?;

        // L'archive n'est plus utile : on libère la place.
        let _ = std::fs::remove_file(&archive);
        emit(InstallPhase::Termine, 1, Some(1));
        tracing::info!(package, version = %release.version, dossier = %dest.display(), "installé");
        Ok(installe)
    }

    /// Liste des paquets installés (dossiers de `bin/` contenant un marqueur),
    /// triés par paquet puis version décroissante.
    pub fn installed(&self) -> Result<Vec<InstalledPackage>> {
        let mut out = Vec::new();
        let bin = self.paths.bin();
        let Ok(pkgs) = std::fs::read_dir(&bin) else { return Ok(out) };
        for pkg in pkgs.flatten().filter(|e| e.path().is_dir()) {
            let Ok(versions) = std::fs::read_dir(pkg.path()) else { continue };
            for v in versions.flatten() {
                if let Some(p) = read_marker(&v.path()) {
                    out.push(p);
                }
            }
        }
        out.sort_by(|a, b| a.package.cmp(&b.package).then_with(|| version::cmp(&b.version, &a.version)));
        Ok(out)
    }

    /// Supprime une version installée.
    pub fn uninstall(&self, package: &str, version: &str) -> Result<()> {
        let dir = self.install_dir(package, version);
        if !dir.join(MARKER).is_file() {
            return Err(Error::NonInstalle { id: package.into(), version: version.into() });
        }
        std::fs::remove_dir_all(&dir).map_err(|e| Error::fichier(&dir, e))?;
        tracing::info!(package, version, "désinstallé");
        Ok(())
    }
}

/// Refuse l'installation si le runtime Visual C++ est absent ou trop ancien :
/// sinon le binaire s'installerait mais refuserait de démarrer (DLL manquante).
pub fn check_vcruntime(release: &Release) -> Result<()> {
    let Some(min) = &release.vcruntime_min else { return Ok(()) };
    let installe = crate::platform::vcruntime_version();
    match &installe {
        Some(v) if version::cmp(v, min) != std::cmp::Ordering::Less => Ok(()),
        _ => Err(Error::VcRuntime { requis: min.clone(), installe }),
    }
}

fn read_marker(dir: &Path) -> Option<InstalledPackage> {
    let txt = std::fs::read_to_string(dir.join(MARKER)).ok()?;
    let mut p: InstalledPackage = serde_json::from_str(&txt).ok()?;
    p.path = dir.to_path_buf();
    Some(p)
}

/// Supprime le dossier de travail à la fin de la portée, même en cas d'erreur.
struct Menage(PathBuf);
impl Drop for Menage {
    fn drop(&mut self) {
        if self.0.exists() {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::extract::tests::make_zip;
    use sha2::{Digest, Sha256};
    use std::sync::Mutex;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// Sert un fichier unique sur http://127.0.0.1:<port>/f.zip
    async fn serveur(contenu: Vec<u8>) -> String {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut s, _)) = l.accept().await {
                let c = contenu.clone();
                tokio::spawn(async move {
                    let mut buf = [0u8; 2048];
                    let _ = s.read(&mut buf).await;
                    let mut r = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", c.len()).into_bytes();
                    r.extend(c);
                    let _ = s.write_all(&r).await;
                });
            }
        });
        format!("http://{addr}/f.zip")
    }

    fn release(url: &str, sha: &str, subdir: Option<&str>) -> Release {
        Release {
            version: "1.2.3".into(),
            branch: None,
            build: None,
            url: url.into(),
            mirrors: vec![],
            sha256: sha.into(),
            subdir: subdir.map(Into::into),
            vcruntime_min: None,
        }
    }

    #[tokio::test]
    async fn installe_liste_desinstalle() {
        let t = tempfile::tempdir().unwrap();
        let paths = Paths::new(t.path());
        paths.ensure_layout().unwrap();

        let zip_path = t.path().join("src.zip");
        make_zip(&zip_path, &[("nginx-1.2.3/nginx.exe", "MZ"), ("nginx-1.2.3/conf/mime.types", "x")]);
        let bytes = std::fs::read(&zip_path).unwrap();
        let sha = hex::encode(Sha256::digest(&bytes));
        let url = serveur(bytes).await;

        let inst = Installer::with_client(paths.clone(), reqwest::Client::builder().no_proxy().build().unwrap());
        let phases = Mutex::new(Vec::new());
        let r = release(&url, &sha, None);
        let p = inst
            .install("nginx", &r, &AtomicBool::new(false), &|p| phases.lock().unwrap().push(p.phase))
            .await
            .unwrap();

        assert!(p.path.join("nginx.exe").is_file(), "dossier racine de l'archive non retiré");
        assert_eq!(p.path, paths.bin().join("nginx").join("nginx-1.2.3"));
        assert_eq!(*phases.lock().unwrap().last().unwrap(), InstallPhase::Termine);
        assert!(!paths.tmp().join("extract").join("nginx-1.2.3").exists(), "dossier temporaire non nettoyé");

        // Seconde installation refusée.
        let err = inst.install("nginx", &r, &AtomicBool::new(false), &|_| {}).await.unwrap_err();
        assert_eq!(err.code(), "installation.deja_installe");

        let liste = inst.installed().unwrap();
        assert_eq!(liste.len(), 1);
        assert_eq!(liste[0].version, "1.2.3");

        inst.uninstall("nginx", "1.2.3").unwrap();
        assert!(inst.installed().unwrap().is_empty());
        assert_eq!(inst.uninstall("nginx", "1.2.3").unwrap_err().code(), "installation.non_installe");
    }

    #[tokio::test]
    async fn echec_ne_laisse_rien_dans_bin() {
        let t = tempfile::tempdir().unwrap();
        let paths = Paths::new(t.path());
        paths.ensure_layout().unwrap();
        let zip_path = t.path().join("src.zip");
        make_zip(&zip_path, &[("ReadMe.txt", "x")]);
        let bytes = std::fs::read(&zip_path).unwrap();
        let sha = hex::encode(Sha256::digest(&bytes));
        let url = serveur(bytes).await;

        let inst = Installer::with_client(paths.clone(), reqwest::Client::builder().no_proxy().build().unwrap());
        // Sous-dossier « Apache24 » absent de l'archive → erreur.
        let err = inst
            .install("apache", &release(&url, &sha, Some("Apache24")), &AtomicBool::new(false), &|_| {})
            .await
            .unwrap_err();
        assert_eq!(err.code(), "installation.archive");
        assert!(!inst.install_dir("apache", "1.2.3").exists());
        assert!(inst.installed().unwrap().is_empty());
    }
}
