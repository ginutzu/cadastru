//! ESRI Shapefile (.shp/.shx/.dbf/.prj/.cpg) – citire și scriere.

use super::{ReadOptions, WriteOptions, encoding_from_label, read_prj_sidecar, write_prj_sidecar};
use crate::model::*;
use anyhow::{Context, Result, bail};
use encoding_rs::Encoding;
use std::fs;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use crate::i18n::tr;

fn be_i32(b: &[u8], o: usize) -> i32 {
    i32::from_be_bytes(b[o..o + 4].try_into().unwrap())
}
fn le_i32(b: &[u8], o: usize) -> i32 {
    i32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
fn le_f64(b: &[u8], o: usize) -> f64 {
    f64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}

fn sibling(path: &Path, ext: &str) -> Option<PathBuf> {
    // caută extensia în ambele variante (mici/mari)
    for e in [ext.to_lowercase(), ext.to_uppercase()] {
        let p = path.with_extension(&e);
        if p.exists() {
            return Some(p);
        }
    }
    None
}

/// Asamblează inelele unui poligon (shapefile, File Geodatabase).
///
/// Clasificarea se face după încadrare, ca în GDAL: un inel conținut într-un număr par
/// de alte inele este exterior, unul conținut într-un număr impar este gol al celui mai
/// mic inel care îl conține. Astfel se citesc corect și fișierele cu orientarea inelelor
/// greșită (goluri în sens orar).
pub fn assemble_rings(rings: Vec<Ring>) -> Option<Geometry> {
    let rings: Vec<Ring> = rings.into_iter().filter(|r| r.len() >= 3).collect();
    if rings.is_empty() {
        return None;
    }
    if rings.len() == 1 {
        return Some(Geometry::Polygon(rings));
    }
    let n = rings.len();
    let bbox: Vec<[f64; 4]> = rings
        .iter()
        .map(|r| {
            r.iter().fold(
                [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY],
                |b, c| [b[0].min(c.x), b[1].min(c.y), b[2].max(c.x), b[3].max(c.y)],
            )
        })
        .collect();
    let area: Vec<f64> = rings.iter().map(|r| ring_signed_area(r).abs()).collect();
    // inelul i este în inelul j? (vot pe câteva vârfuri, pentru inele care se ating)
    let inside = |i: usize, j: usize| -> bool {
        let (bi, bj) = (bbox[i], bbox[j]);
        if area[j] <= area[i] || bi[0] < bj[0] || bi[1] < bj[1] || bi[2] > bj[2] || bi[3] > bj[3]
        {
            return false;
        }
        let r = &rings[i];
        let k = r.len().min(7);
        let step = (r.len() / k).max(1);
        let (mut yes, mut no) = (0, 0);
        for t in (0..r.len()).step_by(step).take(k) {
            if point_in_ring(&rings[j], r[t].x, r[t].y) {
                yes += 1
            } else {
                no += 1
            }
        }
        yes > no
    };
    let mut depth = vec![0usize; n];
    let mut parent: Vec<Option<usize>> = vec![None; n];
    for i in 0..n {
        for j in 0..n {
            if i != j && inside(i, j) {
                depth[i] += 1;
                if parent[i].is_none_or(|p| area[j] < area[p]) {
                    parent[i] = Some(j);
                }
            }
        }
    }
    let mut slot: Vec<Option<usize>> = vec![None; n];
    let mut polys: Vec<Vec<Ring>> = Vec::new();
    // exterioarele (adâncime pară), în ordinea fișierului
    for i in 0..n {
        if depth[i] % 2 == 0 {
            slot[i] = Some(polys.len());
            polys.push(vec![rings[i].clone()]);
        }
    }
    for i in 0..n {
        if depth[i] % 2 == 1 {
            match parent[i].and_then(|p| slot[p]) {
                Some(k) => polys[k].push(rings[i].clone()),
                None => polys.push(vec![rings[i].clone()]),
            }
        }
    }
    if polys.len() == 1 {
        Some(Geometry::Polygon(polys.pop().unwrap()))
    } else {
        Some(Geometry::MultiPolygon(polys))
    }
}

fn read_shape(c: &[u8]) -> Result<Option<Geometry>> {
    if c.len() < 4 {
        return Ok(None);
    }
    let st = le_i32(c, 0);
    let has_z = matches!(st, 11 | 13 | 15 | 18 | 31);
    let need = |n: usize| -> Result<()> {
        if c.len() < n {
            bail!(tr!("înregistrare SHP trunchiată"))
        }
        Ok(())
    };
    match st {
        0 => Ok(None),
        1 | 11 | 21 => {
            need(20)?;
            let z = if has_z && c.len() >= 28 {
                le_f64(c, 20)
            } else {
                0.0
            };
            Ok(Some(Geometry::Point(Coord::xyz(
                le_f64(c, 4),
                le_f64(c, 12),
                z,
            ))))
        }
        8 | 18 | 28 => {
            need(40)?;
            let n = le_i32(c, 36) as usize;
            need(40 + n * 16)?;
            let zoff = 40 + n * 16 + 16;
            let pts = (0..n)
                .map(|i| {
                    let z = if has_z && c.len() >= zoff + (i + 1) * 8 {
                        le_f64(c, zoff + i * 8)
                    } else {
                        0.0
                    };
                    Coord::xyz(le_f64(c, 40 + i * 16), le_f64(c, 48 + i * 16), z)
                })
                .collect();
            Ok(Some(Geometry::MultiPoint(pts)))
        }
        3 | 13 | 23 | 5 | 15 | 25 | 31 => {
            need(44)?;
            let np = le_i32(c, 36) as usize;
            let n = le_i32(c, 40) as usize;
            let mut off = 44;
            let parts: Vec<usize> = (0..np).map(|i| le_i32(c, off + i * 4) as usize).collect();
            off += np * 4;
            let mut part_types = vec![];
            if st == 31 {
                part_types = (0..np).map(|i| le_i32(c, off + i * 4)).collect();
                off += np * 4;
            }
            need(off + n * 16)?;
            let poff = off;
            let zoff = poff + n * 16 + 16;
            let mut pts: Vec<Coord> = (0..n)
                .map(|i| Coord::new(le_f64(c, poff + i * 16), le_f64(c, poff + i * 16 + 8)))
                .collect();
            if has_z && c.len() >= zoff + n * 8 {
                for (i, p) in pts.iter_mut().enumerate() {
                    p.z = le_f64(c, zoff + i * 8);
                }
            }
            let mut segs: Vec<Vec<Coord>> = Vec::with_capacity(np);
            for i in 0..np {
                let s = parts[i].min(n);
                let e = if i + 1 < np { parts[i + 1].min(n) } else { n };
                if s < e {
                    segs.push(pts[s..e].to_vec());
                }
            }
            match st {
                3 | 13 | 23 => Ok(if segs.len() == 1 {
                    Some(Geometry::LineString(segs.pop().unwrap()))
                } else if segs.is_empty() {
                    None
                } else {
                    Some(Geometry::MultiLineString(segs))
                }),
                31 => {
                    // MultiPatch: benzi/evantaie de triunghiuri -> triunghiuri; inele -> poligoane
                    let mut polys: Vec<Vec<Ring>> = vec![];
                    for (i, s) in segs.iter().enumerate() {
                        match part_types.get(i).copied().unwrap_or(5) {
                            0 => {
                                for w in s.windows(3) {
                                    polys.push(vec![vec![w[0], w[1], w[2], w[0]]]);
                                }
                            }
                            1 => {
                                for j in 1..s.len().saturating_sub(1) {
                                    polys.push(vec![vec![s[0], s[j], s[j + 1], s[0]]]);
                                }
                            }
                            _ => polys.push(vec![s.clone()]),
                        }
                    }
                    Ok(Geometry::from_parts(
                        polys.into_iter().map(Geometry::Polygon).collect(),
                    ))
                }
                _ => Ok(assemble_rings(segs)),
            }
        }
        other => bail!("{}", tr!("Tip de formă SHP nesuportat: {other}")),
    }
}

struct DbfField {
    name: String,
    ftype: u8,
    len: usize,
    dec: usize,
}

fn ldid_encoding(ldid: u8) -> Option<&'static Encoding> {
    let label = match ldid {
        0x01 => "ibm437",
        0x02 => "ibm850",
        0x03 | 0x57 => "windows-1252",
        0x64 => "ibm852",
        0x65 => "ibm866",
        0x26 => "ibm866",
        0xC8 => "windows-1250",
        0xC9 => "windows-1251",
        0xCA => "windows-1254",
        0xCB => "windows-1253",
        0xCC => "windows-1257",
        _ => return None,
    };
    Encoding::for_label(label.as_bytes())
}

