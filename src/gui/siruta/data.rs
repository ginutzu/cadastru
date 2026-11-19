//! Citirea și interpretarea fișierului SIRUTA publicat de INS (preluat din proiectul SIRUTA).
//!
//! Format (INS, data.gov.ro): CSV cu separator `;`, UTF-8 (cu BOM), coloane:
//! SIRUTA;DENLOC;CODP;JUD;SIRSUP;TIP;NIV;MED;REGIUNE;FSJ;NUTS;LAU
//! NIV: 1 = judet, 2 = UAT (municipiu/oras/comuna), 3 = localitate.
//! TIP: codul de clasificare din "Legenda SIRUTA" (INS).

use cadastru::i18n::tr;
use std::collections::HashMap;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Capitala,
    MunicipiuResedinta,
    Municipiu,
    OrasResedinta,
    Oras,
    Comuna,
    Sector,
    Localitate,
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub siruta: u32,
    /// Denumirea afisata (fara prefixul MUNICIPIUL / ORAŞ).
    pub name: String,
    /// Denumirea oficiala exacta (coloana DENLOC).
    pub denloc: String,
    pub jud_code: u16,
    pub judet: String,
    pub niv: u8,
    pub tip: u8,
    pub kind: Kind,
    pub clasificare: &'static str,
    /// Pentru localitati: UAT-ul de care apartin.
    pub parent: String,
    pub sirsup: u32,
    /// Cheie normalizata pentru cautare (fara diacritice, litere mici).
    pub key: String,
    pub siruta_str: String,
}

impl Entry {
    pub fn is_uat(&self) -> bool {
        self.niv == 2 || (self.niv == 3 && self.tip == 6)
    }
}

#[derive(Clone, Debug, Default)]
pub struct Dataset {
    pub entries: Vec<Entry>,
    /// (cod JUD, denumire), sortat alfabetic.
    pub judete: Vec<(u16, String)>,
    pub uat_count: usize,
    pub loc_count: usize,
}

/// Clasificarea conform legendei SIRUTA (INS).
pub fn classify(niv: u8, tip: u8) -> (Kind, &'static str) {
    match (niv, tip) {
        (2, 1) => (Kind::MunicipiuResedinta, "Municipiu reședință de județ"),
        (2, 2) => (Kind::Oras, "Oraș"),
        (2, 3) => (Kind::Comuna, "Comună"),
        (2, 4) => (Kind::Municipiu, "Municipiu"),
        (2, 5) => (Kind::OrasResedinta, "Oraș reședință de județ"),
        (2, 9) => (Kind::Capitala, "Municipiul București (capitală)"),
        (2, _) => (Kind::Comuna, "UAT"),
        (3, 6) => (Kind::Sector, "Sector al municipiului București"),
        (3, 9) => (Kind::Localitate, "Localitate componentă, reședință de municipiu"),
        (3, 10) => (Kind::Localitate, "Localitate componentă a municipiului"),
        (3, 11) => (Kind::Localitate, "Sat aparținător de municipiu"),
        (3, 17) => (Kind::Localitate, "Localitate componentă, reședință de oraș"),
        (3, 18) => (Kind::Localitate, "Localitate componentă a orașului"),
        (3, 19) => (Kind::Localitate, "Sat aparținător de oraș"),
        (3, 22) => (Kind::Localitate, "Sat reședință de comună"),
        (3, 23) => (Kind::Localitate, "Sat aparținător de comună"),
        _ => (Kind::Localitate, "Localitate"),
    }
}

/// Decodeaza octetii fisierului: UTF-8 (cu/fara BOM) sau, ca rezerva, Windows-1250.
pub fn decode(bytes: &[u8]) -> String {
    let b = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    match std::str::from_utf8(b) {
        Ok(s) => s.to_string(),
        Err(_) => {
            let (cow, _, _) = encoding_rs::WINDOWS_1250.decode(b);
            cow.into_owned()
        }
    }
}

