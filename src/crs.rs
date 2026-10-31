//! Sisteme de coordonate (CRS) și transformări între ele.
//!
//! Definițiile EPSG provin din `crs-definitions` (șiruri PROJ + WKT), iar calculele
//! sunt făcute de `proj4rs` (implementare Rust pură a PROJ.4).

use anyhow::{Result, anyhow, bail};
use proj4rs::proj::Proj;
use crate::transdatro::{self, HeightModel, Stereo};
use std::fmt;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use crate::i18n::tr;

// ---------------------------------------------------------------------------
// TransDatRO

static TDR_ON: AtomicBool = AtomicBool::new(true);
static TDR_MODEL: AtomicU8 = AtomicU8::new(0);

/// Activează/dezactivează folosirea TransDatRO pentru Stereo 70 (implicit activ).
/// Stereo 30 București se transformă întotdeauna cu TransDatRO.
pub fn set_transdatro(on: bool) {
    TDR_ON.store(on, Ordering::Relaxed);
}

pub fn transdatro_enabled() -> bool {
    TDR_ON.load(Ordering::Relaxed)
}

/// Modelul de cvasigeoid folosit pentru altitudini.
pub fn set_height_model(m: HeightModel) {
    TDR_MODEL.store(if m == HeightModel::Egg97 { 1 } else { 0 }, Ordering::Relaxed);
}

pub fn height_model() -> HeightModel {
    if TDR_MODEL.load(Ordering::Relaxed) == 1 {
        HeightModel::Egg97
    } else {
        HeightModel::Auto
    }
}

/// Definiția internă pentru Stereo 1930 București (doar prin TransDatRO; proiecția de bază
/// este stereografica oblică GRS80 folosită de algoritm, marcată cu +transdatro=st30).
pub const ST30_PROJ: &str = "+proj=sterea +lat_0=46 +lon_0=25 +k=0.99975 +x_0=500000 +y_0=500000 +ellps=GRS80 +units=m +no_defs +transdatro=st30";
pub const ST30_NAME: &str = "Stereo 1930 București (TransDatRO)";
/// WKT scris în .prj: sistem local (alte programe GIS nu au o definiție pentru el).
pub const ST30_WKT: &str = r#"LOCAL_CS["Stereo 1930 Bucuresti (TransDatRO)",LOCAL_DATUM["Hayford 1910 - Bucuresti",32767],UNIT["metre",1],AXIS["Easting",EAST],AXIS["Northing",NORTH]]"#;

#[derive(Clone, Debug)]
pub struct Crs {
    /// Definiția PROJ (+proj=...)
    pub proj: String,
    /// Nume lizibil
    pub name: String,
    /// Codul EPSG, dacă e cunoscut
    pub epsg: Option<u32>,
    /// WKT original (dacă a fost citit dintr-un fișier)
    pub wkt: Option<String>,
}

impl PartialEq for Crs {
    fn eq(&self, other: &Self) -> bool {
        if let (Some(a), Some(b)) = (self.epsg, other.epsg) {
            return a == b;
        }
        normalize_proj(&self.proj) == normalize_proj(&other.proj)
    }
}

impl fmt::Display for Crs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.epsg {
            Some(c) => write!(f, "EPSG:{} - {}", c, self.name),
            None => write!(f, "{}", self.name),
        }
    }
}

/// Sisteme folosite frecvent în România (afișate primele în interfață).
pub const COMMON: &[(u32, &str)] = &[
    (3844, "Stereo 70 (Pulkovo 1942(58) / Stereo70)"),
    (4326, "WGS 84 - geografic (lat/lon)"),
    (4258, "ETRS89 - geografic"),
    (25834, "ETRS89 / UTM zona 34N"),
    (25835, "ETRS89 / UTM zona 35N"),
    (32634, "WGS 84 / UTM zona 34N"),
    (32635, "WGS 84 / UTM zona 35N"),
    (3857, "WGS 84 / Pseudo-Mercator (Web)"),
    (3035, "ETRS89 / LAEA Europa"),
    (31700, "Dealul Piscului 1970 / Stereo 70 (vechi)"),
    (4179, "Pulkovo 1942(58) - geografic"),
];

fn normalize_proj(s: &str) -> String {
    let mut parts: Vec<String> = s
        .split_whitespace()
        .filter(|p| *p != "+no_defs" && *p != "+wktext" && *p != "+type=crs" && *p != "+units=m")
        .map(|p| {
            if let Some(v) = p.strip_prefix("+towgs84=") {
                // 0,0,0 == 0,0,0,0,0,0,0
                let mut x: Vec<f64> = v.split(',').filter_map(|t| t.parse().ok()).collect();
                while x.len() > 3 && x.last() == Some(&0.0) {
                    x.pop();
                }
                return format!(
                    "+towgs84={}",
                    x.iter()
                        .map(|f| format!("{f}"))
                        .collect::<Vec<_>>()
                        .join(",")
                );
            }
            // numere: 46.0 == 46
            if let Some((k, v)) = p.split_once('=')
                && let Ok(f) = v.parse::<f64>()
            {
                return format!("{k}={f}");
            }
            p.to_string()
        })
        .collect();
    parts.sort();
    parts.join(" ")
}