/// Citește un fișier DBF. Întoarce câmpurile, rândurile și indicii rândurilor șterse.
pub fn read_dbf(
    path: &Path,
    opts: &ReadOptions,
) -> Result<(Vec<Field>, Vec<Vec<Value>>, Vec<bool>)> {
    let b = fs::read(path).with_context(|| tr!("Nu pot citi {}", path.display()))?;
    if b.len() < 32 {
        bail!(tr!("DBF invalid"));
    }
    let nrec = u32::from_le_bytes(b[4..8].try_into().unwrap()) as usize;
    let hsize = u16::from_le_bytes(b[8..10].try_into().unwrap()) as usize;
    let rsize = u16::from_le_bytes(b[10..12].try_into().unwrap()) as usize;
    let ldid = b[29];
    let mut fields = vec![];
    let mut o = 32;
    while o + 32 <= b.len() && b[o] != 0x0D && o + 32 <= hsize {
        let raw = &b[o..o + 11];
        let end = raw.iter().position(|&c| c == 0).unwrap_or(11);
        fields.push(DbfField {
            name: String::from_utf8_lossy(&raw[..end]).trim().to_string(),
            ftype: b[o + 11],
            len: b[o + 16] as usize,
            dec: b[o + 17] as usize,
        });
        o += 32;
    }
    // codificarea: opțiune > .cpg > LDID > UTF-8 valid > cp1250
    let mut enc: Option<&'static Encoding> = opts.encoding.as_deref().and_then(encoding_from_label);
    if enc.is_none()
        && let Some(cpg) = sibling(path, "cpg")
        && let Ok(s) = fs::read_to_string(cpg)
    {
        enc = encoding_from_label(s.trim());
    }
    if enc.is_none() {
        enc = ldid_encoding(ldid);
        if ldid == 0x57 || ldid == 0x03 {
            // „ANSI” – în practică poate fi și UTF-8 sau cp1250; verificăm datele
            enc = None;
        }
    }
    let data = &b[hsize.min(b.len())..];
    let enc = enc.unwrap_or_else(|| {
        if std::str::from_utf8(data).is_ok() {
            encoding_rs::UTF_8
        } else {
            encoding_rs::WINDOWS_1250
        }
    });

    let schema: Vec<Field> = fields
        .iter()
        .map(|f| {
            let t = match f.ftype {
                b'N' | b'F' => {
                    if f.dec == 0 && f.len < 19 {
                        FieldType::Integer
                    } else {
                        FieldType::Real
                    }
                }
                b'I' | b'+' => FieldType::Integer,
                b'O' | b'B' | b'Y' => FieldType::Real,
                b'L' => FieldType::Boolean,
                b'D' => FieldType::Date,
                _ => FieldType::Text,
            };
            Field {
                name: enc.decode(f.name.as_bytes()).0.to_string(),
                ftype: t,
                width: f.len,
                precision: f.dec,
            }
        })
        .collect();

    let mut rows = Vec::with_capacity(nrec);
    let mut deleted = Vec::with_capacity(nrec);
    for r in 0..nrec {
        let s = hsize + r * rsize;
        if s + rsize > b.len() {
            break;
        }
        let rec = &b[s..s + rsize];
        deleted.push(rec[0] == b'*');
        let mut o = 1;
        let mut vals = Vec::with_capacity(fields.len());
        for (f, sf) in fields.iter().zip(&schema) {
            let raw = &rec[o..(o + f.len).min(rec.len())];
            o += f.len;
            let v = match f.ftype {
                b'I' | b'+' if raw.len() == 4 => {
                    Value::Int(i32::from_le_bytes(raw.try_into().unwrap()) as i64)
                }
                b'O' if raw.len() == 8 => Value::Real(f64::from_le_bytes(raw.try_into().unwrap())),
                _ => {
                    let txt = enc.decode(raw).0;
                    let t = txt.trim_end_matches(['\0', ' ']).trim_start();
                    match sf.ftype {
                        FieldType::Integer => {
                            if t.is_empty() || t.starts_with('*') {
                                Value::Null
                            } else {
                                t.parse::<i64>().map(Value::Int).unwrap_or_else(|_| {
                                    t.parse::<f64>()
                                        .map(|f| Value::Int(f as i64))
                                        .unwrap_or(Value::Null)
                                })
                            }
                        }
                        FieldType::Real => {
                            if t.is_empty() || t.starts_with('*') {
                                Value::Null
                            } else {
                                t.replace(',', ".")
                                    .parse::<f64>()
                                    .map(Value::Real)
                                    .unwrap_or(Value::Null)
                            }
                        }
                        FieldType::Boolean => match t.chars().next() {
                            Some('T' | 't' | 'Y' | 'y') => Value::Bool(true),
                            Some('F' | 'f' | 'N' | 'n') => Value::Bool(false),
                            _ => Value::Null,
                        },
                        FieldType::Date => {
                            if t.len() == 8 && t.chars().all(|c| c.is_ascii_digit()) {
                                Value::Text(format!("{}-{}-{}", &t[0..4], &t[4..6], &t[6..8]))
                            } else if t.is_empty() {
                                Value::Null
                            } else {
                                Value::Text(t.to_string())
                            }
                        }
                        FieldType::Text => {
                            if f.ftype == b'M' {
                                Value::Null
                            } else {
                                Value::Text(t.to_string())
                            }
                        }
                    }
                }
            };
            vals.push(v);
        }
        rows.push(vals);
    }
    Ok((schema, rows, deleted))
}

