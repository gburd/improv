//! Locale-aware formula entry: the separators and translated function names
//! a locale's users type, mapped onto the one canonical, locale-independent
//! formula language before parsing (`.` decimal, `,` argument separator,
//! English names, as formulas are stored).
//!
//! ```text
//! de:  WENN(Preis > 1,5; RUNDEN(Preis; 2); 0)
//!  ⇒   IF(Preis > 1.5, ROUND(Preis, 2), 0)
//! ```
//!
//! The canonical spelling is always accepted too, so a formula written in
//! the documentation works in every locale. [`Locale::localize`] goes the
//! other way, to show a stored formula in the user's spelling.
//!
//! Translations come from LibreOffice's (`locale/<tag>.tsv`, one
//! `CANONICAL<TAB>LOCAL` row per function whose name differs).

/// A formula locale.
#[derive(Debug, Clone, PartialEq)]
pub struct Locale {
    pub tag: &'static str,
    /// The decimal separator (`.` or `,`).
    pub decimal: char,
    /// The function argument separator (`,` or `;`).
    pub arg_sep: char,
    /// `(canonical, local)` function names, local names upper case.
    names: Vec<(&'static str, &'static str)>,
}

impl Locale {
    /// The canonical locale: English names, `.` decimal, `,` separator.
    pub fn en() -> Locale {
        Locale {
            tag: "en",
            decimal: '.',
            arg_sep: ',',
            names: Vec::new(),
        }
    }

    /// German (`de`): `,` decimal, `;` separator, LibreOffice's names.
    pub fn de() -> Locale {
        Locale {
            tag: "de",
            decimal: ',',
            arg_sep: ';',
            names: table(include_str!("locale/de.tsv")),
        }
    }

    /// The group (thousands) separator: the one of `,` and `.` that is not
    /// the decimal separator (`1,500.25` in English, `1.500,25` in German).
    pub fn group(&self) -> char {
        if self.decimal == ',' {
            '.'
        } else {
            ','
        }
    }

    /// The locale for a tag such as `de`, `de_DE.UTF-8` or `en-US`; English
    /// for anything unknown.
    pub fn from_tag(tag: &str) -> Locale {
        let lang = tag
            .split(['_', '-', '.', '@'])
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        match lang.as_str() {
            "de" => Locale::de(),
            _ => Locale::en(),
        }
    }

    /// The formula locale named by the environment: `IMPROV_LOCALE`, else
    /// the OS locale (`LC_ALL`, `LC_MESSAGES`, `LANG`).
    pub fn from_env() -> Locale {
        ["IMPROV_LOCALE", "LC_ALL", "LC_MESSAGES", "LANG"]
            .iter()
            .find_map(|v| std::env::var(v).ok().filter(|s| !s.is_empty()))
            .map_or_else(Locale::en, |t| Locale::from_tag(&t))
    }

    /// The canonical name a local function name means (case-insensitive).
    pub fn canonical_name(&self, local: &str) -> Option<&'static str> {
        self.names
            .iter()
            .find(|(_, l)| l.eq_ignore_ascii_case(local) || eq_upper(l, local))
            .map(|(c, _)| *c)
    }

    /// The local name of canonical function `name` (itself if untranslated).
    pub fn local_name<'a>(&self, name: &'a str) -> &'a str {
        self.names
            .iter()
            .find(|(c, _)| c.eq_ignore_ascii_case(name))
            .map_or(name, |(_, l)| l)
    }

    /// Rewrite formula text typed in this locale into the canonical
    /// language: numbers' decimal separator to `.`, the argument separator to
    /// `,`, and translated function names (a word directly before `(`) to
    /// their canonical names. Text in double quotes, quoted names in single
    /// quotes and date literals in `#…#` are left exactly as typed.
    pub fn canonicalize(&self, text: &str) -> String {
        if self.tag == "en" {
            return text.to_string();
        }
        self.rewrite(
            text,
            |w| self.canonical_name(w),
            self.decimal,
            '.',
            self.arg_sep,
            ',',
        )
    }

    /// The reverse of [`Self::canonicalize`]: canonical formula text in this
    /// locale's spelling.
    pub fn localize(&self, text: &str) -> String {
        if self.tag == "en" {
            return text.to_string();
        }
        let local = |w: &str| {
            crate::functions::by_name(w)
                .next()
                .map(|d| self.local_name(d.name))
        };
        self.rewrite(text, local, '.', self.decimal, ',', self.arg_sep)
    }

    fn rewrite<'n>(
        &self,
        text: &str,
        name: impl Fn(&str) -> Option<&'n str>,
        dec_from: char,
        dec_to: char,
        sep_from: char,
        sep_to: char,
    ) -> String {
        let chars: Vec<char> = text.chars().collect();
        let mut out = String::with_capacity(text.len());
        let mut i = 0;
        while i < chars.len() {
            let c = chars[i];
            match c {
                // Quoted text, quoted names and dates pass through untouched.
                '"' | '\'' | '#' => {
                    let close = chars[i + 1..].iter().position(|x| *x == c);
                    let end = close.map_or(chars.len(), |p| i + 1 + p + 1);
                    out.extend(&chars[i..end]);
                    i = end;
                }
                c if c.is_ascii_digit() => {
                    // A number: digits, then one decimal separator followed by
                    // a digit.
                    while i < chars.len() && chars[i].is_ascii_digit() {
                        out.push(chars[i]);
                        i += 1;
                    }
                    if i + 1 < chars.len() && chars[i] == dec_from && chars[i + 1].is_ascii_digit()
                    {
                        out.push(dec_to);
                        i += 1;
                        while i < chars.len() && chars[i].is_ascii_digit() {
                            out.push(chars[i]);
                            i += 1;
                        }
                    }
                }
                c if c.is_alphabetic() || c == '_' => {
                    let start = i;
                    while i < chars.len()
                        && (chars[i].is_alphanumeric() || chars[i] == '_' || chars[i] == '.')
                    {
                        i += 1;
                    }
                    let word: String = chars[start..i].iter().collect();
                    let mut j = i;
                    while j < chars.len() && chars[j] == ' ' {
                        j += 1;
                    }
                    let is_call = chars.get(j) == Some(&'(');
                    match name(&word).filter(|_| is_call) {
                        Some(n) => out.push_str(n),
                        None => out.push_str(&word),
                    }
                }
                c if c == sep_from => {
                    out.push(sep_to);
                    i += 1;
                }
                c => {
                    out.push(c);
                    i += 1;
                }
            }
        }
        out
    }
}

