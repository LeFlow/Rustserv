//! rustserv-helper.exe
//!
//! Petit programme séparé, lancé **uniquement quand c'est nécessaire** avec
//! élévation UAC (« Exécuter en tant qu'administrateur »). L'application
//! principale ne tourne jamais en administrateur.
//!
//! Il n'accepte qu'une liste fermée d'opérations (pas de commande arbitraire) :
//! - `hosts-sync <demande.txt>` : remplace le bloc `# BEGIN Rustserv … # END Rustserv`
//!   du fichier hosts par les domaines listés (un par ligne, validés strictement).
//!
//! En cas d'échec, le message est écrit dans `<demande.txt>.err` (une application
//! non élevée ne peut pas lire la sortie d'un processus élevé), code de sortie 1.

// Pas de fenêtre de console.
#![windows_subsystem = "windows"]

use rustserv_helper::{hosts_path, sync_hosts_file};
use std::path::PathBuf;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["--version"] => 0,
        ["hosts-sync", demande] => {
            let demande = PathBuf::from(demande);
            let err = demande.with_extension("err");
            let _ = std::fs::remove_file(&err);
            match sync_hosts_file(&hosts_path(), &demande) {
                Ok(()) => 0,
                Err(e) => {
                    let _ = std::fs::write(&err, e);
                    1
                }
            }
        }
        _ => 2,
    };
    std::process::exit(code);
}
