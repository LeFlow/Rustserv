//! # rustserv-core
//!
//! Toute la logique métier de Rustserv vit ici, sans aucune dépendance à Tauri,
//! afin de pouvoir la tester avec `cargo test` et de la réutiliser ailleurs
//! (CLI, autre interface, autre OS plus tard).
//!
//! Le code spécifique à Windows est isolé dans le module [`platform`].

pub mod catalog;
pub mod db;
pub mod compat;
pub mod conversion;
pub mod error;
pub mod extensions_ops;
pub mod hosts;
pub mod migration;
pub mod orchestrator;
pub mod paths;
pub mod php;
pub mod platform;
pub mod process;
pub mod relocation;
pub mod services;
pub mod settings;
pub mod sites;
pub mod site_type;
pub mod ssl;
pub mod templates;
pub mod terminal;

pub use error::{Error, Result};
pub use paths::Paths;
pub use settings::Settings;
