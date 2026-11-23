//! Reconnaissance du type de site (CMS / framework) et de sa version, d'après ses fichiers.
//!
//! Lecture seule et rapide : quelques fichiers caractéristiques + `composer.lock`.
//! L'ordre compte : les CMS bâtis sur Laravel (Statamic, October) ou Symfony (Drupal,
//! Shopware…) sont testés avant leur framework.

use serde::Serialize;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Framework {
    Wordpress,
    Statamic,
    Octobercms,
    Laravel,
    Craftcms,
    Shopware,
    Drupal,
    Typo3,
    Magento,
    Symfony,
    Prestashop,
    Joomla,
    Codeigniter,
    Cakephp,
    Yii,
    Kirby,
    Grav,
    Moodle,
    Nextcloud,
    Dolibarr,
    Phpbb,
    /// Projet PHP sans framework reconnu.
    Php,
    /// Pages HTML seules.
    Statique,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TypeSite {
    pub framework: Framework,
    pub version: Option<String>,
}

fn lire(dir: &Path, rel: &str) -> Option<String> {
    std::fs::read_to_string(dir.join(rel)).ok()
}

fn existe(dir: &Path, rel: &str) -> bool {
    dir.join(rel).exists()
}

/// Première valeur entre guillemets (simples ou doubles) qui suit `cle` dans `texte`.
fn valeur_apres(texte: &str, cle: &str) -> Option<String> {
    let reste = &texte[texte.find(cle)? + cle.len()..];
    let debut = reste.find(['\'', '"'])?;
    let guillemet = reste[debut..].chars().next()?;
    let reste = &reste[debut + 1..];
    let fin = reste.find(guillemet)?;
    let v = reste[..fin].trim();
    (!v.is_empty() && v.len() < 40).then(|| v.to_string())
}

/// Paquets Composer installés (composer.lock) : nom → version (sans le « v » initial).
fn paquets(dir: &Path) -> Vec<(String, String)> {
    let Some(txt) = lire(dir, "composer.lock") else {
        // Sans composer.lock : les dépendances déclarées (sans version précise).
        return lire(dir, "composer.json")
            .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
            .map(|j| {
                ["require", "require-dev"]
                    .iter()
                    .filter_map(|k| j[k].as_object())
                    .flat_map(|o| o.keys().map(|n| (n.to_lowercase(), String::new())))
                    .collect()
            })
            .unwrap_or_default();
    };
    let Ok(j) = serde_json::from_str::<serde_json::Value>(&txt) else { return vec![] };
    ["packages", "packages-dev"]
        .iter()
        .filter_map(|k| j[k].as_array())
        .flatten()
        .filter_map(|p| {
            let nom = p["name"].as_str()?.to_lowercase();
            let v = p["version"].as_str().unwrap_or("").trim_start_matches('v').to_string();
            Some((nom, v))
        })
        .collect()
}

/// Détecte le type d'un site (dossier du projet). None si rien de reconnaissable.
pub fn detecter(dir: &Path) -> Option<TypeSite> {
    let t = |framework, version: Option<String>| Some(TypeSite { framework, version: version.filter(|v| !v.is_empty()) });

    // WordPress (aussi installé dans un sous-dossier public/ ou web/ : Bedrock).
    for base in ["", "web/wp", "public/wp", "wordpress"] {
        let v = if base.is_empty() { "wp-includes/version.php".to_string() } else { format!("{base}/wp-includes/version.php") };
        if let Some(txt) = lire(dir, &v) {
            return t(Framework::Wordpress, valeur_apres(&txt, "$wp_version"));
        }
    }
    if existe(dir, "wp-config.php") {
        return t(Framework::Wordpress, None);
    }

    let pk = paquets(dir);
    let version_de = |noms: &[&str]| noms.iter().find_map(|n| pk.iter().find(|(p, _)| p == n).map(|(_, v)| v.clone()));
    let regles: &[(Framework, &[&str])] = &[
        (Framework::Statamic, &["statamic/cms"]),
        (Framework::Octobercms, &["october/system", "october/rain"]),
        (Framework::Craftcms, &["craftcms/cms"]),
        (Framework::Shopware, &["shopware/core", "shopware/platform"]),
        (Framework::Drupal, &["drupal/core", "drupal/core-recommended"]),
        (Framework::Typo3, &["typo3/cms-core"]),
        (Framework::Magento, &["magento/framework", "magento/product-community-edition"]),
        (Framework::Laravel, &["laravel/framework"]),
        (Framework::Kirby, &["getkirby/cms"]),
        (Framework::Codeigniter, &["codeigniter4/framework"]),
        (Framework::Cakephp, &["cakephp/cakephp"]),
        (Framework::Yii, &["yiisoft/yii2"]),
        (Framework::Symfony, &["symfony/framework-bundle"]),
    ];
    for (f, noms) in regles {
        if let Some(v) = version_de(noms) {
            return t(*f, Some(v));
        }
    }

    // Sans Composer (archives téléchargées) : fichiers caractéristiques.
    if let Some(txt) = lire(dir, "core/lib/Drupal.php") {
        return t(Framework::Drupal, valeur_apres(&txt, "const VERSION"));
    }
    if existe(dir, "bin/magento") {
        return t(Framework::Magento, None);
    }
    if existe(dir, "classes/PrestaShopAutoload.php") || existe(dir, "config/defines.inc.php") && existe(dir, "classes") {
        let v = lire(dir, "app/AppKernel.php").and_then(|k| valeur_apres(&k, "const VERSION"));
        return t(Framework::Prestashop, v);
    }
    if existe(dir, "libraries/src/Version.php") || existe(dir, "administrator/manifests/files/joomla.xml") {
        let v = lire(dir, "administrator/manifests/files/joomla.xml")
            .and_then(|x| x.split("<version>").nth(1).and_then(|r| r.split('<').next()).map(|s| s.trim().to_string()));
        return t(Framework::Joomla, v);
    }
    if existe(dir, "artisan") {
        return t(Framework::Laravel, None);
    }
    if existe(dir, "system/CodeIgniter.php") || existe(dir, "spark") {
        return t(Framework::Codeigniter, None);
    }
    if existe(dir, "kirby/bootstrap.php") {
        return t(Framework::Kirby, None);
    }
    if let Some(txt) = lire(dir, "system/defines.php").filter(|t| t.contains("GRAV_VERSION")) {
        return t(Framework::Grav, valeur_apres(&txt, "GRAV_VERSION"));
    }
    if existe(dir, "lib/moodlelib.php") {
        return t(Framework::Moodle, lire(dir, "version.php").and_then(|v| valeur_apres(&v, "$release")));
    }
    if existe(dir, "occ") && existe(dir, "lib/versioncheck.php") {
        return t(Framework::Nextcloud, lire(dir, "version.php").and_then(|v| valeur_apres(&v, "$OC_VersionString")));
    }
    for base in ["htdocs/", ""] {
        if let Some(txt) = lire(dir, &format!("{base}filefunc.inc.php")).filter(|t| t.contains("DOL_VERSION")) {
            return t(Framework::Dolibarr, valeur_apres(&txt, "'DOL_VERSION'"));
        }
    }
    if let Some(txt) = lire(dir, "includes/constants.php").filter(|t| t.contains("PHPBB_VERSION")) {
        return t(Framework::Phpbb, valeur_apres(&txt, "'PHPBB_VERSION'"));
    }
    if existe(dir, "bin/console") && existe(dir, "symfony.lock") {
        return t(Framework::Symfony, None);
    }

    // Projet PHP générique ou site statique.
    let fichiers: Vec<String> = std::fs::read_dir(dir)
        .map(|rd| rd.flatten().filter_map(|e| e.file_name().to_str().map(str::to_lowercase)).collect())
        .unwrap_or_default();
    let public: Vec<String> = std::fs::read_dir(dir.join("public"))
        .map(|rd| rd.flatten().filter_map(|e| e.file_name().to_str().map(str::to_lowercase)).collect())
        .unwrap_or_default();
    let tous = || fichiers.iter().chain(public.iter());
    if !pk.is_empty() || tous().any(|f| f.ends_with(".php")) {
        return t(Framework::Php, None);
    }
    if tous().any(|f| f.ends_with(".html") || f.ends_with(".htm")) {
        return t(Framework::Statique, None);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn site(fichiers: &[(&str, &str)]) -> tempfile::TempDir {
        let t = tempfile::tempdir().unwrap();
        for (f, c) in fichiers {
            let p = t.path().join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, c).unwrap();
        }
        t
    }

    fn d(fichiers: &[(&str, &str)]) -> Option<(Framework, Option<String>)> {
        let t = site(fichiers);
        detecter(t.path()).map(|x| (x.framework, x.version))
    }

    #[test]
    fn cms_et_frameworks() {
        assert_eq!(
            d(&[("wp-includes/version.php", "<?php\n$wp_version = '6.7.1';\n$wp_db_version = 58975;"), ("wp-config.php", "")]),
            Some((Framework::Wordpress, Some("6.7.1".into())))
        );
        let lock = |nom: &str, v: &str| format!(r#"{{"packages":[{{"name":"symfony/console","version":"v7.1.0"}},{{"name":"{nom}","version":"{v}"}}]}}"#);
        assert_eq!(d(&[("composer.lock", &lock("laravel/framework", "v11.31.0")), ("artisan", "")]), Some((Framework::Laravel, Some("11.31.0".into()))));
        assert_eq!(d(&[("composer.lock", &lock("statamic/cms", "v5.2.0"))]).unwrap().0, Framework::Statamic, "Statamic avant Laravel");
        assert_eq!(d(&[("composer.lock", &lock("symfony/framework-bundle", "v7.1.6"))]), Some((Framework::Symfony, Some("7.1.6".into()))));
        assert_eq!(d(&[("composer.lock", &lock("drupal/core", "10.3.6"))]).unwrap().0, Framework::Drupal, "Drupal avant Symfony");
        assert_eq!(d(&[("composer.json", r#"{"require":{"laravel/framework":"^11.0"}}"#)]), Some((Framework::Laravel, None)));
        assert_eq!(
            d(&[("administrator/manifests/files/joomla.xml", "<extension><version>5.2.1</version></extension>"), ("libraries/src/Version.php", "")]),
            Some((Framework::Joomla, Some("5.2.1".into())))
        );
        assert_eq!(
            d(&[("classes/PrestaShopAutoload.php", ""), ("app/AppKernel.php", "const VERSION = '8.2.0';")]),
            Some((Framework::Prestashop, Some("8.2.0".into())))
        );
        assert_eq!(d(&[("core/lib/Drupal.php", "const VERSION = '7.101';")]), Some((Framework::Drupal, Some("7.101".into()))));
    }

    #[test]
    fn php_statique_et_vide() {
        assert_eq!(d(&[("index.php", "<?php echo 1;")]), Some((Framework::Php, None)));
        assert_eq!(d(&[("public/index.php", "")]), Some((Framework::Php, None)));
        assert_eq!(d(&[("index.html", "<h1>")]), Some((Framework::Statique, None)));
        assert_eq!(d(&[("notes.txt", "")]), None);
    }
}
