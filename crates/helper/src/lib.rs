//! Logique du fichier hosts, partagée entre :
//! - `rustserv-helper.exe` (lancé en administrateur, il **écrit** le fichier) ;
//! - l'application (qui le **lit** pour savoir si une mise à jour est nécessaire).
//!
//! Sécurité : le helper s'exécute avec des droits élevés et lit une liste de
//! domaines dans un fichier fourni par l'application. Il n'accepte donc que des
//! noms strictement valides, dans des domaines **réservés au réseau local**
//! (`.test`, `.localhost`…), toujours redirigés vers 127.0.0.1. Il est impossible
//! de s'en servir pour détourner un vrai site (ex. google.com).
//!
//! Aucune dépendance externe : le helper reste minuscule.

/// Balises du bloc géré par Rustserv (ASCII uniquement : certains outils lisent mal l'UTF-8).
pub const BEGIN: &str = "# BEGIN Rustserv (bloc gere automatiquement, ne pas modifier)";
pub const END: &str = "# END Rustserv";

/// Domaines de premier niveau autorisés : réservés, jamais attribués sur Internet.
pub const TLD_AUTORISES: &[&str] = &["test", "localhost", "local", "internal", "lan"];

/// Nombre maximal de domaines dans le bloc (garde-fou).
pub const MAX_DOMAINES: usize = 2000;

/// Chemin du fichier hosts de Windows.
pub fn hosts_path() -> std::path::PathBuf {
    let root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
    std::path::PathBuf::from(root).join("System32").join("drivers").join("etc").join("hosts")
}

/// Un nom est-il acceptable dans le bloc ? (`monsite.test`, `blog.mon-site.test`)
pub fn valid_host(h: &str) -> bool {
    if h.is_empty() || h.len() > 253 {
        return false;
    }
    let labels: Vec<&str> = h.split('.').collect();
    if labels.len() < 2 {
        return false;
    }
    let tld = labels[labels.len() - 1];
    if !TLD_AUTORISES.contains(&tld) {
        return false;
    }
    labels.iter().all(|l| {
        !l.is_empty()
            && l.len() <= 63
            && l.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
            && !l.starts_with('-')
            && !l.ends_with('-')
    })
}

