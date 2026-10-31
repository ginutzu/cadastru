//! CGXML (fișiere cadastrale ANCPI, schema CGXMLSchema_new_v3.xsd) -> straturi „teren” și „constructii”.
//!
//! Portare fidelă a nucleului CGXML2SHP v3.0 (github.com/ginutzu/cgxml, `core.py`):
//! aceleași reguli de grupare a punctelor, împărțire în inele, clasificare exterior /
//! enclavă / parte multiplă, orientare ESRI și aceleași câmpuri ca în teren.dbf / constructii.dbf.
//! Fișierele sunt convertite în memorie și afișate ca straturi shapefile; pot fi exportate oricând.

use crate::crs::Crs;
use crate::model::*;
use anyhow::{Context, Result, anyhow, bail};
use std::collections::HashMap;
use std::io::Read;
use std::path::Path;
use crate::i18n::tr;

/// Toleranța la compararea coordonatelor (m), ca în original.
const COORD_EPS: f64 = 0.0005;

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CgxmlOptions {
    /// Închide inelul poligonului dacă ultimul punct diferă de primul.
    pub close_ring: bool,
    /// Atribute extinse (MEAS_AREA, AREA_CALC, AREA_DIFF, E2IDENTIF, CADSECTOR …).
    pub rich_dbf: bool,
    /// Generează și stratul de construcții.
    pub buildings: bool,
    /// Inversează X și Y (X = Nord, Y = Est în fișier).
    pub swap_xy: bool,
    /// Include CNP/CUI în PROPRIETAR (date cu caracter personal).
    pub id_code: bool,
}

impl Default for CgxmlOptions {
    fn default() -> Self {
        CgxmlOptions {
            close_ring: true,
            rich_dbf: true,
            buildings: true,
            swap_xy: false,
            id_code: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Conversii numerice cu semantica JavaScript parseInt / parseFloat
// ---------------------------------------------------------------------------

fn js_int(s: &str) -> Option<i64> {
    let t = s.trim_start();
    let b = t.as_bytes();
    let mut i = 0;
    if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
        i += 1;
    }
    let st = i;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    if i == st {
        return None;
    }
    t[..i].parse::<i64>().ok()
}

fn js_float(s: &str) -> Option<f64> {
    let t = s.trim_start();
    let b = t.as_bytes();
    let mut i = 0;
    if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
        i += 1;
    }
    let mut digits = 0;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
        digits += 1;
    }
    if i < b.len() && b[i] == b'.' {
        i += 1;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
            digits += 1;
        }
    }
    if digits == 0 {
        let r = t.trim();
        if r.starts_with("Infinity") || r.starts_with("+Infinity") {
            return Some(f64::INFINITY);
        }
        if r.starts_with("-Infinity") {
            return Some(f64::NEG_INFINITY);
        }
        return None;
    }
    // exponent
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        let mut j = i + 1;
        if j < b.len() && (b[j] == b'+' || b[j] == b'-') {
            j += 1;
        }
        let es = j;
        while j < b.len() && b[j].is_ascii_digit() {
            j += 1;
        }
        if j > es {
            i = j;
        }
    }
    t[..i].parse::<f64>().ok()
}

// ---------------------------------------------------------------------------
// Parsare
// ---------------------------------------------------------------------------

struct Row(HashMap<String, String>);

impl Row {
    fn ct(&self, k: &str) -> &str {
        self.0.get(k).map(|s| s.as_str()).unwrap_or("")
    }
    fn ci(&self, k: &str) -> Option<i64> {
        let t = self.ct(k);
        if t.is_empty() { None } else { js_int(t) }
    }
    fn cf(&self, k: &str) -> Option<f64> {
        let t = self.ct(k);
        if t.is_empty() { None } else { js_float(t) }
    }
}

/// Ordinea de iterare a cheilor numerice dintr-un obiect JavaScript:
/// nenegativele crescător, apoi negativele în ordinea inserării.
struct JsMap<V> {
    map: HashMap<i64, V>,
    order: Vec<i64>,
}

