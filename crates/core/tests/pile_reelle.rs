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

    // « Tout redémarrer » : nouveau port HTTP pris en compte, nouveaux processus.
    let pid = |s: &Settings, g: usize| match &orch.overview(s, &installed)[g].state {
        ServiceState::EnCours { pid, .. } => *pid,
        s => panic!("{s:?}"),
    };
    let avant = (pid(&settings, 0), pid(&settings, 2));
    let ancien_port = settings.ports.http;
    settings.ports.http = libre();
    let groupes = orch.restart_running(&settings, &installed).await.unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(groupes.len(), 3);
    assert_ne!(avant, (pid(&settings, 0), pid(&settings, 2)), "les processus auraient dû être relancés");
    assert_eq!(get(settings.ports.http, "localhost", "/index.php").await.0, 200, "nouveau port non appliqué");
    assert!(!rustserv_core::process::ports::is_listening(ancien_port), "ancien port toujours ouvert");

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

/// Phase 2 : deux versions de PHP en parallèle, une par site, changement à chaud.
#[tokio::test]
#[ignore]
async fn multi_versions_php_par_site() {
    use rustserv_core::orchestrator::{Group, Orchestrator};
    let paths = Paths::new(racine());
    paths.ensure_layout().unwrap();
    let cat = Catalog::embedded().unwrap();
    let inst = Installer::new(paths.clone()).unwrap();
    for (id, b) in [("nginx", None), ("apache", None), ("php", Some("8.3")), ("php", Some("7.4"))] {
        installe(&inst, &cat, id, b).await;
    }
    for site in ["ancien", "moderne"] {
        let d = paths.www().join(site);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("v.php"), "<?php echo PHP_MAJOR_VERSION . '.' . PHP_MINOR_VERSION;").unwrap();
    }

    let mut settings = Settings::default();
    settings.ports.http = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    settings.versions.php = Some("8.3".into());
    settings.site_php.insert("ancien".into(), "7.4".into());
    let port = settings.ports.http;
    let installed = inst.installed().unwrap();

    // php -m valide pour chaque version installée.
    let st = Stack::new(&paths, &settings, &installed);
    for p in st.php_needed() {
        let ini = rustserv_core::php::ensure_php_ini(&paths, &p.path, &p.version, &settings).unwrap();
        let c = rustserv_core::php::check(&p.path, &p.version, &ini).unwrap_or_else(|e| panic!("{e}"));
        assert!(c.modules.contains(&"mbstring".to_string()), "PHP {} : {:?}", p.version, c.modules);
    }

    let orch = Orchestrator::new(paths.clone(), Supervisor::new().unwrap());
    for serveur in [WebServer::Nginx, WebServer::Apache] {
        settings.web_server = serveur;
        orch.start(Group::Web, &settings, &installed).await.unwrap_or_else(|e| panic!("{e}\n{}", e.params()));
        for (site, attendu) in [("ancien", "7.4"), ("moderne", "8.3")] {
            let (c1, par_domaine) = get(port, &format!("{site}.test"), "/v.php").await;
            let (c2, par_dossier) = get(port, "localhost", &format!("/{site}/v.php")).await;
            println!("{serveur:?} {site} : domaine → {c1} {par_domaine} | /{site}/ → {c2} {par_dossier}");
            assert_eq!(par_domaine, attendu, "{serveur:?} {site} par domaine");
            assert_eq!(par_dossier, attendu, "{serveur:?} {site} par dossier");
        }

        // Changement à chaud : « moderne » passe en 7.4, sans redémarrage manuel.
        settings.site_php.insert("moderne".into(), "7.4".into());
        orch.apply(&settings, &installed).await.unwrap();
        let (_, v) = get(port, "moderne.test", "/v.php").await;
        assert_eq!(v, "7.4", "{serveur:?} : changement à chaud non appliqué");
        settings.site_php.remove("moderne");
        orch.apply(&settings, &installed).await.unwrap();
        orch.stop(Group::Php).await.unwrap();
    }
    orch.supervisor().stop_all().await;
}

