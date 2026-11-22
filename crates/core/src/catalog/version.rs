//! Comparaison de numéros de version (« 8.3.9 » < « 8.3.15 »).
//!
//! Une comparaison de chaînes classerait « 8.3.9 » après « 8.3.15 » : on compare
//! donc segment par segment, numériquement. Les suffixes (RC1, beta…) sont ignorés
//! pour l'ordre mais une version avec suffixe est considérée plus ancienne
//! que la même version sans suffixe.

use std::cmp::Ordering;

/// Découpe « 8.3.15 » en `[8, 3, 15]` et indique si un suffixe non numérique existe.
pub fn parse(v: &str) -> (Vec<u64>, bool) {
    let mut nums = Vec::new();
    let mut suffixe = false;
    for seg in v.split('.') {
        let digits: String = seg.chars().take_while(|c| c.is_ascii_digit()).collect();
        if digits.len() != seg.len() {
            suffixe = true;
        }
        match digits.parse() {
            Ok(n) => nums.push(n),
            Err(_) => break,
        }
        if suffixe {
            break;
        }
    }
    (nums, suffixe)
}

/// Compare deux versions numériquement.
pub fn cmp(a: &str, b: &str) -> Ordering {
    let (na, sa) = parse(a);
    let (nb, sb) = parse(b);
    let len = na.len().max(nb.len());
    for i in 0..len {
        let x = na.get(i).copied().unwrap_or(0);
        let y = nb.get(i).copied().unwrap_or(0);
        match x.cmp(&y) {
            Ordering::Equal => {}
            o => return o,
        }
    }
    // À numéros égaux : 8.6.0RC1 < 8.6.0
    sb.cmp(&sa)
}

/// Branche « majeur.mineur » d'une version (8.3.15 → 8.3).
pub fn branch(v: &str) -> String {
    let (n, _) = parse(v);
    match n.as_slice() {
        [a, b, ..] => format!("{a}.{b}"),
        [a] => a.to_string(),
        [] => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordre_numerique() {
        assert_eq!(cmp("8.3.9", "8.3.15"), Ordering::Less);
        assert_eq!(cmp("8.10.0", "8.9.99"), Ordering::Greater);
        assert_eq!(cmp("1.30.7", "1.30.7"), Ordering::Equal);
        assert_eq!(cmp("8.3", "8.3.0"), Ordering::Equal);
    }

    #[test]
    fn suffixes() {
        assert_eq!(cmp("8.6.0RC1", "8.6.0"), Ordering::Less);
        assert_eq!(cmp("8.6.0RC1", "8.5.13"), Ordering::Greater);
    }

    #[test]
    fn branches() {
        assert_eq!(branch("8.3.37"), "8.3");
        assert_eq!(branch("12.3.4"), "12.3");
    }
}
