//! Import de sites depuis Laragon ou WAMP :
//! - [`detect`] : sources, sites, bases (lecture seule) ;
//! - [`run`] : copie des fichiers, export/import des bases, adaptation des
//!   configurations, conversion des adresses WordPress (WP-CLI).

pub mod detect;
pub mod run;