/// Domaines actuellement présents dans le bloc Rustserv.
pub fn read_block(text: &str) -> Vec<String> {
    let mut dans = false;
    let mut out = Vec::new();
    for l in text.lines() {
        let t = l.trim();
        if t == BEGIN {
            dans = true;
        } else if t == END {
            dans = false;
        } else if dans && !t.starts_with('#') {
            out.extend(t.split_whitespace().skip(1).map(str::to_string));
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Remplace (ou ajoute) le bloc Rustserv ; le reste du fichier est conservé à l'identique
/// (BOM, fins de ligne, entrées d'autres logiciels). Une liste vide retire le bloc.
pub fn replace_block(text: &str, hosts: &[String]) -> Result<String, String> {
    if hosts.len() > MAX_DOMAINES {
        return Err(format!("trop de domaines ({})", hosts.len()));
    }
    for h in hosts {
        if !valid_host(h) {
            return Err(format!("domaine refusé : « {h} »"));
        }
    }
    let (bom, corps) = match text.strip_prefix('\u{feff}') {
        Some(rest) => ("\u{feff}", rest),
        None => ("", text),
    };
    let eol = if corps.contains("\r\n") || corps.is_empty() { "\r\n" } else { "\n" };

    // Lignes hors bloc.
    let mut garde: Vec<&str> = Vec::new();
    let mut dans = false;
    for l in corps.lines() {
        match l.trim() {
            t if t == BEGIN => dans = true,
            t if t == END => dans = false,
            _ if dans => {}
            _ => garde.push(l),
        }
    }
    while garde.last().is_some_and(|l| l.trim().is_empty()) {
        garde.pop();
    }

    let mut out = String::from(bom);
    out.push_str(&garde.join(eol));
    if !garde.is_empty() {
        out.push_str(eol);
    }
    let mut liste: Vec<&String> = hosts.iter().collect();
    liste.sort();
    liste.dedup();
    if !liste.is_empty() {
        out.push_str(eol);
        out.push_str(BEGIN);
        out.push_str(eol);
        for h in liste {
            out.push_str(&format!("127.0.0.1\t{h}{eol}"));
        }
        out.push_str(END);
        out.push_str(eol);
    }
    Ok(out)
}

/// Lit la demande (un domaine par ligne) et met à jour le fichier hosts donné.
/// Écrit d'abord une copie de sécurité `hosts.rustserv.bak`.
pub fn sync_hosts_file(hosts: &std::path::Path, request: &std::path::Path) -> Result<(), String> {
    let demande = std::fs::read_to_string(request).map_err(|e| format!("lecture de la demande : {e}"))?;
    let noms: Vec<String> = demande.lines().map(|l| l.trim().to_ascii_lowercase()).filter(|l| !l.is_empty()).collect();
    let actuel = std::fs::read(hosts).map_err(|e| format!("lecture du fichier hosts : {e}"))?;
    let texte = String::from_utf8_lossy(&actuel).into_owned();
    let nouveau = replace_block(&texte, &noms)?;
    if nouveau == texte {
        return Ok(());
    }
    let bak = hosts.with_file_name("hosts.rustserv.bak");
    std::fs::write(&bak, &actuel).map_err(|e| format!("copie de sécurité : {e}"))?;
    std::fs::write(hosts, nouveau.as_bytes()).map_err(|e| format!("écriture du fichier hosts : {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXISTANT: &str = "\u{feff}# Copyright (c) Microsoft\r\n127.0.0.1 localhost\r\n127.0.0.1 agence-web.test #laragon magic!\r\n";

    #[test]
    fn noms_valides_et_refuses() {
        for ok in ["demo.test", "mon-site.test", "a.b.test", "x.localhost"] {
            assert!(valid_host(ok), "{ok}");
        }
        for ko in ["google.com", "test", "demo.dev", "Demo.test", "-a.test", "a..test", "a b.test", "a.test\n127.0.0.1 x.com", ""] {
            assert!(!valid_host(ko), "{ko:?}");
        }
    }

    #[test]
    fn ajoute_remplace_et_retire_le_bloc() {
        let v = replace_block(EXISTANT, &["demo.test".into(), "test.test".into()]).unwrap();
        assert!(v.starts_with("\u{feff}# Copyright"), "BOM conservé");
        assert!(v.contains("agence-web.test #laragon magic!\r\n"), "entrées tierces conservées");
        assert!(v.contains(&format!("{BEGIN}\r\n127.0.0.1\tdemo.test\r\n127.0.0.1\ttest.test\r\n{END}\r\n")));
        assert_eq!(read_block(&v), ["demo.test", "test.test"]);

        let v2 = replace_block(&v, &["autre.test".into()]).unwrap();
        assert_eq!(read_block(&v2), ["autre.test"]);
        assert_eq!(v2.matches(BEGIN).count(), 1, "un seul bloc");

        let v3 = replace_block(&v2, &[]).unwrap();
        assert!(!v3.contains(BEGIN));
        assert_eq!(v3, EXISTANT, "retour exact à l'état d'origine");
    }

    #[test]
    fn refuse_les_domaines_dangereux() {
        assert!(replace_block(EXISTANT, &["google.com".into()]).is_err());
        assert!(replace_block(EXISTANT, &["a.test\r\n1.2.3.4 banque.fr".into()]).is_err());
    }

    #[test]
    fn synchronise_un_fichier() {
        let t = std::env::temp_dir().join(format!("rustserv-hosts-{}", std::process::id()));
        std::fs::create_dir_all(&t).unwrap();
        let (h, r) = (t.join("hosts"), t.join("demande.txt"));
        std::fs::write(&h, EXISTANT).unwrap();
        std::fs::write(&r, "demo.test\nDEMO.test\n\n").unwrap();
        sync_hosts_file(&h, &r).unwrap();
        assert_eq!(read_block(&std::fs::read_to_string(&h).unwrap()), ["demo.test"]);
        assert_eq!(std::fs::read_to_string(t.join("hosts.rustserv.bak")).unwrap(), EXISTANT);
        std::fs::write(&r, "pirate.com\n").unwrap();
        assert!(sync_hosts_file(&h, &r).is_err());
        let _ = std::fs::remove_dir_all(&t);
    }
}
