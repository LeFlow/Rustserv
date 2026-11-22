//! Lecture des journaux des services.

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// Taille maximale lue en fin de fichier (les journaux peuvent être énormes).
const MAX_TAIL: u64 = 64 * 1024;

/// Renvoie les `n` dernières lignes d'un fichier (chaîne vide si absent).
pub fn tail(path: &Path, n: usize) -> String {
    let Ok(mut f) = std::fs::File::open(path) else { return String::new() };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let start = len.saturating_sub(MAX_TAIL);
    if f.seek(SeekFrom::Start(start)).is_err() {
        return String::new();
    }
    let mut buf = Vec::new();
    let _ = f.read_to_end(&mut buf);
    // Les binaires Windows écrivent parfois en ANSI : on reste tolérant.
    let txt = String::from_utf8_lossy(&buf);
    let lines: Vec<&str> = txt.lines().filter(|l| !l.trim().is_empty()).collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dernieres_lignes() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("a.log");
        std::fs::write(&p, "1\n2\n\n3\n4\n").unwrap();
        assert_eq!(tail(&p, 2), "3\n4");
        assert_eq!(tail(&p, 10), "1\n2\n3\n4");
        assert_eq!(tail(&t.path().join("absent.log"), 3), "");
    }
}