/// Caută codul EPSG cu exact aceeași definiție PROJ.
fn match_epsg(proj: &str) -> Option<u32> {
    static IDX: OnceLock<std::collections::HashMap<String, u32>> = OnceLock::new();
    let idx = IDX.get_or_init(|| {
        let mut m = std::collections::HashMap::new();
        // codurile uzuale au prioritate la definiții identice
        for (c, _) in COMMON.iter().rev() {
            if let Ok(crs) = Crs::from_epsg(*c) {
                m.insert(normalize_proj(&crs.proj), *c);
            }
        }
        for code in 0..=u16::MAX {
            if let Some(d) = crs_definitions::from_code(code) {
                m.entry(normalize_proj(d.proj4)).or_insert(code as u32);
            }
        }
        m
    });
    idx.get(&normalize_proj(proj)).copied()
}

fn wkt_root_name(wkt: &str) -> Option<String> {
    let s = wkt.find('"')?;
    let e = wkt[s + 1..].find('"')?;
    Some(wkt[s + 1..s + 1 + e].to_string())
}

/// Index cod EPSG -> nume (construit o singură dată).
pub fn epsg_index() -> &'static Vec<(u32, String)> {
    static IDX: OnceLock<Vec<(u32, String)>> = OnceLock::new();
    IDX.get_or_init(|| {
        let mut v = Vec::with_capacity(6500);
        for code in 0..=u16::MAX {
            if let Some(d) = crs_definitions::from_code(code) {
                v.push((code as u32, wkt_root_name(d.wkt).unwrap_or_default()));
            }
        }
        v
    })
}

/// Caută sisteme după cod sau nume.
pub fn search(query: &str, limit: usize) -> Vec<(u32, String)> {
    let q = query.trim().to_lowercase();
    let q = q.strip_prefix("epsg:").unwrap_or(&q).to_string();
    if q.is_empty() {
        return COMMON.iter().map(|(c, n)| (*c, crate::i18n::trs(n).into_owned())).collect();
    }
    let words: Vec<&str> = q.split_whitespace().collect();
    let mut out: Vec<(u32, String)> = Vec::new();
    for (c, n) in epsg_index() {
        let ln = n.to_lowercase();
        let cs = c.to_string();
        if cs.starts_with(&q) || words.iter().all(|w| ln.contains(w)) {
            out.push((*c, n.clone()));
            if out.len() >= limit {
                break;
            }
        }
    }
    out
}

/// Definiții corectate. Parametrii EPSG pentru Stereo 70 (transformarea 15994) sunt publicați
/// în convenția „coordinate frame”; +towgs84 cere „position vector”, deci rotațiile au semn opus.
const OVERRIDES: &[(u32, &str)] = &[
    (
        3844,
        "+proj=sterea +lat_0=46 +lon_0=25 +k=0.99975 +x_0=500000 +y_0=500000 +ellps=krass +towgs84=2.329,-147.042,-92.08,-0.309,0.325,0.497,5.69 +units=m +no_defs",
    ),
    (
        4179,
        "+proj=longlat +ellps=krass +towgs84=2.329,-147.042,-92.08,-0.309,0.325,0.497,5.69 +no_defs",
    ),
];

/// Coduri din baza de date cu semnul rotațiilor inversat (verificat față de PROJ 9.8).
const FLIP_ROTATIONS: &[u32] = &[3296, 3297, 3298, 3299, 3302, 4687, 4689];