pub fn read(path: &Path, opts: &ReadOptions) -> Result<VectorLayer> {
    let b = fs::read(path).with_context(|| tr!("Nu pot citi {}", path.display()))?;
    if b.len() < 100 || be_i32(&b, 0) != 9994 {
        bail!("{}", tr!("{} nu este un fișier shapefile valid", path.display()));
    }
    let gtype = le_i32(&b, 32);
    let mut geoms: Vec<Option<Geometry>> = vec![];
    let mut o = 100;
    while o + 8 <= b.len() {
        let clen = be_i32(&b, o + 4) as usize * 2;
        let s = o + 8;
        let e = (s + clen).min(b.len());
        geoms.push(read_shape(&b[s..e]).unwrap_or(None));
        o = s + clen;
    }
    let mut layer = VectorLayer::new(
        path.file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default(),
    );
    layer.source = Some(path.to_path_buf());
    layer.has_z = matches!(gtype, 11 | 13 | 15 | 18 | 31);
    let (fields, rows, deleted) = match sibling(path, "dbf") {
        Some(dbf) => read_dbf(&dbf, opts)?,
        None => (vec![], vec![], vec![]),
    };
    layer.fields = fields;
    let n = geoms.len().max(rows.len());
    for i in 0..n {
        if deleted.get(i).copied().unwrap_or(false) {
            continue;
        }
        let geom = geoms.get_mut(i).and_then(|g| g.take());
        let attrs = rows
            .get(i)
            .cloned()
            .unwrap_or_else(|| vec![Value::Null; layer.fields.len()]);
        layer.features.push(Feature { geom, attrs });
    }
    layer.crs = read_prj_sidecar(path);
    Ok(layer)
}

