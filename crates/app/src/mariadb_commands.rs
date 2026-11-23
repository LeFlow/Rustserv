//! Commandes des bases de données (MariaDB, MySQL, PostgreSQL) : versions installées et
//! base de données d'un site.

use crate::state::AppState;
use rustserv_core::db::{ChoixBase, Moteur};
use rustserv_core::migration::run;
use rustserv_core::services::Stack;
use rustserv_core::{Error, Result};
use serde::Serialize;
use tauri::State;

#[derive(Serialize)]
pub struct BaseVersion {
    moteur: Moteur,
    branch: String,
    version: String,
    port: u16,
    defaut: bool,
    /// Valeur à enregistrer pour un site (« mysql:8.4 »).
    choix: String,
}

/// Branches installées de chaque moteur (la plus récente de chaque branche).
#[tauri::command]
pub fn bases_versions(state: State<'_, AppState>) -> Result<Vec<BaseVersion>> {
    let settings = state.settings();
    let installed = state.installer.installed()?;
    let st = Stack::new(&state.paths, &settings, &installed);
    let mut v = Vec::new();
    for m in Moteur::TOUS {
        let defaut = st.default_db(m).ok().map(|p| m.branche(&p.version));
        let mut branches: Vec<String> = installed.iter().filter(|p| p.package == m.paquet()).map(|p| m.branche(&p.version)).collect();
        branches.sort_by(|a, b| rustserv_core::catalog::version::cmp(b, a));
        branches.dedup();
        for b in branches {
            if let Some(p) = st.db_for_branch(m, &b) {
                v.push(BaseVersion {
                    moteur: m,
                    defaut: defaut.as_deref() == Some(b.as_str()),
                    port: st.db_port(p),
                    version: p.version.clone(),
                    choix: ChoixBase { moteur: m, branche: b.clone() }.texte(),
                    branch: b,
                });
            }
        }
    }
    Ok(v)
}

#[derive(Serialize)]
pub struct Deplacement {
    base: Option<String>,
    /// Moteur + version d'origine et de destination (« MariaDB 12.3.4 »).
    de: String,
    vers: String,
    port: u16,
    /// Les données ont été copiées (MariaDB ↔ MySQL ou autre version).
    copiee: bool,
    /// Conversion MariaDB / MySQL ↔ PostgreSQL : tables, lignes, points d'attention.
    rapport: Option<rustserv_core::conversion::RapportConversion>,
}

/// Migre la base d'un site vers une autre instance (« mysql:8.4 », None = base par défaut).
/// - MariaDB ↔ MySQL (ou autre version) : la base est **copiée** ;
/// - vers / depuis PostgreSQL : la base est **convertie**, données comprises ;
/// l'ancienne base est toujours conservée, puis la configuration du site (wp-config.php /
/// .env) pointe vers la nouvelle instance.
#[tauri::command]
pub async fn site_set_base(state: State<'_, AppState>, site: String, choix: Option<String>) -> Result<Deplacement> {
    let ancien = state.settings();
    let mut nouveau = ancien.clone();
    let installed = state.installer.installed()?;
    let mut c = choix.as_deref().and_then(ChoixBase::parse);
    // Choisir explicitement la base par défaut = revenir au défaut (sinon cette branche,
    // devenue « choisie pour un site », ne serait plus la version par défaut).
    if let (Some(ch), Ok(defaut)) = (&c, Stack::new(&state.paths, &ancien, &installed).default_site_db()) {
        let m = Stack::moteur_de(defaut);
        if ch.moteur == m && ch.branche == m.branche(&defaut.version) {
            c = None;
        }
    }
    nouveau.set_choix_base(&site, c.as_ref());
    let (src, dst, port_dst) = {
        let a = Stack::new(&state.paths, &ancien, &installed);
        let n = Stack::new(&state.paths, &nouveau, &installed);
        let src = a.site_db(&site)?.clone();
        let dst = n.site_db(&site)?.clone();
        let pd = n.db_port(&dst);
        (src, dst, pd)
    };
    let (ms, md) = (Stack::moteur_de(&src), Stack::moteur_de(&dst));
    let dossier = state.paths.www().join(&site);
    let mut base = None;
    let mut copiee = false;
    let mut rapport = None;
    let meme_instance = ms == md && ms.branche(&src.version) == md.branche(&dst.version);
    if dossier.is_dir() {
        if meme_instance {
            run::configurer_base(&dossier, md, port_dst)?;
        } else if ms.famille_mysql() && md.famille_mysql() {
            let port_src = state.orchestrator.ensure_db(&ancien, &installed, &src).await?;
            state.orchestrator.ensure_db(&nouveau, &installed, &dst).await?;
            base = run::deplacer_base(&state.paths, &dossier, &src, port_src, &dst, port_dst).await?;
            copiee = base.is_some();
        } else {
            if md == Moteur::Postgresql && dossier.join("wp-config.php").is_file() {
                return Err(Error::WordpressPostgresql { site });
            }
            let port_src = state.orchestrator.ensure_db(&ancien, &installed, &src).await?;
            state.orchestrator.ensure_db(&nouveau, &installed, &dst).await?;
            let (cs, cd) = {
                let st = Stack::new(&state.paths, &nouveau, &installed);
                (st.db_client(&src)?, st.db_client(&dst)?)
            };
            let php = rustserv_core::conversion::php_pour_conversion(&installed)?.clone();
            let (b, r) = run::changer_de_moteur(
                &state.paths,
                &dossier,
                &site,
                &php,
                &run::Instance { pkg: &src, client: cs, port: port_src },
                &run::Instance { pkg: &dst, client: cd, port: port_dst },
            )
            .await?;
            base = b;
            rapport = r;
            if md == Moteur::Postgresql {
                // Laravel / Symfony : extensions PostgreSQL du PHP du site.
                crate::ext_commands::activer_pgsql(&state, &nouveau, &installed, Some(&site)).await;
            }
        }
    }
    nouveau.save(&state.paths)?;
    *state.settings.write().map_err(|e| Error::Interne(e.to_string()))? = nouveau.clone();
    state.orchestrator.apply(&nouveau, &installed).await?;
    Ok(Deplacement {
        base,
        de: format!("{} {}", ms.nom(), src.version),
        vers: format!("{} {}", md.nom(), dst.version),
        port: port_dst,
        copiee,
        rapport,
    })
}