/// Phase 2 : mise à jour de branche réelle 8.3.36 → 8.3.37 (dossier dédié, recréé à chaque fois).
#[tokio::test]
#[ignore]
async fn mise_a_jour_de_branche_reelle() {
    use rustserv_core::catalog::Release;
    use rustserv_core::orchestrator::{Group, Orchestrator};
    use rustserv_core::php::ini::IniFile;
    use sha2::{Digest, Sha256};

    let root = racine().with_file_name("pile-maj");
    let _ = std::fs::remove_dir_all(&root);
    let paths = Paths::new(&root);
    paths.ensure_layout().unwrap();
    let cat = Catalog::embedded().unwrap();
    let inst = Installer::new(paths.clone()).unwrap();
    installe(&inst, &cat, "nginx", None).await;

    // Ancienne version 8.3.36 (archives) : SHA256 calculé au téléchargement, uniquement pour ce test.
    let mut ancienne: Release = cat.latest("php", Some("8.3")).unwrap().clone();
    let nouvelle = ancienne.clone();
    ancienne.version = "8.3.36".into();
    ancienne.url = "https://windows.php.net/downloads/releases/archives/php-8.3.36-nts-Win32-vs16-x64.zip".into();
    ancienne.mirrors = vec![];
    let octets = inst.client().get(&ancienne.url).send().await.unwrap().bytes().await.unwrap();
    ancienne.sha256 = hex::encode(Sha256::digest(&octets));
    inst.install("php", &ancienne, &AtomicBool::new(false), &|_| {}).await.unwrap();
    let old_dir = inst.install_dir("php", "8.3.36");

    // Personnalisations de l'utilisateur : réglage + DLL ajoutée à la main.
    let settings = Settings { ports: rustserv_core::settings::Ports { http: libre(), ..Default::default() }, ..Default::default() };
    let ini_old = rustserv_core::php::ensure_php_ini(&paths, &old_dir, "8.3.36", &settings).unwrap();
    let mut ini = IniFile::parse(&std::fs::read_to_string(&ini_old).unwrap());
    ini.set("memory_limit", "777M");
    std::fs::write(&ini_old, ini.to_text()).unwrap();
    std::fs::write(old_dir.join("ext").join("php_rustserv_test.dll"), "faux").unwrap();

    let site = paths.www().join("app");
    std::fs::create_dir_all(&site).unwrap();
    std::fs::write(site.join("i.php"), "<?php echo PHP_VERSION . ' ' . ini_get('memory_limit');").unwrap();

    let orch = Orchestrator::new(paths.clone(), Supervisor::new().unwrap());
    orch.start(Group::Web, &settings, &inst.installed().unwrap()).await.unwrap();
    let port = settings.ports.http;
    assert_eq!(get(port, "app.test", "/i.php").await.1, "8.3.36 777M");

    // 1. Échec de validation → retour arrière complet, l'ancienne version continue de servir.
    let mut casse = ini.clone();
    casse.enable_extension("rustserv_absente", false);
    std::fs::write(&ini_old, casse.to_text()).unwrap();
    let err = orch.upgrade_php_branch(&inst, &settings, "8.3", &nouvelle, false, &AtomicBool::new(false), &|_| {}).await.unwrap_err();
    println!("échec attendu : {err}");
    assert_eq!(err.code(), "php.invalide");
    assert!(!inst.is_installed("php", &nouvelle.version), "la nouvelle version aurait dû être retirée");
    assert!(!rustserv_core::php::php_ini_path(&paths, &nouvelle.version).exists());
    assert_eq!(get(port, "app.test", "/i.php").await.1, "8.3.36 777M");

    // 2. Mise à jour réussie, sans conserver l'ancienne version.
    std::fs::write(&ini_old, ini.to_text()).unwrap();
    let (rapport, nouveaux) = orch
        .upgrade_php_branch(&inst, &settings, "8.3", &nouvelle, false, &AtomicBool::new(false), &|_| {})
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    println!("{rapport:?}");
    assert!(rapport.ini_repris);
    assert_eq!(rapport.dll_copiees, ["ext/php_rustserv_test.dll"]);
    let (code, txt) = get(port, "app.test", "/i.php").await;
    println!("après mise à jour : {code} {txt}");
    assert_eq!(txt, format!("{} 777M", nouvelle.version), "bascule à chaud + réglage repris");
    assert!(!old_dir.exists(), "l'ancienne version aurait dû être supprimée");
    assert_eq!(nouveaux, settings);
    orch.supervisor().stop_all().await;
}

