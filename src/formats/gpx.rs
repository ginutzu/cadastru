//! GPX (GPS Exchange) – puncte (wpt), rute (rte) și trasee (trk).

use super::WriteOptions;
use super::kml::{pick_name_field, xml_escape};
use crate::crs::{Crs, reproject_layer};
use crate::model::*;
use anyhow::{Context, Result, anyhow};
use roxmltree::Node;
use std::fs;
use std::path::Path;
use crate::i18n::tr;

fn child_text(n: Node, name: &str) -> Option<String> {
    n.children()
        .find(|c| c.is_element() && c.tag_name().name() == name)
        .and_then(|c| c.text())
        .map(|t| t.trim().to_string())
}

fn pt(n: Node) -> Option<Coord> {
    let lat: f64 = n.attribute("lat")?.trim().parse().ok()?;
    let lon: f64 = n.attribute("lon")?.trim().parse().ok()?;
    let ele = child_text(n, "ele")
        .and_then(|e| e.parse().ok())
        .unwrap_or(0.0);
    Some(Coord::xyz(lon, lat, ele))
}

fn common_attrs(n: Node) -> Vec<(String, Value)> {
    let mut a = vec![];
    for k in ["name", "desc", "cmt", "type", "sym", "time"] {
        a.push((
            k.to_string(),
            child_text(n, k).map(Value::Text).unwrap_or(Value::Null),
        ));
    }
    a
}

pub fn read(path: &Path) -> Result<Vec<VectorLayer>> {
    let b = fs::read(path).with_context(|| tr!("Nu pot citi {}", path.display()))?;
    let txt = String::from_utf8_lossy(&b);
    let doc = roxmltree::Document::parse(txt.trim_start_matches('\u{feff}'))
        .map_err(|e| anyhow!("{}", tr!("GPX invalid: {e}")))?;
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let root = doc.root_element();
    let mut wpts = vec![];
    let mut rtes = vec![];
    let mut trks = vec![];
    for n in root.children().filter(|c| c.is_element()) {
        match n.tag_name().name() {
            "wpt" => {
                if let Some(c) = pt(n) {
                    let mut a = common_attrs(n);
                    a.push(("ele".into(), Value::Real(c.z)));
                    wpts.push((Some(Geometry::Point(c)), a));
                }
            }
            "rte" => {
                let pts: Vec<Coord> = n
                    .children()
                    .filter(|c| c.is_element() && c.tag_name().name() == "rtept")
                    .filter_map(pt)
                    .collect();
                rtes.push((Some(Geometry::LineString(pts)), common_attrs(n)));
            }
            "trk" => {
                let segs: Vec<Vec<Coord>> = n
                    .children()
                    .filter(|c| c.is_element() && c.tag_name().name() == "trkseg")
                    .map(|s| {
                        s.children()
                            .filter(|c| c.is_element() && c.tag_name().name() == "trkpt")
                            .filter_map(pt)
                            .collect::<Vec<_>>()
                    })
                    .filter(|v| !v.is_empty())
                    .collect();
                let g = if segs.len() == 1 {
                    Geometry::LineString(segs.into_iter().next().unwrap())
                } else {
                    Geometry::MultiLineString(segs)
                };
                trks.push((Some(g), common_attrs(n)));
            }
            _ => {}
        }
    }
    let mut out = vec![];
    for (suffix, recs) in [("puncte", wpts), ("rute", rtes), ("trasee", trks)] {
        if !recs.is_empty() {
            let mut l = VectorLayer::from_named_records(&format!("{stem}_{suffix}"), recs);
            l.crs = Some(Crs::wgs84());
            l.has_z = true;
            l.source = Some(path.to_path_buf());
            out.push(l);
        }
    }
    if out.is_empty() {
        let mut l = VectorLayer::new(stem);
        l.crs = Some(Crs::wgs84());
        out.push(l);
    }
    Ok(out)
}

pub fn write(layer: &VectorLayer, path: &Path, opts: &WriteOptions) -> Result<()> {
    let mut l = layer.clone();
    if l.crs.as_ref().is_some_and(|c| c.epsg != Some(4326)) {
        reproject_layer(&mut l, &Crs::wgs84())?;
    }
    let name_idx = pick_name_field(&l, opts.name_field.as_deref());
    let desc_idx = l
        .field_index("desc")
        .or_else(|| l.field_index("description"));
    let mut s = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<gpx version=\"1.1\" creator=\"Cadastru\" xmlns=\"http://www.topografix.com/GPX/1/1\">\n",
    );
    let ele = |c: &Coord| {
        if l.has_z {
            format!("<ele>{:.3}</ele>", c.z)
        } else {
            String::new()
        }
    };
    let head = |f: &Feature| {
        let mut h = String::new();
        if let Some(i) = name_idx {
            h.push_str(&format!(
                "<name>{}</name>",
                xml_escape(&f.attrs[i].to_text())
            ));
        }
        if let Some(i) = desc_idx {
            h.push_str(&format!(
                "<desc>{}</desc>",
                xml_escape(&f.attrs[i].to_text())
            ));
        }
        h
    };
    // puncte
    for f in &l.features {
        if let Some(g) = &f.geom {
            for c in g.points() {
                s.push_str(&format!(
                    "<wpt lat=\"{:.9}\" lon=\"{:.9}\">{}{}</wpt>\n",
                    c.y,
                    c.x,
                    ele(&c),
                    head(f)
                ));
            }
        }
    }
    // linii și contururi de poligoane ca trasee
    for f in &l.features {
        if let Some(g) = &f.geom {
            let mut segs: Vec<Vec<Coord>> = g.lines().into_iter().cloned().collect();
            for p in g.polygons() {
                segs.extend(p.iter().cloned());
            }
            if segs.is_empty() {
                continue;
            }
            s.push_str(&format!("<trk>{}", head(f)));
            for seg in segs {
                s.push_str("<trkseg>");
                for c in seg {
                    s.push_str(&format!(
                        "<trkpt lat=\"{:.9}\" lon=\"{:.9}\">{}</trkpt>",
                        c.y,
                        c.x,
                        ele(&c)
                    ));
                }
                s.push_str("</trkseg>");
            }
            s.push_str("</trk>\n");
        }
    }
    s.push_str("</gpx>\n");
    fs::write(path, s).with_context(|| tr!("Nu pot scrie {}", path.display()))?;
    Ok(())
}
