//! GeoJSON – citire și scriere.

use crate::crs::Crs;
use crate::model::*;
use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Map, Value as J, json};
use std::fs;
use std::io::{BufWriter, Write};
use std::path::Path;
use crate::i18n::tr;

fn coord(v: &J) -> Option<Coord> {
    let a = v.as_array()?;
    Some(Coord::xyz(
        a.first()?.as_f64()?,
        a.get(1)?.as_f64()?,
        a.get(2).and_then(|z| z.as_f64()).unwrap_or(0.0),
    ))
}
fn coords(v: &J) -> Vec<Coord> {
    v.as_array()
        .map(|a| a.iter().filter_map(coord).collect())
        .unwrap_or_default()
}
fn rings(v: &J) -> Vec<Ring> {
    v.as_array()
        .map(|a| a.iter().map(coords).collect())
        .unwrap_or_default()
}

pub fn parse_geometry(g: &J) -> Option<Geometry> {
    let t = g.get("type")?.as_str()?;
    let c = g.get("coordinates");
    Some(match t {
        "Point" => Geometry::Point(coord(c?)?),
        "MultiPoint" => Geometry::MultiPoint(coords(c?)),
        "LineString" => Geometry::LineString(coords(c?)),
        "MultiLineString" => Geometry::MultiLineString(rings(c?)),
        "Polygon" => Geometry::Polygon(rings(c?)),
        "MultiPolygon" => Geometry::MultiPolygon(c?.as_array()?.iter().map(rings).collect()),
        "GeometryCollection" => Geometry::Collection(
            g.get("geometries")?
                .as_array()?
                .iter()
                .filter_map(parse_geometry)
                .collect(),
        ),
        _ => return None,
    })
}

fn json_to_value(v: &J) -> Value {
    match v {
        J::Null => Value::Null,
        J::Bool(b) => Value::Bool(*b),
        J::Number(n) => n
            .as_i64()
            .map(Value::Int)
            .unwrap_or_else(|| Value::Real(n.as_f64().unwrap_or(f64::NAN))),
        J::String(s) => Value::Text(s.clone()),
        other => Value::Text(other.to_string()),
    }
}

fn crs_from_member(root: &J) -> Option<Crs> {
    let name = root.get("crs")?.get("properties")?.get("name")?.as_str()?;
    // urn:ogc:def:crs:EPSG::3844 / EPSG:3844 / urn:ogc:def:crs:OGC:1.3:CRS84
    if name.contains("CRS84") {
        return Some(Crs::wgs84());
    }
    let code: u32 = name.rsplit(':').next()?.parse().ok()?;
    Crs::from_epsg(code).ok()
}

pub fn read(path: &Path) -> Result<VectorLayer> {
    let txt =
        fs::read_to_string(path).with_context(|| tr!("Nu pot citi {}", path.display()))?;
    let root: J = serde_json::from_str(txt.trim_start_matches('\u{feff}'))
        .map_err(|e| anyhow!("{}", tr!("JSON invalid: {e}")))?;
    let name = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let mut recs: Vec<(Option<Geometry>, Vec<(String, Value)>)> = vec![];
    let mut handle_feature = |f: &J| {
        let geom = f.get("geometry").and_then(parse_geometry);
        let mut attrs = vec![];
        if let Some(p) = f.get("properties").and_then(|p| p.as_object()) {
            for (k, v) in p {
                attrs.push((k.clone(), json_to_value(v)));
            }
        }
        if let Some(id) = f.get("id")
            && !attrs.iter().any(|(k, _)| k == "id")
        {
            attrs.insert(0, ("id".into(), json_to_value(id)));
        }
        recs.push((geom, attrs));
    };
    match root.get("type").and_then(|t| t.as_str()) {
        Some("FeatureCollection") => {
            for f in root
                .get("features")
                .and_then(|f| f.as_array())
                .ok_or_else(|| anyhow!(tr!("FeatureCollection fără „features”")))?
            {
                handle_feature(f);
            }
        }
        Some("Feature") => handle_feature(&root),
        Some(_) => {
            let g =
                parse_geometry(&root).ok_or_else(|| anyhow!(tr!("Geometrie GeoJSON nerecunoscută")))?;
            recs.push((Some(g), vec![]));
        }
        None => bail!(tr!("Nu este un document GeoJSON")),
    }
    let mut layer = VectorLayer::from_named_records(&name, recs);
    layer.source = Some(path.to_path_buf());
    layer.crs = Some(crs_from_member(&root).unwrap_or_else(Crs::wgs84));
    Ok(layer)
}