/// Phase 3 : extensions, dépendances, PECL, profils (dossier `target/pile-ext`).
#[tokio::test]
#[ignore]
async fn extensions_pecl_et_profils_reels() {
    use rustserv_core::orchestrator::{Group, Orchestrator};
    use rustserv_core::php::extensions::ExtState;
    use rustserv_core::process::ServiceState;

    let root = racine().with_file_name("pile-ext");
    let paths = Paths::new(&root);
    paths.ensure_layout().unwrap();
    let _ = std::fs::remove_dir_all(paths.etc().join("php")); // php.ini neufs à chaque exécution
    let cat = Catalog::embedded().unwrap();
    let inst = Installer::new(paths.clone()).unwrap();
    // Les DLL PECL d'un essai précédent restent dans bin/ : on repart d'une installation propre.
    for b in ["8.3", "7.4"] {
        let v = cat.latest("php", Some(b)).unwrap().version.clone();
        let _ = inst.uninstall("php", &v);
        installe(&inst, &cat, "php", Some(b)).await;
    }
    let settings = Settings::default();
    let orch = Orchestrator::new(paths.clone(), Supervisor::new().unwrap());
    let v83 = cat.latest("php", Some("8.3")).unwrap().version.clone();
    let v74 = cat.latest("php", Some("7.4")).unwrap().version.clone();
    let etat = |l: &[rustserv_core::php::extensions::ExtensionInfo], n: &str| l.iter().find(|e| e.name == n).map(|e| e.state);

    // 1. Analyse
    let l = orch.extensions(&inst, &settings, &v83).await.unwrap();
    let compte = |s| l.iter().filter(|e| e.state == s).count();
    println!(
        "PHP {v83} : {} actives, {} inactives, {} intégrées, {} en erreur",
        compte(ExtState::Active), compte(ExtState::Inactive), compte(ExtState::Integree), compte(ExtState::Erreur)
    );
    assert_eq!(etat(&l, "mbstring"), Some(ExtState::Active));
    assert_eq!(etat(&l, "core"), Some(ExtState::Integree));
    assert_eq!(compte(ExtState::Erreur), 0);

    // 2. Dépendances : mbstring est requise par exif.
    let e = orch.set_extension(&inst, &settings, &v83, "mbstring", false).await.unwrap_err();
    assert_eq!(e.code(), "extension.requise");
    orch.set_extension(&inst, &settings, &v83, "exif", false).await.unwrap();
    orch.set_extension(&inst, &settings, &v83, "mbstring", false).await.unwrap();
    let c = orch.set_extension(&inst, &settings, &v83, "exif", true).await.unwrap();
    assert!(c.modules.contains(&"mbstring".into()) && c.modules.contains(&"exif".into()), "mbstring réactivée automatiquement");

    // 3. Extension qui exige une bibliothèque absente → refus et php.ini restauré.
    let ini = rustserv_core::php::php_ini_path(&paths, &v83);
    let avant = std::fs::read_to_string(&ini).unwrap();
    let e = orch.set_extension(&inst, &settings, &v83, "pdo_firebird", true).await;
    println!("pdo_firebird → {:?}", e.as_ref().map(|_| "chargée").map_err(|e| e.code()));
    if let Err(e) = e {
        assert_eq!(e.code(), "php.invalide");
        assert_eq!(std::fs::read_to_string(&ini).unwrap(), avant, "php.ini non restauré");
    }

    // 4. PECL sur 8.3 (avec pool en marche : il doit être relancé).
    orch.start(Group::Php, &Settings { versions: rustserv_core::settings::Versions { php: Some("8.3".into()), ..Default::default() }, ..Default::default() }, &inst.installed().unwrap())
        .await
        .unwrap();
    let pid_avant = match orch.supervisor().state(&format!("php-{v83}#0")) { ServiceState::EnCours { pid, .. } => pid, s => panic!("{s:?}") };
    for ext in ["xdebug", "apcu", "redis"] {
        let r = orch.install_pecl(&inst, &settings, &v83, ext, &AtomicBool::new(false), &|_| {}).await.unwrap_or_else(|e| panic!("{ext} : {e}"));
        println!("PECL {ext} {} installée : {:?}", r.version, r.files);
    }
    let l = orch.extensions(&inst, &settings, &v83).await.unwrap();
    for ext in ["xdebug", "apcu", "redis"] {
        let info = l.iter().find(|e| e.name == ext).unwrap();
        assert_eq!(info.state, ExtState::Active, "{ext} : {:?}", info.erreur);
        assert!(info.pecl.is_some());
    }
    assert!(l.iter().find(|e| e.name == "xdebug").unwrap().zend);
    let pid_apres = match orch.supervisor().state(&format!("php-{v83}#0")) { ServiceState::EnCours { pid, .. } => pid, s => panic!("{s:?}") };
    assert_ne!(pid_avant, pid_apres, "le pool PHP aurait dû être relancé");

    // 5. Profil WordPress (imagick depuis PECL).
    let plan = orch.profile_plan(&inst, &settings, &v83, "wordpress").await.unwrap();
    println!("plan WordPress : {plan:?}");
    let r = orch.apply_profile(&inst, &settings, &v83, "wordpress", &AtomicBool::new(false), &|_| {}).await.unwrap_or_else(|e| panic!("{e}"));
    println!("échecs PECL : {:?}", r.echecs_pecl);
    assert!(r.echecs_pecl.is_empty());
    let apres = orch.profile_plan(&inst, &settings, &v83, "wordpress").await.unwrap();
    assert!(apres.is_empty(), "profil non entièrement appliqué : {apres:?}");

    // 6. PHP 7.4 (compilateur vc15).
    let l74 = orch.extensions(&inst, &settings, &v74).await.unwrap();
    let erreurs: Vec<_> = l74.iter().filter(|e| e.state == ExtState::Erreur).map(|e| (&e.name, &e.erreur)).collect();
    assert!(erreurs.is_empty(), "PHP 7.4 : extensions faussement en erreur {erreurs:?}");
    assert_eq!(etat(&l74, "gd2"), Some(ExtState::Active), "gd2 (7.4) doit être reconnue via le module gd");
    let r = orch.install_pecl(&inst, &settings, &v74, "apcu", &AtomicBool::new(false), &|_| {}).await.unwrap_or_else(|e| panic!("7.4 apcu : {e}"));
    println!("PHP {v74} : apcu {}", r.version);
    let r = orch.apply_profile(&inst, &settings, &v74, "laravel", &AtomicBool::new(false), &|_| {}).await.unwrap_or_else(|e| panic!("{e}"));
    println!("Laravel sur 7.4 : {:?} / échecs {:?}", r.plan, r.echecs_pecl);
    orch.supervisor().stop_all().await;
}

