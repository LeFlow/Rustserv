//! Tests du superviseur avec de vrais processus (le faux service `rustserv-fake-service`).

use rustserv_core::process::{logs, ServiceSpec, ServiceState, StopStrategy, Supervisor};
use std::time::Duration;

const FAKE: &str = env!("CARGO_BIN_EXE_rustserv-fake-service");

fn port_libre() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

fn spec(id: &str, dir: &std::path::Path, args: &[&str]) -> ServiceSpec {
    let mut s = ServiceSpec::new(id, FAKE, dir.join(format!("{id}.log")));
    s.args = args.iter().map(|a| a.to_string()).collect();
    s.start_timeout = Duration::from_secs(5);
    s.stop_timeout = Duration::from_secs(3);
    s
}

/// Le processus `pid` est-il terminé (attente max 3 s) ?
fn est_mort(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
    use windows_sys::Win32::System::Threading::{OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE};
    unsafe {
        let h = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
        if h.is_null() {
            return true; // n'existe plus
        }
        let r = WaitForSingleObject(h, 3000);
        CloseHandle(h);
        r == WAIT_OBJECT_0
    }
}

#[tokio::test]
async fn demarre_attend_le_port_et_arrete() {
    let t = tempfile::tempdir().unwrap();
    let sup = Supervisor::new().unwrap();
    let port = port_libre();
    let mut s = spec("web", t.path(), &["--port", &port.to_string()]);
    s.ports = vec![port];

    sup.start(s.clone()).await.unwrap();
    let ServiceState::EnCours { pid, .. } = sup.state("web") else { panic!("{:?}", sup.state("web")) };
    assert!(rustserv_core::process::ports::is_listening(port));
    assert_eq!(sup.start(s).await.unwrap_err().code(), "service.deja_demarre");

    sup.stop("web").await.unwrap();
    assert_eq!(sup.state("web"), ServiceState::Arrete);
    assert!(est_mort(pid));
    assert!(!rustserv_core::process::ports::is_listening(port));
}

#[tokio::test]
async fn refuse_un_port_occupe() {
    let t = tempfile::tempdir().unwrap();
    let sup = Supervisor::new().unwrap();
    let occupe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = occupe.local_addr().unwrap().port();
    let mut s = spec("web", t.path(), &[]);
    s.ports = vec![port];
    let err = sup.start(s).await.unwrap_err();
    assert_eq!(err.code(), "port.occupe");
    assert_eq!(err.params()["pid"], std::process::id().to_string());
    assert!(matches!(sup.state("web"), ServiceState::Erreur { .. }));
}

#[tokio::test]
async fn arret_immediat_donne_le_journal() {
    let t = tempfile::tempdir().unwrap();
    let sup = Supervisor::new().unwrap();
    let port = port_libre();
    let mut s = spec("db", t.path(), &["--exit-after-ms", "50"]);
    s.ports = vec![port];
    let err = sup.start(s).await.unwrap_err();
    assert_eq!(err.code(), "service.demarrage");
    assert!(err.params()["journal"].as_str().unwrap().contains("plantage simulé"), "{:?}", err.params());
}

#[tokio::test]
async fn executable_absent() {
    let t = tempfile::tempdir().unwrap();
    let sup = Supervisor::new().unwrap();
    let s = ServiceSpec::new("x", t.path().join("absent.exe"), t.path().join("x.log"));
    assert_eq!(sup.start(s).await.unwrap_err().code(), "service.executable");
}

#[tokio::test]
async fn arret_propre_par_evenement_nomme() {
    let t = tempfile::tempdir().unwrap();
    let sup = Supervisor::new().unwrap();
    let prefixe = format!("RustservTest{}_", std::process::id());
    let mut s = spec("db", t.path(), &["--event", &prefixe]);
    s.stop = StopStrategy::NamedEvent(format!("{prefixe}{{pid}}"));
    sup.start(s.clone()).await.unwrap();
    sup.stop("db").await.unwrap();
    assert!(logs::tail(&s.log_file, 5).contains("arrêt propre"), "le service a été tué au lieu d'être arrêté proprement");
}

#[tokio::test]
async fn relance_automatique_apres_plantage() {
    let t = tempfile::tempdir().unwrap();
    let sup = Supervisor::new().unwrap();
    let mut s = spec("php", t.path(), &["--exit-after-ms", "800"]);
    s.restart = true;
    let mut rx = sup.subscribe();
    sup.start(s).await.unwrap();
    let ServiceState::EnCours { pid: pid1, .. } = sup.state("php") else { panic!() };

    // On attend : plantage → Demarrage → EnCours avec un nouveau PID.
    let pid2 = tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            let ev = rx.recv().await.unwrap();
            if let ServiceState::EnCours { pid, .. } = ev.state {
                if pid != pid1 {
                    return pid;
                }
            }
        }
    })
    .await
    .expect("pas de relance automatique");
    assert_ne!(pid1, pid2);
    sup.stop("php").await.unwrap();
}

#[tokio::test]
async fn abandonne_apres_trop_de_plantages() {
    let t = tempfile::tempdir().unwrap();
    let sup = Supervisor::new().unwrap();
    let mut s = spec("php", t.path(), &["--exit-after-ms", "600"]);
    s.restart = false;
    sup.start(s).await.unwrap();
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let ServiceState::Erreur { erreur } = sup.state("php") else { panic!("{:?}", sup.state("php")) };
    assert_eq!(erreur["code"], "service.plantage");
    // Un nouveau démarrage est possible après une erreur.
    assert!(sup.start(spec("php", t.path(), &[])).await.is_ok());
    sup.stop_all().await;
}

/// Fermer le Job (ex. plantage de Rustserv) tue aussi les sous-processus.
#[tokio::test]
async fn le_job_tue_les_sous_processus() {
    use rustserv_core::platform::{spawn_in_job, JobObject};
    let t = tempfile::tempdir().unwrap();
    let log = t.path().join("job.log");
    let job = JobObject::new().unwrap();
    let f = std::fs::File::create(&log).unwrap();
    let mut cmd = tokio::process::Command::new(FAKE);
    cmd.arg("--spawn-child").stdout(f);
    let mut child = spawn_in_job(&job, &mut cmd).unwrap();
    let parent = child.id().unwrap();

    let mut enfant = None;
    for _ in 0..50 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        if let Some(l) = std::fs::read_to_string(&log).unwrap().lines().find(|l| l.starts_with("ENFANT ")) {
            enfant = l[7..].trim().parse::<u32>().ok();
            break;
        }
    }
    let enfant = enfant.expect("le sous-processus n'a pas été lancé");

    drop(job); // équivalent d'un arrêt brutal de Rustserv
    assert!(est_mort(parent), "le processus principal survit au Job");
    assert!(est_mort(enfant), "le sous-processus survit au Job");
    let _ = child.kill().await;
}
