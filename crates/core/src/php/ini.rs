//! Éditeur de php.ini **fidèle** : modifie uniquement les lignes nécessaires et
//! conserve commentaires, ordre, sections et fins de ligne (CRLF/LF).
//!
//! Règles importantes :
//! - PHP garde la **dernière** valeur active d'une directive : `get` fait de même.
//! - Une ligne commentée n'est considérée comme une directive « réactivable »
//!   que si le `;` est **collé** au nom (`;extension=curl`, `;date.timezone =`).
//!   Les exemples de documentation (`;   extension=/path/to/mysqli.so`)
//!   commencent par des espaces : on ne les touche jamais.

/// Fichier php.ini chargé en mémoire.
#[derive(Debug, Clone)]
pub struct IniFile {
    lines: Vec<String>,
    eol: &'static str,
    final_newline: bool,
}

/// Une ligne de directive (active ou commentée réactivable).
#[derive(Debug, Clone, PartialEq, Eq)]
struct Directive {
    key: String,
    value: String,
    active: bool,
}

/// Extension trouvée dans le php.ini.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionLine {
    /// Nom normalisé (`curl`, `pdo_mysql`, `opcache`…).
    pub name: String,
    /// `zend_extension=` (opcache, xdebug) plutôt que `extension=`.
    pub zend: bool,
    pub active: bool,
    /// Index de ligne (0-based).
    pub line: usize,
}

fn is_key_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-')
}

fn parse_directive(line: &str) -> Option<Directive> {
    let t = line.trim_start();
    let (active, body) = match t.strip_prefix(';') {
        // Commentée : le nom doit suivre immédiatement le « ; ».
        Some(rest) => {
            if !rest.starts_with(|c: char| c.is_ascii_alphabetic()) {
                return None;
            }
            (false, rest)
        }
        None => (true, t),
    };
    let eq = body.find('=')?;
    let key = body[..eq].trim();
    if key.is_empty() || !key.chars().all(is_key_char) {
        return None;
    }
    Some(Directive { key: key.to_string(), value: body[eq + 1..].trim().to_string(), active })
}

/// Valeur « propre » : sans guillemets ni commentaire de fin de ligne.
pub fn clean_value(raw: &str) -> String {
    let raw = raw.trim();
    if let Some(rest) = raw.strip_prefix('"') {
        if let Some(end) = rest.find('"') {
            return rest[..end].to_string();
        }
    }
    raw.split(';').next().unwrap_or("").trim().to_string()
}

/// Nom normalisé d'une extension à partir de la valeur (`php_curl.dll`, `"C:\x\php_gd.dll"`, `curl`).
pub fn extension_name(value: &str) -> String {
    let v = clean_value(value);
    let file = v.rsplit(['/', '\\']).next().unwrap_or(&v);
    let file = file.strip_suffix(".dll").or_else(|| file.strip_suffix(".so")).unwrap_or(file);
    let file = file.strip_prefix("php_").unwrap_or(file);
    file.to_ascii_lowercase()
}

/// Entoure une valeur de guillemets (chemins Windows avec espaces).
pub fn quote(v: &str) -> String {
    format!("\"{v}\"")
}

impl IniFile {
    pub fn parse(text: &str) -> Self {
        let eol = if text.contains("\r\n") { "\r\n" } else { "\n" };
        let final_newline = text.ends_with('\n');
        let lines = text.lines().map(|l| l.trim_end_matches('\r').to_string()).collect();
        Self { lines, eol, final_newline }
    }

    pub fn to_text(&self) -> String {
        let mut s = self.lines.join(self.eol);
        if self.final_newline || self.lines.is_empty() {
            s.push_str(self.eol);
        }
        s
    }

    pub fn lines(&self) -> &[String] {
        &self.lines
    }

