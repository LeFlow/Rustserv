//! Terminaux intégrés (pseudo-console Windows ConPTY via `portable-pty`).
//!
//! Chaque terminal s'ouvre dans le dossier d'un site, avec un environnement prêt :
//! - `PATH` commençant par le PHP **du site**, puis Composer, le client de la base du site
//!   (`mariadb`, `mysql` ou `psql`, avec `MYSQL_TCP_PORT` / `PGPORT`) ;
//! - `PHPRC` pointant vers le php.ini de Rustserv pour cette version (mêmes
//!   extensions en ligne de commande que dans le navigateur).

use crate::catalog::{version, InstalledPackage};
use crate::error::{Error, Result};
use crate::php::php_ini_path;
use crate::services::Stack;
use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use serde::Serialize;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

/// Environnement préparé pour un terminal.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct TerminalEnv {
    pub cwd: PathBuf,
    /// Version de PHP utilisée (si PHP est installé).
    pub php: Option<String>,
    /// Dossiers ajoutés en tête du PATH, dans l'ordre.
    pub path_prefix: Vec<PathBuf>,
    pub vars: Vec<(String, String)>,
}

impl Stack<'_> {
    /// Environnement d'un terminal pour un site (ou pour www/ si `site` est vide).
    pub fn terminal_env(&self, site: Option<&str>) -> Result<TerminalEnv> {
        let sites = crate::sites::discover(self.paths, &self.settings.tld)?;
        let site = site.and_then(|n| sites.into_iter().find(|s| s.name == n));
        let php: Option<&InstalledPackage> = match &site {
            Some(s) => self.site_php(&s.name),
            None => self.default_php(),
        };
        let mut prefix = Vec::new();
        let mut vars = Vec::new();
        if let Some(p) = php {
            prefix.push(p.path.clone());
            let ini = php_ini_path(self.paths, &p.version);
            if let Some(dir) = ini.parent() {
                vars.push(("PHPRC".into(), dir.display().to_string()));
            }
        }
        // Outils facultatifs : la version la plus récente installée.
        // Client de la base du site (MariaDB, MySQL ou PostgreSQL), sinon de la base par défaut.
        let base = match &site {
            Some(s) => self.site_db(&s.name).ok(),
            None => self.default_site_db().ok(),
        };
        if let Some(m) = base {
            prefix.push(m.path.join("bin"));
            let port = self.db_port(m).to_string();
            if Stack::moteur_de(m).famille_mysql() {
                vars.push(("MYSQL_TCP_PORT".into(), port));
            } else {
                // psql, pg_dump… se connectent directement à la bonne instance.
                vars.extend([("PGHOST".into(), "127.0.0.1".into()), ("PGPORT".into(), port), ("PGUSER".into(), "postgres".into())]);
            }
        }
        for (pkg, sous) in [("composer", ""), ("nodejs", "")] {
            if let Some(p) = self.installed.iter().filter(|p| p.package == pkg).max_by(|a, b| version::cmp(&a.version, &b.version)) {
                prefix.push(if sous.is_empty() { p.path.clone() } else { p.path.join(sous) });
            }
        }
        let path = std::env::join_paths(prefix.iter().cloned().chain(std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())))
            .map_err(|e| Error::Interne(e.to_string()))?;
        vars.push(("PATH".into(), path.to_string_lossy().into_owned()));
        vars.push(("RUSTSERV_ROOT".into(), self.paths.root().display().to_string()));
        if let Some(s) = &site {
            vars.push(("RUSTSERV_SITE".into(), s.host.clone()));
        }
        Ok(TerminalEnv {
            cwd: site.map(|s| s.dir).unwrap_or_else(|| self.paths.www()),
            php: php.map(|p| p.version.clone()),
            path_prefix: prefix,
            vars,
        })
    }
}

/// Programme de shell : PowerShell 7 s'il est installé, sinon Windows PowerShell.
fn shell() -> CommandBuilder {
    let pwsh = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()).map(|d| d.join("pwsh.exe")).find(|p| p.is_file());
    let mut c = CommandBuilder::new(pwsh.map(|p| p.into_os_string()).unwrap_or_else(|| "powershell.exe".into()));
    c.arg("-NoLogo");
    c
}

type OnOutput = Arc<dyn Fn(u32, String) + Send + Sync>;
type OnExit = Arc<dyn Fn(u32) + Send + Sync>;

struct Session {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
}

/// Gestionnaire des terminaux ouverts.
#[derive(Default)]
pub struct Terminals {
    next: AtomicU32,
    sessions: Mutex<HashMap<u32, Session>>,
}

fn pty_err(e: impl std::fmt::Display) -> Error {
    Error::Interne(format!("terminal : {e}"))
}

/// Découpe un flux d'octets en texte UTF-8 sans couper un caractère multi-octets.
/// Les octets incomplets en fin de tampon sont conservés dans `reste`.
pub fn decode_utf8(reste: &mut Vec<u8>, nouveaux: &[u8]) -> String {
    reste.extend_from_slice(nouveaux);
    match std::str::from_utf8(reste) {
        Ok(s) => {
            let out = s.to_string();
            reste.clear();
            out
        }
        Err(e) if e.error_len().is_none() => {
            // Caractère coupé en fin de tampon : on garde la fin pour la prochaine lecture.
            let ok = e.valid_up_to();
            let out = String::from_utf8_lossy(&reste[..ok]).into_owned();
            reste.drain(..ok);
            out
        }
        Err(_) => {
            let out = String::from_utf8_lossy(reste).into_owned();
            reste.clear();
            out
        }
    }
}