impl<V> JsMap<V> {
    fn new() -> Self {
        JsMap {
            map: HashMap::new(),
            order: vec![],
        }
    }
    fn entry(&mut self, k: i64, init: impl FnOnce() -> V) -> &mut V {
        if !self.map.contains_key(&k) {
            self.order.push(k);
            self.map.insert(k, init());
        }
        self.map.get_mut(&k).unwrap()
    }
    fn keys(&self) -> Vec<i64> {
        let mut nonneg: Vec<i64> = self.order.iter().copied().filter(|k| *k >= 0).collect();
        nonneg.sort();
        nonneg.extend(self.order.iter().copied().filter(|k| *k < 0));
        nonneg
    }
}

fn decode_xml(data: &[u8]) -> String {
    if let Some((enc, bom)) = encoding_rs::Encoding::for_bom(data) {
        return enc.decode_without_bom_handling(&data[bom..]).0.into_owned();
    }
    // declarația <?xml ... encoding="..."?>
    let head = String::from_utf8_lossy(&data[..data.len().min(200)]).to_lowercase();
    if let Some(p) = head.find("encoding=") {
        let rest = &head[p + 9..];
        let q = rest.chars().next().unwrap_or('"');
        if let Some(e) = rest[1..].find(q) {
            let label = &rest[1..1 + e];
            if let Some(enc) = encoding_rs::Encoding::for_label(label.as_bytes())
                && enc != encoding_rs::UTF_8
            {
                return enc.decode_without_bom_handling(data).0.into_owned();
            }
        }
    }
    String::from_utf8_lossy(data).into_owned()
}

/// Statistici de conversie (pentru jurnal / bara de stare).
#[derive(Clone, Debug, Default)]
pub struct CgxmlStats {
    pub files: usize,
    pub lands: usize,
    pub raw_points: usize,
    pub building_points: usize,
    pub skipped: usize,
    pub dropped_points: usize,
    pub enclaves: usize,
    pub multipart: usize,
    pub errors: Vec<String>,
}

struct PolyOut {
    geom: Geometry,
    n_outer: usize,
    n_holes: usize,
    area: f64,
}

