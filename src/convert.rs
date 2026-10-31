//! Conversie de fișiere (format și/sau sistem de coordonate) – folosită de CLI și GUI.

use crate::crs::{self, Crs};
use crate::formats::{self, Kind, ReadOptions, WriteOptions};
use crate::model::*;
use crate::rasterops::{self, Resampling};
use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};
use crate::i18n::tr;

#[derive(Clone, Debug, Default)]
pub struct ConvertOptions {
    pub read: ReadOptions,
    pub write: WriteOptions,
    pub target: Option<Crs>,
    pub layer: Option<String>,
    pub res: Option<f64>,
    pub resampling: Resampling,
}

/// Rezultatul unei conversii: fișierele scrise și avertismentele.
pub struct ConvertResult {
    pub written: Vec<PathBuf>,
    pub warnings: Vec<String>,
}

pub fn convert_file(input: &Path, output: &Path, o: &ConvertOptions) -> Result<ConvertResult> {
    let ds = formats::open(input, &o.read)?;
    let out_kind = formats::format_for(output)
        .map(|f| f.kind)
        .with_context(|| tr!("Format de ieșire necunoscut: {}", output.display()))?;
    let mut vecs: Vec<VectorLayer> = vec![];
    let mut written = vec![];
    let mut warnings = vec![];
    for d in ds {
        match d {
            Dataset::Vector(mut v) => {
                if let Some(l) = &o.layer
                    && !v.name.eq_ignore_ascii_case(l)
                {
                    continue;
                }
                if out_kind != Kind::Vector {
                    bail!(
                        "{}", tr!("Nu pot scrie date vectoriale în formatul raster {}",
                        output.display()
                    ));
                }
                if let Some(t) = &o.target {
                    if v.crs.is_none() {
                        bail!(
                            "{}", tr!("Stratul „{}” nu are sistem de coordonate definit (indicați sistemul sursă)",
                            v.name
                        ));
                    }
                    let failed = crs::reproject_layer(&mut v, t)?;
                    if failed > 0 {
                        warnings.push(tr!(
                            "{failed} entități din „{}” nu au putut fi transformate complet",
                            v.name
                        ));
                    }
                }
                vecs.push(v);
            }
            Dataset::Raster(r) => {
                if out_kind != Kind::Raster {
                    bail!(
                        "{}", tr!("Nu pot scrie un raster în formatul vectorial {}",
                        output.display()
                    ));
                }
                let r2 = match (&o.target, r.crs.as_ref()) {
                    (Some(t), Some(c)) if c != t => rasterops::warp(&r, t, o.res, o.resampling)?,
                    (Some(_), None) => bail!(
                        tr!("Rasterul nu are sistem de coordonate definit (indicați sistemul sursă)")
                    ),
                    _ => match o.res {
                        Some(res) => rasterops::resize(&r, res, o.resampling)?,
                        None => r,
                    },
                };
                written.extend(formats::write_raster(&r2, output, &o.write)?);
            }
        }
    }
    if !vecs.is_empty() {
        let refs: Vec<&VectorLayer> = vecs.iter().collect();
        written.extend(formats::write_vector(&refs, output, &o.write)?);
    } else if written.is_empty() {
        bail!(
            "{}", tr!("Nu s-a găsit niciun strat de convertit{}",
            o.layer
                .as_ref()
                .map(|l| tr!(" (stratul „{l}”)"))
                .unwrap_or_default()
        ));
    }
    Ok(ConvertResult { written, warnings })
}

/// Fișierele de intrare pentru conversia în lot.
pub fn collect_files(dir: &Path, recursive: bool, exts: &[String]) -> Result<Vec<PathBuf>> {
    let mut out = vec![];
    collect(dir, recursive, exts, &mut out)?;
    out.sort();
    Ok(out)
}