/// Normalizare pentru cautare: litere mici, fara diacritice, cratimele devin spatii.
pub fn normalize(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut last_space = true;
    for ch in s.chars() {
        let c = match ch {
            'ă' | 'Ă' | 'â' | 'Â' | 'á' | 'Á' | 'à' | 'ä' | 'Ä' => 'a',
            'î' | 'Î' | 'í' | 'Í' => 'i',
            'ș' | 'Ș' | 'ş' | 'Ş' => 's',
            'ț' | 'Ț' | 'ţ' | 'Ţ' => 't',
            'é' | 'É' | 'ë' => 'e',
            'ö' | 'Ö' | 'ó' | 'ő' => 'o',
            'ü' | 'Ü' | 'ú' | 'ű' => 'u',
            '-' | '_' | '.' | ',' | '\'' | '"' | '(' | ')' | '/' => ' ',
            c => c.to_lowercase().next().unwrap_or(c),
        };
        if c.is_whitespace() {
            if !last_space {
                out.push(' ');
                last_space = true;
            }
        } else {
            out.push(c);
            last_space = false;
        }
    }
    if out.ends_with(' ') {
        out.pop();
    }
    out
}

fn strip_prefixes<'a>(s: &'a str, prefixes: &[&str]) -> &'a str {
    for p in prefixes {
        if let Some(rest) = s.strip_prefix(p) {
            return rest.trim();
        }
    }
    s
}

fn uat_display_name(denloc: &str) -> String {
    strip_prefixes(
        denloc,
        &["MUNICIPIUL ", "MUNICIPIU ", "ORAŞ ", "ORAȘ ", "ORAS "],
    )
    .to_string()
}

fn judet_display_name(denloc: &str) -> String {
    strip_prefixes(
        denloc,
        &["JUDEŢUL ", "JUDEȚUL ", "JUDETUL ", "MUNICIPIUL "],
    )
    .to_string()
}

