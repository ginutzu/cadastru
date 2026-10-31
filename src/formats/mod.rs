//! Citirea și scrierea fișierelor: detecție după extensie și funcții comune.

pub mod cgxml;
pub mod gdb;
pub mod csv;
pub mod dxf;
pub mod geojson;
pub mod gpkg;
pub mod gpx;
pub mod kml;
pub mod raster;
pub mod shp;

use crate::crs::Crs;
use crate::model::{Dataset, Raster, VectorLayer};
use anyhow::{Context, Result, bail};
use encoding_rs::Encoding;
use std::fs;
use std::path::{Path, PathBuf};
use crate::i18n::tr;

#[derive(Clone, Debug, Default)]
pub struct ReadOptions {
    /// Codificarea textului (ex. "UTF-8", "cp1250"); implicit: detecție automată.
    pub encoding: Option<String>,
    /// Coloanele X/Y/Z pentru CSV (implicit: detecție automată).
    pub x_field: Option<String>,
    pub y_field: Option<String>,
    pub z_field: Option<String>,
    /// Sistemul de coordonate impus (suprascrie .prj / informația din fișier).
    pub crs: Option<Crs>,
    /// DXF: polilinii închise ca linii (implicit devin poligoane).
    pub dxf_closed_as_lines: bool,
    /// Opțiunile de conversie CGXML.
    pub cgxml: cgxml::CgxmlOptions,
    /// Opțiunile CGXML au fost alese explicit (nu se înlocuiesc cu setările aplicației).
    pub cgxml_custom: bool,
    /// Păstrează doar entitățile din zona dată (ex. încadrarea României în EPSG:3844).
    pub area: Option<crate::area::AreaFilter>,
    /// Păstrează doar entitățile care îndeplinesc expresia (stil SQL WHERE).
    pub where_expr: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct WriteOptions {
    /// Codificare pentru DBF/DXF (implicit UTF-8 pentru DBF).
    pub encoding: Option<String>,
    /// Suprascrie fișierele existente.
    pub overwrite: bool,
    /// Numele stratului în fișiere multi-strat (GeoPackage).
    pub layer_name: Option<String>,
    /// Adaugă la un GeoPackage existent în loc să îl recreeze.
    pub append: bool,
    /// CSV: scrie geometria ca WKT (implicit X/Y pentru puncte).
    pub csv_wkt: bool,
    /// Numele câmpului folosit ca etichetă/nume (KML, GPX, DXF text).
    pub name_field: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Vector,
    Raster,
}

pub struct FormatInfo {
    pub name: &'static str,
    pub exts: &'static [&'static str],
    pub kind: Kind,
    pub read: bool,
    pub write: bool,
}

impl FormatInfo {
    /// Numele formatului în limba interfeței.
    pub fn label(&self) -> &'static str {
        crate::i18n::trs_static(self.name)
    }
}

pub const FORMATS: &[FormatInfo] = &[
    FormatInfo {
        name: "ESRI Shapefile",
        exts: &["shp"],
        kind: Kind::Vector,
        read: true,
        write: true,
    },
    FormatInfo {
        name: "GeoPackage",
        exts: &["gpkg"],
        kind: Kind::Vector,
        read: true,
        write: true,
    },
    FormatInfo {
        name: "GeoJSON",
        exts: &["geojson", "json"],
        kind: Kind::Vector,
        read: true,
        write: true,
    },
    FormatInfo {
        name: "KML / KMZ (Google Earth)",
        exts: &["kml", "kmz"],
        kind: Kind::Vector,
        read: true,
        write: true,
    },
    FormatInfo {
        name: "GPX (GPS)",
        exts: &["gpx"],
        kind: Kind::Vector,
        read: true,
        write: true,
    },
    FormatInfo {
        name: "AutoCAD DXF",
        exts: &["dxf"],
        kind: Kind::Vector,
        read: true,
        write: true,
    },
    FormatInfo {
        name: "CSV (X/Y sau WKT)",
        exts: &["csv", "txt"],
        kind: Kind::Vector,
        read: true,
        write: true,
    },
    FormatInfo {
        name: "CGXML (cadastru ANCPI)",
        exts: &["cgxml"],
        kind: Kind::Vector,
        read: true,
        write: false,
    },
    FormatInfo {
        name: "Esri File Geodatabase (dosar .gdb)",
        exts: &["gdb", "gdbtable"],
        kind: Kind::Vector,
        read: true,
        write: false,
    },
    FormatInfo {
        name: "Arhivă ZIP cu fișiere CGXML",
        exts: &["zip"],
        kind: Kind::Vector,
        read: true,
        write: false,
    },
    FormatInfo {
        name: "GeoTIFF",
        exts: &["tif", "tiff"],
        kind: Kind::Raster,
        read: true,
        write: true,
    },
    FormatInfo {
        name: "ESRI ASCII Grid",
        exts: &["asc"],
        kind: Kind::Raster,
        read: true,
        write: true,
    },
    FormatInfo {
        name: "PNG + world file",
        exts: &["png"],
        kind: Kind::Raster,
        read: true,
        write: true,
    },
    FormatInfo {
        name: "JPEG + world file",
        exts: &["jpg", "jpeg"],
        kind: Kind::Raster,
        read: true,
        write: true,
    },
    FormatInfo {
        name: "BMP + world file",
        exts: &["bmp"],
        kind: Kind::Raster,
        read: true,
        write: false,
    },
];

