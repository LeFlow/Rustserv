//! Moteur de gabarits (minijinja, syntaxe Jinja/Twig).
//!
//! Les gabarits par défaut sont embarqués dans l'exécutable. Un utilisateur
//! avancé peut les surcharger en plaçant un fichier du même nom dans
//! `etc/templates/` (ex. `etc/templates/nginx.conf.j2`).

use crate::error::{Error, Result};
use crate::paths::Paths;
use serde::Serialize;

pub const NGINX_CONF: &str = "nginx.conf.j2";
pub const HTTPD_CONF: &str = "httpd.conf.j2";
pub const MY_INI: &str = "my.ini.j2";

fn embedded(name: &str) -> Option<&'static str> {
    match name {
        NGINX_CONF => Some(include_str!("nginx.conf.j2")),
        HTTPD_CONF => Some(include_str!("httpd.conf.j2")),
        MY_INI => Some(include_str!("my.ini.j2")),
        _ => None,
    }
}

/// Source du gabarit : surcharge utilisateur si présente, sinon version intégrée.
pub fn source(paths: &Paths, name: &str) -> Result<String> {
    let custom = paths.etc().join("templates").join(name);
    if custom.is_file() {
        return std::fs::read_to_string(&custom).map_err(|e| Error::fichier(&custom, e));
    }
    embedded(name).map(str::to_string).ok_or_else(|| Error::Gabarit { nom: name.into(), detail: "gabarit inconnu".into() })
}

/// Rend un gabarit avec le contexte donné.
pub fn render(paths: &Paths, name: &str, ctx: impl Serialize) -> Result<String> {
    let src = source(paths, name)?;
    render_str(name, &src, ctx)
}

pub fn render_str(name: &str, src: &str, ctx: impl Serialize) -> Result<String> {
    let err = |e: minijinja::Error| Error::Gabarit { nom: name.into(), detail: format!("{e:#}") };
    let mut env = minijinja::Environment::new();
    // Une variable absente est une erreur (évite une config silencieusement fausse).
    env.set_undefined_behavior(minijinja::UndefinedBehavior::Strict);
    let syntax = minijinja::syntax::SyntaxConfig::builder().keep_trailing_newline(true).build().map_err(err)?;
    env.set_syntax(syntax);
    env.add_template(name, src).map_err(err)?;
    env.get_template(name).map_err(err)?.render(minijinja::value::Serde(ctx)).map_err(err)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn variable_manquante_refusee() {
        let e = render_str("t", "listen {{ port }};", json!({})).unwrap_err();
        assert_eq!(e.code(), "config.gabarit");
    }

    #[test]
    fn surcharge_utilisateur() {
        let t = tempfile::tempdir().unwrap();
        let p = Paths::new(t.path());
        std::fs::create_dir_all(p.etc().join("templates")).unwrap();
        std::fs::write(p.etc().join("templates").join(NGINX_CONF), "perso {{ x }}").unwrap();
        assert_eq!(render(&p, NGINX_CONF, json!({ "x": 1 })).unwrap(), "perso 1");
        assert!(source(&p, HTTPD_CONF).unwrap().contains("ServerRoot"));
    }
}