/// Interpreteaza continutul CSV SIRUTA.
pub fn parse(text: &str) -> Result<Dataset, String> {
    let text = text.trim_start_matches('\u{feff}');
    let mut lines = text.lines().filter(|l| !l.trim().is_empty());
    let header = lines.next().ok_or("Fișierul SIRUTA este gol")?;
    let sep = if header.contains(';') { ';' } else { ',' };
    let cols: Vec<String> = header
        .split(sep)
        .map(|c| c.trim().trim_matches('"').to_uppercase())
        .collect();
    let idx = |name: &str| cols.iter().position(|c| c == name);
    let (i_sir, i_den, i_jud, i_sup, i_tip, i_niv) = match (
        idx("SIRUTA"),
        idx("DENLOC"),
        idx("JUD"),
        idx("SIRSUP"),
        idx("TIP"),
        idx("NIV"),
    ) {
        (Some(a), Some(b), Some(c), Some(d), Some(e), Some(f)) => (a, b, c, d, e, f),
        _ => {
            return Err(tr!(
                "Antetul fișierului nu are coloanele SIRUTA/DENLOC/JUD/SIRSUP/TIP/NIV: {header}"
            ))
        }
    };
    let need = *[i_sir, i_den, i_jud, i_sup, i_tip, i_niv].iter().max().unwrap();

    struct Raw {
        siruta: u32,
        denloc: String,
        jud: u16,
        sirsup: u32,
        tip: u8,
        niv: u8,
    }
    let mut raws = Vec::with_capacity(17_000);
    for line in lines {
        let f: Vec<&str> = line.split(sep).map(|x| x.trim().trim_matches('"')).collect();
        if f.len() <= need {
            continue;
        }
        let (Ok(siruta), Ok(jud), Ok(tip), Ok(niv)) = (
            f[i_sir].parse::<u32>(),
            f[i_jud].parse::<u16>(),
            f[i_tip].parse::<u8>(),
            f[i_niv].parse::<u8>(),
        ) else {
            continue;
        };
        raws.push(Raw {
            siruta,
            denloc: f[i_den].to_string(),
            jud,
            sirsup: f[i_sup].parse().unwrap_or(0),
            tip,
            niv,
        });
    }

    let mut judete_map: HashMap<u16, String> = HashMap::new();
    let mut uat_names: HashMap<u32, String> = HashMap::new();
    for r in &raws {
        match r.niv {
            1 => {
                judete_map.insert(r.jud, judet_display_name(&r.denloc));
            }
            2 => {
                uat_names.insert(r.siruta, uat_display_name(&r.denloc));
            }
            _ => {}
        }
    }

    let mut entries = Vec::with_capacity(raws.len());
    let (mut uat_count, mut loc_count) = (0, 0);
    for r in raws {
        if r.niv != 2 && r.niv != 3 {
            continue;
        }
        let (kind, clasificare) = classify(r.niv, r.tip);
        let name = if r.niv == 2 {
            uat_display_name(&r.denloc)
        } else {
            r.denloc.clone()
        };
        let judet = judete_map
            .get(&r.jud)
            .cloned()
            .unwrap_or_else(|| format!("JUD {}", r.jud));
        let parent = if r.niv == 3 {
            uat_names.get(&r.sirsup).cloned().unwrap_or_default()
        } else {
            String::new()
        };
        let e = Entry {
            siruta: r.siruta,
            key: normalize(&name),
            siruta_str: r.siruta.to_string(),
            name,
            denloc: r.denloc,
            jud_code: r.jud,
            judet,
            niv: r.niv,
            tip: r.tip,
            kind,
            clasificare,
            parent,
            sirsup: r.sirsup,
        };
        if e.is_uat() {
            uat_count += 1;
        } else {
            loc_count += 1;
        }
        entries.push(e);
    }

    if uat_count < 1000 {
        return Err(tr!(
            "Fișierul pare incomplet: doar {uat_count} UAT-uri recunoscute"
        ));
    }

    let mut judete: Vec<(u16, String)> = judete_map.into_iter().collect();
    judete.sort_by(|a, b| normalize(&a.1).cmp(&normalize(&b.1)));

    Ok(Dataset {
        entries,
        judete,
        uat_count,
        loc_count,
    })
}