fn flip_rotations(proj: &str) -> String {
    proj.split_whitespace()
        .map(|p| match p.strip_prefix("+towgs84=") {
            Some(v) => {
                let mut x: Vec<f64> = v.split(',').filter_map(|t| t.parse().ok()).collect();
                if x.len() == 7 {
                    for r in &mut x[3..6] {
                        *r = -*r;
                    }
                }
                format!(
                    "+towgs84={}",
                    x.iter().map(|f| fmt_g(*f)).collect::<Vec<_>>().join(",")
                )
            }
            None => p.to_string(),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn fmt_g(f: f64) -> String {
    let s = format!("{f}");
    if s == "-0" { "0".into() } else { s }
}

/// Elimină nodul TOWGS84[...] din WKT.
fn strip_wkt_towgs84(wkt: &str) -> String {
    let Some(s) = wkt.find(",TOWGS84[") else {
        return wkt.to_string();
    };
    let Some(e) = wkt[s..].find(']') else {
        return wkt.to_string();
    };
    format!("{}{}", &wkt[..s], &wkt[s + e + 1..])
}

/// Denumiri ESRI uzuale (fișiere .prj fără cod EPSG).
const ESRI_ALIASES: &[(&str, u32)] = &[
    ("stereo70", 3844),
    ("stereo_70", 3844),
    ("pulkovo_1942_stereo_70", 3844),
    ("pulkovo1942stereo70", 3844),
    ("dealul_piscului_1970_stereo_70", 31700),
    ("gcs_wgs_1984", 4326),
    ("wgs84", 4326),
    ("gcs_etrs_1989", 4258),
    ("etrs_1989_utm_zone_34n", 25834),
    ("etrs_1989_utm_zone_35n", 25835),
    ("wgs_1984_utm_zone_34n", 32634),
    ("wgs_1984_utm_zone_35n", 32635),
    ("wgs_1984_web_mercator_auxiliary_sphere", 3857),
    ("wgs_1984_web_mercator", 3857),
    ("etrs_1989_laea", 3035),
    ("etrs_1989_europe_laea", 3035),
];

impl Crs {
    /// Stereo 1930 cu plan secant București (sistem Hayford 1910), prin TransDatRO.
    pub fn stereo30() -> Crs {
        Crs {
            proj: ST30_PROJ.into(),
            name: crate::i18n::trs_static(ST30_NAME).into(),
            epsg: None,
            wkt: Some(ST30_WKT.into()),
        }
    }

    /// Sistemul național tratat de TransDatRO (Stereo 70 = EPSG:3844, Stereo 30 București).
    pub fn stereo_kind(&self) -> Option<Stereo> {
        if self.epsg == Some(3844) {
            Some(Stereo::S70)
        } else if self.proj.contains("+transdatro=st30") {
            Some(Stereo::S30)
        } else {
            None
        }
    }

    pub fn wgs84() -> Crs {
        Crs::from_epsg(4326).expect("EPSG:4326")
    }

    pub fn from_epsg(code: u32) -> Result<Crs> {
        let c16: u16 = code
            .try_into()
            .map_err(|_| anyhow!("{}", tr!("Cod EPSG necunoscut: {code}")))?;
        let d = crs_definitions::from_code(c16)
            .ok_or_else(|| anyhow!("{}", tr!("Cod EPSG necunoscut: {code}")))?;
        let mut proj = d.proj4.to_string();
        if let Some((_, p)) = OVERRIDES.iter().find(|(c, _)| *c == code) {
            proj = p.to_string();
        } else if FLIP_ROTATIONS.contains(&code) {
            proj = flip_rotations(&proj);
        }
        Proj::from_proj_string(&proj)
            .map_err(|e| anyhow!("{}", tr!("EPSG:{code} nu poate fi folosit: {e:?}")))?;
        Ok(Crs {
            proj,
            name: wkt_root_name(d.wkt).unwrap_or_else(|| format!("EPSG:{code}")),
            epsg: Some(code),
            wkt: None,
        })
    }

    /// Interpretează un text introdus de utilizator: EPSG:xxxx, cod, șir PROJ, WKT sau alias.
    pub fn parse(s: &str) -> Result<Crs> {
        let t = s.trim();
        if t.is_empty() {
            bail!(tr!("Sistem de coordonate gol"));
        }
        let low = t.to_lowercase();
        if let Some(rest) = low
            .strip_prefix("epsg:")
            .or_else(|| low.strip_prefix("epsg "))
        {
            let code: u32 = rest
                .trim()
                .parse()
                .map_err(|_| anyhow!("{}", tr!("Cod EPSG invalid: {t}")))?;
            return Crs::from_epsg(code);
        }
        if let Ok(code) = t.parse::<u32>() {
            return Crs::from_epsg(code);
        }
        if t.starts_with('+') && t.contains("+transdatro=st30") {
            return Ok(Crs::stereo30());
        }
        if t.starts_with('+') {
            Proj::from_proj_string(t).map_err(|e| anyhow!("{}", tr!("Șir PROJ invalid: {e:?}")))?;
            if let Some(c) = detect_stereo70(t).or_else(|| match_epsg(t)) {
                return Crs::from_epsg(c);
            }
            return Ok(Crs {
                proj: t.to_string(),
                name: tr!("Proiecție personalizată").into(),
                epsg: None,
                wkt: None,
            });
        }
        let up = t.trim_start().to_uppercase();
        if [
            "LOCAL_CS",
            "ENGCRS",
            "PROJCS",
            "GEOGCS",
            "PROJCRS",
            "GEOGCRS",
            "GEODCRS",
            "COMPD_CS",
            "COMPOUNDCRS",
            "BOUNDCRS",
        ]
        .iter()
        .any(|k| up.starts_with(k))
        {
            return Crs::from_wkt(t);
        }
        let key: String = low.chars().filter(|c| c.is_alphanumeric()).collect();
        if is_st30_name(&key) {
            return Ok(Crs::stereo30());
        }
        let alias = match key.as_str() {
            "wgs84" | "wgs" | "latlon" | "lonlat" | "geografic" => Some(4326),
            "stereo70" | "stereo" | "s70" => Some(3844),
            "etrs89" => Some(4258),
            "utm34" | "utm34n" => Some(25834),
            "utm35" | "utm35n" => Some(25835),
            "webmercator" | "mercator" | "pseudomercator" | "google" => Some(3857),
            "laea" => Some(3035),
            _ => None,
        };
        if let Some(c) = alias {
            return Crs::from_epsg(c);
        }
        let found = search(t, 2);
        if found.len() == 1 {
            return Crs::from_epsg(found[0].0);
        }
        bail!(
            "{}", tr!("Nu recunosc sistemul de coordonate „{t}”. Folosiți EPSG:cod, un șir PROJ (+proj=...) sau WKT."
        ))
    }

    /// Citește WKT (de ex. dintr-un fișier .prj).
    pub fn from_wkt(wkt: &str) -> Result<Crs> {
        let wkt = wkt.trim().trim_start_matches('\u{feff}');
        // 0. Stereo 1930 București (scris de Cadastru ca LOCAL_CS)
        if let Some(n) = wkt_root_name(wkt) {
            let k: String = n.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect();
            if is_st30_name(&k) {
                return Ok(Crs::stereo30());
            }
        }
        // 1. Autoritate EPSG la nivelul rădăcinii
        if let Some(code) = root_epsg_authority(wkt)
            && let Ok(mut c) = Crs::from_epsg(code)
        {
            c.wkt = Some(wkt.to_string());
            return Ok(c);
        }
        // 2. Nume ESRI cunoscute
        let name = wkt_root_name(wkt).unwrap_or_else(|| tr!("Sistem din WKT").into());
        let lname = name.to_lowercase();
        let compact: String = lname.chars().filter(|c| c.is_alphanumeric()).collect();
        for (alias, code) in ESRI_ALIASES {
            let ac: String = alias.chars().filter(|c| c.is_alphanumeric()).collect();
            if lname == *alias || compact == ac {
                let mut c = Crs::from_epsg(*code)?;
                c.wkt = Some(wkt.to_string());
                return Ok(c);
            }
        }
        // 3. Conversie generală WKT -> PROJ
        let proj =
            proj4wkt::wkt_to_projstring(wkt).map_err(|e| anyhow!("{}", tr!("WKT nerecunoscut: {e:?}")))?;
        if let Some(code) = detect_stereo70(&proj) {
            let mut c = Crs::from_epsg(code)?;
            c.wkt = Some(wkt.to_string());
            return Ok(c);
        }
        // 4. Definiție PROJ identică cu un cod EPSG
        if let Some(code) = match_epsg(&proj)
            && let Ok(mut c) = Crs::from_epsg(code)
        {
            c.wkt = Some(wkt.to_string());
            return Ok(c);
        }
        // 5. Nume identic cu o definiție EPSG
        for (c, n) in epsg_index() {
            if n.eq_ignore_ascii_case(&name)
                && let Ok(mut crs) = Crs::from_epsg(*c)
            {
                crs.wkt = Some(wkt.to_string());
                return Ok(crs);
            }
        }
        Proj::from_proj_string(&proj)
            .map_err(|e| anyhow!("{}", tr!("Proiecție nesuportată ({proj}): {e:?}")))?;
        Ok(Crs {
            proj,
            name,
            epsg: None,
            wkt: Some(wkt.to_string()),
        })
    }

    /// WKT pentru scrierea fișierelor .prj / GeoPackage.
    pub fn to_wkt(&self) -> Option<String> {
        if let Some(c) = self.epsg
            && let Some(d) = u16::try_from(c).ok().and_then(crs_definitions::from_code)
        {
            // fără TOWGS84: programele GIS recunosc codul EPSG și aplică transformarea lor implicită
            return Some(strip_wkt_towgs84(d.wkt));
        }
        self.wkt.clone()
    }

    pub fn is_geographic(&self) -> bool {
        self.proj.contains("+proj=longlat")
            || self.proj.contains("+proj=latlong")
            || self.proj.contains("+proj=lonlat")
            || self.proj.contains("+proj=latlon")
    }

    pub fn short(&self) -> String {
        match self.epsg {
            Some(c) => format!("EPSG:{c}"),
            None => self.name.clone(),
        }
    }

    /// Șirul de identificare folosit pentru salvarea în proiecte.
    pub fn to_user_string(&self) -> String {
        match self.epsg {
            Some(c) => format!("EPSG:{c}"),
            None => self.wkt.clone().unwrap_or_else(|| self.proj.clone()),
        }
    }

    pub fn proj(&self) -> Result<Proj> {
        let p: String = if self.proj.contains("+transdatro=") {
            self.proj
                .split_whitespace()
                .filter(|t| !t.starts_with("+transdatro="))
                .collect::<Vec<_>>()
                .join(" ")
        } else {
            self.proj.clone()
        };
        Proj::from_proj_string(&p).map_err(|e| anyhow!("{}", tr!("Proiecție invalidă {}: {e:?}", self.proj)))
    }
}

/// Numele (oricum scris) desemnează Stereo 1930 București.
pub fn is_stereo30_name(s: &str) -> bool {
    let k: String = s.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect();
    is_st30_name(&k)
}

fn is_st30_name(k: &str) -> bool {
    matches!(k, "stereo30" | "st30" | "s30" | "stereo1930" | "stereo30bucuresti")
        || k.starts_with("stereo1930bucuresti")
        || k.starts_with("stereo30bucuresti")
}

/// Sisteme speciale (fără cod EPSG) care se potrivesc căutării.
pub fn special_crs(query: &str) -> Vec<Crs> {
    let q: String = query
        .to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric() || c.is_whitespace())
        .collect();
    let hay = "stereo 1930 30 bucuresti bucurești transdatro hayford st30";
    if q.trim().is_empty() || q.split_whitespace().all(|w| hay.contains(w)) {
        vec![Crs::stereo30()]
    } else {
        vec![]
    }
}

/// Găsește AUTHORITY["EPSG","n"] / ID["EPSG",n] la nivelul 1 al WKT-ului.
fn root_epsg_authority(wkt: &str) -> Option<u32> {
    let bytes = wkt.as_bytes();
    let mut depth = 0i32;
    let mut in_str = false;
    let mut found = None;
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if c == b'"' {
            in_str = !in_str;
        } else if !in_str {
            match c {
                b'[' | b'(' => depth += 1,
                b']' | b')' => depth -= 1,
                _ => {
                    if depth == 1 {
                        let rest = &wkt[i..];
                        let ru = rest.get(..12).unwrap_or(rest).to_uppercase();
                        if ru.starts_with("AUTHORITY[") || ru.starts_with("ID[") {
                            let open = rest.find('[')?;
                            let close = rest.find(']')?;
                            let inner = &rest[open + 1..close];
                            let parts: Vec<String> = inner
                                .split(',')
                                .map(|p| p.trim().trim_matches('"').to_string())
                                .collect();
                            if parts.len() >= 2 && parts[0].eq_ignore_ascii_case("EPSG") {
                                found = parts[1].parse::<u32>().ok();
                            }
                        }
                    }
                }
            }
        }
        i += 1;
    }
    found
}

fn proj_param(proj: &str, key: &str) -> Option<String> {
    for p in proj.split_whitespace() {
        if let Some(v) = p
            .strip_prefix('+')
            .and_then(|p| p.strip_prefix(key))
            .and_then(|p| p.strip_prefix('='))
        {
            return Some(v.to_string());
        }
    }
    None
}

/// Recunoaște parametrii Stereo 70 fără transformare de datum și întoarce EPSG:3844.
fn detect_stereo70(proj: &str) -> Option<u32> {
    let num = |k: &str| proj_param(proj, k).and_then(|v| v.parse::<f64>().ok());
    let is_sterea = proj_param(proj, "proj").as_deref() == Some("sterea");
    let close = |v: Option<f64>, t: f64| v.is_some_and(|x| (x - t).abs() < 1e-6);
    let krass = proj_param(proj, "ellps").as_deref() == Some("krass") || close(num("a"), 6378245.0);
    if is_sterea
        && krass
        && close(num("lat_0"), 46.0)
        && close(num("lon_0"), 25.0)
        && close(num("k"), 0.99975)
        && close(num("x_0"), 500000.0)
        && close(num("y_0"), 500000.0)
    {
        if proj_param(proj, "towgs84").is_none() {
            return Some(3844);
        }
        if let Some(t) = proj_param(proj, "towgs84") {
            let v: Vec<f64> = t.split(',').filter_map(|x| x.parse().ok()).collect();
            if v.len() >= 3 && (v[0] - 28.0).abs() < 0.5 && (v[1] + 121.0).abs() < 0.5 {
                return Some(31700);
            }
            if v.len() >= 3 && (v[0] - 2.329).abs() < 0.5 && (v[1] + 147.042).abs() < 0.5 {
                return Some(3844);
            }
        }
    }
    None
}

/// Ghicește sistemul pe baza extinderii, când fișierul nu are proiecție.
pub fn guess_crs(minx: f64, miny: f64, maxx: f64, maxy: f64) -> Option<Crs> {
    if minx >= -180.0 && maxx <= 180.0 && miny >= -90.0 && maxy <= 90.0 {
        return Crs::from_epsg(4326).ok();
    }
    // Extinderea României în Stereo 70 (cu marjă)
    if minx >= 100_000.0 && maxx <= 950_000.0 && miny >= 150_000.0 && maxy <= 800_000.0 {
        return Crs::from_epsg(3844).ok();
    }
    None
}

/// Metoda folosită pentru transformarea unui punct.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    /// Sisteme identice
    Identity,
    /// PROJ (proj4rs) — parametri de datum din definiții (towgs84)
    Proj,
    /// TransDatRO (grid oficial ANCPI)
    TransDatRO,
    /// În afara gridului TransDatRO: Helmert 7 parametri (EPSG:15994)
    Fallback,
}

