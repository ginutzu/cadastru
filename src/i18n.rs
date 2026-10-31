//! Limba interfeței: română (implicit) sau engleză.
//!
//! Textele sunt scrise în cod în limba română și trec prin macroul [`tr!`](crate::tr).
//! Traducerile englezești sunt în pachetul `lang/en.tsv` (text românesc → text englez),
//! inclus în executabil la compilare. Limba se poate schimba oricând, în timpul rulării.

use std::sync::atomic::{AtomicU8, Ordering};

pub use cadastru_i18n_macros::{tr, tr_table};

/// Limbile disponibile.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Lang {
    #[default]
    #[serde(rename = "ro")]
    Ro,
    #[serde(rename = "en")]
    En,
}

impl Lang {
    /// Codul scurt afișat pe butonul de comutare.
    pub fn code(self) -> &'static str {
        match self {
            Lang::Ro => "RO",
            Lang::En => "EN",
        }
    }
    /// Numele limbii, în limba ei.
    pub fn native_name(self) -> &'static str {
        match self {
            Lang::Ro => "Română",
            Lang::En => "English",
        }
    }
    pub fn other(self) -> Lang {
        match self {
            Lang::Ro => Lang::En,
            Lang::En => Lang::Ro,
        }
    }
}

static LANG: AtomicU8 = AtomicU8::new(0);

/// Schimbă limba curentă.
pub fn set_lang(l: Lang) {
    LANG.store(l as u8, Ordering::Relaxed);
}

/// Limba curentă.
pub fn lang() -> Lang {
    if LANG.load(Ordering::Relaxed) == Lang::En as u8 {
        Lang::En
    } else {
        Lang::Ro
    }
}

/// „de ” înaintea substantivului după numerele românești care o cer (20, 21, … 101 → nu, 120 …):
/// „20 de entități”, dar „19 entități”. În engleză, șir gol.
pub fn de(n: usize) -> &'static str {
    let r = n % 100;
    if !is_en() && (r >= 20 || (r == 0 && n > 0)) { "de " } else { "" }
}

/// Număr cu substantivul acordat, în limba curentă: „1 fișier”, „5 fișiere”, „20 de fișiere”;
/// „1 file”, „5 files”.
pub fn count(n: usize, ro_one: &str, ro_many: &str, en_one: &str, en_many: &str) -> String {
    match (is_en(), n) {
        (true, 1) => format!("1 {en_one}"),
        (true, _) => format!("{n} {en_many}"),
        (false, 1) => format!("1 {ro_one}"),
        (false, _) => format!("{n} {}{ro_many}", de(n)),
    }
}

/// Adevărat dacă interfața este în engleză.
#[inline]
pub fn is_en() -> bool {
    LANG.load(Ordering::Relaxed) == Lang::En as u8
}

/// Pachetul englez, ca tabel (text sursă → traducere), cu secvențele de escape ale sursei.
static TABLE: &[(&str, &str)] = tr_table!();

fn lookup(s: &str) -> Option<&'static str> {
    use std::collections::HashMap;
    use std::sync::OnceLock;
    static MAP: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    MAP.get_or_init(|| TABLE.iter().copied().collect())
        .get(s)
        .copied()
}

/// Traduce la rulare un text care nu este literal în cod (de exemplu numele din constante:
/// formate, hărți de fundal, sisteme uzuale). Textul necunoscut rămâne neschimbat.
pub fn trs(s: &str) -> std::borrow::Cow<'_, str> {
    match (is_en(), lookup(s)) {
        (true, Some(en)) => std::borrow::Cow::Borrowed(en),
        _ => std::borrow::Cow::Borrowed(s),
    }
}

/// Ca [`trs`], pentru texte statice.
pub fn trs_static(s: &'static str) -> &'static str {
    if is_en() { lookup(s).unwrap_or(s) } else { s }
}

// Fișierul pachetului este urmărit de cargo (recompilare la modificare).
const _PACK: &[u8] = include_bytes!("../lang/en.tsv");

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn switch_language() {
        set_lang(Lang::Ro);
        assert_eq!(tr!("Deschide"), "Deschide");
        let n = 3;
        assert_eq!(tr!("{} fișiere găsite", n), "3 fișiere găsite");
        set_lang(Lang::En);
        assert_eq!(tr!("Deschide"), "Open");
        assert_eq!(tr!("{} fișiere găsite", n), "3 files found");
        let e = "x";
        assert_eq!(tr!("Eroare: {e}"), "Error: x");
        assert_eq!(trs("Altele"), "Others");
        assert_eq!(trs("necunoscut"), "necunoscut");
        set_lang(Lang::Ro);
    }
}

#[cfg(test)]
mod de_tests {
    #[test]
    fn romanian_de() {
        // rulează doar în română; testul de limbă poate comuta temporar în paralel
        if super::is_en() {
            return;
        }
        let f = super::de;
        assert_eq!((f(1), f(19), f(20), f(21), f(101), f(119), f(120), f(200), f(1000)), ("", "", "de ", "de ", "", "", "de ", "de ", "de "));
    }
}
