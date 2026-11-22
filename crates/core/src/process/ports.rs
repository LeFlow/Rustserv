//! Vérification de la disponibilité des ports.

use crate::error::{Error, Result};
use crate::platform;
use std::io::ErrorKind;

/// Vérifie qu'un port TCP est libre ; sinon, indique **qui** l'utilise.
///
/// 1. Recherche dans la table TCP de Windows (donne le PID et le nom du programme).
/// 2. Essai d'ouverture réel : détecte aussi les ports réservés par Windows
///    (plages exclues Hyper-V/WSL), qui renvoient « accès refusé ».
pub fn ensure_free(port: u16) -> Result<()> {
    if let Some(o) = platform::port_owner(port) {
        return Err(Error::PortOccupe { port, process: o.name, pid: Some(o.pid) });
    }
    match std::net::TcpListener::bind(("0.0.0.0", port)) {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == ErrorKind::PermissionDenied => Err(Error::PortReserve { port }),
        Err(e) if e.kind() == ErrorKind::AddrInUse => Err(Error::PortOccupe { port, process: None, pid: None }),
        Err(e) => Err(Error::Io(e)),
    }
}

/// Vérifie une liste de ports et renvoie la première erreur rencontrée.
pub fn ensure_all_free(ports: &[u16]) -> Result<()> {
    ports.iter().try_for_each(|&p| ensure_free(p))
}

/// Un programme écoute-t-il sur ce port ?
pub fn is_listening(port: u16) -> bool {
    platform::port_owner(port).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detecte_un_port_occupe() {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let err = ensure_free(port).unwrap_err();
        assert_eq!(err.code(), "port.occupe");
        assert_eq!(err.params()["pid"], std::process::id().to_string());
        assert!(is_listening(port));
        drop(l);
        ensure_free(port).unwrap();
        assert!(!is_listening(port));
    }
}