    fn directives(&self) -> impl Iterator<Item = (usize, Directive)> + '_ {
        self.lines.iter().enumerate().filter_map(|(i, l)| parse_directive(l).map(|d| (i, d)))
    }

    /// Valeur active de `key` (la dernière, comme PHP), nettoyée.
    pub fn get(&self, key: &str) -> Option<String> {
        self.directives().filter(|(_, d)| d.active && d.key.eq_ignore_ascii_case(key)).last().map(|(_, d)| clean_value(&d.value))
    }

    /// Définit `key = value` :
    /// 1. remplace la dernière occurrence active (et commente les doublons) ;
    /// 2. sinon réactive la dernière occurrence commentée ;
    /// 3. sinon ajoute la ligne en fin de fichier.
    pub fn set(&mut self, key: &str, value: &str) {
        let new_line = format!("{key} = {value}");
        let actives: Vec<usize> = self.directives().filter(|(_, d)| d.active && d.key.eq_ignore_ascii_case(key)).map(|(i, _)| i).collect();
        if let Some((&last, others)) = actives.split_last() {
            for &i in others {
                self.lines[i] = format!(";{}", self.lines[i].trim_start());
            }
            self.lines[last] = new_line;
            return;
        }
        if let Some((i, _)) = self.directives().filter(|(_, d)| !d.active && d.key.eq_ignore_ascii_case(key)).last() {
            self.lines[i] = new_line;
            return;
        }
        self.lines.push(new_line);
    }

    /// Toutes les lignes d'extension (actives et commentées réactivables).
    pub fn extensions(&self) -> Vec<ExtensionLine> {
        self.directives()
            .filter_map(|(i, d)| {
                let zend = d.key.eq_ignore_ascii_case("zend_extension");
                if !zend && !d.key.eq_ignore_ascii_case("extension") {
                    return None;
                }
                Some(ExtensionLine { name: extension_name(&d.value), zend, active: d.active, line: i })
            })
            .collect()
    }

    pub fn is_extension_enabled(&self, name: &str) -> bool {
        let n = name.to_ascii_lowercase();
        self.extensions().iter().any(|e| e.active && e.name == n)
    }

    /// Active une extension. Renvoie `true` si le fichier a changé.
    pub fn enable_extension(&mut self, name: &str, zend: bool) -> bool {
        let n = name.to_ascii_lowercase();
        let exts = self.extensions();
        if exts.iter().any(|e| e.active && e.name == n) {
            return false;
        }
        if let Some(e) = exts.iter().rev().find(|e| !e.active && e.name == n) {
            // On garde le nom tel qu'écrit dans le fichier, seul le « ; » disparaît.
            let l = &self.lines[e.line];
            self.lines[e.line] = l.trim_start().trim_start_matches(';').to_string();
            return true;
        }
        let key = if zend { "zend_extension" } else { "extension" };
        let line = format!("{key}={n}");
        match exts.last() {
            Some(last) => self.lines.insert(last.line + 1, line),
            None => self.lines.push(line),
        }
        true
    }

    /// Garantit que la ligne active de `dep` est placée **avant** celle de `ext`
    /// (ordre de chargement : ex. mbstring avant exif). Renvoie `true` si une ligne a bougé.
    pub fn ensure_loaded_before(&mut self, dep: &str, ext: &str) -> bool {
        let exts = self.extensions();
        let pos = |n: &str| exts.iter().find(|e| e.active && e.name == n).map(|e| e.line);
        let (Some(d), Some(e)) = (pos(&dep.to_ascii_lowercase()), pos(&ext.to_ascii_lowercase())) else { return false };
        if d < e {
            return false;
        }
        let line = self.lines.remove(d);
        self.lines.insert(e, line);
        true
    }

    /// La directive est-elle définie (ligne active) ?
    pub fn is_set(&self, key: &str) -> bool {
        self.get(key).is_some()
    }

    /// Désactive une extension (commente toutes ses lignes actives). Renvoie `true` si changé.
    pub fn disable_extension(&mut self, name: &str) -> bool {
        let n = name.to_ascii_lowercase();
        let mut changed = false;
        for e in self.extensions().into_iter().filter(|e| e.active && e.name == n) {
            self.lines[e.line] = format!(";{}", self.lines[e.line].trim_start());
            changed = true;
        }
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Extrait représentatif de php.ini-development (fins de ligne CRLF).
    const SAMPLE: &str = "[PHP]\r\n\
; Notes for Windows environments :\r\n\
;   extension=/path/to/extension/mysqli.so\r\n\
;   Default Value: E_ALL\r\n\
memory_limit = 128M\r\n\
error_reporting = E_ALL\r\n\
;extension_dir = \"ext\"\r\n\
;extension=bz2\r\n\
;extension=curl\r\n\
;extension=mbstring\r\n\
;extension=exif      ; Must be after mbstring as it depends on it\r\n\
;extension=mysqli\r\n\
;zend_extension=opcache\r\n\
\r\n\
[Date]\r\n\
;date.timezone =\r\n";

    #[test]
    fn aller_retour_identique() {
        assert_eq!(IniFile::parse(SAMPLE).to_text(), SAMPLE);
        let lf = "a = 1\nb = 2";
        assert_eq!(IniFile::parse(lf).to_text(), lf, "pas de saut de ligne final ajouté");
    }

    #[test]
    fn lecture_des_valeurs() {
        let ini = IniFile::parse(SAMPLE);
        assert_eq!(ini.get("memory_limit").as_deref(), Some("128M"));
        assert_eq!(ini.get("date.timezone"), None, "une ligne commentée n'est pas active");
        let ini = IniFile::parse("a = 1\na = \"deux ; trois\"\nb = x ; commentaire\n");
        assert_eq!(ini.get("a").as_deref(), Some("deux ; trois"), "dernière valeur, guillemets retirés");
        assert_eq!(ini.get("b").as_deref(), Some("x"), "commentaire de fin retiré");
    }

    #[test]
    fn modification_en_place() {
        let mut ini = IniFile::parse(SAMPLE);
        ini.set("memory_limit", "512M");
        ini.set("date.timezone", "Europe/Paris");
        ini.set("extension_dir", &quote(r"C:\rustserv\bin\php\php-8.3.37\ext"));
        ini.set("nouvelle.directive", "On");
        let txt = ini.to_text();
        assert!(txt.contains("memory_limit = 512M\r\n"));
        assert!(txt.contains("[Date]\r\ndate.timezone = Europe/Paris\r\n"), "réactivée à sa place d'origine");
        assert!(txt.contains("extension_dir = \"C:\\rustserv\\bin\\php\\php-8.3.37\\ext\""));
        assert!(txt.ends_with("nouvelle.directive = On\r\n"));
        assert_eq!(ini.lines().len(), IniFile::parse(SAMPLE).lines().len() + 1);
    }

    #[test]
    fn doublons_actifs() {
        let mut ini = IniFile::parse("x = 1\nx = 2\n");
        ini.set("x", "3");
        assert_eq!(ini.to_text(), ";x = 1\nx = 3\n");
    }

    #[test]
    fn noms_d_extension() {
        assert_eq!(extension_name("curl"), "curl");
        assert_eq!(extension_name("php_pdo_mysql.dll"), "pdo_mysql");
        assert_eq!(extension_name(r#""C:\php\ext\php_xdebug.dll""#), "xdebug");
        assert_eq!(extension_name("exif      ; Must be after mbstring"), "exif");
        assert_eq!(extension_name("/usr/lib/redis.so"), "redis");
    }

    #[test]
    fn active_et_desactive_les_extensions() {
        let mut ini = IniFile::parse(SAMPLE);
        assert!(!ini.is_extension_enabled("curl"));
        assert!(ini.enable_extension("curl", false));
        assert!(!ini.enable_extension("curl", false), "déjà active : rien ne change");
        assert!(ini.enable_extension("opcache", true));
        assert!(ini.is_extension_enabled("curl") && ini.is_extension_enabled("opcache"));
        let txt = ini.to_text();
        assert!(txt.contains("\r\nextension=curl\r\n"));
        assert!(txt.contains("\r\nzend_extension=opcache\r\n"));
        // L'exemple de documentation n'est jamais touché.
        assert!(txt.contains(";   extension=/path/to/extension/mysqli.so"));
        assert!(!ini.is_extension_enabled("mysqli"));

        // Le commentaire de fin de ligne est conservé.
        ini.enable_extension("exif", false);
        assert!(ini.to_text().contains("\r\nextension=exif      ; Must be after mbstring"));

        assert!(ini.disable_extension("curl"));
        assert!(!ini.is_extension_enabled("curl"));
        assert!(ini.to_text().contains("\r\n;extension=curl\r\n"));
    }

    #[test]
    fn ordre_de_chargement() {
        let mut ini = IniFile::parse("extension=exif\nextension=curl\nextension=mbstring\n");
        assert!(ini.ensure_loaded_before("mbstring", "exif"));
        assert_eq!(ini.to_text(), "extension=mbstring\nextension=exif\nextension=curl\n");
        assert!(!ini.ensure_loaded_before("mbstring", "exif"), "déjà dans le bon ordre");
        assert!(!ini.ensure_loaded_before("absente", "exif"));
    }

    #[test]
    fn extension_absente_ajoutee_apres_les_autres() {
        let mut ini = IniFile::parse(SAMPLE);
        ini.enable_extension("redis", false);
        let lines = ini.lines();
        let i = lines.iter().position(|l| l == "extension=redis").expect("ligne ajoutée");
        assert_eq!(lines[i - 1], ";zend_extension=opcache", "insérée juste après la dernière ligne d'extension");
    }
}