pub fn ext_of(path: &Path) -> String {
    path.extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

pub fn format_for(path: &Path) -> Option<&'static FormatInfo> {
    let e = ext_of(path);
    FORMATS.iter().find(|f| f.exts.contains(&e.as_str()))
}

/// Filtru pentru dialogul de deschidere: toate extensiile suportate.
pub fn all_read_exts() -> Vec<&'static str> {
    FORMATS
        .iter()
        .filter(|f| f.read)
        .flat_map(|f| f.exts.iter().copied())
        .collect()
}

/// Deschide un fișier și întoarce straturile conținute.
pub fn open(path: &Path, opts: &ReadOptions) -> Result<Vec<Dataset>> {
    open_ex(path, opts).map(|(d, _)| d)
}

/// Ca `open`, dar întoarce și mesaje informative (ex. statisticile conversiei CGXML).
/// Filtrele din opțiuni (zonă, expresie) se aplică după citire; rezumatul lor se adaugă la
/// primul mesaj.
pub fn open_ex(path: &Path, opts: &ReadOptions) -> Result<(Vec<Dataset>, Vec<String>)> {
    let (mut out, mut info) = open_raw(path, opts)?;
    if opts.area.is_none() && opts.where_expr.is_none() {
        return Ok((out, info));
    }
    let mut notes: Vec<String> = vec![];
    let n_vec = out.iter().filter(|d| matches!(d, Dataset::Vector(_))).count();
    let mut emptied = 0;
    for d in &mut out {
        let Dataset::Vector(v) = d else { continue };
        let before = v.features.len();
        if let Some(w) = &opts.where_expr {
            crate::expr::filter_layer(v, w)
                .map_err(|e| anyhow::anyhow!("{}", tr!("{}: expresia de filtrare: {}", v.name, e)))?;
            if opts.area.is_none() {
                notes.push(tr!("{}: păstrate {} din {}", v.name, v.features.len(), before));
            }
        }
        if let Some(a) = &opts.area {
            let mut st = crate::area::filter_layer(v, a);
            st.by_expr = before - st.total;
            st.total = before;
            notes.push(st.message(&v.name));
        }
        if before > 0 && v.features.is_empty() {
            emptied += 1;
        }
    }
    // straturile golite de filtru nu se mai încarcă
    if emptied > 0 {
        out.retain(|d| !matches!(d, Dataset::Vector(v) if v.features.is_empty()));
        if out.is_empty() && n_vec > 0 {
            bail!(
                "{}",
                tr!(
                    "{}: nicio entitate nu trece de filtru ({})",
                    path.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default(),
                    notes.join("; ")
                )
            );
        }
    }
    let summary = notes.join("; ");
    if summary.is_empty() {
        return Ok((out, info));
    }
    let summary = match &opts.area {
        Some(a) if a.is_romania() => tr!("Filtru România: {}", summary),
        Some(_) => tr!("Filtru zonă: {}", summary),
        None => tr!("Filtru: {}", summary),
    };
    match info.first_mut() {
        Some(f) => {
            f.push_str(" · ");
            f.push_str(&summary);
        }
        None => info.push(summary),
    }
    Ok((out, info))
}

