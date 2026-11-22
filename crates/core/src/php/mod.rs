//! PHP : php.ini, préparation des versions installées, pool php-cgi.

pub mod ini;
pub mod setup;

pub use setup::{ensure_php_ini, php_ini_path, pool_ports};