impl Method {
    pub fn label(&self) -> &'static str {
        match self {
            Method::Identity => tr!("fără transformare"),
            Method::Proj => tr!("PROJ (parametri de datum)"),
            Method::TransDatRO => tr!("TransDatRO (grid oficial ANCPI)"),
            Method::Fallback => tr!("Helmert 7 parametri — punct în afara gridului TransDatRO"),
        }
    }
}

struct ProjPair {
    src: Proj,
    dst: Proj,
    src_geo: bool,
    dst_geo: bool,
}

impl ProjPair {
    fn new(src: &Crs, dst: &Crs) -> Result<ProjPair> {
        let sp = src.proj()?;
        let dp = dst.proj()?;
        Ok(ProjPair {
            src_geo: sp.is_latlong(),
            dst_geo: dp.is_latlong(),
            src: sp,
            dst: dp,
        })
    }

    #[inline]
    fn xy(&self, x: f64, y: f64) -> Option<(f64, f64)> {
        let mut p = if self.src_geo {
            (x.to_radians(), y.to_radians(), 0.0)
        } else {
            (x, y, 0.0)
        };
        proj4rs::transform::transform(&self.src, &self.dst, &mut p).ok()?;
        if self.dst_geo {
            p.0 = p.0.to_degrees();
            p.1 = p.1.to_degrees();
        }
        if p.0.is_finite() && p.1.is_finite() {
            Some((p.0, p.1))
        } else {
            None
        }
    }
}

