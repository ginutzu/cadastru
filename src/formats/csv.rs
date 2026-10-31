//! CSV / text delimitat cu coordonate X/Y(/Z) sau geometrie WKT.

use super::{ReadOptions, WriteOptions, encoding_from_label, read_prj_sidecar};
use crate::crs::Crs;
use crate::geomio::{parse_wkt, to_wkt};
use crate::model::*;
use anyhow::{Context, Result, bail};
use std::fs;
use std::path::Path;
use crate::i18n::tr;

pub const X_NAMES: &[&str] = &[
    "x",
    "lon",
    "long",
    "lng",
    "longitude",
    "longitudine",
    "easting",
    "est",
    "east",
    "e",
    "coord_x",
    "xcoord",
    "x_coord",
    "point_x",
];
pub const Y_NAMES: &[&str] = &[
    "y",
    "lat",
    "latitude",
    "latitudine",
    "northing",
    "nord",
    "north",
    "n",
    "coord_y",
    "ycoord",
    "y_coord",
    "point_y",
];
pub const Z_NAMES: &[&str] = &[
    "z",
    "ele",
    "elev",
    "elevation",
    "alt",
    "altitude",
    "h",
    "cota",
    "height",
    "altitudine",
    "point_z",
];
const WKT_NAMES: &[&str] = &["wkt", "geometry", "geom", "the_geom", "shape", "geometrie"];
const GEO_NAMES: &[&str] = &[
    "lon",
    "long",
    "lng",
    "longitude",
    "longitudine",
    "lat",
    "latitude",
    "latitudine",
];

pub fn detect_delimiter(first_line: &str) -> u8 {
    let cands = [b';', b',', b'\t', b'|'];
    *cands
        .iter()
        .max_by_key(|&&c| first_line.bytes().filter(|&b| b == c).count())
        .unwrap_or(&b',')
}

pub fn parse_num(s: &str) -> Option<f64> {
    let t = s.trim();
    if t.is_empty() {
        return None;
    }
    t.parse::<f64>().ok().or_else(|| {
        if !t.contains('.') {
            t.replace(',', ".").parse().ok()
        } else {
            None
        }
    })
}

fn find_col(headers: &[String], names: &[&str]) -> Option<usize> {
    for n in names {
        if let Some(i) = headers
            .iter()
            .position(|h| h.trim().eq_ignore_ascii_case(n))
        {
            return Some(i);
        }
    }
    None
}

/// Citește textul și antetul (folosit și de dialogul de import din interfață).
pub fn read_table(path: &Path, opts: &ReadOptions) -> Result<(Vec<String>, Vec<Vec<String>>)> {
    let bytes = fs::read(path).with_context(|| tr!("Nu pot citi {}", path.display()))?;
    let text = match opts.encoding.as_deref().and_then(encoding_from_label) {
        Some(e) => e.decode(&bytes).0.into_owned(),
        None => match String::from_utf8(bytes.clone()) {
            Ok(s) => s,
            Err(_) => encoding_rs::WINDOWS_1250.decode(&bytes).0.into_owned(),
        },
    };
    let text = text.trim_start_matches('\u{feff}');
    let first = text.lines().next().unwrap_or("");
    let delim = detect_delimiter(first);
    let mut rdr = csv::ReaderBuilder::new()
        .delimiter(delim)
        .flexible(true)
        .has_headers(true)
        .from_reader(text.as_bytes());
    let headers: Vec<String> = rdr
        .headers()?
        .iter()
        .map(|h| h.trim().to_string())
        .collect();
    let mut rows = vec![];
    for r in rdr.records() {
        let r = r?;
        rows.push(r.iter().map(|s| s.to_string()).collect());
    }
    Ok((headers, rows))
}

/// Coloanele detectate automat: (x, y, z, wkt).
pub fn detect_columns(
    headers: &[String],
) -> (Option<usize>, Option<usize>, Option<usize>, Option<usize>) {
    (
        find_col(headers, X_NAMES),
        find_col(headers, Y_NAMES),
        find_col(headers, Z_NAMES),
        find_col(headers, WKT_NAMES),
    )
}

