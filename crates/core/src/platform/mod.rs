//! Code dépendant du système d'exploitation.
//!
//! Le reste du noyau n'appelle que les fonctions exposées ici. Pour porter
//! Rustserv sur un autre OS, il suffira d'ajouter un module `unix` qui fournit
//! les mêmes fonctions.

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use self::windows::*;