// ---------------------------------------------------------------------------
// Scriere
// ---------------------------------------------------------------------------

fn kind_suffix(k: GeomKind) -> &'static str {
    match k {
        GeomKind::Point => "puncte",
        GeomKind::Line => "linii",
        GeomKind::Polygon => "poligoane",
        GeomKind::Mixed => "mixt",
    }
}

/// Scrie stratul; dacă are mai multe tipuri de geometrie se creează câte un fișier pentru fiecare.
pub fn write(layer: &VectorLayer, path: &Path, opts: &WriteOptions) -> Result<Vec<PathBuf>> {
    // grupare pe tipuri de geometrie (indici în layer.features)
    let mut idx_groups: Vec<(GeomKind, Vec<(Option<Geometry>, usize)>)> = vec![];
    let mut push = |k: GeomKind, g: Option<Geometry>, i: usize| {
        if let Some(e) = idx_groups.iter_mut().find(|(gk, _)| *gk == k) {
            e.1.push((g, i));
        } else {
            idx_groups.push((k, vec![(g, i)]));
        }
    };
    for (i, f) in layer.features.iter().enumerate() {
        match &f.geom {
            None => push(GeomKind::Mixed, None, i),
            Some(Geometry::Collection(parts)) => {
                for k in [GeomKind::Point, GeomKind::Line, GeomKind::Polygon] {
                    let sub: Vec<Geometry> =
                        parts.iter().filter(|p| p.kind() == k).cloned().collect();
                    if !sub.is_empty() {
                        push(k, Geometry::from_parts(sub), i);
                    }
                }
            }
            Some(g) => push(g.kind(), Some(g.clone()), i),
        }
    }
    // entitățile fără geometrie merg în primul grup
    let nulls: Vec<_> = idx_groups
        .iter()
        .position(|(k, _)| *k == GeomKind::Mixed)
        .map(|i| idx_groups.remove(i).1)
        .unwrap_or_default();
    if idx_groups.is_empty() {
        idx_groups.push((layer.geom_kind_or_point(), vec![]));
    }
    idx_groups[0].1.extend(nulls);
    let groups: Vec<(GeomKind, Vec<(Option<Geometry>, &Vec<Value>)>)> = idx_groups
        .into_iter()
        .map(|(k, v)| {
            (
                k,
                v.into_iter()
                    .map(|(g, i)| (g, &layer.features[i].attrs))
                    .collect(),
            )
        })
        .collect();
    let multi = groups.len() > 1;
    let mut written = vec![];
    for (k, feats) in &groups {
        let p = if multi {
            let stem = path.file_stem().unwrap_or_default().to_string_lossy();
            path.with_file_name(format!("{}_{}.shp", stem, kind_suffix(*k)))
        } else {
            path.with_extension("shp")
        };
        write_one(layer, *k, feats, &p, opts)?;
        written.push(p);
    }
    Ok(written)
}