fn collect(dir: &Path, recursive: bool, exts: &[String], out: &mut Vec<PathBuf>) -> Result<()> {
    for e in
        std::fs::read_dir(dir).with_context(|| tr!("Nu pot citi dosarul {}", dir.display()))?
    {
        let p = e?.path();
        if p.is_dir() && formats::gdb::gdb_root(&p).is_some() {
            // un File Geodatabase e un dosar, dar se convertește ca un singur fișier
            if exts.iter().any(|e| e == "gdb") {
                out.push(p);
            }
        } else if p.is_dir() {
            if recursive {
                collect(&p, recursive, exts, out)?;
            }
        } else if exts.contains(&formats::ext_of(&p)) {
            out.push(p);
        }
    }
    Ok(())
}

/// Extensiile de intrare implicite pentru un format de ieșire (același tip: vector/raster).
pub fn default_input_exts(to_ext: &str) -> Vec<String> {
    let kind = formats::format_for(Path::new(&format!("x.{to_ext}")))
        .map(|t| t.kind)
        .unwrap_or(Kind::Vector);
    formats::FORMATS
        .iter()
        .filter(|f| f.read && f.kind == kind)
        .flat_map(|f| f.exts.iter().map(|e| e.to_string()))
        .filter(|e| e != "txt" && e != "json" && e != "zip" && e != "gdbtable")
        .collect()
}