/// O parte (sursă sau destinație) a unei transformări prin ETRS89.
enum Side {
    /// Sistem național prin TransDatRO; opțional transformare de rezervă (PROJ) în afara gridului
    Stereo(Stereo, Option<ProjPair>),
    /// Alt sistem: PROJ între el și ETRS89 geografic
    Proj(ProjPair),
}

struct TdrPipe {
    src: Side,
    dst: Side,
}

/// Transformă coordonate dintr-un sistem în altul.
pub struct Transformer {
    direct: Option<ProjPair>,
    tdr: Option<TdrPipe>,
}

fn etrs89_geo() -> Crs {
    Crs::from_epsg(4258).expect("EPSG:4258")
}

impl Transformer {
    /// Transformare între două sisteme; TransDatRO se folosește automat pentru Stereo 70
    /// (dacă e activat în setări) și întotdeauna pentru Stereo 30 București.
    pub fn new(src: &Crs, dst: &Crs) -> Result<Transformer> {
        Transformer::with_transdatro(src, dst, transdatro_enabled())
    }

    pub fn with_transdatro(src: &Crs, dst: &Crs, use_tdr: bool) -> Result<Transformer> {
        if src == dst {
            return Ok(Transformer::identity());
        }
        let ks = src.stereo_kind();
        let kd = dst.stereo_kind();
        let need = |k: Option<Stereo>| match k {
            Some(Stereo::S30) => true,
            Some(Stereo::S70) => use_tdr,
            None => false,
        };
        if need(ks) || need(kd) {
            let etrs = etrs89_geo();
            let side = |c: &Crs, k: Option<Stereo>, to_etrs: bool| -> Result<Side> {
                let pair = |c: &Crs| {
                    if to_etrs {
                        ProjPair::new(c, &etrs)
                    } else {
                        ProjPair::new(&etrs, c)
                    }
                };
                Ok(match k {
                    Some(Stereo::S30) => Side::Stereo(Stereo::S30, None),
                    Some(Stereo::S70) if use_tdr => Side::Stereo(Stereo::S70, pair(c).ok()),
                    _ => Side::Proj(pair(c)?),
                })
            };
            return Ok(Transformer {
                direct: None,
                tdr: Some(TdrPipe {
                    src: side(src, ks, true)?,
                    dst: side(dst, kd, false)?,
                }),
            });
        }
        Ok(Transformer {
            direct: Some(ProjPair::new(src, dst)?),
            tdr: None,
        })
    }