fn num(v: f64, geo: bool) -> J {
    // rotunjire: 1e-9 grade / 1e-4 m
    let f = if geo { 1e9 } else { 1e4 };
    let r = (v * f).round() / f;
    if r.fract() == 0.0 && r.abs() < 9e15 {
        json!(r as i64)
    } else {
        json!(r)
    }
}

fn jc(c: &Coord, z: bool, geo: bool) -> J {
    if z {
        J::Array(vec![num(c.x, geo), num(c.y, geo), num(c.z, false)])
    } else {
        J::Array(vec![num(c.x, geo), num(c.y, geo)])
    }
}
fn jcs(v: &[Coord], z: bool, geo: bool) -> J {
    J::Array(v.iter().map(|c| jc(c, z, geo)).collect())
}
fn jrings(p: &[Ring], z: bool, geo: bool) -> J {
    J::Array(p.iter().map(|r| jcs(r, z, geo)).collect())
}

pub fn geometry_to_json(g: &Geometry, z: bool, geo: bool) -> J {
    match g {
        Geometry::Point(c) => json!({"type": "Point", "coordinates": jc(c, z, geo)}),
        Geometry::MultiPoint(v) => json!({"type": "MultiPoint", "coordinates": jcs(v, z, geo)}),
        Geometry::LineString(v) => json!({"type": "LineString", "coordinates": jcs(v, z, geo)}),
        Geometry::MultiLineString(v) => {
            json!({"type": "MultiLineString", "coordinates": jrings(v, z, geo)})
        }
        Geometry::Polygon(p) => {
            // RFC 7946: exterior anti-orar, goluri orar
            let p: Vec<Ring> = p
                .iter()
                .enumerate()
                .map(|(i, r)| {
                    let mut r = r.clone();
                    let a = ring_signed_area(&r);
                    if (i == 0 && a < 0.0) || (i > 0 && a > 0.0) {
                        r.reverse();
                    }
                    r
                })
                .collect();
            json!({"type": "Polygon", "coordinates": jrings(&p, z, geo)})
        }
        Geometry::MultiPolygon(v) => {
            let parts: Vec<J> = v
                .iter()
                .map(|p| {
                    geometry_to_json(&Geometry::Polygon(p.clone()), z, geo)["coordinates"].clone()
                })
                .collect();
            json!({"type": "MultiPolygon", "coordinates": parts})
        }
        Geometry::Collection(v) => {
            json!({"type": "GeometryCollection", "geometries": v.iter().map(|g| geometry_to_json(g, z, geo)).collect::<Vec<_>>()})
        }
    }
}

fn value_to_json(v: &Value) -> J {
    match v {
        Value::Null => J::Null,
        Value::Int(i) => json!(i),
        Value::Real(r) => {
            if r.is_finite() {
                json!(r)
            } else {
                J::Null
            }
        }
        Value::Text(s) => json!(s),
        Value::Bool(b) => json!(b),
    }
}

pub fn write(layer: &VectorLayer, path: &Path) -> Result<()> {
    let geo = layer.crs.as_ref().is_none_or(|c| c.is_geographic());
    let file =
        fs::File::create(path).with_context(|| tr!("Nu pot scrie {}", path.display()))?;
    let mut w = BufWriter::new(file);
    writeln!(w, "{{")?;
    writeln!(w, "\"type\": \"FeatureCollection\",")?;
    writeln!(w, "\"name\": {},", json!(layer.name))?;
    if let Some(c) = &layer.crs
        && c.epsg != Some(4326)
        && let Some(code) = c.epsg
    {
        writeln!(
            w,
            "\"crs\": {{ \"type\": \"name\", \"properties\": {{ \"name\": \"urn:ogc:def:crs:EPSG::{code}\" }} }},"
        )?;
    }
    writeln!(w, "\"features\": [")?;
    let n = layer.features.len();
    for (i, f) in layer.features.iter().enumerate() {
        let mut props = Map::new();
        for (fd, v) in layer.fields.iter().zip(&f.attrs) {
            props.insert(fd.name.clone(), value_to_json(v));
        }
        let g = f
            .geom
            .as_ref()
            .map(|g| geometry_to_json(g, layer.has_z, geo))
            .unwrap_or(J::Null);
        let feat = json!({"type": "Feature", "properties": J::Object(props), "geometry": g});
        write!(w, "{}", serde_json::to_string(&feat)?)?;
        writeln!(w, "{}", if i + 1 < n { "," } else { "" })?;
    }
    writeln!(w, "]")?;
    writeln!(w, "}}")?;
    w.flush()?;
    Ok(())
}