struct Parsed {
    lands: Vec<(PolyOut, Vec<(&'static str, Value)>)>,
    buildings: Vec<(PolyOut, Vec<(&'static str, Value)>)>,
    n_lands: usize,
    n_raw: usize,
    n_bld_pts: usize,
    n_skipped: usize,
    n_dropped: usize,
}

fn parse_cgxml(data: &[u8], filename: &str, o: &CgxmlOptions) -> Result<Parsed> {
    let text = decode_xml(data);
    let doc = roxmltree::Document::parse_with_options(
        text.trim_start_matches('\u{feff}'),
        roxmltree::ParsingOptions {
            allow_dtd: true,
            ..Default::default()
        },
    )
    .map_err(|e| anyhow!("{}", tr!("XML invalid: {e}")))?;
    let wanted = [
        "FileDescription",
        "Deed",
        "Person",
        "Land",
        "Parcel",
        "Building",
        "Points",
    ];
    let mut t: HashMap<&str, Vec<Row>> = wanted.iter().map(|n| (*n, vec![])).collect();
    for el in doc.descendants().filter(|n| n.is_element()) {
        let name = el.tag_name().name();
        if let Some(v) = t.get_mut(name) {
            let mut m = HashMap::new();
            for ch in el.children().filter(|c| c.is_element()) {
                let k = ch.tag_name().name().to_string();
                if !m.contains_key(&k) {
                    let s: String = ch
                        .descendants()
                        .filter(|d| d.is_text())
                        .map(|d| d.text().unwrap_or(""))
                        .collect();
                    m.insert(k, s.trim().to_string());
                }
            }
            v.push(Row(m));
        }
    }

    let mut fd_map: HashMap<i64, (String, String, String)> = HashMap::new();
    for r in &t["FileDescription"] {
        if let Some(fid) = r.ci("FILEID") {
            fd_map.insert(
                fid,
                (
                    r.ct("FILENAME").to_string(),
                    r.ct("EXPORTDATE").chars().take(10).collect(),
                    r.ct("OPERATIONTYPE").to_string(),
                ),
            );
        }
    }
    let mut deeds: HashMap<i64, Vec<String>> = HashMap::new();
    for r in &t["Deed"] {
        let Some(fid) = r.ci("FILEID") else { continue };
        let date: String = r.ct("DEEDDATE").chars().take(10).collect();
        let s = [r.ct("DEEDTYPE"), r.ct("DEEDNUMBER"), date.as_str()]
            .iter()
            .filter(|x| !x.is_empty())
            .copied()
            .collect::<Vec<_>>()
            .join("/");
        if !s.is_empty() {
            deeds.entry(fid).or_default().push(s);
        }
    }
    let mut persons: HashMap<i64, Vec<String>> = HashMap::new();
    for r in &t["Person"] {
        let Some(fid) = r.ci("FILEID") else { continue };
        let mut s = [r.ct("FIRSTNAME"), r.ct("LASTNAME")]
            .iter()
            .filter(|x| !x.is_empty())
            .copied()
            .collect::<Vec<_>>()
            .join(" ");
        let idc = r.ct("IDCODE");
        if !idc.is_empty() && o.id_code {
            if !s.is_empty() {
                s.push(' ');
            }
            s.push_str(&format!("({idc})"));
        }
        if !s.is_empty() {
            persons.entry(fid).or_default().push(s);
        }
    }
    struct Land {
        fileid: Option<i64>,
        cadsector: String,
        measured: Option<f64>,
        papercadno: String,
        e2: String,
        topono: String,
    }
    let mut land_map: HashMap<i64, Land> = HashMap::new();
    for r in &t["Land"] {
        if let Some(lid) = r.ci("LANDID") {
            land_map.insert(
                lid,
                Land {
                    fileid: r.ci("FILEID"),
                    cadsector: r.ct("CADSECTOR").to_string(),
                    measured: r.cf("MEASUREDAREA"),
                    papercadno: r.ct("PAPERCADNO").to_string(),
                    e2: r.ct("E2IDENTIFIER").to_string(),
                    topono: r.ct("TOPONO").to_string(),
                },
            );
        }
    }
    let mut parcels: HashMap<i64, Vec<String>> = HashMap::new();
    for r in &t["Parcel"] {
        let Some(lid) = r.ci("LANDID") else { continue };
        let uc = r.ct("USECATEGORY");
        let l = parcels.entry(lid).or_default();
        if !uc.is_empty() && !l.iter().any(|x| x == uc) {
            l.push(uc.to_string());
        }
    }
    let mut bld_map: HashMap<i64, (Option<i64>, Option<i64>, Option<f64>)> = HashMap::new();
    if o.buildings {
        for r in &t["Building"] {
            if let Some(bid) = r.ci("BUILDINGID") {
                bld_map.insert(bid, (r.ci("LANDID"), r.ci("BUILDNO"), r.cf("MEASUREDAREA")));
            }
        }
    }

    // puncte: cele cu BUILDINGID aparțin construcției și nu intră în conturul terenului
    let mut pts_land: JsMap<Vec<(f64, f64, f64)>> = JsMap::new();
    let mut pts_bld: JsMap<Vec<(f64, f64, f64)>> = JsMap::new();
    let (mut n_raw, mut n_bld_pts) = (0usize, 0usize);
    for r in &t["Points"] {
        let imm = r.ci("IMMOVABLEID");
        let bid = r.ci("BUILDINGID");
        let (Some(x), Some(y)) = (r.cf("X"), r.cf("Y")) else {
            continue;
        };
        if x.is_nan() || y.is_nan() {
            continue;
        }
        n_raw += 1;
        let no_s = r.ct("NO");
        let no = if no_s.is_empty() {
            Some(n_raw as f64)
        } else {
            js_float(no_s)
        };
        let no = match no {
            Some(v) if !v.is_nan() => v,
            _ => n_raw as f64,
        };
        let pt = if o.swap_xy { (no, y, x) } else { (no, x, y) };
        if let Some(b) = bid
            && b != 0
        {
            n_bld_pts += 1;
            if o.buildings {
                pts_bld.entry(b, Vec::new).push(pt);
            }
            continue;
        }
        if let Some(i) = imm {
            pts_land.entry(i, Vec::new).push(pt);
        }
    }
    let to_coords = |pts: &Vec<(f64, f64, f64)>| -> Vec<[f64; 2]> {
        let mut p = pts.clone();
        // sortare stabilă după NO (ca Array.sort / sorted)
        p.sort_by(|a, b| a.0.total_cmp(&b.0));
        p.iter().map(|q| [q.1, q.2]).collect()
    };

    let mut out = Parsed {
        lands: vec![],
        buildings: vec![],
        n_lands: land_map.len(),
        n_raw,
        n_bld_pts,
        n_skipped: 0,
        n_dropped: 0,
    };
    for lid in pts_land.keys() {
        let geom = build_polygon(&to_coords(&pts_land.map[&lid]), o.close_ring);
        out.n_dropped += geom.1;
        let Some(poly) = geom.0 else {
            out.n_skipped += 1;
            continue;
        };
        let land = land_map.get(&lid);
        let fd = land.and_then(|l| l.fileid).and_then(|f| fd_map.get(&f));
        let meas = land
            .and_then(|l| l.measured)
            .filter(|m| !m.is_nan())
            .unwrap_or(0.0);
        let diff = if meas > 0.0 {
            (poly.area - meas).abs() / meas * 100.0
        } else {
            0.0
        };
        let fileid = land.and_then(|l| l.fileid);
        let txt = |s: Option<&String>| Value::Text(s.cloned().unwrap_or_default());
        let mut a: Vec<(&'static str, Value)> = vec![("LANDID", Value::Int(lid))];
        if o.rich_dbf {
            a.push(("MEAS_AREA", Value::Real(meas)));
            a.push(("AREA_CALC", Value::Real(poly.area)));
            a.push(("AREA_DIFF", Value::Real(diff)));
        }
        a.push(("PAPERCADNO", txt(land.map(|l| &l.papercadno))));
        if o.rich_dbf {
            a.push(("E2IDENTIF", txt(land.map(|l| &l.e2))));
            a.push(("CADSECTOR", txt(land.map(|l| &l.cadsector))));
            a.push(("TOPONO", txt(land.map(|l| &l.topono))));
        }
        a.push((
            "CATFOLOS",
            Value::Text(parcels.get(&lid).map(|v| v.join(",")).unwrap_or_default()),
        ));
        a.push((
            "ACTE",
            Value::Text(
                fileid
                    .and_then(|f| deeds.get(&f))
                    .map(|v| v.join("; "))
                    .unwrap_or_default(),
            ),
        ));
        a.push((
            "PROPRIETAR",
            Value::Text(
                fileid
                    .and_then(|f| persons.get(&f))
                    .map(|v| v.join("; "))
                    .unwrap_or_default(),
            ),
        ));
        a.push((
            "FILENAME",
            Value::Text(
                fd.map(|f| f.0.clone())
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| filename.to_string()),
            ),
        ));
        if o.rich_dbf {
            a.push((
                "OPERTYPE",
                Value::Text(fd.map(|f| f.2.clone()).unwrap_or_default()),
            ));
            a.push((
                "EXPORTDATE",
                Value::Text(fd.map(|f| f.1.clone()).unwrap_or_default()),
            ));
        }
        out.lands.push((poly, a));
    }
    if o.buildings {
        for bid in pts_bld.keys() {
            let geom = build_polygon(&to_coords(&pts_bld.map[&bid]), o.close_ring);
            out.n_dropped += geom.1;
            let Some(poly) = geom.0 else { continue };
            let b = bld_map.get(&bid);
            let ma = b.and_then(|b| b.2).filter(|m| !m.is_nan()).unwrap_or(0.0);
            let a: Vec<(&'static str, Value)> = vec![
                ("BUILDINGID", Value::Int(bid)),
                ("LANDID", Value::Int(b.and_then(|b| b.0).unwrap_or(0))),
                ("BUILDNO", Value::Int(b.and_then(|b| b.1).unwrap_or(0))),
                ("MEAS_AREA", Value::Real(ma)),
                ("AREA_CALC", Value::Real(poly.area)),
                ("FILENAME", Value::Text(filename.to_string())),
            ];
            out.buildings.push((poly, a));
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Geometrie (identică cu core.py)
// ---------------------------------------------------------------------------

fn same_pt(a: [f64; 2], b: [f64; 2]) -> bool {
    (a[0] - b[0]).abs() < COORD_EPS && (a[1] - b[1]).abs() < COORD_EPS
}

/// Arie cu semn (shoelace): pozitivă = CCW, negativă = CW.
fn signed_area(r: &[[f64; 2]]) -> f64 {
    let m = r.len();
    if m == 0 {
        return 0.0;
    }
    let mut a = 0.0;
    let mut j = m - 1;
    for i in 0..m {
        a += (r[j][0] + r[i][0]) * (r[j][1] - r[i][1]);
        j = i;
    }
    -a / 2.0
}

fn pt_in_ring(p: [f64; 2], r: &[[f64; 2]]) -> bool {
    let mut inside = false;
    let n = r.len();
    if n == 0 {
        return false;
    }
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi, xj, yj) = (r[i][0], r[i][1], r[j][0], r[j][1]);
        if ((yi > p[1]) != (yj > p[1])) && (p[0] < (xj - xi) * (p[1] - yi) / (yj - yi) + xi) {
            inside = !inside;
        }
        j = i;
    }
    inside
}

fn ring_inside(inner: &[[f64; 2]], outer: &[[f64; 2]]) -> bool {
    let mut n = inner.len();
    if n > 1 && same_pt(inner[0], inner[n - 1]) {
        n -= 1;
    }
    let (mut votes, mut total) = (0, 0);
    for p in &inner[..n] {
        if outer.iter().any(|o| same_pt(*p, *o)) {
            continue;
        }
        total += 1;
        if pt_in_ring(*p, outer) {
            votes += 1;
        }
    }
    total > 0 && votes * 2 > total
}

fn split_into_rings(coords: &[[f64; 2]], do_close: bool) -> (Vec<Vec<[f64; 2]>>, usize) {
    let mut c: Vec<[f64; 2]> = vec![];
    for p in coords {
        if c.last().is_none_or(|l| !same_pt(*l, *p)) {
            c.push(*p);
        }
    }
    let mut rings = vec![];
    let mut start = 0;
    let mut i = start + 1;
    while i < c.len() {
        if i - start >= 3 && same_pt(c[i], c[start]) {
            let mut ring = c[start..=i].to_vec();
            let l = ring.len();
            ring[l - 1] = ring[0];
            rings.push(ring);
            start = i + 1;
            i = start;
        }
        i += 1;
    }
    let mut rem: Vec<[f64; 2]> = c[start.min(c.len())..].to_vec();
    let mut dropped = 0;
    if rem.len() >= 3 {
        if do_close && !same_pt(rem[0], rem[rem.len() - 1]) {
            rem.push(rem[0]);
        }
        rings.push(rem);
    } else {
        dropped += rem.len();
    }
    (rings, dropped)
}

/// Inele orientate ESRI (exterior CW, enclave CCW); inelele disjuncte devin părți multiple.
fn build_polygon(coords: &[[f64; 2]], do_close: bool) -> (Option<PolyOut>, usize) {
    let (mut rings, dropped) = split_into_rings(coords, do_close);
    if rings.is_empty() {
        return (None, dropped);
    }
    // sortare stabilă descrescător după arie
    rings.sort_by(|a, b| signed_area(b).abs().total_cmp(&signed_area(a).abs()));
    let mut oriented: Vec<(Vec<[f64; 2]>, bool)> = vec![];
    let (mut n_outer, mut n_holes, mut area) = (0, 0, 0.0);
    for i in 0..rings.len() {
        let depth = (0..i)
            .filter(|&j| ring_inside(&rings[i], &rings[j]))
            .count();
        let is_hole = depth % 2 == 1;
        let sa = signed_area(&rings[i]);
        let mut r = rings[i].clone();
        if (is_hole && sa < 0.0) || (!is_hole && sa > 0.0) {
            r.reverse();
        }
        if is_hole {
            n_holes += 1;
            area -= sa.abs();
        } else {
            n_outer += 1;
            area += sa.abs();
        }
        oriented.push((r, is_hole));
    }
    // grupare pentru modelul de date: fiecare enclavă la cel mai mic exterior care o conține
    let mut polys: Vec<Vec<Ring>> = vec![];
    let mut outer_idx: Vec<(usize, Vec<[f64; 2]>)> = vec![];
    let to_ring =
        |r: &Vec<[f64; 2]>| -> Ring { r.iter().map(|p| Coord::new(p[0], p[1])).collect() };
    for (r, hole) in &oriented {
        if !hole {
            outer_idx.push((polys.len(), r.clone()));
            polys.push(vec![to_ring(r)]);
        }
    }
    for (r, hole) in &oriented {
        if *hole {
            // inelele sunt sortate descrescător: ultimul exterior care conține enclava e cel mai mic
            let target = outer_idx
                .iter()
                .rev()
                .find(|(_, o)| ring_inside(r, o))
                .map(|(i, _)| *i);
            match target {
                Some(i) => polys[i].push(to_ring(r)),
                None => polys.push(vec![to_ring(r)]),
            }
        }
    }
    let geom = if polys.len() == 1 {
        Geometry::Polygon(polys.pop().unwrap())
    } else {
        Geometry::MultiPolygon(polys)
    };
    (
        Some(PolyOut {
            geom,
            n_outer,
            n_holes,
            area,
        }),
        dropped,
    )
}

// ---------------------------------------------------------------------------
// Colectarea intrărilor și construirea straturilor
// ---------------------------------------------------------------------------

fn is_cgxml_name(rel: &str) -> bool {
    let p = rel.replace('\\', "/");
    let base = p.rsplit('/').next().unwrap_or("");
    if p.to_lowercase().contains("__macosx/") || base.starts_with("._") {
        return false;
    }
    base.to_lowercase().ends_with(".cgxml")
}

/// Sursele CGXML dintr-o cale: fișier .cgxml, arhivă .zip sau dosar (recursiv, inclusiv arhive).
pub fn collect_sources(path: &Path) -> Result<Vec<(String, Vec<u8>)>> {
    let mut out = vec![];
    if path.is_dir() {
        let mut stack = vec![path.to_path_buf()];
        let mut files = vec![];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d)? {
                let p = e?.path();
                if p.is_dir() {
                    stack.push(p);
                } else {
                    files.push(p);
                }
            }
        }
        files.sort();
        for f in files {
            let rel = f
                .strip_prefix(path)
                .unwrap_or(&f)
                .to_string_lossy()
                .replace('\\', "/");
            if is_cgxml_name(&rel) {
                out.push((rel, std::fs::read(&f)?));
            } else if super::ext_of(&f) == "zip" {
                for (n, d) in zip_sources(&f)? {
                    out.push((format!("{rel}:{n}"), d));
                }
            }
        }
    } else if super::ext_of(path) == "zip" {
        out = zip_sources(path)?;
    } else {
        let name = path
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        out.push((
            name,
            std::fs::read(path).with_context(|| tr!("Nu pot citi {}", path.display()))?,
        ));
    }
    Ok(out)
}

fn zip_sources(path: &Path) -> Result<Vec<(String, Vec<u8>)>> {
    let f = std::fs::File::open(path).with_context(|| tr!("Nu pot citi {}", path.display()))?;
    let mut z = zip::ZipArchive::new(f)
        .map_err(|e| anyhow!("{}", tr!("Arhivă ZIP invalidă: {} ({e})", path.display())))?;
    let mut out = vec![];
    for i in 0..z.len() {
        let mut e = z.by_index(i)?;
        if e.is_dir() {
            continue;
        }
        let name = e.name().to_string();
        if is_cgxml_name(&name) {
            let mut d = Vec::with_capacity(e.size() as usize);
            e.read_to_end(&mut d)?;
            out.push((name, d));
        }
    }
    Ok(out)
}

/// Verifică rapid dacă o arhivă ZIP conține fișiere CGXML.
pub fn zip_has_cgxml(path: &Path) -> bool {
    std::fs::File::open(path)
        .ok()
        .and_then(|f| zip::ZipArchive::new(f).ok())
        .is_some_and(|z| z.file_names().any(is_cgxml_name))
}

fn layer_from(
    name: &str,
    items: Vec<(PolyOut, Vec<(&'static str, Value)>)>,
    kind_building: bool,
    rich: bool,
) -> VectorLayer {
    let mut l = VectorLayer::new(name);
    // schema identică cu teren.dbf / constructii.dbf
    let schema: Vec<(&str, FieldType, usize, usize)> = if kind_building {
        vec![
            ("ID", FieldType::Integer, 10, 0),
            ("BUILDINGID", FieldType::Integer, 10, 0),
            ("LANDID", FieldType::Integer, 10, 0),
            ("BUILDNO", FieldType::Integer, 6, 0),
            ("MEAS_AREA", FieldType::Real, 16, 3),
            ("AREA_CALC", FieldType::Real, 16, 3),
            ("FILENAME", FieldType::Text, 100, 0),
        ]
    } else if rich {
        vec![
            ("ID", FieldType::Integer, 10, 0),
            ("LANDID", FieldType::Integer, 10, 0),
            ("MEAS_AREA", FieldType::Real, 16, 3),
            ("AREA_CALC", FieldType::Real, 16, 3),
            ("AREA_DIFF", FieldType::Real, 10, 2),
            ("PAPERCADNO", FieldType::Text, 50, 0),
            ("E2IDENTIF", FieldType::Text, 50, 0),
            ("CADSECTOR", FieldType::Text, 50, 0),
            ("TOPONO", FieldType::Text, 50, 0),
            ("CATFOLOS", FieldType::Text, 50, 0),
            ("ACTE", FieldType::Text, 254, 0),
            ("PROPRIETAR", FieldType::Text, 254, 0),
            ("FILENAME", FieldType::Text, 100, 0),
            ("OPERTYPE", FieldType::Text, 50, 0),
            ("EXPORTDATE", FieldType::Text, 10, 0),
        ]
    } else {
        vec![
            ("ID", FieldType::Integer, 10, 0),
            ("LANDID", FieldType::Integer, 10, 0),
            ("PAPERCADNO", FieldType::Text, 50, 0),
            ("CATFOLOS", FieldType::Text, 50, 0),
            ("ACTE", FieldType::Text, 254, 0),
            ("PROPRIETAR", FieldType::Text, 254, 0),
            ("FILENAME", FieldType::Text, 100, 0),
        ]
    };
    l.fields = schema
        .iter()
        .map(|(n, t, w, p)| Field {
            name: n.to_string(),
            ftype: *t,
            width: *w,
            precision: *p,
        })
        .collect();
    for (i, (poly, attrs)) in items.into_iter().enumerate() {
        let mut vals = vec![Value::Null; l.fields.len()];
        vals[0] = Value::Int(i as i64 + 1);
        for (k, v) in attrs {
            if let Some(fi) = l.fields.iter().position(|f| f.name == k) {
                vals[fi] = v;
            }
        }
        l.features.push(Feature {
            geom: Some(poly.geom),
            attrs: vals,
        });
    }
    l.crs = Crs::from_epsg(3844).ok();
    l
}

/// Convertește toate fișierele CGXML dintr-o cale în straturile „teren” și (opțional) „constructii”.
pub fn read(path: &Path, o: &CgxmlOptions) -> Result<(Vec<VectorLayer>, CgxmlStats)> {
    let sources = collect_sources(path)?;
    if sources.is_empty() {
        bail!("{}", tr!("Nu există fișiere .cgxml în {}", path.display()));
    }
    let mut st = CgxmlStats {
        files: sources.len(),
        ..Default::default()
    };
    let mut lands = vec![];
    let mut blds = vec![];
    for (name, data) in &sources {
        let fname = name
            .rsplit('/')
            .next()
            .unwrap_or(name)
            .rsplit(':')
            .next()
            .unwrap_or(name);
        match parse_cgxml(data, fname, o) {
            Ok(p) => {
                st.lands += p.n_lands;
                st.raw_points += p.n_raw;
                st.building_points += p.n_bld_pts;
                st.skipped += p.n_skipped;
                st.dropped_points += p.n_dropped;
                for (poly, _) in &p.lands {
                    if poly.n_holes > 0 {
                        st.enclaves += 1;
                    }
                    if poly.n_outer > 1 {
                        st.multipart += 1;
                    }
                }
                lands.extend(p.lands);
                blds.extend(p.buildings);
            }
            Err(e) => st.errors.push(format!("{name}: {e:#}")),
        }
    }
    if lands.is_empty() {
        bail!(
            "{}", tr!("Niciun poligon Land valid{}",
            if st.errors.is_empty() {
                String::new()
            } else {
                format!(" ({})", st.errors.join("; "))
            }
        ));
    }
    let mut out = vec![];
    let mut teren = layer_from("teren", lands, false, o.rich_dbf);
    teren.source = Some(path.to_path_buf());
    out.push(teren);
    if o.buildings && !blds.is_empty() {
        let mut c = layer_from("constructii", blds, true, false);
        c.source = Some(path.to_path_buf());
        out.push(c);
    }
    Ok((out, st))
}

impl CgxmlStats {
    pub fn summary(&self) -> String {
        let mut s = tr!(
            "CGXML: {} fișier(e), {} terenuri, {} puncte",
            self.files, self.lands, self.raw_points
        );
        if self.enclaves > 0 {
            s.push_str(&tr!(", {} cu enclave", self.enclaves));
        }
        if self.multipart > 0 {
            s.push_str(&tr!(", {} multipart", self.multipart));
        }
        if self.skipped > 0 {
            s.push_str(&tr!(", {} ignorate (< 3 puncte)", self.skipped));
        }
        if self.dropped_points > 0 {
            s.push_str(&tr!(
                ", {} puncte în afara unui inel valid",
                self.dropped_points
            ));
        }
        if !self.errors.is_empty() {
            s.push_str(&tr!(", {} fișiere cu erori", self.errors.len()));
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn js_numbers() {
        assert_eq!(js_int(" 12abc"), Some(12));
        assert_eq!(js_int("x"), None);
        assert_eq!(js_float("587123.45m"), Some(587123.45));
        assert_eq!(js_float("1e3"), Some(1000.0));
        assert_eq!(js_float(".5"), Some(0.5));
    }

    #[test]
    fn rings_enclave_multipart() {
        // exterior 100x100, enclavă 10x10, parte separată
        let mut c = vec![
            [0.0, 0.0],
            [100.0, 0.0],
            [100.0, 100.0],
            [0.0, 100.0],
            [0.0, 0.0],
        ];
        c.extend([
            [40.0, 40.0],
            [50.0, 40.0],
            [50.0, 50.0],
            [40.0, 50.0],
            [40.0, 40.0],
        ]);
        c.extend([[200.0, 0.0], [220.0, 0.0], [220.0, 20.0]]);
        let (p, d) = build_polygon(&c, true);
        let p = p.unwrap();
        assert_eq!((p.n_outer, p.n_holes, d), (2, 1, 0));
        assert!((p.area - (10000.0 - 100.0 + 200.0)).abs() < 1e-9);
        match p.geom {
            Geometry::MultiPolygon(v) => {
                assert_eq!(v.len(), 2);
                assert_eq!(v[0].len(), 2);
            }
            _ => panic!(),
        }
    }
}
