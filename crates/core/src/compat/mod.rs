//! Outils de rétrocompatibilité (phase 5) :
//! - [`runner`] : scripts et suites de tests sur plusieurs versions de PHP en parallèle ;
//! - [`tools`] : installation de PHP_CodeSniffer/PHPCompatibility et PHPStan ;
//! - [`analysis`] : analyses statiques et rapports ;
//! - [`ops`] : opérations complètes appelées par l'interface.

use crate::error::{Error, Result};
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// Attend `fut`, en vérifiant toutes les 200 ms si l'utilisateur a demandé l'annulation.
/// En cas d'annulation, `fut` est abandonné : les processus lancés avec `kill_on_drop`
/// (PHPUnit, phpcs, PHPStan…) sont alors **réellement arrêtés**.
pub async fn ou_annuler<T>(fut: impl Future<Output = T>, cancel: &AtomicBool) -> Result<T> {
    tokio::pin!(fut);
    loop {
        tokio::select! {
            r = &mut fut => return Ok(r),
            _ = tokio::time::sleep(Duration::from_millis(200)) => {
                if cancel.load(Ordering::Relaxed) {
                    return Err(Error::Annule);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[tokio::test]
    async fn annulation_rapide() {
        let flag = Arc::new(AtomicBool::new(false));
        let f2 = flag.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(300)).await;
            f2.store(true, Ordering::Relaxed);
        });
        let debut = std::time::Instant::now();
        let r = ou_annuler(tokio::time::sleep(Duration::from_secs(30)), &flag).await;
        assert_eq!(r.unwrap_err().code(), "annule");
        assert!(debut.elapsed() < Duration::from_secs(2));
        // Sans annulation, le résultat est rendu normalement.
        assert_eq!(ou_annuler(async { 42 }, &AtomicBool::new(false)).await.unwrap(), 42);
    }
}

pub mod analysis;
pub mod ops;
pub mod runner;
pub mod tools;
