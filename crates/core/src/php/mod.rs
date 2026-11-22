//! PHP : php.ini, préparation des versions installées, validation, mises à jour,
//! extensions (intégrées, DLL, PECL) et profils.

pub mod check;
pub mod extensions;
pub mod ini;
pub mod pecl;
pub mod profiles;
pub mod setup;
pub mod transaction;
pub mod upgrade;

pub use check::{check, PhpCheck};
pub use setup::{ensure_php_ini, php_ini_path, pool_ports};
pub use upgrade::{copy_extra_dlls, migrate_ini, MigrationReport};
