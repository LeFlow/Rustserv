//! Gestion des processus des services (nginx, apache, mariadb, php-cgi…).
//!
//! - [`ports`] : ports libres / occupés (et par qui) ;
//! - [`logs`] : lecture de la fin des journaux ;
//! - [`supervisor`] : démarrage, surveillance, redémarrage auto, arrêt propre.

pub mod logs;
pub mod ports;
pub mod supervisor;

pub use supervisor::{ServiceSpec, ServiceState, StatusEvent, StopStrategy, Supervisor};