/// Rulează conversia în lot; `log` primește câte o linie pentru fiecare fișier.
pub fn batch(
    input_dir: &Path,
    output_dir: &Path,
    to_ext: &str,
    exts: &[String],
    recursive: bool,
    o: &ConvertOptions,
    log: &mut dyn FnMut(String),
) -> Result<(usize, usize)> {
    let files = collect_files(input_dir, recursive, exts)?;
    let (mut ok, mut err) = (0, 0);
    log(tr!("{} fișiere găsite", files.len()));
    for f in &files {
        let rel = f.strip_prefix(input_dir).unwrap_or(f);
        let mut dst = output_dir.join(rel);
        dst.set_extension(to_ext);
        if dst == *f {
            continue;
        }
        match convert_file(f, &dst, o) {
            Ok(r) => {
                ok += 1;
                log(format!(
                    "OK   {} -> {}",
                    f.display(),
                    r.written
                        .iter()
                        .map(|p| p.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
                for w in r.warnings {
                    log(tr!("     atenție: {w}"));
                }
            }
            Err(e) => {
                err += 1;
                log(tr!("EROARE {}: {e:#}", f.display()));
            }
        }
    }
    Ok((ok, err))
}

// ---------------------------------------------------------------------------
// Export pe tipuri de geometrie (de exemplu DXF → trei shapefile: puncte, linii, poligoane)
// ---------------------------------------------------------------------------

/// Opțiunile exportului pe tipuri de geometrie.
#[derive(Clone, Debug)]
pub struct SplitOptions {
    /// tipurile exportate: puncte, linii, poligoane
    pub points: bool,
    pub lines: bool,
    pub polygons: bool,
    /// poliliniile închise, cercurile și elipsele merg în stratul de linii
    /// (hașurile și suprafețele pline rămân poligoane)
    pub closed_as_lines: bool,
    /// numele câmpului adăugat cu stratul (fișierul) de origine; `None` = fără
    pub source_field: Option<String>,
}

impl Default for SplitOptions {
    fn default() -> Self {
        SplitOptions {
            points: true,
            lines: true,
            polygons: true,
            closed_as_lines: false,
            source_field: Some("Fisier".into()),
        }
    }
}

/// Ce s-a scris: fișierul, tipul și numărul de entități.
pub struct SplitResult {
    pub written: Vec<(PathBuf, GeomKind, usize)>,
    pub warnings: Vec<String>,
}

/// Uniunea câmpurilor (după nume); câmpurile cu tipuri diferite devin text.
fn merged_fields(layers: &[&VectorLayer]) -> Vec<Field> {
    let mut out: Vec<Field> = vec![];
    for l in layers {
        for f in &l.fields {
            match out.iter_mut().find(|o| o.name.eq_ignore_ascii_case(&f.name)) {
                Some(o) => {
                    if o.ftype != f.ftype {
                        let num = |t: FieldType| matches!(t, FieldType::Integer | FieldType::Real);
                        o.ftype = if num(o.ftype) && num(f.ftype) {
                            FieldType::Real
                        } else {
                            FieldType::Text
                        };
                        o.width = 0;
                        o.precision = 0;
                    } else {
                        o.width = o.width.max(f.width);
                        o.precision = o.precision.max(f.precision);
                    }
                }
                None => out.push(f.clone()),
            }
        }
    }
    out
}

/// Unește mai multe straturi într-unul singur (câmpurile sunt reunite după nume).
/// Cu `source_field`, fiecare entitate primește numele stratului din care provine.
pub fn merge_layers(layers: &[&VectorLayer], name: &str, source_field: Option<&str>) -> VectorLayer {
    let mut fields = merged_fields(layers);
    let src_idx = source_field.map(|n| {
        let mut n = n.to_string();
        while fields.iter().any(|f| f.name.eq_ignore_ascii_case(&n)) {
            n.push('_');
        }
        fields.push(Field::new(n, FieldType::Text));
        fields.len() - 1
    });
    let mut out = VectorLayer::new(name);
    out.crs = layers.first().and_then(|l| l.crs.clone());
    out.has_z = layers.iter().any(|l| l.has_z);
    for l in layers {
        let map: Vec<Option<usize>> = fields
            .iter()
            .map(|f| l.fields.iter().position(|g| g.name.eq_ignore_ascii_case(&f.name)))
            .collect();
        for ft in &l.features {
            let attrs = fields
                .iter()
                .enumerate()
                .map(|(i, f)| {
                    if Some(i) == src_idx {
                        return Value::Text(l.name.clone());
                    }
                    map[i]
                        .and_then(|j| ft.attrs.get(j))
                        .map(|v| v.coerce(f.ftype))
                        .unwrap_or(Value::Null)
                })
                .collect();
            out.features.push(Feature {
                geom: ft.geom.clone(),
                attrs,
            });
        }
    }
    out.fields = fields;
    out
}

/// Entitățile DXF de suprafață, care rămân poligoane și cu `closed_as_lines`.
fn is_fill_entity(attrs: &[Value], et: Option<usize>) -> bool {
    et.and_then(|i| attrs.get(i))
        .map(|v| {
            matches!(
                v.to_text().to_uppercase().as_str(),
                "HATCH" | "SOLID" | "3DFACE" | "TRACE" | "REGION" | "MPOLYGON"
            )
        })
        .unwrap_or(false)
}

/// Inelele unui poligon ca linii.
fn polygon_as_lines(g: &Geometry) -> Option<Geometry> {
    let lines: Vec<Vec<Coord>> = g
        .polygons()
        .into_iter()
        .flat_map(|p| p.iter().cloned())
        .filter(|r| r.len() > 1)
        .collect();
    match lines.len() {
        0 => None,
        1 => Some(Geometry::LineString(lines.into_iter().next().unwrap())),
        _ => Some(Geometry::MultiLineString(lines)),
    }
}

/// Împarte un strat în trei: puncte, linii, poligoane. Colecțiile (de exemplu blocurile
/// DXF) sunt despărțite pe tipuri; entitățile fără geometrie sunt omise.
pub fn split_by_kind(layer: &VectorLayer, closed_as_lines: bool) -> [VectorLayer; 3] {
    let mk = |suffix: &str| {
        let mut l = VectorLayer::new(format!("{}_{suffix}", layer.name));
        l.fields = layer.fields.clone();
        l.crs = layer.crs.clone();
        l.has_z = layer.has_z;
        l
    };
    let mut out = [mk("puncte"), mk("linii"), mk("poligoane")];
    let et = layer.field_index("EntityType");
    for ft in &layer.features {
        let Some(g) = &ft.geom else { continue };
        let parts: Vec<Geometry> = match g {
            Geometry::Collection(p) => p.clone(),
            g => vec![g.clone()],
        };
        let mut by_kind: [Vec<Geometry>; 3] = [vec![], vec![], vec![]];
        for p in parts {
            match p.kind() {
                GeomKind::Point => by_kind[0].push(p),
                GeomKind::Line => by_kind[1].push(p),
                GeomKind::Polygon => {
                    if closed_as_lines && !is_fill_entity(&ft.attrs, et) {
                        if let Some(l) = polygon_as_lines(&p) {
                            by_kind[1].push(l);
                        }
                    } else {
                        by_kind[2].push(p);
                    }
                }
                GeomKind::Mixed => {}
            }
        }
        for (k, parts) in by_kind.into_iter().enumerate() {
            if let Some(g) = Geometry::from_parts(parts) {
                out[k].features.push(Feature {
                    geom: Some(g),
                    attrs: ft.attrs.clone(),
                });
            }
        }
    }
    out
}

/// Exportă straturile, unite, în câte un shapefile pentru fiecare tip de geometrie:
/// `<baza>_puncte.shp`, `<baza>_linii.shp`, `<baza>_poligoane.shp`.
/// Cu `target`, straturile sunt mai întâi reproiectate în acel sistem; fără, ele trebuie
/// să aibă același sistem.
pub fn export_by_kind(
    layers: &[VectorLayer],
    target: Option<&Crs>,
    base: &Path,
    so: &SplitOptions,
    wo: &WriteOptions,
) -> Result<SplitResult> {
    if layers.is_empty() {
        bail!(tr!("Nimic de scris"));
    }
    let mut warnings = vec![];
    let mut prepared: Vec<VectorLayer> = Vec::with_capacity(layers.len());
    for l in layers {
        let mut l = l.clone();
        if let Some(t) = target
            && l.crs.as_ref() != Some(t)
        {
            if l.crs.is_none() {
                bail!(
                    "{}",
                    tr!("Stratul „{}” nu are sistem de coordonate definit", l.name)
                );
            }
            let failed = crs::reproject_layer(&mut l, t)?;
            if failed > 0 {
                warnings.push(tr!(
                    "{failed} entități din „{}” nu au putut fi transformate complet",
                    l.name
                ));
            }
        }
        prepared.push(l);
    }
    if target.is_none() {
        let first = prepared[0].crs.clone();
        if prepared.iter().any(|l| l.crs != first) {
            bail!(tr!(
                "Straturile au sisteme de coordonate diferite – alegeți un sistem de ieșire comun"
            ));
        }
    }
    let refs: Vec<&VectorLayer> = prepared.iter().collect();
    let stem = base
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "export".into());
    let merged = merge_layers(&refs, &stem, so.source_field.as_deref());
    let no_geom = merged.features.iter().filter(|f| f.geom.is_none()).count();
    if no_geom > 0 {
        warnings.push(tr!("{no_geom} entități fără geometrie au fost omise"));
    }
    let parts = split_by_kind(&merged, so.closed_as_lines);
    let wanted = [so.points, so.lines, so.polygons];
    let kinds = [GeomKind::Point, GeomKind::Line, GeomKind::Polygon];
    let dir = base.parent().map(Path::to_path_buf).unwrap_or_default();
    if !dir.as_os_str().is_empty() {
        std::fs::create_dir_all(&dir)
            .with_context(|| tr!("Nu pot crea dosarul {}", dir.display()))?;
    }
    let mut written = vec![];
    for (i, part) in parts.iter().enumerate() {
        if !wanted[i] {
            continue;
        }
        if part.features.is_empty() {
            warnings.push(tr!("Nu există entități de tip {}", kinds[i].label()));
            continue;
        }
        let p = dir.join(format!("{}.shp", part.name));
        let files = formats::shp::write(part, &p, wo)?;
        let shp = files.into_iter().next().unwrap_or(p);
        written.push((shp, kinds[i], part.features.len()));
    }
    if written.is_empty() {
        bail!(tr!("Nicio entitate de exportat pentru tipurile alese"));
    }
    Ok(SplitResult { written, warnings })
}

#[cfg(test)]
mod split_tests {
    use super::*;

    fn dxf_like(name: &str, extra: &str) -> VectorLayer {
        let mut l = VectorLayer::new(name);
        l.fields = vec![
            Field::new("Layer", FieldType::Text),
            Field::new("EntityType", FieldType::Text),
            Field::new(extra, FieldType::Integer),
        ];
        let sq = vec![
            Coord::new(0.0, 0.0),
            Coord::new(1.0, 0.0),
            Coord::new(1.0, 1.0),
            Coord::new(0.0, 0.0),
        ];
        let rows: Vec<(&str, Option<Geometry>)> = vec![
            ("POINT", Some(Geometry::Point(Coord::new(1.0, 2.0)))),
            ("LINE", Some(Geometry::LineString(vec![Coord::new(0.0, 0.0), Coord::new(5.0, 5.0)]))),
            ("LWPOLYLINE", Some(Geometry::Polygon(vec![sq.clone()]))),
            ("HATCH", Some(Geometry::Polygon(vec![sq.clone()]))),
            (
                "INSERT",
                Some(Geometry::Collection(vec![
                    Geometry::Point(Coord::new(9.0, 9.0)),
                    Geometry::LineString(vec![Coord::new(0.0, 1.0), Coord::new(2.0, 3.0)]),
                ])),
            ),
            ("TEXT", None),
        ];
        for (t, g) in rows {
            l.features.push(Feature {
                geom: g,
                attrs: vec![Value::Text("L1".into()), Value::Text(t.into()), Value::Int(7)],
            });
        }
        l
    }

    #[test]
    fn merge_and_split() {
        let a = dxf_like("a", "Color");
        let b = dxf_like("b", "Weight");
        let m = merge_layers(&[&a, &b], "m", Some("Fisier"));
        let names: Vec<&str> = m.fields.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["Layer", "EntityType", "Color", "Weight", "Fisier"]);
        assert_eq!(m.features.len(), 12);
        assert_eq!(m.features[0].attrs[3], Value::Null);
        assert_eq!(m.features[6].attrs[4], Value::Text("b".into()));

        let [p, l, g] = split_by_kind(&m, false);
        // 2 straturi × (POINT + punctul din bloc)
        assert_eq!(p.features.len(), 4);
        // LINE + linia din bloc
        assert_eq!(l.features.len(), 4);
        // LWPOLYLINE închisă + HATCH
        assert_eq!(g.features.len(), 4);

        let [p2, l2, g2] = split_by_kind(&m, true);
        assert_eq!(p2.features.len(), 4);
        // polilinia închisă trece la linii, hașura rămâne poligon
        assert_eq!(l2.features.len(), 6);
        assert_eq!(g2.features.len(), 2);
        assert!(l2.features.iter().all(|f| f.geom.as_ref().unwrap().kind() == GeomKind::Line));
    }

    #[test]
    fn writes_three_shapefiles() {
        let dir = std::env::temp_dir().join(format!("cadastru_split_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let a = dxf_like("a", "Color");
        let so = SplitOptions::default();
        let wo = WriteOptions { overwrite: true, ..Default::default() };
        let r = export_by_kind(&[a], None, &dir.join("plan.shp"), &so, &wo).unwrap();
        let names: Vec<String> = r
            .written
            .iter()
            .map(|(p, _, n)| format!("{}:{n}", p.file_name().unwrap().to_string_lossy()))
            .collect();
        assert_eq!(names, ["plan_puncte.shp:2", "plan_linii.shp:2", "plan_poligoane.shp:2"]);
        let back = formats::shp::read(&dir.join("plan_linii.shp"), &Default::default()).unwrap();
        assert_eq!(back.features.len(), 2);
        assert!(back.field_index("Fisier").is_some());
        assert_eq!(r.warnings.len(), 1); // TEXT fără geometrie
        let _ = std::fs::remove_dir_all(&dir);
    }
}