impl VectorLayer {
    fn geom_kind_or_point(&self) -> GeomKind {
        match self.geom_kind() {
            GeomKind::Mixed => GeomKind::Point,
            k => k,
        }
    }
}

fn write_one(
    layer: &VectorLayer,
    kind: GeomKind,
    feats: &[(Option<Geometry>, &Vec<Value>)],
    path: &Path,
    opts: &WriteOptions,
) -> Result<()> {
    let z = layer.has_z;
    let any_multipoint = feats
        .iter()
        .any(|(g, _)| matches!(g, Some(Geometry::MultiPoint(_))));
    let stype: i32 = match (kind, any_multipoint) {
        (GeomKind::Point, false) => 1,
        (GeomKind::Point, true) => 8,
        (GeomKind::Line, _) => 3,
        _ => 5,
    } + if z { 10 } else { 0 };

    let mut shp: Vec<u8> = vec![0; 100];
    let mut shx: Vec<u8> = vec![0; 100];
    let mut bb = Bounds::empty();
    let (mut zmin, mut zmax) = (f64::INFINITY, f64::NEG_INFINITY);

    for (i, (g, _)) in feats.iter().enumerate() {
        let mut c: Vec<u8> = vec![];
        match g {
            None => c.extend_from_slice(&0i32.to_le_bytes()),
            Some(g) => {
                let gb = g.bounds();
                bb.merge(&gb);
                g.for_each_coord(&mut |p| {
                    zmin = zmin.min(p.z);
                    zmax = zmax.max(p.z);
                });
                c.extend_from_slice(&stype.to_le_bytes());
                match stype % 10 {
                    1 => {
                        let p = g.points().first().copied().unwrap_or_default();
                        c.extend_from_slice(&p.x.to_le_bytes());
                        c.extend_from_slice(&p.y.to_le_bytes());
                        if z {
                            c.extend_from_slice(&p.z.to_le_bytes());
                            c.extend_from_slice(&0f64.to_le_bytes());
                        }
                    }
                    8 => {
                        let pts = g.points();
                        for v in [gb.minx, gb.miny, gb.maxx, gb.maxy] {
                            c.extend_from_slice(&v.to_le_bytes());
                        }
                        c.extend_from_slice(&(pts.len() as i32).to_le_bytes());
                        for p in &pts {
                            c.extend_from_slice(&p.x.to_le_bytes());
                            c.extend_from_slice(&p.y.to_le_bytes());
                        }
                        if z {
                            let (a, b) = pts
                                .iter()
                                .fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), p| {
                                    (a.min(p.z), b.max(p.z))
                                });
                            c.extend_from_slice(&a.to_le_bytes());
                            c.extend_from_slice(&b.to_le_bytes());
                            for p in &pts {
                                c.extend_from_slice(&p.z.to_le_bytes());
                            }
                        }
                    }
                    _ => {
                        let parts: Vec<Vec<Coord>> = if stype % 10 == 3 {
                            g.lines().into_iter().cloned().collect()
                        } else {
                            let mut v = vec![];
                            for p in g.polygons() {
                                for (ri, r) in p.iter().enumerate() {
                                    if r.len() < 3 {
                                        continue;
                                    }
                                    let mut r = r.clone();
                                    if r.first() != r.last() {
                                        r.push(r[0]);
                                    }
                                    let a = ring_signed_area(&r);
                                    // exterior în sens orar (arie negativă), goluri anti-orar
                                    if (ri == 0 && a > 0.0) || (ri > 0 && a < 0.0) {
                                        r.reverse();
                                    }
                                    v.push(r);
                                }
                            }
                            v
                        };
                        let npts: usize = parts.iter().map(|p| p.len()).sum();
                        for v in [gb.minx, gb.miny, gb.maxx, gb.maxy] {
                            c.extend_from_slice(&v.to_le_bytes());
                        }
                        c.extend_from_slice(&(parts.len() as i32).to_le_bytes());
                        c.extend_from_slice(&(npts as i32).to_le_bytes());
                        let mut idx = 0i32;
                        for p in &parts {
                            c.extend_from_slice(&idx.to_le_bytes());
                            idx += p.len() as i32;
                        }
                        for p in parts.iter().flatten() {
                            c.extend_from_slice(&p.x.to_le_bytes());
                            c.extend_from_slice(&p.y.to_le_bytes());
                        }
                        if z {
                            let (a, b) = parts
                                .iter()
                                .flatten()
                                .fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), p| {
                                    (a.min(p.z), b.max(p.z))
                                });
                            c.extend_from_slice(&a.to_le_bytes());
                            c.extend_from_slice(&b.to_le_bytes());
                            for p in parts.iter().flatten() {
                                c.extend_from_slice(&p.z.to_le_bytes());
                            }
                        }
                    }
                }
            }
        }
        let off = shp.len() / 2;
        shx.extend_from_slice(&(off as i32).to_be_bytes());
        shx.extend_from_slice(&((c.len() / 2) as i32).to_be_bytes());
        shp.extend_from_slice(&((i + 1) as i32).to_be_bytes());
        shp.extend_from_slice(&((c.len() / 2) as i32).to_be_bytes());
        shp.extend_from_slice(&c);
    }
    if !bb.is_valid() {
        bb = Bounds::new(0.0, 0.0, 0.0, 0.0);
    }
    if !zmin.is_finite() {
        zmin = 0.0;
        zmax = 0.0;
    }
    for (buf, len) in [(&mut shp, 0usize), (&mut shx, 0usize)] {
        let total = if len == 0 { buf.len() } else { len };
        buf[0..4].copy_from_slice(&9994i32.to_be_bytes());
        buf[24..28].copy_from_slice(&((total / 2) as i32).to_be_bytes());
        buf[28..32].copy_from_slice(&1000i32.to_le_bytes());
        buf[32..36].copy_from_slice(&stype.to_le_bytes());
        for (j, v) in [
            bb.minx,
            bb.miny,
            bb.maxx,
            bb.maxy,
            if z { zmin } else { 0.0 },
            if z { zmax } else { 0.0 },
            0.0,
            0.0,
        ]
        .iter()
        .enumerate()
        {
            buf[36 + j * 8..44 + j * 8].copy_from_slice(&v.to_le_bytes());
        }
    }
    fs::write(path, &shp).with_context(|| tr!("Nu pot scrie {}", path.display()))?;
    fs::write(path.with_extension("shx"), &shx)?;
    let rows: Vec<&Vec<Value>> = feats.iter().map(|(_, a)| *a).collect();
    write_dbf(&path.with_extension("dbf"), &layer.fields, &rows, opts)?;
    if let Some(c) = &layer.crs {
        write_prj_sidecar(path, c)?;
    }
    Ok(())
}