fn open_raw(path: &Path, opts: &ReadOptions) -> Result<(Vec<Dataset>, Vec<String>)> {
    if !path.exists() {
        bail!("{}", tr!("Fișierul nu există: {}", path.display()));
    }
    let e = ext_of(path);
    let mut info: Vec<String> = vec![];
    if gdb::gdb_root(path).is_some() {
        let (layers, notes) = gdb::read(path)?;
        info.extend(notes);
        let mut out: Vec<Dataset> = layers.into_iter().map(Dataset::Vector).collect();
        if let Some(c) = &opts.crs {
            for d in &mut out {
                if let Dataset::Vector(v) = d {
                    v.crs = Some(c.clone());
                }
            }
        }
        return Ok((out, info));
    }
    if path.is_dir() || e == "cgxml" || e == "zip" {
        if e == "zip" && !cgxml::zip_has_cgxml(path) {
            bail!("{}", tr!("Arhiva {} nu conține fișiere .cgxml", path.display()));
        }
        let (layers, st) = cgxml::read(path, &opts.cgxml)?;
        info.push(st.summary());
        info.extend(st.errors.iter().cloned());
        return Ok((layers.into_iter().map(Dataset::Vector).collect(), info));
    }
    let mut out: Vec<Dataset> = match e.as_str() {
        "shp" | "dbf" => vec![Dataset::Vector(shp::read(
            &path.with_extension(if e == "dbf" { "shp" } else { &e }),
            opts,
        )?)],
        "gpkg" => gpkg::read(path, opts)?
            .into_iter()
            .map(Dataset::Vector)
            .collect(),
        "geojson" | "json" => vec![Dataset::Vector(geojson::read(path)?)],
        "kml" | "kmz" => kml::read(path)?.into_iter().map(Dataset::Vector).collect(),
        "gpx" => gpx::read(path)?.into_iter().map(Dataset::Vector).collect(),
        "dxf" => vec![Dataset::Vector(dxf::read(path, opts)?)],
        "csv" | "txt" => vec![Dataset::Vector(csv::read(path, opts)?)],
        "tif" | "tiff" | "asc" | "png" | "jpg" | "jpeg" | "bmp" => {
            vec![Dataset::Raster(raster::read(path)?)]
        }
        _ => bail!("{}", tr!("Format necunoscut: .{e}")),
    };
    if let Some(c) = &opts.crs {
        for d in &mut out {
            match d {
                Dataset::Vector(v) => v.crs = Some(c.clone()),
                Dataset::Raster(r) => r.crs = Some(c.clone()),
            }
        }
    }
    Ok((out, info))
}

fn check_overwrite(path: &Path, opts: &WriteOptions) -> Result<()> {
    if path.exists() && !opts.overwrite && !opts.append {
        bail!(
            "{}", tr!("Fișierul există deja: {} (folosiți opțiunea de suprascriere)",
            path.display()
        ));
    }
    if let Some(dir) = path.parent()
        && !dir.as_os_str().is_empty()
        && !dir.exists()
    {
        fs::create_dir_all(dir)
            .with_context(|| tr!("Nu pot crea dosarul {}", dir.display()))?;
    }
    Ok(())
}

/// Scrie unul sau mai multe straturi vectoriale. Întoarce fișierele create.
pub fn write_vector(
    layers: &[&VectorLayer],
    path: &Path,
    opts: &WriteOptions,
) -> Result<Vec<PathBuf>> {
    if layers.is_empty() {
        bail!(tr!("Nimic de scris"));
    }
    check_overwrite(path, opts)?;
    let e = ext_of(path);
    if e == "gpkg" {
        gpkg::write(layers, path, opts)?;
        return Ok(vec![path.to_path_buf()]);
    }
    let mut written = vec![];
    for (i, l) in layers.iter().enumerate() {
        let p = if layers.len() == 1 || i == 0 && layers.len() == 1 {
            path.to_path_buf()
        } else {
            let stem = path.file_stem().unwrap_or_default().to_string_lossy();
            path.with_file_name(format!("{}_{}.{}", stem, sanitize(&l.name), e))
        };
        match e.as_str() {
            "shp" => written.extend(shp::write(l, &p, opts)?),
            "geojson" | "json" => {
                geojson::write(l, &p)?;
                written.push(p)
            }
            "kml" | "kmz" => {
                kml::write(l, &p, opts)?;
                written.push(p)
            }
            "gpx" => {
                gpx::write(l, &p, opts)?;
                written.push(p)
            }
            "dxf" => {
                dxf::write(l, &p, opts)?;
                written.push(p)
            }
            "csv" | "txt" => {
                csv::write(l, &p, opts)?;
                written.push(p)
            }
            _ => bail!("{}", tr!("Formatul .{e} nu poate fi folosit pentru date vectoriale")),
        }
    }
    Ok(written)
}

pub fn write_raster(r: &Raster, path: &Path, opts: &WriteOptions) -> Result<Vec<PathBuf>> {
    check_overwrite(path, opts)?;
    raster::write(r, path)
}

