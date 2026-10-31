//! KML și KMZ (Google Earth) – citire și scriere. Coordonatele sunt întotdeauna WGS84.

use super::WriteOptions;
use crate::crs::{Crs, reproject_layer};
use crate::model::*;
use anyhow::{Context, Result, anyhow};
use roxmltree::Node;
use std::fs;
use std::io::{Read, Write};
use std::path::Path;
use crate::i18n::tr;

pub fn xml_escape(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            '\'' => o.push_str("&apos;"),
            c if (c as u32) < 0x20 && c != '\n' && c != '\t' && c != '\r' => {}
            c => o.push(c),
        }
    }
    o
}

fn child<'a>(n: Node<'a, 'a>, name: &str) -> Option<Node<'a, 'a>> {
    n.children()
        .find(|c| c.is_element() && c.tag_name().name() == name)
}

fn text_of(n: Option<Node>) -> Option<String> {
    n.map(|n| {
        n.descendants()
            .filter(|d| d.is_text())
            .map(|d| d.text().unwrap_or(""))
            .collect::<String>()
            .trim()
            .to_string()
    })
}

fn parse_coords(s: &str) -> Vec<Coord> {
    s.split_whitespace()
        .filter_map(|t| {
            let p: Vec<f64> = t.split(',').filter_map(|v| v.trim().parse().ok()).collect();
            if p.len() >= 2 {
                Some(Coord::xyz(p[0], p[1], p.get(2).copied().unwrap_or(0.0)))
            } else {
                None
            }
        })
        .collect()
}

fn ring_of(n: Node) -> Vec<Coord> {
    n.descendants()
        .find(|d| d.is_element() && d.tag_name().name() == "coordinates")
        .and_then(|c| text_of(Some(c)))
        .map(|t| parse_coords(&t))
        .unwrap_or_default()
}

fn geometry_of(n: Node) -> Vec<Geometry> {
    let mut out = vec![];
    for c in n.children().filter(|c| c.is_element()) {
        match c.tag_name().name() {
            "Point" => {
                if let Some(p) = ring_of(c).first() {
                    out.push(Geometry::Point(*p));
                }
            }
            "LineString" => out.push(Geometry::LineString(ring_of(c))),
            "LinearRing" => out.push(Geometry::Polygon(vec![ring_of(c)])),
            "Polygon" => {
                let mut rings = vec![];
                if let Some(o) = child(c, "outerBoundaryIs") {
                    rings.push(ring_of(o));
                }
                for i in c
                    .children()
                    .filter(|x| x.is_element() && x.tag_name().name() == "innerBoundaryIs")
                {
                    for lr in i
                        .children()
                        .filter(|x| x.is_element() && x.tag_name().name() == "LinearRing")
                    {
                        rings.push(ring_of(lr));
                    }
                }
                out.push(Geometry::Polygon(rings));
            }
            "MultiGeometry" => out.extend(geometry_of(c)),
            "Track" => {
                let pts: Vec<Coord> = c
                    .children()
                    .filter(|x| x.is_element() && x.tag_name().name() == "coord")
                    .filter_map(|x| {
                        let v: Vec<f64> = x
                            .text()
                            .unwrap_or("")
                            .split_whitespace()
                            .filter_map(|t| t.parse().ok())
                            .collect();
                        if v.len() >= 2 {
                            Some(Coord::xyz(v[0], v[1], v.get(2).copied().unwrap_or(0.0)))
                        } else {
                            None
                        }
                    })
                    .collect();
                out.push(Geometry::LineString(pts));
            }
            "MultiTrack" => out.extend(geometry_of(c)),
            _ => {}
        }
    }
    out
}

fn folder_path(n: Node) -> String {
    let mut parts: Vec<String> = n
        .ancestors()
        .filter(|a| a.is_element() && a.tag_name().name() == "Folder")
        .filter_map(|a| text_of(child(a, "name")))
        .collect();
    parts.reverse();
    parts.join("/")
}

