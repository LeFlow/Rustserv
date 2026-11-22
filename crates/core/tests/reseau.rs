//! Tests d'intégration qui utilisent Internet : ignorés par défaut.
//! Lancer avec : `cargo test -p rustserv-core --test reseau -- --ignored --nocapture`

use rustserv_core::catalog::{Catalog, Installer};
use rustserv_core::Paths;
use std::sync::atomic::AtomicBool;

/// Chaque version du catalogue intégré doit avoir au moins une URL qui répond.
#[tokio::test]
#[ignore]
async fn toutes_les_urls_du_catalogue_repondent() {
    let cat = Catalog::embedded().unwrap();
    let client = reqwest::Client::builder().user_agent("Rustserv-test").build().unwrap();
    let mut echecs = Vec::new();
    for (id, pkg) in &cat.packages {
        for r in &pkg.releases {
            let mut ok = false;
            for url in r.urls() {
                match client.head(url).send().await {
                    Ok(resp) if resp.status().is_success() => {
                        println!("OK   {id} {} → {url} ({:?} octets)", r.version, resp.content_length());
                        ok = true;
                        break;
                    }
                    Ok(resp) => println!("  {} {url}", resp.status()),
                    Err(e) => println!("  ERR {url} : {e}"),
                }
            }
            if !ok {
                echecs.push(format!("{id} {}", r.version));
            }
        }
    }
    assert!(echecs.is_empty(), "versions injoignables : {echecs:?}");
}

/// releases.json de windows.php.net est lisible et cohérent avec le catalogue.
#[tokio::test]
#[ignore]
async fn releases_php_officielles() {
    let client = reqwest::Client::builder().user_agent("Rustserv-test").build().unwrap();
    let r = rustserv_core::catalog::php_remote::fetch(&client).await.unwrap();
    for x in &r {
        println!("PHP {} ({}) {}", x.version, x.build.as_ref().unwrap().compiler, x.url);
    }
    assert!(r.iter().any(|x| x.branch.as_deref() == Some("8.3")));
    let mut cat = Catalog::embedded().unwrap();
    let ajout = cat.merge("php", r).unwrap();
    println!("versions absentes du catalogue intégré : {ajout:?}");
}

/// Installation réelle de Nginx dans un dossier temporaire.
#[tokio::test]
#[ignore]
async fn installe_nginx_reel() {
    let t = tempfile::tempdir().unwrap();
    let paths = Paths::new(t.path());
    paths.ensure_layout().unwrap();
    let cat = Catalog::embedded().unwrap();
    let r = cat.latest("nginx", None).unwrap();
    let inst = Installer::new(paths).unwrap();
    let p = inst.install("nginx", r, &AtomicBool::new(false), &|_| {}).await.unwrap();
    assert!(p.path.join("nginx.exe").is_file());
    assert!(p.path.join("conf").join("mime.types").is_file());
    println!("installé dans {}", p.path.display());
}
