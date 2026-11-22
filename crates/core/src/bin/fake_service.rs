//! Faux service utilisé **uniquement par les tests** du superviseur.
//!
//! Options (combinables) :
//! - `--port N`          écoute en TCP sur 127.0.0.1:N
//! - `--exit-after-ms N` se termine avec le code 3 après N ms (simule un plantage)
//! - `--event PREFIXE`   crée l'événement Windows « PREFIXE<pid> » et s'arrête
//!                       proprement (code 0) quand il est déclenché
//! - `--spawn-child`     lance un sous-processus et affiche « ENFANT <pid> »
//! - `--echo TEXTE`      écrit TEXTE sur la sortie standard au démarrage

#![allow(unsafe_code)]

use std::io::Write;
use std::time::Duration;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let val = |flag: &str| args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).cloned();

    if let Some(t) = val("--echo") {
        println!("{t}");
    }
    if args.iter().any(|a| a == "--spawn-child") {
        let exe = std::env::current_exe().unwrap();
        let c = std::process::Command::new(exe).arg("--echo").arg("enfant").spawn().unwrap();
        println!("ENFANT {}", c.id());
        std::mem::forget(c);
    }
    std::io::stdout().flush().ok();

    if let Some(p) = val("--port") {
        let l = std::net::TcpListener::bind(("127.0.0.1", p.parse::<u16>().unwrap())).unwrap();
        std::thread::spawn(move || for _ in l.incoming() {});
    }
    if let Some(ms) = val("--exit-after-ms") {
        let ms: u64 = ms.parse().unwrap();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(ms));
            eprintln!("plantage simulé");
            std::process::exit(3);
        });
    }
    if let Some(prefix) = val("--event") {
        wait_event(&format!("{prefix}{}", std::process::id()));
        println!("arrêt propre");
        std::process::exit(0);
    }
    loop {
        std::thread::sleep(Duration::from_secs(3600));
    }
}

#[cfg(windows)]
fn wait_event(name: &str) {
    use windows_sys::Win32::System::Threading::{CreateEventW, WaitForSingleObject, INFINITE};
    let w: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        let h = CreateEventW(std::ptr::null(), 1, 0, w.as_ptr());
        assert!(!h.is_null(), "CreateEventW");
        WaitForSingleObject(h, INFINITE);
    }
}

#[cfg(not(windows))]
fn wait_event(_: &str) {
    loop {
        std::thread::sleep(Duration::from_secs(3600));
    }
}