    pub fn identity() -> Transformer {
        Transformer {
            direct: None,
            tdr: None,
        }
    }

    pub fn is_identity(&self) -> bool {
        self.direct.is_none() && self.tdr.is_none()
    }

    /// Folosește TransDatRO (cel puțin pentru o parte a transformării).
    pub fn uses_transdatro(&self) -> bool {
        self.tdr.is_some()
    }

    #[inline]
    pub fn xy(&self, x: f64, y: f64) -> Option<(f64, f64)> {
        self.xy_method(x, y).map(|(p, _)| p)
    }

    /// Transformă un punct și întoarce și metoda folosită.
    pub fn xy_method(&self, x: f64, y: f64) -> Option<((f64, f64), Method)> {
        if let Some(d) = &self.direct {
            return d.xy(x, y).map(|p| (p, Method::Proj));
        }
        let Some(t) = &self.tdr else {
            return Some(((x, y), Method::Identity));
        };
        let mut fallback = false;
        // 1. sursă -> ETRS89 (λ, φ)
        let (lon, lat) = match &t.src {
            Side::Stereo(k, fb) => match transdatro::stereo_to_etrs89(x, y, *k) {
                Ok((lat, lon)) => (lon, lat),
                Err(_) => {
                    fallback = true;
                    fb.as_ref()?.xy(x, y)?
                }
            },
            Side::Proj(p) => p.xy(x, y)?,
        };
        // 2. ETRS89 -> destinație
        let out = match &t.dst {
            Side::Stereo(k, fb) => match transdatro::etrs89_to_stereo(lat, lon, *k) {
                Ok(en) => en,
                Err(_) => {
                    fallback = true;
                    fb.as_ref()?.xy(lon, lat)?
                }
            },
            Side::Proj(p) => p.xy(lon, lat)?,
        };
        let m = if fallback {
            Method::Fallback
        } else {
            Method::TransDatRO
        };
        Some((out, m))
    }

