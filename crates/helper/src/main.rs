//! rustserv-helper.exe
//!
//! Petit programme séparé, lancé **uniquement quand c'est nécessaire** avec
//! élévation UAC (« Exécuter en tant qu'administrateur »). L'application
//! principale ne tourne jamais en administrateur.
//!
//! Il n'accepte qu'une liste fermée d'opérations (pas de commande arbitraire) :
//! - `hosts-sync <fichier>` : remplace le bloc `# BEGIN Rustserv … # END Rustserv`
//!   du fichier hosts par le contenu fourni (phase 4).

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--version") => println!("rustserv-helper {}", env!("CARGO_PKG_VERSION")),
        _ => {
            eprintln!("usage : rustserv-helper <opération> [arguments]");
            std::process::exit(2);
        }
    }
}
