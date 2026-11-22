//! Test de bout en bout avec les vrais binaires (téléchargés une fois dans
//! `target/pile-test`). Ignoré par défaut :
//! `cargo test -p rustserv-core --test pile_reelle -- --ignored --nocapture`

use rustserv_core::catalog::{Catalog, Installer};
use rustserv_core::process::{ports, Supervisor};
use rustserv_core::services::Stack;
use rustserv_core::settings::WebServer;
use rustserv_core::{Paths, Settings};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

fn racine() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("..").join("target").join("pile-test")
}

async fn installe(inst: &Installer, cat: &Catalog, id: &str, branche: Option<&str>) {
    let r = cat.latest(id, branche).unwrap();
    if !inst.is_installed(id, &r.version) {
        println!("installation de {id} {}…", r.version);
        inst.install(id, r, &AtomicBool::new(false), &|_| {}).await.unwrap();
    }
}

async fn get(port: u16, host: &str, path: &str) -> (u16, String) {
    let c = reqwest::Client::builder().no_proxy().build().unwrap();
    let r = c.get(format!("http://127.0.0.1:{port}{path}")).header("Host", host).send().await.unwrap();
    (r.status().as_u16(), r.text().await.unwrap())
}

/// Même chemin que les boutons de l'écran Services.
#[tokio::test]
#[ignore]
async fn orchestrateur_tout_demarrer_tout_arreter() {
    use rustserv_core::orchestrator::{Group, Orchestrator};
    use rustserv_core::process::ServiceState;
    let paths = Paths::new(racine());
    paths.ensure_layout().unwrap();
    let cat = Catalog::embedded().unwrap();
    let inst = Installer::new(paths.clone()).unwrap();
    for (id, b) in [("nginx", None), ("php", Some("8.3")), ("mariadb", None)] {
        installe(&inst, &cat, id, b).await;
    }
    let libre = || std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let mut settings = Settings::default();
    settings.ports.http = libre();
    settings.ports.mariadb = libre();
    let installed = inst.installed().unwrap();
    let orch = Orchestrator::new(paths.clone(), Supervisor::new().unwrap());

    orch.start_all(&settings, &installed).await.unwrap_or_else(|e| panic!("{e}\n{}", e.params()));
    for g in orch.overview(&settings, &installed) {
        println!("{:?} {} {:?} → {:?}", g.group, g.software, g.version, g.state);
        assert!(matches!(g.state, ServiceState::EnCours { .. }), "{:?} pas en marche", g.group);
    }
    let (code, txt) = get(settings.ports.http, "localhost", "/index.php").await;
    assert_eq!(code, 200, "{txt}");

    // Arrêter PHP arrête aussi le serveur web qui en dépend.
    orch.stop(Group::Php).await.unwrap();
    let v = orch.overview(&settings, &installed);
    assert_eq!(v[0].state, ServiceState::Arrete, "le serveur web devrait être arrêté");
    assert!(matches!(v[2].state, ServiceState::EnCours { .. }), "MariaDB doit rester en marche");

    // Redémarrer le web relance PHP automatiquement.
    orch.start(Group::Web, &settings, &installed).await.unwrap();
    assert!(matches!(orch.overview(&settings, &installed)[1].state, ServiceState::EnCours { .. }));

    orch.supervisor().stop_all().await;
    for g in orch.overview(&settings, &installed) {
        assert_eq!(g.state, ServiceState::Arrete);
    }
}

/// Exécute du code PHP en ligne de commande avec le php.ini de Rustserv.
fn php_cli(paths: &Paths, php: &rustserv_core::catalog::InstalledPackage, code: &str) -> String {
    let ini = rustserv_core::php::ensure_php_ini(paths, &php.path, &php.version, &Settings::default()).unwrap();
    let out = std::process::Command::new(php.path.join("php.exe")).arg("-c").arg(&ini).arg("-r").arg(code).output().unwrap();
    format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr))
}