/// Cautare: returneaza indicii intrarilor potrivite, ordonati dupa relevanta.
pub fn search(
    ds: &Dataset,
    query: &str,
    judet: Option<u16>,
    include_localitati: bool,
) -> Vec<usize> {
    let q = normalize(query);
    let tokens: Vec<&str> = q.split(' ').filter(|t| !t.is_empty()).collect();
    let numeric = !q.is_empty() && q.chars().all(|c| c.is_ascii_digit());

    let mut hits: Vec<(u8, usize)> = Vec::new();
    for (i, e) in ds.entries.iter().enumerate() {
        if !include_localitati && !e.is_uat() {
            continue;
        }
        if let Some(j) = judet {
            if e.jud_code != j {
                continue;
            }
        }
        let rank = if tokens.is_empty() {
            Some(3)
        } else if numeric {
            if e.siruta_str == q {
                Some(0)
            } else if e.siruta_str.starts_with(&q) {
                Some(1)
            } else {
                None
            }
        } else if e.key == q {
            Some(0)
        } else if e.key.starts_with(&q) {
            Some(1)
        } else if tokens.iter().all(|t| e.key.contains(t)) {
            if e.key.split(' ').any(|w| w.starts_with(tokens[0])) {
                Some(2)
            } else {
                Some(3)
            }
        } else {
            None
        };
        if let Some(mut r) = rank {
            // UAT-urile inaintea localitatilor cu acelasi rang
            r = r * 2 + if e.is_uat() { 0 } else { 1 };
            hits.push((r, i));
        }
    }
    hits.sort_by(|a, b| {
        a.0.cmp(&b.0).then_with(|| {
            let (ea, eb) = (&ds.entries[a.1], &ds.entries[b.1]);
            ea.key.cmp(&eb.key).then_with(|| ea.judet.cmp(&eb.judet))
        })
    });
    hits.into_iter().map(|(_, i)| i).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\u{feff}SIRUTA;DENLOC;CODP;JUD;SIRSUP;TIP;NIV;MED;REGIUNE;FSJ;NUTS;LAU\n\
10;JUDEŢUL ALBA;0;1;1;40;1;0;7;1;RO121;\n\
1017;MUNICIPIUL ALBA IULIA;0;1;10;1;2;1;7;1;RO121;RO_1017\n\
1026;ALBA IULIA;510005;1;1017;9;3;1;7;1;RO121;\n\
1035;BĂRĂBANŢ;510001;1;1017;10;3;1;7;1;RO121;\n\
1213;MUNICIPIUL AIUD;0;1;10;4;2;1;7;1;RO121;RO_1213\n\
127;JUDEŢUL CLUJ;0;12;1;40;1;0;6;13;RO113;\n\
54975;MUNICIPIUL CLUJ-NAPOCA;0;12;127;1;2;1;6;13;RO113;RO_54975\n\
207;JUDEŢUL HUNEDOARA;0;20;1;40;1;0;5;22;RO423;\n\
88868;BUCUREŞCI;0;20;207;3;2;3;5;22;RO423;RO_88868\n\
403;MUNICIPIUL BUCUREŞTI;0;40;1;40;1;0;8;42;RO321;\n\
179132;MUNICIPIUL BUCUREŞTI;0;40;403;9;2;1;8;42;RO321;RO_179132\n\
179141;BUCUREŞTI SECTORUL 1;10013;40;179132;6;3;1;8;42;RO321;\n";

    fn ds() -> Dataset {
        // pragul minim de 1000 UAT nu se aplica la test: construim manual
        let mut text = String::from(SAMPLE);
        for k in 0..1100 {
            text.push_str(&format!("{};COMUNA TEST {};0;1;10;3;2;3;7;1;RO121;\n", 900000 + k, k));
        }
        parse(&text).unwrap()
    }

    #[test]
    fn parse_and_classify() {
        let d = ds();
        let cj = d.entries.iter().find(|e| e.siruta == 54975).unwrap();
        assert_eq!(cj.name, "CLUJ-NAPOCA");
        assert_eq!(cj.judet, "CLUJ");
        assert_eq!(cj.clasificare, "Municipiu reședință de județ");
        let b = d.entries.iter().find(|e| e.siruta == 179132).unwrap();
        assert_eq!(b.judet, "BUCUREŞTI");
        assert_eq!(b.kind, Kind::Capitala);
        let s1 = d.entries.iter().find(|e| e.siruta == 179141).unwrap();
        assert!(s1.is_uat());
        let sat = d.entries.iter().find(|e| e.siruta == 1035).unwrap();
        assert_eq!(sat.parent, "ALBA IULIA");
        assert!(!sat.is_uat());
        assert_eq!(d.judete.len(), 4);
    }

    #[test]
    fn search_works() {
        let d = ds();
        let r = search(&d, "cluj napoca", None, false);
        assert_eq!(d.entries[r[0]].siruta, 54975);
        let r = search(&d, "Bucuresci", None, false);
        assert_eq!(d.entries[r[0]].siruta, 88868);
        let r = search(&d, "barabant", None, false);
        assert!(r.is_empty());
        let r = search(&d, "barabant", None, true);
        assert_eq!(r.len(), 1);
        let r = search(&d, "549", None, false);
        assert_eq!(d.entries[r[0]].siruta, 54975);
        let r = search(&d, "alba", None, true);
        assert_eq!(d.entries[r[0]].siruta, 1017);
        assert_eq!(normalize("Bistrița-Năsăud"), "bistrita nasaud");
        assert_eq!(normalize("ŞTEFAN CEL MARE"), "stefan cel mare");
    }
}