fn truncate_bytes(enc: &'static Encoding, s: &str, max: usize) -> Vec<u8> {
    let mut b = super::encode_lossy(enc, s);
    if b.len() > max {
        if enc == encoding_rs::UTF_8 {
            let mut cut = max;
            while cut > 0 && !s.is_char_boundary(cut) {
                cut -= 1;
            }
            b = s.as_bytes()[..cut].to_vec();
        } else {
            b.truncate(max);
        }
    }
    b
}

pub fn write_dbf(
    path: &Path,
    fields: &[Field],
    rows: &[&Vec<Value>],
    opts: &WriteOptions,
) -> Result<()> {
    let enc = opts
        .encoding
        .as_deref()
        .and_then(encoding_from_label)
        .unwrap_or(encoding_rs::UTF_8);
    struct Def {
        name: Vec<u8>,
        t: u8,
        len: usize,
        dec: usize,
    }
    let mut used: Vec<Vec<u8>> = vec![];
    let mut defs: Vec<Def> = vec![];
    for (fi, f) in fields.iter().enumerate() {
        // nume max. 10 octeți, unic
        let mut name = truncate_bytes(enc, &f.name, 10);
        let mut k = 1;
        while used.iter().any(|u| u.eq_ignore_ascii_case(&name)) {
            let suf = format!("_{k}");
            let mut base = truncate_bytes(enc, &f.name, 10 - suf.len());
            base.extend_from_slice(suf.as_bytes());
            name = base;
            k += 1;
        }
        used.push(name.clone());
        let vals = rows.iter().map(|r| r.get(fi).unwrap_or(&Value::Null));
        let (t, len, dec) = match f.ftype {
            FieldType::Integer => {
                let w = vals
                    .map(|v| v.coerce(FieldType::Integer).to_text().len())
                    .max()
                    .unwrap_or(1)
                    .max(f.width.min(20))
                    .clamp(1, 20);
                (b'N', w, 0)
            }
            FieldType::Real => {
                let dec = if f.precision > 0 {
                    f.precision.min(15)
                } else {
                    10
                };
                let ip = vals
                    .filter_map(|v| v.as_f64())
                    .map(|x| format!("{:.0}", x.trunc()).len())
                    .max()
                    .unwrap_or(1);
                (b'N', (ip + dec + 2).clamp(dec + 3, 33), dec)
            }
            FieldType::Boolean => (b'L', 1, 0),
            FieldType::Date => {
                let ok = rows.iter().all(|r| match r.get(fi) {
                    Some(Value::Text(s)) => s.len() == 10 && s.as_bytes()[4] == b'-',
                    Some(Value::Null) | None => true,
                    _ => false,
                });
                if ok { (b'D', 8, 0) } else { (b'C', 10, 0) }
            }
            FieldType::Text => {
                let w = vals
                    .map(|v| super::encode_lossy(enc, &v.to_text()).len())
                    .max()
                    .unwrap_or(1)
                    .max(f.width.min(254))
                    .clamp(1, 254);
                (b'C', w, 0)
            }
        };
        defs.push(Def { name, t, len, dec });
    }
    let hsize = 32 + 32 * defs.len() + 1;
    let rsize = 1 + defs.iter().map(|d| d.len).sum::<usize>();
    let file =
        fs::File::create(path).with_context(|| tr!("Nu pot scrie {}", path.display()))?;
    let mut w = BufWriter::new(file);
    let mut h = vec![0u8; 32];
    h[0] = 0x03;
    h[1] = 126; // 2026
    h[2] = 1;
    h[3] = 1;
    h[4..8].copy_from_slice(&(rows.len() as u32).to_le_bytes());
    h[8..10].copy_from_slice(&(hsize as u16).to_le_bytes());
    h[10..12].copy_from_slice(&(rsize as u16).to_le_bytes());
    h[29] = if enc == encoding_rs::WINDOWS_1250 {
        0xC8
    } else if enc == encoding_rs::WINDOWS_1252 {
        0x03
    } else {
        0x00
    };
    w.write_all(&h)?;
    for d in &defs {
        let mut fd = [0u8; 32];
        fd[..d.name.len()].copy_from_slice(&d.name);
        fd[11] = d.t;
        fd[16] = d.len as u8;
        fd[17] = d.dec as u8;
        w.write_all(&fd)?;
    }
    w.write_all(&[0x0D])?;
    for r in rows {
        let mut rec = Vec::with_capacity(rsize);
        rec.push(b' ');
        for (fi, d) in defs.iter().enumerate() {
            let v = r.get(fi).cloned().unwrap_or(Value::Null);
            let s: Vec<u8> = match d.t {
                b'N' => {
                    let txt = if v.is_null() {
                        String::new()
                    } else if d.dec == 0 {
                        v.coerce(FieldType::Integer).to_text()
                    } else {
                        v.as_f64()
                            .map(|x| format!("{:.*}", d.dec, x))
                            .unwrap_or_default()
                    };
                    let txt = if txt.len() > d.len {
                        "*".repeat(d.len)
                    } else {
                        txt
                    };
                    format!("{:>width$}", txt, width = d.len).into_bytes()
                }
                b'L' => vec![match v.coerce(FieldType::Boolean) {
                    Value::Bool(true) => b'T',
                    Value::Bool(false) => b'F',
                    _ => b'?',
                }],
                b'D' => match &v {
                    Value::Text(s) if s.len() == 10 => s.replace('-', "").into_bytes(),
                    _ => b"        ".to_vec(),
                },
                _ => {
                    let mut b = truncate_bytes(enc, &v.to_text(), d.len);
                    b.resize(d.len, b' ');
                    b
                }
            };
            let mut s = s;
            s.resize(d.len, b' ');
            rec.extend_from_slice(&s);
        }
        w.write_all(&rec)?;
    }
    w.write_all(&[0x1A])?;
    w.flush()?;
    let cpg = if enc == encoding_rs::UTF_8 {
        "UTF-8".to_string()
    } else {
        enc.name().trim_start_matches("windows-").to_string()
    };
    fs::write(path.with_extension("cpg"), cpg)?;
    Ok(())
}
