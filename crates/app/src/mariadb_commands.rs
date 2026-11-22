//! Commandes MariaDB multi-versions : versions disponibles et MariaDB d'un site.

use crate::state::AppState;
use rustserv_core::catalog::version;
use rustserv_core::migration::run;
use rustserv_core::services::Stack;
use rustserv_core::{Error, Result};
use serde::Serialize;
use tauri::State;

#[derive(Serialize)]
pub struct MariadbVersion {
    branch: String,
    version: String,
    port: u16,
    defaut: bool,
}

/// Branches de MariaDB installées (la plus récente de chaque branche).
#[tauri::command]
pub fn mariadb_versions(state: State<'_, AppState>) -> Result<Vec<MariadbVersion>> {
    let settings = state.settings();
    let installed = state.installer.installed()?;
    let st = Stack::new(&state.paths, &settings, &installed);
    let defaut = st.default_mariadb().ok().map(|p| version::branch(&p.version));
    let mut branches: Vec<String> = installed.iter().filter(|p| p.package == "mariadb").map(|p| version::branch(&p.version)).collect();
    branches.sort_by(|a, b| version::cmp(b, a));
    branches.dedup();
    Ok(branches
        .into_iter()
        .filter_map(|b| st.mariadb_for_branch(&b).map(|p| (b, p)))
        .map(|(b, p)| MariadbVersion { defaut: defaut.as_deref() == Some(b.as_str()), port: st.mariadb_port(p), version: p.version.clone(), branch: b })
        .collect())
}

#[derive(Serialize)]
pub struct Deplacement {
    base: Option<String>,
    de: String,
    vers: String,
    port: u16,
}

/// Choisit la MariaDB d'un site (None = par défaut). Si la version change, la base du
/// site est **copiée** vers la nouvelle instance (l'ancienne est conservée) et la
/// configuration du site (wp-config.php / .env) pointe vers le nouveau port.
#[tauri::command]
pub async fn site_set_mariadb(state: State<'_, AppState>, site: String, branch: Option<String>) -> Result<Deplacement> {
    let ancien = state.settings();
    let mut nouveau = ancien.clone();
    match &branch {
        Some(b) => {
            nouveau.site_mariadb.insert(site.clone(), b.clone());
        }
        None => {
            nouveau.site_mariadb.remove(&site);
        }
    }
    let installed = state.installer.installed()?;
    let (src, dst, port_src_attendu, port_dst) = {
        let a = Stack::new(&state.paths, &ancien, &installed);
        let n = Stack::new(&state.paths, &nouveau, &installed);
        let src = a.site_mariadb(&site)?.clone();
        let dst = n.site_mariadb(&site)?.clone();
        let (ps, pd) = (a.mariadb_port(&src), n.mariadb_port(&dst));
        (src, dst, ps, pd)
    };
    let dossier = state.paths.www().join(&site);
    let mut base = None;
    if version::branch(&src.version) != version::branch(&dst.version) && dossier.is_dir() {
        let port_src = state.orchestrator.ensure_mariadb(&ancien, &installed, &src).await?;
        debug_assert_eq!(port_src, port_src_attendu);
        state.orchestrator.ensure_mariadb(&nouveau, &installed, &dst).await?;
        base = run::deplacer_base(&state.paths, &dossier, &src, port_src, &dst, port_dst).await?;
    } else if dossier.is_dir() {
        run::configurer_port(&dossier, port_dst)?;
    }
    nouveau.save(&state.paths)?;
    *state.settings.write().map_err(|e| Error::Interne(e.to_string()))? = nouveau.clone();
    state.orchestrator.apply(&nouveau, &installed).await?;
    Ok(Deplacement { base, de: src.version, vers: dst.version, port: port_dst })
}