/// Phase 4 : HTTPS réel sur Nginx puis Apache, avec la CA locale (dossier `target/pile-https`).
/// La CA est approuvée **uniquement par le client de test** (pas d'installation dans Windows).
#[tokio::test]
#[ignore]
async fn https_nginx_et_apache() {
    use rustserv_core::orchestrator::{Group, Orchestrator};
    let root = racine().with_file_name("pile-https");
    let paths = Paths::new(&root);
    paths.ensure_layout().unwrap();
    let _ = rustserv_core::ssl::delete_all(&paths);
    let cat = Catalog::embedded().unwrap();
    let inst = Installer::new(paths.clone()).unwrap();
    for (id, b) in [("nginx", None), ("apache", None), ("php", Some("8.3"))] {
        installe(&inst, &cat, id, b).await;
    }
    let site = paths.www().join("demo");
    std::fs::create_dir_all(&site).unwrap();
    std::fs::write(site.join("index.php"), "<?php echo ($_SERVER['HTTPS'] ?? 'off') . ' ' . $_SERVER['HTTP_HOST'];").unwrap();

    let ca = rustserv_core::ssl::ensure_ca(&paths).unwrap();
    let ca_pem = std::fs::read(rustserv_core::ssl::ca_cert_path(&paths)).unwrap();
    let mut settings = Settings::default();
    settings.ports.http = libre();
    settings.ports.https = libre();
    let (http, https) = (settings.ports.http, settings.ports.https);
    let orch = Orchestrator::new(paths.clone(), Supervisor::new().unwrap());

    let client = reqwest::Client::builder()
        .no_proxy()
        .add_root_certificate(reqwest::Certificate::from_pem(&ca_pem).unwrap())
        .resolve("demo.test", std::net::SocketAddr::from(([127, 0, 0, 1], https)))
        .resolve("localhost", std::net::SocketAddr::from(([127, 0, 0, 1], https)))
        .build()
        .unwrap();

    for serveur in [WebServer::Nginx, WebServer::Apache] {
        settings.web_server = serveur;
        orch.start(Group::Web, &settings, &inst.installed().unwrap()).await.unwrap_or_else(|e| panic!("{serveur:?} : {e}\n{}", e.params()));
        for url in [format!("https://demo.test:{https}/"), format!("https://localhost:{https}/demo/index.php")] {
            let r = client.get(&url).send().await.unwrap_or_else(|e| panic!("{serveur:?} {url} : {e:?}"));
            let (code, version) = (r.status().as_u16(), r.version());
            let txt = r.text().await.unwrap();
            println!("{serveur:?} {url} → {code} {version:?} « {txt} »");
            assert_eq!(code, 200);
            assert!(txt.starts_with("on "), "PHP doit voir HTTPS=on : {txt}");
        }
        // Le HTTP reste disponible.
        assert_eq!(get(http, "demo.test", "/").await.1, format!("off demo.test"));
        orch.stop(Group::Php).await.unwrap();
    }
    println!("CA : {}", ca.info.nom);
    orch.supervisor().stop_all().await;
}