#[tokio::test]
#[ignore]
async fn mariadb_initialise_persiste_et_s_arrete_proprement() {
    let paths = Paths::new(racine());
    paths.ensure_layout().unwrap();
    let cat = Catalog::embedded().unwrap();
    let inst = Installer::new(paths.clone()).unwrap();
    installe(&inst, &cat, "mariadb", None).await;
    installe(&inst, &cat, "php", Some("8.3")).await;

    let mut settings = Settings::default();
    settings.ports.mariadb = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let port = settings.ports.mariadb;
    let installed = inst.installed().unwrap();
    let stack = Stack::new(&paths, &settings, &installed);
    let php = stack.default_php().unwrap();
    let sup = Supervisor::new().unwrap();

    // Premier démarrage (initialise data/mariadb-<branche> si besoin).
    let spec = stack.mariadb().unwrap_or_else(|e| panic!("{e}"));
    sup.start(spec).await.unwrap_or_else(|e| panic!("{e}\n{}", e.params()));
    let sql = format!(
        "$m = new mysqli('127.0.0.1', 'root', '', '', {port}); \
         $m->query('CREATE DATABASE IF NOT EXISTS rustserv_test'); \
         echo 'VERSION ' . $m->server_info;"
    );
    let out = php_cli(&paths, php, &sql);
    println!("{out}");
    assert!(out.contains("VERSION ") && out.contains("MariaDB"), "{out}");

    // Arrêt propre puis redémarrage : la base doit toujours exister.
    sup.stop("mariadb").await.unwrap();
    assert!(!ports::is_listening(port));
    let journal = std::fs::read_to_string(paths.logs().join("mariadb-error.log")).unwrap_or_default();
    assert!(journal.lines().rev().take(10).any(|l| l.contains("Shutdown complete")), "arrêt non propre :\n{journal}");

    let stack = Stack::new(&paths, &settings, &installed);
    sup.start(stack.mariadb().unwrap()).await.unwrap();
    let out = php_cli(&paths, php, &format!("$m = new mysqli('127.0.0.1', 'root', '', 'rustserv_test', {port}); echo 'BASE OK';"));
    println!("{out}");
    assert!(out.contains("BASE OK"), "{out}");
    sup.stop_all().await;
}

#[tokio::test]
#[ignore]
async fn nginx_puis_apache_servent_php() {
    let paths = Paths::new(racine());
    paths.ensure_layout().unwrap();
    let cat = Catalog::embedded().unwrap();
    let inst = Installer::new(paths.clone()).unwrap();
    installe(&inst, &cat, "nginx", None).await;
    installe(&inst, &cat, "apache", None).await;
    installe(&inst, &cat, "php", Some("8.3")).await;

    // Projets de test
    std::fs::write(paths.www().join("index.php"), "<?php echo 'ACCUEIL ' . PHP_VERSION;").unwrap();
    let demo = paths.www().join("demo").join("public");
    std::fs::create_dir_all(&demo).unwrap();
    std::fs::write(
        demo.join("index.php"),
        "<?php echo 'SITE ' . $_SERVER['HTTP_HOST'] . ' ' . ($_SERVER['REQUEST_URI'] ?? '') . ' ext=' . (extension_loaded('mbstring') ? 'mbstring' : 'non');",
    )
    .unwrap();
    // Réécriture façon WordPress/Laravel pour Apache.
    std::fs::write(
        demo.join(".htaccess"),
        "RewriteEngine On\nRewriteCond %{REQUEST_FILENAME} !-f\nRewriteCond %{REQUEST_FILENAME} !-d\nRewriteRule ^ index.php [L]\n",
    )
    .unwrap();

    let mut settings = Settings::default();
    settings.ports.http = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let port = settings.ports.http;
    let installed = inst.installed().unwrap();
    let sup = Supervisor::new().unwrap();

    // Pool PHP
    let stack = Stack::new(&paths, &settings, &installed);
    let php = stack.default_php().expect("PHP installé");
    for s in stack.php_specs(php).unwrap() {
        sup.start(s).await.unwrap();
    }

    for serveur in [WebServer::Nginx, WebServer::Apache] {
        settings.web_server = serveur;
        let stack = Stack::new(&paths, &settings, &installed);
        let spec = match stack.web_server() {
            Ok(s) => s,
            Err(e) => panic!("{serveur:?} : {e}"),
        };
        let id = spec.id.clone();
        if let Err(e) = sup.start(spec).await {
            panic!("{serveur:?} : {e}\n{}", e.params());
        }

        let (code, txt) = get(port, "localhost", "/index.php").await;
        println!("{serveur:?} localhost → {code} {txt}");
        assert_eq!(code, 200);
        assert!(txt.starts_with("ACCUEIL 8.3."), "{txt}");

        let (code, txt) = get(port, "demo.test", "/").await;
        println!("{serveur:?} demo.test → {code} {txt}");
        assert!(txt.starts_with("SITE demo.test /") && txt.ends_with("ext=mbstring"), "{txt}");

        // URL « jolie » réécrite vers index.php (try_files nginx / .htaccess Apache).
        let (code, txt) = get(port, "demo.test", "/articles/42").await;
        println!("{serveur:?} demo.test/articles/42 → {code} {txt}");
        assert_eq!(code, 200);
        assert!(txt.contains("/articles/42"), "{txt}");

        // Requêtes simultanées : le pool répartit la charge.
        let reqs = (0..16).map(|_| get(port, "demo.test", "/"));
        for (code, _) in futures_util::future::join_all(reqs).await {
            assert_eq!(code, 200);
        }

        sup.stop(&id).await.unwrap();
        assert!(!ports::is_listening(port), "{serveur:?} n'a pas libéré le port");
        let journal = rustserv_core::process::logs::tail(&paths.logs().join(format!("{id}.log")), 5);
        println!("{serveur:?} arrêté. Journal :\n{journal}");
    }
    sup.stop_all().await;
    assert!(!ports::is_listening(9830));
}
