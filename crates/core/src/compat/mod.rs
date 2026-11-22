//! Outils de rétrocompatibilité (phase 5) :
//! - [`runner`] : scripts et suites de tests sur plusieurs versions de PHP en parallèle ;
//! - [`tools`] : installation de PHP_CodeSniffer/PHPCompatibility et PHPStan ;
//! - [`analysis`] : analyses statiques et rapports ;
//! - [`ops`] : opérations complètes appelées par l'interface.

pub mod analysis;
pub mod ops;
pub mod runner;
pub mod tools;