/// Phase 4 : terminal intégré (vrai PowerShell via ConPTY) avec le PHP du site.
#[tokio::test]
#[ignore]
async fn terminal_avec_le_php_du_site() {
    use rustserv_core::terminal::Terminals;
    use std::sync::{Arc, Mutex};
    let paths = Paths::new(racine());
    paths.ensure_layout().unwrap();
    let cat = Catalog::embedded().unwrap();
    let inst = Installer::new(paths.clone()).unwrap();
    installe(&inst, &cat, "php", Some("7.4")).await;
    installe(&inst, &cat, "php", Some("8.3")).await;
    std::fs::create_dir_all(paths.www().join("ancien")).unwrap();
    let mut settings = Settings::default();
    settings.site_php.insert("ancien".into(), "7.4".into());
    let installed = inst.installed().unwrap();
    let st = Stack::new(&paths, &settings, &installed);
    let env = st.terminal_env(Some("ancien")).unwrap();
    let php = st.site_php("ancien").unwrap();
    rustserv_core::php::ensure_php_ini(&paths, &php.path, &php.version, &settings).unwrap();

    let sortie = Arc::new(Mutex::new(String::new()));
    let fini = Arc::new(Mutex::new(false));
    let (s2, f2) = (sortie.clone(), fini.clone());
    let terms = Terminals::default();
    let id = terms
        .open(&env, 120, 30, Arc::new(move |_, t| s2.lock().unwrap().push_str(&t)), Arc::new(move |_| *f2.lock().unwrap() = true))
        .unwrap();
    eprintln!("[test] terminal {id} ouvert");
    // ConPTY demande la position du curseur (ESC[6n) et attend la réponse avant de
    // lancer le shell. Dans l'app, xterm.js répond ; ici on simule le terminal.
    for _ in 0..50 {
        if sortie.lock().unwrap().contains("\x1b[6n") {
            terms.write(id, "\x1b[1;1R").unwrap();
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    terms.write(id, "php -r \"echo 'VERSION=' . PHP_VERSION . ' INI=' . php_ini_loaded_file();\"\r\n").unwrap();
    eprintln!("[test] commande 1 envoyée");
    terms.write(id, "Write-Output \"DOSSIER=$((Get-Location).Path)\"\r\n").unwrap();
    eprintln!("[test] commande 2 envoyée");

    let attendu = format!("VERSION={}", php.version);
    let mut ok = false;
    for _ in 0..100 {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let s = sortie.lock().unwrap().clone();
        if s.contains(&attendu) && s.contains("DOSSIER=") && s.matches("DOSSIER=").count() >= 2 {
            ok = true;
            break;
        }
    }
    eprintln!("[test] attente terminée, ok = {ok}, {} caractères reçus", sortie.lock().unwrap().len());
    let s = sortie.lock().unwrap().clone();
    // Nettoyage des séquences d'échappement pour l'affichage.
    let lisible: String = s.lines().filter(|l| l.contains("VERSION=") || l.contains("DOSSIER=")).collect::<Vec<_>>().join("\n");
    println!("{lisible}");
    assert!(ok, "sortie inattendue :\n{s}");
    assert!(s.contains(&format!("{}", rustserv_core::php::php_ini_path(&paths, &php.version).display()).replace('\\', "\\")) || s.contains("php.ini"), "php.ini de Rustserv non utilisé");
    assert!(s.contains("www\\ancien"), "mauvais dossier de départ");

    eprintln!("[test] fermeture…");
    terms.close(id);
    eprintln!("[test] close() terminé");
    for _ in 0..50 {
        if *fini.lock().unwrap() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(*fini.lock().unwrap(), "le terminal ne s'est pas fermé");
}

/// Phase 5 : tests multi-versions et analyses de compatibilité sur un projet « historique ».
#[tokio::test]
#[ignore]
async fn retrocompatibilite_reelle() {
    use rustserv_core::compat::{analysis::Gravite, ops, runner::{Statut, TestKind}, tools};
    let paths = Paths::new(racine());
    paths.ensure_layout().unwrap();
    let cat = Catalog::embedded().unwrap();
    let inst = Installer::new(paths.clone()).unwrap();
    installe(&inst, &cat, "php", Some("7.4")).await;
    installe(&inst, &cat, "php", Some("8.3")).await;
    let settings = Settings::default();
    let v74 = cat.latest("php", Some("7.4")).unwrap().version.clone();
    let v83 = cat.latest("php", Some("8.3")).unwrap().version.clone();

    // Projet de test.
    let site = paths.www().join("historique");
    std::fs::create_dir_all(site.join("tests")).unwrap();
    std::fs::write(
        site.join("legacy.php"),
        "<?php\n$a = ['x' => 1];\nwhile (list($k, $v) = each($a)) {\n    echo \"$k=$v\\n\";\n}\necho utf8_encode('e'), \"\\n\";\n",
    )
    .unwrap();
    std::fs::write(site.join("script.php"), "<?php\necho str_contains('abc', 'b') ? 'oui' : 'non';\n").unwrap();
    std::fs::write(
        site.join("tests/ExempleTest.php"),
        "<?php\nuse PHPUnit\\Framework\\TestCase;\nfinal class ExempleTest extends TestCase {\n    public function testAddition(): void { $this->assertSame(4, 2 + 2); }\n    public function testStrContains(): void { $this->assertTrue(str_contains('abc', 'b')); }\n}\n",
    )
    .unwrap();

    // 1. Outils (installe aussi Composer).
    let t0 = std::time::Instant::now();
    let st = if tools::status(&paths).installes { tools::status(&paths) } else { ops::install_tools(&paths, &settings, &inst, &cat).await.unwrap_or_else(|e| panic!("{e}")) };
    println!("outils ({:.0?}) : phpcs {:?}, PHPCompatibility {:?}, PHPStan {:?}", t0.elapsed(), st.phpcs, st.phpcompatibility, st.phpstan);
    assert!(st.installes);

    // 2. Script : str_contains existe en 8.x, pas en 7.4.
    let r = ops::run_tests(&paths, &settings, &inst, "historique", TestKind::Script, Some("script.php".into()), &[v83.clone(), v74.clone()]).await.unwrap();
    for x in &r {
        println!("script PHP {} → {:?} ({} ms) : {}", x.version, x.statut, x.duree_ms, x.sortie.lines().find(|l| !l.trim().is_empty()).unwrap_or(""));
    }
    assert_eq!(r[0].version, v74, "résultats triés par version");
    assert_eq!(r[0].statut, Statut::Erreur);
    assert_eq!((r[1].statut, r[1].sortie.trim()), (Statut::Reussi, "oui"));

    // 3. PHPUnit 9.6 (compatible 7.4 → 8.x) installé dans le projet avec Composer.
    if !site.join("vendor/phpunit/phpunit/phpunit").is_file() {
        let php74 = inst.installed().unwrap().into_iter().find(|p| p.version == v74).unwrap();
        let ini = rustserv_core::php::ensure_php_ini(&paths, &php74.path, &php74.version, &settings).unwrap();
        let composer = inst.installed().unwrap().into_iter().find(|p| p.package == "composer").unwrap();
        let out = std::process::Command::new(php74.path.join("php.exe"))
            .arg("-c").arg(&ini).arg(composer.path.join("composer.phar"))
            .args(["require", "--dev", "phpunit/phpunit:^9.6", "--no-interaction", "--no-progress"])
            .current_dir(&site)
            .env("COMPOSER_HOME", paths.tmp().join("composer"))
            .output()
            .unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    }
    let r = ops::run_tests(&paths, &settings, &inst, "historique", TestKind::Phpunit, None, &[v74.clone(), v83.clone()]).await.unwrap();
    for x in &r {
        println!("PHPUnit PHP {} → {:?} {:?}", x.version, x.statut, x.resume);
    }
    assert_eq!(r[0].statut, Statut::Echec);
    assert_eq!(r[0].resume.as_ref().map(|s| (s.tests, s.erreurs)), Some((2, 1)));
    assert_eq!(r[1].statut, Statut::Reussi);
    assert_eq!(r[1].resume.as_ref().map(|s| s.tests), Some(2));

    // 4. PHPCompatibility : du code PHP 7.4 vers PHP 8.x (vendor ignoré).
    let rap = ops::phpcompatibility(&paths, &settings, &inst, "historique", "7.4", None).await.unwrap_or_else(|e| panic!("{e}"));
    println!("PHPCompatibility {} : {} erreur(s), {} avertissement(s), {} ms", rap.cible, rap.erreurs, rap.avertissements, rap.duree_ms);
    for f in &rap.fichiers {
        for m in &f.messages {
            println!("  {}:{} [{:?}] {}", f.chemin, m.ligne, m.gravite, m.message);
        }
    }
    assert!(rap.fichiers.iter().all(|f| !f.chemin.starts_with("vendor/")), "vendor doit être ignoré");
    let legacy = rap.fichiers.iter().find(|f| f.chemin == "legacy.php").expect("legacy.php non signalé");
    assert!(legacy.messages.iter().any(|m| m.gravite == Gravite::Erreur && m.message.contains("each")));
    assert!(legacy.messages.iter().any(|m| m.message.contains("utf8_encode")));

    // 5. PHPStan niveau 5 pour PHP 8.3.
    let rap = ops::phpstan(&paths, &settings, &inst, "historique", 5, "8.3").await.unwrap_or_else(|e| panic!("{e}"));
    println!("PHPStan {} : {} erreur(s), {} ms", rap.cible, rap.erreurs, rap.duree_ms);
    for f in &rap.fichiers {
        for m in &f.messages {
            println!("  {}:{} {}", f.chemin, m.ligne, m.message);
        }
    }
    assert!(rap.fichiers.iter().any(|f| f.chemin == "legacy.php" && f.messages.iter().any(|m| m.message.contains("each"))));
}

/// Installation de PHPUnit / Pest dans un projet neuf (sans composer.json), puis exécution.
#[tokio::test]
#[ignore]
async fn installation_phpunit_et_pest() {
    use rustserv_core::compat::{ops, runner::{Statut, TestKind}};
    let paths = Paths::new(racine());
    paths.ensure_layout().unwrap();
    let cat = Catalog::embedded().unwrap();
    let inst = Installer::new(paths.clone()).unwrap();
    installe(&inst, &cat, "php", Some("7.4")).await;
    installe(&inst, &cat, "php", Some("8.3")).await;
    let settings = Settings::default();
    let v74 = cat.latest("php", Some("7.4")).unwrap().version.clone();
    let v83 = cat.latest("php", Some("8.3")).unwrap().version.clone();

    // PHPUnit pour 7.4 + 8.3 → Composer doit choisir une version compatible 7.4 (9.x).
    let site = paths.www().join("neuf-phpunit");
    let _ = std::fs::remove_dir_all(&site);
    std::fs::create_dir_all(site.join("tests")).unwrap();
    std::fs::write(site.join("tests/SommeTest.php"), "<?php\nuse PHPUnit\\Framework\\TestCase;\nfinal class SommeTest extends TestCase {\n    public function testSomme(): void { $this->assertSame(4, 2 + 2); }\n}\n").unwrap();
    let versions = vec![v74.clone(), v83.clone()];
    let assert_absent = ops::run_tests(&paths, &settings, &inst, "neuf-phpunit", TestKind::Phpunit, None, &versions).await.unwrap_err();
    assert_eq!(assert_absent.code(), "compat.phpunit_absent");
    let v = ops::install_test_framework(&paths, &settings, &inst, &cat, "neuf-phpunit", TestKind::Phpunit, &versions).await.unwrap_or_else(|e| panic!("{e}"));
    println!("PHPUnit installé : {v}");
    assert!(v.starts_with("9."), "PHPUnit compatible PHP 7.4 attendu, obtenu {v}");
    let r = ops::run_tests(&paths, &settings, &inst, "neuf-phpunit", TestKind::Phpunit, None, &versions).await.unwrap();
    for x in &r {
        println!("  PHPUnit PHP {} → {:?} {:?}", x.version, x.statut, x.resume);
        assert_eq!(x.statut, Statut::Reussi, "{}", x.sortie);
    }

    // Pest pour PHP 8.3 seulement.
    let site = paths.www().join("neuf-pest");
    let _ = std::fs::remove_dir_all(&site);
    std::fs::create_dir_all(site.join("tests")).unwrap();
    std::fs::write(site.join("tests/SommeTest.php"), "<?php\ntest('somme', function () {\n    expect(2 + 2)->toBe(4);\n});\n").unwrap();
    let v = ops::install_test_framework(&paths, &settings, &inst, &cat, "neuf-pest", TestKind::Pest, &[v83.clone()]).await.unwrap_or_else(|e| panic!("{e}"));
    println!("Pest installé : {v}");
    let r = ops::run_tests(&paths, &settings, &inst, "neuf-pest", TestKind::Pest, None, &[v83.clone()]).await.unwrap();
    println!("  Pest PHP {} → {:?} {:?}", r[0].version, r[0].statut, r[0].resume);
    assert_eq!(r[0].statut, Statut::Reussi, "{}", r[0].sortie);
    assert_eq!(r[0].resume.as_ref().map(|s| s.tests), Some(1));
}

/// Adminer servi sous /adminer/ par nginx puis Apache, connecté automatiquement à MariaDB.
#[tokio::test]
#[ignore]
async fn adminer_connexion_automatique() {
    use rustserv_core::orchestrator::{Group, Orchestrator};
    let paths = Paths::new(racine());
    paths.ensure_layout().unwrap();
    let cat = Catalog::embedded().unwrap();
    let inst = Installer::new(paths.clone()).unwrap();
    for (id, b) in [("nginx", None), ("apache", None), ("php", Some("8.3")), ("mariadb", None), ("adminer", None)] {
        installe(&inst, &cat, id, b).await;
    }
    let mut settings = Settings::default();
    settings.ports.http = libre();
    settings.ports.mariadb = libre();
    let port = settings.ports.http;
    let installed = inst.installed().unwrap();
    let orch = Orchestrator::new(paths.clone(), Supervisor::new().unwrap());
    orch.start(Group::Mariadb, &settings, &installed).await.unwrap_or_else(|e| panic!("{e}"));

    for serveur in [WebServer::Nginx, WebServer::Apache] {
        settings.web_server = serveur;
        orch.start(Group::Web, &settings, &installed).await.unwrap_or_else(|e| panic!("{serveur:?} : {e}\n{}", e.params()));
        let client = reqwest::Client::builder().no_proxy().cookie_store(true).redirect(reqwest::redirect::Policy::none()).build().unwrap();
        let base = format!("http://127.0.0.1:{port}");

        // /adminer → /adminer/
        let r = client.get(format!("{base}/adminer")).send().await.unwrap();
        assert_eq!(r.status().as_u16(), 301, "{serveur:?} : redirection /adminer");

        // Première visite : connexion automatique puis redirection vers ?server=…&username=root.
        let r = client.get(format!("{base}/adminer/")).send().await.unwrap();
        let code = r.status().as_u16();
        let lieu = r.headers().get("location").map(|v| v.to_str().unwrap().to_string()).unwrap_or_default();
        println!("{serveur:?} /adminer/ → {code} {lieu}");
        assert!(code == 302 || code == 303, "{serveur:?} : connexion automatique attendue, reçu {code} : {}", r.text().await.unwrap_or_default().chars().take(500).collect::<String>());
        assert!(lieu.contains("username=root"), "{lieu}");

        // Adresse relative à /adminer/ (« ?server=… »).
        let url = if lieu.starts_with("http") { lieu.clone() } else if lieu.starts_with('/') { format!("{base}{lieu}") } else { format!("{base}/adminer/{lieu}") };
        let r = client.get(&url).send().await.unwrap();
        let code = r.status().as_u16();
        let page = r.text().await.unwrap();
        println!("{serveur:?} {url} → {code}, {} octets", page.len());
        assert_eq!(code, 200);
        assert!(page.contains("information_schema"), "{serveur:?} : liste des bases absente :\n{}", page.chars().take(800).collect::<String>());
        assert!(page.contains("Rustserv"), "nom personnalisé absent");
        orch.stop(Group::Php).await.unwrap();
    }
    orch.supervisor().stop_all().await;
}

fn libre() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
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