pub fn sanitize(s: &str) -> String {
    let t: String = s
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if t.is_empty() { "strat".into() } else { t }
}

pub fn encoding_from_label(s: &str) -> Option<&'static Encoding> {
    let t = s.trim().to_lowercase();
    let t = t.trim_start_matches("ansi").trim();
    let t = t
        .trim_start_matches("cp")
        .trim_start_matches("windows-")
        .trim();
    match t {
        "" => None,
        "utf8" | "utf-8" | "65001" => Some(encoding_rs::UTF_8),
        "1250" => Some(encoding_rs::WINDOWS_1250),
        "1251" => Some(encoding_rs::WINDOWS_1251),
        "1252" => Some(encoding_rs::WINDOWS_1252),
        "1253" => Some(encoding_rs::WINDOWS_1253),
        "1254" => Some(encoding_rs::WINDOWS_1254),
        "1257" => Some(encoding_rs::WINDOWS_1257),
        "852" => Encoding::for_label(b"ibm852"),
        "866" => Some(encoding_rs::IBM866),
        "88592" | "8859-2" | "iso-8859-2" | "iso88592" | "latin2" => Some(encoding_rs::ISO_8859_2),
        "88591" | "8859-1" | "iso-8859-1" | "latin1" => Some(encoding_rs::WINDOWS_1252),
        "88596" | "iso-8859-16" | "885916" => Some(encoding_rs::ISO_8859_16),
        other => Encoding::for_label(other.as_bytes())
            .or_else(|| Encoding::for_label(s.trim().as_bytes())),
    }
}

/// Codifică textul în `enc`; diacriticele românești cu virgulă devin cu sedilă (Ș->Ş etc.)
/// când codificarea nu le conține, iar restul caracterelor nereprezentabile devin „?”.
pub fn encode_lossy(enc: &'static Encoding, s: &str) -> Vec<u8> {
    if enc == encoding_rs::UTF_8 {
        return s.as_bytes().to_vec();
    }
    let mut out = Vec::with_capacity(s.len());
    let mut buf = [0u8; 4];
    for ch in s.chars() {
        let try_enc = |c: char, buf: &mut [u8; 4]| -> Option<Vec<u8>> {
            let (b, _, err) = enc.encode(c.encode_utf8(buf));
            if err { None } else { Some(b.into_owned()) }
        };
        if let Some(b) = try_enc(ch, &mut buf) {
            out.extend(b);
            continue;
        }
        let alt = match ch {
            'Ș' => 'Ş',
            'ș' => 'ş',
            'Ț' => 'Ţ',
            'ț' => 'ţ',
            _ => '?',
        };
        out.extend(try_enc(alt, &mut buf).unwrap_or_else(|| b"?".to_vec()));
    }
    out
}

/// Citește sistemul din fișierul .prj de lângă `path` (sau .aux.xml).
pub fn read_prj_sidecar(path: &Path) -> Option<Crs> {
    for ext in ["prj", "PRJ"] {
        let p = path.with_extension(ext);
        if let Ok(s) = fs::read_to_string(&p)
            && !s.trim().is_empty()
            && let Ok(c) = Crs::from_wkt(&s).or_else(|_| Crs::parse(&s))
        {
            return Some(c);
        }
    }
    // GDAL .aux.xml
    let mut aux = path.as_os_str().to_owned();
    aux.push(".aux.xml");
    if let Ok(s) = fs::read_to_string(PathBuf::from(aux))
        && let (Some(a), Some(b)) = (s.find("<SRS"), s.find("</SRS>"))
        && let Some(gt) = s[a..b].find('>')
    {
        let wkt = s[a + gt + 1..b]
            .replace("&quot;", "\"")
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&amp;", "&");
        return Crs::from_wkt(&wkt).ok();
    }
    None
}

pub fn write_prj_sidecar(path: &Path, crs: &Crs) -> Result<()> {
    if let Some(w) = crs.to_wkt() {
        fs::write(path.with_extension("prj"), w)?;
    }
    Ok(())
}

/// Lista fișierelor auxiliare care însoțesc un fișier (pentru copiere/ștergere).
pub fn companions(path: &Path) -> Vec<PathBuf> {
    let mut v = vec![path.to_path_buf()];
    if ext_of(path) == "shp" {
        for e in ["shx", "dbf", "prj", "cpg", "qix", "sbn", "sbx"] {
            let p = path.with_extension(e);
            if p.exists() {
                v.push(p);
            }
        }
    }
    v
}
