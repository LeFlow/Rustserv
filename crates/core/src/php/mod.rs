//! PHP : php.ini, préparation des versions installées, validation, mises à jour.

pub mod check;
pub mod ini;
pub mod setup;
pub mod upgrade;

pub use check::{check, PhpCheck};
pub use setup::{ensure_php_ini, php_ini_path, pool_ports};
pub use upgrade::{copy_extra_dlls, migrate_ini, MigrationReport};