pub fn parse_kml(txt: &str, name: &str) -> Result<VectorLayer> {
    let doc = roxmltree::Document::parse_with_options(
        txt,
        roxmltree::ParsingOptions {
            allow_dtd: true,
            ..Default::default()
        },
    )
    .map_err(|e| anyhow!("{}", tr!("KML invalid: {e}")))?;
    let mut recs = vec![];
    for pm in doc
        .descendants()
        .filter(|n| n.is_element() && n.tag_name().name() == "Placemark")
    {
        let mut attrs: Vec<(String, Value)> = vec![];
        attrs.push((
            "name".into(),
            text_of(child(pm, "name"))
                .map(Value::Text)
                .unwrap_or(Value::Null),
        ));
        let desc = text_of(child(pm, "description")).filter(|s| !s.is_empty());
        attrs.push((
            "description".into(),
            desc.map(Value::Text).unwrap_or(Value::Null),
        ));
        let fp = folder_path(pm);
        if !fp.is_empty() {
            attrs.push(("folder".into(), Value::Text(fp)));
        }
        if let Some(ed) = child(pm, "ExtendedData") {
            for d in ed.descendants().filter(|d| d.is_element()) {
                match d.tag_name().name() {
                    "Data" => {
                        if let Some(k) = d.attribute("name") {
                            attrs.push((
                                k.to_string(),
                                infer_value(&text_of(child(d, "value")).unwrap_or_default()),
                            ));
                        }
                    }
                    "SimpleData" => {
                        if let Some(k) = d.attribute("name") {
                            attrs.push((
                                k.to_string(),
                                infer_value(&text_of(Some(d)).unwrap_or_default()),
                            ));
                        }
                    }
                    _ => {}
                }
            }
        }
        let geom = Geometry::from_parts(geometry_of(pm));
        recs.push((geom, attrs));
    }
    let mut l = VectorLayer::from_named_records(name, recs);
    l.crs = Some(Crs::wgs84());
    Ok(l)
}

pub fn read(path: &Path) -> Result<Vec<VectorLayer>> {
    let name = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let txt = if super::ext_of(path) == "kmz" {
        let f = fs::File::open(path)?;
        let mut z = zip::ZipArchive::new(f).map_err(|e| anyhow!("{}", tr!("KMZ invalid: {e}")))?;
        let idx = (0..z.len())
            .find(|&i| {
                z.by_index(i)
                    .map(|e| e.name().to_lowercase().ends_with(".kml"))
                    .unwrap_or(false)
            })
            .ok_or_else(|| anyhow!(tr!("KMZ fără fișier .kml")))?;
        let mut s = String::new();
        z.by_index(idx)?.read_to_string(&mut s)?;
        s
    } else {
        let b = fs::read(path).with_context(|| tr!("Nu pot citi {}", path.display()))?;
        String::from_utf8_lossy(&b).into_owned()
    };
    let mut l = parse_kml(txt.trim_start_matches('\u{feff}'), &name)?;
    l.source = Some(path.to_path_buf());
    Ok(vec![l])
}

/// Câmpul folosit ca nume al entității.
pub fn pick_name_field(layer: &VectorLayer, pref: Option<&str>) -> Option<usize> {
    if let Some(p) = pref
        && let Some(i) = layer.field_index(p)
    {
        return Some(i);
    }
    for n in [
        "name", "nume", "denumire", "Name", "NAME", "label", "eticheta", tr!("text"),
    ] {
        if let Some(i) = layer.field_index(n) {
            return Some(i);
        }
    }
    layer.fields.iter().position(|f| f.ftype == FieldType::Text)
}