/// Case-insensitive equality that also folds non-ASCII (`KÜRZEN`/`kürzen`).
fn eq_upper(a: &str, b: &str) -> bool {
    a.chars()
        .flat_map(char::to_uppercase)
        .eq(b.chars().flat_map(char::to_uppercase))
}

fn table(tsv: &'static str) -> Vec<(&'static str, &'static str)> {
    tsv.lines()
        .skip(1)
        .filter_map(|l| l.split_once('\t'))
        .collect()
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn german_entry_round_trips() {
        let de = Locale::de();
        let typed = "WENN(Preis > 1,5; RUNDEN(Preis; 2); kürzen(Preis))";
        let canon = de.canonicalize(typed);
        assert_eq!(canon, "IF(Preis > 1.5, ROUND(Preis, 2), TRUNC(Preis))");
        assert_eq!(de.canonicalize("RUNDEN(1,5e-3; 2)"), "ROUND(1.5e-3, 2)");
        assert_eq!(de.localize("ROUND(1.5e-3, 2)"), "RUNDEN(1,5e-3; 2)");
        assert_eq!(
            de.localize(&canon),
            "WENN(Preis > 1,5; RUNDEN(Preis; 2); KÜRZEN(Preis))"
        );
        // Dotted names, and canonical names typed in a German locale.
        assert_eq!(
            de.canonicalize("UNTERGRENZE.MATHEMATIK(x; 2)"),
            "FLOOR.MATH(x, 2)"
        );
        assert_eq!(de.canonicalize("ROUND(x; 2)"), "ROUND(x, 2)");
        // Quoted text, names and dates are untouched; a measure named like a
        // function (no "(") is not translated.
        assert_eq!(
            de.canonicalize(r#"VERKETTEN("1,5; WENN("; 'Summe 1,5'; #2025-01-01#) & SUMME"#),
            r#"CONCATENATE("1,5; WENN(", 'Summe 1,5', #2025-01-01#) & SUMME"#
        );
        assert_eq!(de.local_name("MIN"), "MIN");
        assert_eq!(de.canonical_name("summe"), Some("SUM"));
    }

    #[test]
    fn english_is_identity_and_tags_resolve() {
        let en = Locale::en();
        assert_eq!(en.canonicalize("ROUND(1.5, 0)"), "ROUND(1.5, 0)");
        assert_eq!(en.localize("ROUND(1.5, 0)"), "ROUND(1.5, 0)");
        assert_eq!(Locale::from_tag("de_DE.UTF-8").tag, "de");
        assert_eq!(Locale::from_tag("de-CH").tag, "de");
        assert_eq!(Locale::from_tag("fr_FR").tag, "en");
        assert_eq!(Locale::from_tag("").tag, "en");
        assert_eq!((en.group(), Locale::de().group()), (',', '.'));
    }

    #[test]
    fn every_translation_names_a_function_or_a_planned_one() {
        let de = Locale::de();
        assert!(de.names.len() > 300);
        for name in ["SUM", "IF", "ROUND", "LEFT", "MOD", "POWER", "CEILING"] {
            assert_ne!(de.local_name(name), name, "{name} has a German name");
        }
    }
}