    /// Transformă toate coordonatele unei geometrii. Întoarce false dacă vreun punct a eșuat.
    pub fn geometry(&self, g: &mut crate::model::Geometry) -> bool {
        if self.is_identity() {
            return true;
        }
        let mut ok = true;
        g.for_each_coord_mut(&mut |c| match self.xy(c.x, c.y) {
            Some((x, y)) => {
                c.x = x;
                c.y = y;
            }
            None => {
                ok = false;
                c.x = f64::NAN;
                c.y = f64::NAN;
            }
        });
        ok
    }

    /// Transformă un dreptunghi (prin densificarea laturilor).
    pub fn bounds(&self, b: &crate::model::Bounds) -> crate::model::Bounds {
        if self.is_identity() {
            return *b;
        }
        let mut out = crate::model::Bounds::empty();
        let n = 20;
        for i in 0..=n {
            let t = i as f64 / n as f64;
            let x = b.minx + t * b.width();
            let y = b.miny + t * b.height();
            for (px, py) in [(x, b.miny), (x, b.maxy), (b.minx, y), (b.maxx, y)] {
                if let Some((tx, ty)) = self.xy(px, py) {
                    out.extend(tx, ty);
                }
            }
        }
        out
    }
}

/// Transformă un strat vectorial în alt sistem (in-place).
pub fn reproject_layer(layer: &mut crate::model::VectorLayer, dst: &Crs) -> Result<usize> {
    let src = layer.crs.clone().ok_or_else(|| {
        anyhow!(
            "{}", tr!("Stratul „{}” nu are sistem de coordonate definit",
            layer.name
        ))
    })?;
    let t = Transformer::new(&src, dst)?;
    let mut failed = 0;
    for f in &mut layer.features {
        if let Some(g) = &mut f.geom
            && !t.geometry(g)
        {
            failed += 1;
        }
    }
    layer.crs = Some(dst.clone());
    Ok(failed)
}