fn kml_coords(v: &[Coord], z: bool) -> String {
    v.iter()
        .map(|c| {
            if z {
                format!("{:.9},{:.9},{:.3}", c.x, c.y, c.z)
            } else {
                format!("{:.9},{:.9}", c.x, c.y)
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn kml_geom(g: &Geometry, z: bool, out: &mut String) {
    match g {
        Geometry::Point(c) => out.push_str(&format!(
            "<Point><coordinates>{}</coordinates></Point>",
            kml_coords(&[*c], z)
        )),
        Geometry::LineString(v) => out.push_str(&format!(
            "<LineString><tessellate>1</tessellate><coordinates>{}</coordinates></LineString>",
            kml_coords(v, z)
        )),
        Geometry::Polygon(p) => {
            out.push_str("<Polygon>");
            for (i, r) in p.iter().enumerate() {
                let mut r = r.clone();
                if r.first() != r.last() && !r.is_empty() {
                    r.push(r[0]);
                }
                let tag = if i == 0 {
                    "outerBoundaryIs"
                } else {
                    "innerBoundaryIs"
                };
                out.push_str(&format!(
                    "<{tag}><LinearRing><coordinates>{}</coordinates></LinearRing></{tag}>",
                    kml_coords(&r, z)
                ));
            }
            out.push_str("</Polygon>");
        }
        Geometry::MultiPoint(v) => {
            out.push_str("<MultiGeometry>");
            v.iter()
                .for_each(|c| kml_geom(&Geometry::Point(*c), z, out));
            out.push_str("</MultiGeometry>");
        }
        Geometry::MultiLineString(v) => {
            out.push_str("<MultiGeometry>");
            v.iter()
                .for_each(|l| kml_geom(&Geometry::LineString(l.clone()), z, out));
            out.push_str("</MultiGeometry>");
        }
        Geometry::MultiPolygon(v) => {
            out.push_str("<MultiGeometry>");
            v.iter()
                .for_each(|p| kml_geom(&Geometry::Polygon(p.clone()), z, out));
            out.push_str("</MultiGeometry>");
        }
        Geometry::Collection(v) => {
            out.push_str("<MultiGeometry>");
            v.iter().for_each(|g| kml_geom(g, z, out));
            out.push_str("</MultiGeometry>");
        }
    }
}

pub fn to_kml_string(layer: &VectorLayer, opts: &WriteOptions) -> Result<String> {
    let mut l = layer.clone();
    if l.crs.as_ref().is_some_and(|c| c.epsg != Some(4326)) {
        reproject_layer(&mut l, &Crs::wgs84())?;
    }
    let name_idx = pick_name_field(&l, opts.name_field.as_deref());
    let mut s = String::new();
    s.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<kml xmlns=\"http://www.opengis.net/kml/2.2\">\n<Document>\n");
    s.push_str(&format!("<name>{}</name>\n", xml_escape(&l.name)));
    s.push_str("<Style id=\"s\"><LineStyle><color>ff0000ff</color><width>2</width></LineStyle><PolyStyle><color>4d0000ff</color></PolyStyle></Style>\n");
    s.push_str(&format!("<Folder><name>{}</name>\n", xml_escape(&l.name)));
    for f in &l.features {
        s.push_str("<Placemark>");
        if let Some(i) = name_idx {
            s.push_str(&format!(
                "<name>{}</name>",
                xml_escape(&f.attrs[i].to_text())
            ));
        }
        s.push_str("<styleUrl>#s</styleUrl>");
        if !l.fields.is_empty() {
            s.push_str("<ExtendedData>");
            for (fd, v) in l.fields.iter().zip(&f.attrs) {
                s.push_str(&format!(
                    "<Data name=\"{}\"><value>{}</value></Data>",
                    xml_escape(&fd.name),
                    xml_escape(&v.to_text())
                ));
            }
            s.push_str("</ExtendedData>");
        }
        if let Some(g) = &f.geom {
            kml_geom(g, l.has_z, &mut s);
        }
        s.push_str("</Placemark>\n");
    }
    s.push_str("</Folder>\n</Document>\n</kml>\n");
    Ok(s)
}

pub fn write(layer: &VectorLayer, path: &Path, opts: &WriteOptions) -> Result<()> {
    let s = to_kml_string(layer, opts)?;
    if super::ext_of(path) == "kmz" {
        let f =
            fs::File::create(path).with_context(|| tr!("Nu pot scrie {}", path.display()))?;
        let mut z = zip::ZipWriter::new(f);
        z.start_file(
            "doc.kml",
            zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated),
        )?;
        z.write_all(s.as_bytes())?;
        z.finish()?;
    } else {
        fs::write(path, s).with_context(|| tr!("Nu pot scrie {}", path.display()))?;
    }
    Ok(())
}