impl Terminals {
    /// Ouvre un terminal ; la sortie arrive via `on_output`, la fermeture via `on_exit`.
    pub fn open(&self, env: &TerminalEnv, cols: u16, rows: u16, on_output: OnOutput, on_exit: OnExit) -> Result<u32> {
        let pty = native_pty_system();
        let pair = pty.openpty(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 }).map_err(pty_err)?;
        let mut cmd = shell();
        cmd.cwd(&env.cwd);
        for (k, v) in &env.vars {
            cmd.env(k, v);
        }
        let child = pair.slave.spawn_command(cmd).map_err(pty_err)?;
        drop(pair.slave);
        let mut reader = pair.master.try_clone_reader().map_err(pty_err)?;
        let writer = pair.master.take_writer().map_err(pty_err)?;
        let id = self.next.fetch_add(1, Ordering::Relaxed) + 1;

        std::thread::Builder::new()
            .name(format!("terminal-{id}"))
            .spawn(move || {
                let mut buf = [0u8; 8192];
                let mut reste = Vec::new();
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            let txt = decode_utf8(&mut reste, &buf[..n]);
                            if !txt.is_empty() {
                                on_output(id, txt);
                            }
                        }
                    }
                }
                on_exit(id);
            })
            .map_err(pty_err)?;

        self.sessions.lock().unwrap().insert(id, Session { master: pair.master, writer, child });
        tracing::info!(id, cwd = %env.cwd.display(), php = ?env.php, "terminal ouvert");
        Ok(id)
    }

    pub fn write(&self, id: u32, data: &str) -> Result<()> {
        let mut map = self.sessions.lock().unwrap();
        let s = map.get_mut(&id).ok_or_else(|| Error::Interne(format!("terminal {id} fermé")))?;
        s.writer.write_all(data.as_bytes()).map_err(pty_err)?;
        s.writer.flush().map_err(pty_err)
    }

    pub fn resize(&self, id: u32, cols: u16, rows: u16) -> Result<()> {
        let map = self.sessions.lock().unwrap();
        if let Some(s) = map.get(&id) {
            s.master.resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 }).map_err(pty_err)?;
        }
        Ok(())
    }

    /// Ferme un terminal (tue le shell et ses sous-processus).
    pub fn close(&self, id: u32) {
        if let Some(mut s) = self.sessions.lock().unwrap().remove(&id) {
            let _ = s.child.kill();
        }
    }

    pub fn close_all(&self) {
        let ids: Vec<u32> = self.sessions.lock().unwrap().keys().copied().collect();
        for id in ids {
            self.close(id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::Paths;

    #[test]
    fn utf8_coupe_entre_deux_lectures() {
        let mut reste = Vec::new();
        let e = "é".as_bytes(); // 2 octets
        assert_eq!(decode_utf8(&mut reste, &[b'a', e[0]]), "a");
        assert_eq!(reste, [e[0]]);
        assert_eq!(decode_utf8(&mut reste, &[e[1], b'b']), "éb");
        assert!(reste.is_empty());
        assert_eq!(decode_utf8(&mut reste, &[0xff, b'c']), "\u{fffd}c");
    }

    #[test]
    fn environnement_du_terminal() {
        let t = tempfile::tempdir().unwrap();
        let paths = Paths::new(t.path());
        paths.ensure_layout().unwrap();
        std::fs::create_dir_all(paths.www().join("ancien")).unwrap();
        let pkg = |n: &str, v: &str| InstalledPackage {
            package: n.into(),
            version: v.into(),
            sha256: String::new(),
            installed_at: 0,
            path: paths.bin().join(n).join(format!("{n}-{v}")),
        };
        let installed = vec![pkg("php", "8.3.37"), pkg("php", "7.4.33"), pkg("mariadb", "12.3.4")];
        let mut settings = crate::Settings::default();
        settings.site_php.insert("ancien".into(), "7.4".into());
        let st = Stack::new(&paths, &settings, &installed);

        let env = st.terminal_env(Some("ancien")).unwrap();
        assert_eq!(env.cwd, paths.www().join("ancien"));
        assert_eq!(env.php.as_deref(), Some("7.4.33"));
        assert!(env.path_prefix[0].ends_with("php-7.4.33"), "PHP du site en premier");
        assert!(env.path_prefix[1].ends_with("bin"), "client MariaDB");
        let path = &env.vars.iter().find(|(k, _)| k == "PATH").unwrap().1;
        assert!(path.starts_with(&env.path_prefix[0].display().to_string()));
        assert!(env.vars.iter().any(|(k, v)| k == "PHPRC" && v.ends_with("7.4.33")));

        let defaut = st.terminal_env(None).unwrap();
        assert_eq!(defaut.cwd, paths.www());
        assert_eq!(defaut.php.as_deref(), Some("8.3.37"));
    }
}