/// Formatează grade în grade-minute-secunde.
pub fn to_dms(v: f64, is_lat: bool) -> String {
    let hemi = if is_lat {
        if v >= 0.0 { 'N' } else { 'S' }
    } else if v >= 0.0 {
        'E'
    } else {
        'V'
    };
    let a = v.abs();
    let mut d = a.floor();
    let mut m = ((a - d) * 60.0).floor();
    let mut s = ((a - d) * 60.0 - m) * 60.0;
    if s >= 59.9995 {
        s = 0.0;
        m += 1.0;
    }
    if m >= 60.0 {
        m = 0.0;
        d += 1.0;
    }
    format!("{}°{:02}'{:06.3}\"{}", d as i64, m as i64, s, hemi)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stereo70_to_wgs84() {
        let s70 = Crs::from_epsg(3844).unwrap();
        let wgs = Crs::wgs84();
        let t = Transformer::new(&wgs, &s70).unwrap();
        // Originea proiecției: 46N 25E -> aprox. (500000, 500000) (cu decalaj de datum)
        let (x, y) = t.xy(25.0, 46.0).unwrap();
        assert!(
            (x - 500000.0).abs() < 200.0 && (y - 500000.0).abs() < 200.0,
            "{x} {y}"
        );
        let back = Transformer::new(&s70, &wgs).unwrap();
        let (lon, lat) = back.xy(x, y).unwrap();
        assert!((lon - 25.0).abs() < 1e-7 && (lat - 46.0).abs() < 1e-7);
    }

    #[test]
    fn stereo70_matches_proj_epsg_pipeline() {
        // Referință: PROJ 9.8, EPSG:3844 -> EPSG:4326 (transformarea 15994)
        let t = Transformer::with_transdatro(&Crs::from_epsg(3844).unwrap(), &Crs::wgs84(), false)
            .unwrap();
        let (lon, lat) = t.xy(588000.0, 328000.0).unwrap();
        assert!(
            (lon - 26.10407513286427).abs() < 1e-7 && (lat - 44.44636554732523).abs() < 1e-7,
            "{lon} {lat}"
        );
        let w = Crs::from_epsg(3844).unwrap().to_wkt().unwrap();
        assert!(
            !w.contains("TOWGS84") && w.contains("AUTHORITY[\"EPSG\",\"3844\"]]"),
            "{w}"
        );
        // Web Mercator -> Stereo 70 (referință PROJ 9.8)
        let t = Transformer::with_transdatro(
            &Crs::from_epsg(3857).unwrap(),
            &Crs::from_epsg(3844).unwrap(),
            false,
        )
        .unwrap();
        let (x, y) = t.xy(2905000.0, 5532000.0).unwrap();
        assert!(
            (x - 587388.8754561432).abs() < 0.01 && (y - 326010.0093383705).abs() < 0.01,
            "{x} {y}"
        );
    }

    #[test]
    fn transformer_transdatro() {
        let s70 = Crs::from_epsg(3844).unwrap();
        let etrs = Crs::from_epsg(4258).unwrap();
        // P1 din setul oficial: 47°42'56.4" 22°28'32" -> N 693771.731, E 310723.518
        let lat = 47.0 + 42.0 / 60.0 + 56.4 / 3600.0;
        let lon = 22.0 + 28.0 / 60.0 + 32.0 / 3600.0;
        let t = Transformer::with_transdatro(&etrs, &s70, true).unwrap();
        let ((e, n), m) = t.xy_method(lon, lat).unwrap();
        assert_eq!(m, Method::TransDatRO);
        assert!((e - 310723.518).abs() < 0.003 && (n - 693771.731).abs() < 0.003);
        // în afara gridului: rezervă Helmert
        let ((e2, _), m2) = t.xy_method(19.0, 47.0).unwrap();
        assert_eq!(m2, Method::Fallback);
        assert!(e2.is_finite());
        // Stereo 30 <-> Stereo 70 prin ETRS89
        let s30 = Crs::stereo30();
        let t = Transformer::new(&s30, &s70).unwrap();
        let ((e, n), m) = t.xy_method(543496.555, 350692.549).unwrap();
        assert_eq!(m, Method::TransDatRO);
        let back = Transformer::new(&s70, &s30).unwrap();
        let (e3, n3) = back.xy(e, n).unwrap();
        assert!((e3 - 543496.555).abs() < 0.002 && (n3 - 350692.549).abs() < 0.002);
        // .prj pentru Stereo 30 se recitește corect
        let w = s30.to_wkt().unwrap();
        assert_eq!(Crs::from_wkt(&w).unwrap(), s30);
        assert_eq!(Crs::parse("stereo30").unwrap(), s30);
        assert_eq!(Crs::parse(&s30.to_user_string()).unwrap(), s30);
    }

    #[test]
    fn esri_stereo70_prj() {
        let prj = r#"PROJCS["Stereo_70",GEOGCS["GCS_Pulkovo_1942",DATUM["D_Pulkovo_1942",SPHEROID["Krasovsky_1940",6378245.0,298.3]],PRIMEM["Greenwich",0.0],UNIT["Degree",0.0174532925199433]],PROJECTION["Double_Stereographic"],PARAMETER["False_Easting",500000.0],PARAMETER["False_Northing",500000.0],PARAMETER["Central_Meridian",25.0],PARAMETER["Scale_Factor",0.99975],PARAMETER["Latitude_Of_Origin",46.0],UNIT["Meter",1.0]]"#;
        let c = Crs::from_wkt(prj).unwrap();
        assert_eq!(c.epsg, Some(3844));
        let mut renamed = prj.replace("Stereo_70", "Proiectie_X");
        renamed = renamed.replace("GCS_Pulkovo_1942", "GCS_X");
        let c2 = Crs::from_wkt(&renamed).unwrap();
        assert_eq!(c2.epsg, Some(3844), "{}", c2.proj);
    }

    #[test]
    fn epsg_wkt_roundtrip() {
        for code in [3844, 4326, 25835, 32635, 3857, 3035, 4258] {
            let c = Crs::from_epsg(code).unwrap();
            let w = c.to_wkt().unwrap();
            let c2 = Crs::from_wkt(&w).unwrap();
            assert_eq!(c2.epsg, Some(code));
        }
    }

    #[test]
    fn parse_variants() {
        assert_eq!(Crs::parse("EPSG:3844").unwrap().epsg, Some(3844));
        assert_eq!(Crs::parse("3844").unwrap().epsg, Some(3844));
        assert_eq!(Crs::parse("stereo70").unwrap().epsg, Some(3844));
        assert_eq!(
            Crs::parse(
                "+proj=sterea +lat_0=46 +lon_0=25 +k=0.99975 +x_0=500000 +y_0=500000 +ellps=krass"
            )
            .unwrap()
            .epsg,
            Some(3844)
        );
        assert_eq!(
            Crs::parse("+proj=utm +zone=35 +ellps=GRS80 +towgs84=0,0,0 +units=m +no_defs")
                .unwrap()
                .epsg,
            Some(25835)
        );
        assert!(
            Crs::parse("+proj=utm +zone=35 +ellps=intl")
                .unwrap()
                .epsg
                .is_none()
        );
    }
}