pub fn read(path: &Path, opts: &ReadOptions) -> Result<VectorLayer> {
    let (headers, rows) = read_table(path, opts)?;
    let pick = |opt: &Option<String>| -> Result<Option<usize>> {
        match opt {
            Some(n) => headers
                .iter()
                .position(|h| h.eq_ignore_ascii_case(n))
                .map(Some)
                .with_context(|| tr!("Coloana „{n}” nu există în CSV")),
            None => Ok(None),
        }
    };
    let (ax, ay, az, aw) = detect_columns(&headers);
    let xi = pick(&opts.x_field)?.or(ax);
    let yi = pick(&opts.y_field)?.or(ay);
    let zi = pick(&opts.z_field)?.or(if opts.x_field.is_some() { None } else { az });
    let wi = if opts.x_field.is_some() { None } else { aw };
    if wi.is_none() && (xi.is_none() || yi.is_none()) {
        bail!(
            "{}", tr!("Nu am găsit coloanele de coordonate în CSV (X/Y, lon/lat sau WKT). Coloane: {}",
            headers.join(", ")
        ));
    }
    let mut recs = Vec::with_capacity(rows.len());
    for r in &rows {
        let get = |i: usize| r.get(i).map(|s| s.as_str()).unwrap_or("");
        let geom = if let Some(w) = wi {
            parse_wkt(get(w)).ok()
        } else {
            match (parse_num(get(xi.unwrap())), parse_num(get(yi.unwrap()))) {
                (Some(x), Some(y)) => Some(Geometry::Point(Coord::xyz(
                    x,
                    y,
                    zi.and_then(|z| parse_num(get(z))).unwrap_or(0.0),
                ))),
                _ => None,
            }
        };
        let attrs: Vec<(String, Value)> = headers
            .iter()
            .enumerate()
            .filter(|(i, _)| Some(*i) != wi)
            .map(|(i, h)| {
                let s = get(i);
                let v = match infer_value(s) {
                    Value::Text(t) => parse_num(&t)
                        .filter(|_| t.contains(',') && !t.contains(' '))
                        .map(Value::Real)
                        .unwrap_or(Value::Text(t)),
                    v => v,
                };
                (h.clone(), v)
            })
            .collect();
        recs.push((geom, attrs));
    }
    let name = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let mut l = VectorLayer::from_named_records(&name, recs);
    l.source = Some(path.to_path_buf());
    l.has_z = zi.is_some();
    l.crs = read_prj_sidecar(path);
    if l.crs.is_none() {
        let geo_names =
            xi.is_some_and(|i| GEO_NAMES.iter().any(|n| headers[i].eq_ignore_ascii_case(n)));
        if geo_names {
            l.crs = Some(Crs::wgs84());
        }
    }
    Ok(l)
}

pub fn write(layer: &VectorLayer, path: &Path, opts: &WriteOptions) -> Result<()> {
    let all_points = layer
        .features
        .iter()
        .all(|f| matches!(f.geom, Some(Geometry::Point(_)) | None));
    let use_xy = all_points && !opts.csv_wkt;
    let geo = layer.crs.as_ref().is_some_and(|c| c.is_geographic());
    let mut w = csv::WriterBuilder::new()
        .from_path(path)
        .with_context(|| tr!("Nu pot scrie {}", path.display()))?;
    let mut head: Vec<String> = vec![];
    if use_xy {
        head.push(if geo { "lon".into() } else { "X".into() });
        head.push(if geo { "lat".into() } else { "Y".into() });
        if layer.has_z {
            head.push("Z".into());
        }
    } else {
        head.push("WKT".into());
    }
    let mut taken: Vec<String> = head.iter().map(|h| h.to_lowercase()).collect();
    for f in &layer.fields {
        let mut n = f.name.clone();
        while taken.contains(&n.to_lowercase()) {
            n.push('_');
        }
        taken.push(n.to_lowercase());
        head.push(n);
    }
    w.write_record(&head)?;
    let prec = if geo { 9 } else { 4 };
    for f in &layer.features {
        let mut rec: Vec<String> = vec![];
        if use_xy {
            match &f.geom {
                Some(Geometry::Point(c)) => {
                    rec.push(format!("{:.*}", prec, c.x));
                    rec.push(format!("{:.*}", prec, c.y));
                    if layer.has_z {
                        rec.push(format!("{:.3}", c.z));
                    }
                }
                _ => {
                    rec.push(String::new());
                    rec.push(String::new());
                    if layer.has_z {
                        rec.push(String::new());
                    }
                }
            }
        } else {
            rec.push(
                f.geom
                    .as_ref()
                    .map(|g| to_wkt(g, layer.has_z))
                    .unwrap_or_default(),
            );
        }
        rec.extend(f.attrs.iter().map(|v| v.to_text()));
        w.write_record(&rec)?;
    }
    w.flush()?;
    if let Some(c) = &layer.crs {
        super::write_prj_sidecar(path, c)?;
    }
    Ok(())
}
