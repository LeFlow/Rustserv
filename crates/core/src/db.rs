//! Moteurs de bases de données : MariaDB, MySQL et PostgreSQL.
//!
//! Les trois fonctionnent de la même façon dans Rustserv : choix par **branche**
//! (« 11.4 », « 8.4 », « 17 »), un dossier de données par branche
//! (`data/<moteur>-<branche>`), une instance et un port par branche. L'instance **par
//! défaut** d'un moteur utilise le port des réglages (3306, 3307, 5432) ; les autres un
//! port calculé (voir [`Moteur::port_secondaire`]).

use crate::catalog::version;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Moteur {
    Mariadb,
    Mysql,
    Postgresql,
}

impl Moteur {
    pub const TOUS: [Moteur; 3] = [Moteur::Mariadb, Moteur::Mysql, Moteur::Postgresql];

    /// Identifiant du paquet dans le catalogue (et nom du service par défaut).
    pub fn paquet(self) -> &'static str {
        match self {
            Moteur::Mariadb => "mariadb",
            Moteur::Mysql => "mysql",
            Moteur::Postgresql => "postgresql",
        }
    }

    pub fn depuis_paquet(p: &str) -> Option<Self> {
        Self::TOUS.into_iter().find(|m| m.paquet() == p)
    }

    /// MariaDB et MySQL partagent outils (mysqldump, client) et format SQL :
    /// une base peut passer de l'un à l'autre. Pas avec PostgreSQL.
    pub fn famille_mysql(self) -> bool {
        self != Moteur::Postgresql
    }

    /// Branche d'une version : « 11.4 » pour MariaDB/MySQL, la majeure « 17 » pour
    /// PostgreSQL (depuis la version 10, seule la majeure change le format des données).
    pub fn branche(self, v: &str) -> String {
        match self {
            Moteur::Postgresql => version::parse(v).0.first().map(|n| n.to_string()).unwrap_or_default(),
            _ => version::branch(v),
        }
    }

    /// Port d'une instance **non par défaut** :
    /// MariaDB 3MMmm (11.4 → 31104), MySQL 4MMmm (8.0 → 40800), PostgreSQL 5MM00 (16 → 51600).
    pub fn port_secondaire(self, v: &str) -> u16 {
        let (n, _) = version::parse(v);
        let maj = n.first().copied().unwrap_or(0).min(99);
        let min = if self == Moteur::Postgresql { 0 } else { n.get(1).copied().unwrap_or(0).min(99) };
        let base = match self {
            Moteur::Mariadb => 30000,
            Moteur::Mysql => 40000,
            Moteur::Postgresql => 50000,
        };
        (base + maj * 100 + min) as u16
    }

    /// Le dossier de données a-t-il déjà été initialisé ?
    pub fn donnees_initialisees(self, datadir: &Path) -> bool {
        match self {
            Moteur::Postgresql => datadir.join("PG_VERSION").is_file(),
            _ => datadir.join("mysql").is_dir(),
        }
    }

    /// Nom affiché.
    pub fn nom(self) -> &'static str {
        match self {
            Moteur::Mariadb => "MariaDB",
            Moteur::Mysql => "MySQL",
            Moteur::Postgresql => "PostgreSQL",
        }
    }
}

/// Base de données choisie pour un site : moteur + branche.
/// Enregistrée sous la forme « mysql:8.4 » ; « 11.4 » seul (anciens réglages) = MariaDB.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChoixBase {
    pub moteur: Moteur,
    pub branche: String,
}

impl ChoixBase {
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        let (moteur, branche) = match s.split_once(':') {
            Some((m, b)) => (Moteur::depuis_paquet(m)?, b),
            None => (Moteur::Mariadb, s),
        };
        (!branche.is_empty()).then(|| Self { branche: moteur.branche(branche), moteur })
    }

    pub fn texte(&self) -> String {
        format!("{}:{}", self.moteur.paquet(), self.branche)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn branches_et_ports() {
        assert_eq!(Moteur::Postgresql.branche("17.12"), "17");
        assert_eq!(Moteur::Mysql.branche("8.4.13"), "8.4");
        assert_eq!(Moteur::Mariadb.port_secondaire("11.4.14"), 31104);
        assert_eq!(Moteur::Mariadb.port_secondaire("10.11.15"), 31011);
        assert_eq!(Moteur::Mysql.port_secondaire("8.0.46"), 40800);
        assert_eq!(Moteur::Mysql.port_secondaire("9.6.0"), 40906);
        assert_eq!(Moteur::Postgresql.port_secondaire("16.16"), 51600);
    }

    #[test]
    fn choix_de_base() {
        assert_eq!(ChoixBase::parse("11.4"), Some(ChoixBase { moteur: Moteur::Mariadb, branche: "11.4".into() }), "anciens réglages");
        let c = ChoixBase::parse("postgresql:17.12").unwrap();
        assert_eq!((c.moteur, c.branche.as_str()), (Moteur::Postgresql, "17"));
        assert_eq!(ChoixBase::parse("mysql:8.4").unwrap().texte(), "mysql:8.4");
        assert_eq!(ChoixBase::parse("oracle:23"), None);
        assert_eq!(ChoixBase::parse(""), None);
    }
}
