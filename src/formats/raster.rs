//! Formate raster: GeoTIFF, ESRI ASCII Grid, PNG/JPEG/BMP cu world file.

use super::{ext_of, read_prj_sidecar, write_prj_sidecar};
use crate::crs::Crs;
use crate::model::*;
use anyhow::{Context, Result, anyhow, bail};
use std::fs;
use std::io::{BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use tiff::decoder::{Decoder, DecodingResult, Limits};
use tiff::encoder::{Compression, DeflateLevel, TiffEncoder, colortype};
use tiff::tags::Tag;
use crate::i18n::tr;

pub fn read(path: &Path) -> Result<Raster> {
    let mut r = match ext_of(path).as_str() {
        "tif" | "tiff" => read_geotiff(path)?,
        "asc" => read_asc(path)?,
        "png" | "jpg" | "jpeg" | "bmp" => read_image(path)?,
        e => bail!("{}", tr!("Format raster necunoscut: .{e}")),
    };
    r.source = Some(path.to_path_buf());
    if r.name.is_empty() {
        r.name = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
    }
    Ok(r)
}

pub fn write(r: &Raster, path: &Path) -> Result<Vec<PathBuf>> {
    match ext_of(path).as_str() {
        "tif" | "tiff" => write_geotiff(r, path)?,
        "asc" => write_asc(r, path)?,
        "png" | "jpg" | "jpeg" => return write_image(r, path),
        e => bail!("{}", tr!("Nu pot scrie raster în formatul .{e}")),
    }
    Ok(vec![path.to_path_buf()])
}

// ---------------------------------------------------------------------------
// World files
// ---------------------------------------------------------------------------

fn world_exts(ext: &str) -> Vec<String> {
    let e = ext.to_lowercase();
    let mut v = vec![];
    if e.len() >= 2 {
        v.push(format!("{}{}w", &e[..1], &e[e.len() - 1..]));
    }
    v.push(format!("{e}w"));
    v.push("wld".into());
    v
}

pub fn read_world_file(path: &Path) -> Option<[f64; 6]> {
    let ext = ext_of(path);
    for we in world_exts(&ext) {
        for cand in [
            path.with_extension(&we),
            path.with_extension(we.to_uppercase()),
        ] {
            if let Ok(s) = fs::read_to_string(&cand) {
                let v: Vec<f64> = s
                    .split_whitespace()
                    .filter_map(|t| t.replace(',', ".").parse().ok())
                    .collect();
                if v.len() >= 6 {
                    let (a, d, b, e, c, f) = (v[0], v[1], v[2], v[3], v[4], v[5]);
                    return Some([c - a / 2.0 - b / 2.0, a, b, f - d / 2.0 - e / 2.0, d, e]);
                }
            }
        }
    }
    None
}

pub fn write_world_file(path: &Path, gt: &[f64; 6]) -> Result<PathBuf> {
    let we = world_exts(&ext_of(path))
        .into_iter()
        .next()
        .unwrap_or_else(|| "wld".into());
    let p = path.with_extension(we);
    let (a, b, d, e) = (gt[1], gt[2], gt[4], gt[5]);
    let c = gt[0] + a / 2.0 + b / 2.0;
    let f = gt[3] + d / 2.0 + e / 2.0;
    fs::write(
        &p,
        format!("{a:.12}\n{d:.12}\n{b:.12}\n{e:.12}\n{c:.12}\n{f:.12}\n"),
    )?;
    Ok(p)
}

// ---------------------------------------------------------------------------
// GeoTIFF
// ---------------------------------------------------------------------------

fn split_bands<T: Copy>(
    v: Vec<T>,
    n: usize,
    planar: bool,
    w: usize,
    h: usize,
    mk: fn(Vec<T>) -> BandData,
) -> Vec<BandData> {
    let px = w * h;
    if n == 1 {
        return vec![mk(v)];
    }
    if planar {
        return (0..n)
            .filter(|b| (b + 1) * px <= v.len())
            .map(|b| mk(v[b * px..(b + 1) * px].to_vec()))
            .collect();
    }
    (0..n)
        .map(|b| mk((0..px).map(|i| v[i * n + b]).collect()))
        .collect()
}

struct GeoKeys {
    keys: Vec<(u16, u16, u16, u16)>,
    doubles: Vec<f64>,
    ascii: String,
}

impl GeoKeys {
    fn short(&self, id: u16) -> Option<u16> {
        self.keys
            .iter()
            .find(|k| k.0 == id && k.1 == 0)
            .map(|k| k.3)
    }
    fn double(&self, id: u16) -> Option<f64> {
        self.keys
            .iter()
            .find(|k| k.0 == id && k.1 == 34736)
            .and_then(|k| self.doubles.get(k.3 as usize).copied())
    }
    fn ascii(&self, id: u16) -> Option<String> {
        let k = self.keys.iter().find(|k| k.0 == id && k.1 == 34737)?;
        let s = self
            .ascii
            .get(k.3 as usize..(k.3 as usize + k.2 as usize))?;
        Some(s.trim_end_matches(['|', '\0']).to_string())
    }
}

/// Construiește CRS-ul din cheile GeoTIFF.
fn crs_from_geokeys(g: &GeoKeys) -> Option<Crs> {
    let model = g.short(1024).unwrap_or(0);
    if let Some(pcs) = g.short(3072)
        && pcs != 32767
        && pcs != 0
        && let Ok(c) = Crs::from_epsg(pcs as u32)
    {
        return Some(c);
    }
    if model == 2
        && let Some(gcs) = g.short(2048)
        && gcs != 32767
        && let Ok(c) = Crs::from_epsg(gcs as u32)
    {
        return Some(c);
    }
    // Stereo 1930 București (scris de Cadastru ca sistem local)
    if let Some(cit) = g.ascii(1026)
        && crate::crs::is_stereo30_name(&cit)
    {
        return Some(Crs::stereo30());
    }
    // WKT ESRI în citare (scris de GDAL/ArcGIS)
    for id in [1026u16, 3073, 2049] {
        if let Some(cit) = g.ascii(id)
            && let Some(p) = cit
                .find("PROJCS[")
                .or_else(|| cit.find("GEOGCS["))
                .or_else(|| cit.find("LOCAL_CS["))
            && let Ok(c) = Crs::from_wkt(&cit[p..])
        {
            return Some(c);
        }
    }
    // sistem definit de utilizator (ex. exporturi Global Mapper)
    if model == 1 {
        let datum_of = |g: &GeoKeys| -> String {
            if let Some(gcs) = g.short(2048).filter(|&c| c != 32767)
                && let Ok(gc) = Crs::from_epsg(gcs as u32)
            {
                return gc
                    .proj
                    .split_whitespace()
                    .filter(|p| !p.starts_with("+proj") && *p != "+no_defs")
                    .collect::<Vec<_>>()
                    .join(" ");
            }
            match g.short(2056) {
                Some(7024) => "+ellps=krass".into(),
                Some(7030) => "+datum=WGS84".into(),
                Some(7019) => "+ellps=GRS80 +towgs84=0,0,0".into(),
                Some(7022) => "+ellps=intl".into(),
                Some(7004) => "+ellps=bessel".into(),
                _ => match (g.double(2057), g.double(2059)) {
                    (Some(a), Some(rf)) => format!("+a={a} +rf={rf}"),
                    _ => "+datum=WGS84".into(),
                },
            }
        };
        // proiecții UTM date prin cod (ProjectionGeoKey 16001-16060 nord, 16101-16160 sud)
        if g.short(3075).is_none()
            && let Some(p) = g.short(3074)
        {
            let (zone, south) = match p {
                16001..=16060 => (p - 16000, false),
                16101..=16160 => (p - 16100, true),
                _ => return None,
            };
            let s = format!(
                "+proj=utm +zone={zone}{} {} +units=m +no_defs",
                if south { " +south" } else { "" },
                datum_of(g)
            );
            return Crs::parse(&s).ok();
        }
        let ct = g.short(3075)?;
        let proj = match ct {
            1 => "tmerc",
            16 => "sterea",
            8 => "lcc",
            10 => "laea",
            7 => "merc",
            14 => "stere",
            11 => "aea",
            _ => return None,
        };
        let d = |ids: &[u16]| ids.iter().find_map(|&i| g.double(i));
        let lat0 = d(&[3081, 3089, 3085]).unwrap_or(0.0);
        let lon0 = d(&[3080, 3088, 3084]).unwrap_or(0.0);
        let k = d(&[3092, 3093]).unwrap_or(1.0);
        let x0 = d(&[3082, 3086]).unwrap_or(0.0);
        let y0 = d(&[3083, 3087]).unwrap_or(0.0);
        let mut s = format!("+proj={proj} +lat_0={lat0} +lon_0={lon0} +k={k} +x_0={x0} +y_0={y0}");
        if ct == 8 || ct == 11 {
            s.push_str(&format!(
                " +lat_1={} +lat_2={}",
                d(&[3078]).unwrap_or(lat0),
                d(&[3079]).unwrap_or(lat0)
            ));
        }
        let datum = datum_of(g);
        s.push(' ');
        s.push_str(&datum);
        s.push_str(" +units=m +no_defs");
        return Crs::parse(&s).ok();
    }
    None
}

fn read_geotiff(path: &Path) -> Result<Raster> {
    let f = BufReader::new(
        fs::File::open(path).with_context(|| tr!("Nu pot citi {}", path.display()))?,
    );
    let mut dec = Decoder::new(f)
        .map_err(|e| anyhow!("{}", tr!("TIFF invalid: {e}")))?
        .with_limits(Limits::unlimited());
    let (w, h) = dec.dimensions()?;
    let (w, h) = (w as usize, h as usize);
    let ct = dec.colortype()?;
    let spp = ct.num_samples() as usize;
    let planar = dec
        .find_tag_unsigned::<u16>(Tag::PlanarConfiguration)?
        .unwrap_or(1)
        == 2;
    let colormap = dec
        .find_tag(Tag::ColorMap)?
        .and_then(|v| v.into_u16_vec().ok());
    let scale = dec
        .find_tag(Tag::ModelPixelScaleTag)?
        .and_then(|v| v.into_f64_vec().ok());
    let tie = dec
        .find_tag(Tag::ModelTiepointTag)?
        .and_then(|v| v.into_f64_vec().ok());
    let xform = dec
        .find_tag(Tag::ModelTransformationTag)?
        .and_then(|v| v.into_f64_vec().ok());
    let gkd = dec
        .find_tag(Tag::GeoKeyDirectoryTag)?
        .and_then(|v| v.into_u16_vec().ok());
    let gdp = dec
        .find_tag(Tag::GeoDoubleParamsTag)?
        .and_then(|v| v.into_f64_vec().ok())
        .unwrap_or_default();
    let gap = dec
        .find_tag(Tag::GeoAsciiParamsTag)?
        .and_then(|v| v.into_string().ok())
        .unwrap_or_default();
    let nodata = dec
        .find_tag(Tag::GdalNodata)?
        .and_then(|v| v.into_string().ok())
        .and_then(|s| {
            let t = s.trim_matches(char::from(0)).trim();
            if t.eq_ignore_ascii_case("nan") {
                Some(f64::NAN)
            } else {
                t.parse::<f64>().ok()
            }
        });

    let mut buf = DecodingResult::U8(vec![]);
    dec.read_image_to_buffer(&mut buf)
        .map_err(|e| anyhow!("{}", tr!("Nu pot decoda imaginea TIFF: {e}")))?;
    let mut bands: Vec<BandData> = match buf {
        DecodingResult::U8(v) => split_bands(v, spp, planar, w, h, BandData::U8),
        DecodingResult::I8(v) => split_bands(v, spp, planar, w, h, BandData::I8),
        DecodingResult::U16(v) => split_bands(v, spp, planar, w, h, BandData::U16),
        DecodingResult::I16(v) => split_bands(v, spp, planar, w, h, BandData::I16),
        DecodingResult::U32(v) => split_bands(v, spp, planar, w, h, BandData::U32),
        DecodingResult::I32(v) => split_bands(v, spp, planar, w, h, BandData::I32),
        DecodingResult::F32(v) => split_bands(v, spp, planar, w, h, BandData::F32),
        DecodingResult::F64(v) => split_bands(v, spp, planar, w, h, BandData::F64),
        DecodingResult::U64(v) => split_bands(
            v.into_iter().map(|x| x as f64).collect(),
            spp,
            planar,
            w,
            h,
            BandData::F64,
        ),
        DecodingResult::I64(v) => split_bands(
            v.into_iter().map(|x| x as f64).collect(),
            spp,
            planar,
            w,
            h,
            BandData::F64,
        ),
        DecodingResult::F16(v) => split_bands(
            v.into_iter().map(|x| x.to_f32()).collect(),
            spp,
            planar,
            w,
            h,
            BandData::F32,
        ),
    };
    // paletă -> RGB
    if let (Some(cm), true) = (colormap, bands.len() == 1) {
        let n = cm.len() / 3;
        let idx = &bands[0];
        let mut rgb = vec![vec![0u8; w * h]; 3];
        for i in 0..w * h {
            let k = (idx.get(i) as usize).min(n.saturating_sub(1));
            for (c, band) in rgb.iter_mut().enumerate() {
                band[i] = (cm[c * n + k] >> 8) as u8;
            }
        }
        bands = rgb.into_iter().map(BandData::U8).collect();
    }

    let mut gt = [0.0, 1.0, 0.0, 0.0, 0.0, -1.0];
    let mut georef = false;
    let keys = gkd.map(|d| GeoKeys {
        keys: d
            .get(4..)
            .unwrap_or(&[])
            .chunks(4)
            .filter(|c| c.len() == 4)
            .map(|c| (c[0], c[1], c[2], c[3]))
            .collect(),
        doubles: gdp,
        ascii: gap,
    });
    if let Some(m) = xform.filter(|m| m.len() >= 16) {
        gt = [m[3], m[0], m[1], m[7], m[4], m[5]];
        georef = true;
    } else if let (Some(s), Some(t)) = (scale, tie)
        && s.len() >= 2
        && t.len() >= 6
    {
        gt = [
            t[3] - t[0] * s[0],
            s[0],
            0.0,
            t[4] + t[1] * s[1],
            0.0,
            -s[1],
        ];
        georef = true;
    }
    if georef && keys.as_ref().and_then(|k| k.short(1025)) == Some(2) {
        // PixelIsPoint: decalaj de jumătate de pixel (convenția GDAL)
        gt[0] -= 0.5 * gt[1] + 0.5 * gt[2];
        gt[3] -= 0.5 * gt[4] + 0.5 * gt[5];
    }
    if !georef && let Some(wf) = read_world_file(path) {
        gt = wf;
    }
    let crs = read_prj_sidecar(path).or_else(|| keys.as_ref().and_then(crs_from_geokeys));
    Ok(Raster {
        name: String::new(),
        source: None,
        width: w,
        height: h,
        bands,
        gt,
        nodata,
        crs,
        full: None,
    })
}

fn geokeys_for(crs: Option<&Crs>) -> (Vec<u16>, String) {
    let mut keys: Vec<[u16; 4]> = vec![];
    let mut ascii = String::new();
    match crs {
        Some(c) if c.epsg.is_some() => {
            let code = c.epsg.unwrap() as u16;
            if c.is_geographic() {
                keys.push([1024, 0, 1, 2]);
                keys.push([1025, 0, 1, 1]);
                keys.push([2048, 0, 1, code]);
            } else {
                keys.push([1024, 0, 1, 1]);
                keys.push([1025, 0, 1, 1]);
                keys.push([3072, 0, 1, code]);
            }
            ascii = format!("{}|", c.name);
            keys.push([1026, 34737, ascii.len() as u16, 0]);
        }
        Some(c) if c.stereo_kind().is_some() => {
            // Stereo 1930 București: sistem local (model definit de utilizator), numele în citare
            keys.push([1024, 0, 1, 32767]);
            keys.push([1025, 0, 1, 1]);
            ascii = "Stereo 1930 Bucuresti (TransDatRO)|".to_string();
            keys.push([1026, 34737, ascii.len() as u16, 0]);
        }
        Some(c) => {
            // sistem fără cod EPSG: WKT ESRI în citare (citit de GDAL)
            keys.push([1024, 0, 1, if c.is_geographic() { 2 } else { 1 }]);
            keys.push([1025, 0, 1, 1]);
            if let Some(w) = c.to_wkt() {
                ascii = format!("ESRI PE String = {w}|");
                keys.push([1026, 34737, ascii.len() as u16, 0]);
            }
            keys.push([if c.is_geographic() { 2048 } else { 3072 }, 0, 1, 32767]);
        }
        None => {
            keys.push([1025, 0, 1, 1]);
        }
    }
    keys.sort_by_key(|k| k[0]);
    let mut dir: Vec<u16> = vec![1, 1, 0, keys.len() as u16];
    for k in keys {
        dir.extend_from_slice(&k);
    }
    (dir, ascii)
}

macro_rules! write_tiff_image {
    ($enc:expr, $ct:ty, $w:expr, $h:expr, $data:expr, $r:expr) => {{
        let mut img = $enc.new_image::<$ct>($w as u32, $h as u32)?;
        write_geo_tags(img.encoder(), $r)?;
        img.write_data($data)?;
    }};
}

fn write_geo_tags<W: std::io::Write + std::io::Seek, K: tiff::encoder::TiffKind>(
    enc: &mut tiff::encoder::DirectoryEncoder<'_, W, K>,
    r: &Raster,
) -> Result<()> {
    let g = &r.gt;
    if g[2] == 0.0 && g[4] == 0.0 {
        enc.write_tag(Tag::ModelPixelScaleTag, &[g[1], -g[5], 0.0][..])?;
        enc.write_tag(Tag::ModelTiepointTag, &[0.0, 0.0, 0.0, g[0], g[3], 0.0][..])?;
    } else {
        let m = [
            g[1], g[2], 0.0, g[0], g[4], g[5], 0.0, g[3], 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0,
        ];
        enc.write_tag(Tag::ModelTransformationTag, &m[..])?;
    }
    let (dir, ascii) = geokeys_for(r.crs.as_ref());
    enc.write_tag(Tag::GeoKeyDirectoryTag, &dir[..])?;
    if !ascii.is_empty() {
        enc.write_tag(Tag::GeoAsciiParamsTag, ascii.as_str())?;
    }
    if let Some(nd) = r.nodata {
        let s = if nd.is_nan() {
            "nan".to_string()
        } else {
            fmt_real(nd).trim_end_matches(".0").to_string()
        };
        enc.write_tag(Tag::GdalNodata, s.as_str())?;
    }
    Ok(())
}

fn interleave<T: Copy + Default>(bands: &[&Vec<T>]) -> Vec<T> {
    let n = bands.len();
    let px = bands[0].len();
    let mut out = vec![T::default(); px * n];
    for (b, band) in bands.iter().enumerate() {
        for i in 0..px {
            out[i * n + b] = band[i];
        }
    }
    out
}

fn as_f32(b: &BandData) -> Vec<f32> {
    (0..b.len()).map(|i| b.get(i) as f32).collect()
}

fn write_geotiff(r: &Raster, path: &Path) -> Result<()> {
    let file = BufWriter::new(
        fs::File::create(path).with_context(|| tr!("Nu pot scrie {}", path.display()))?,
    );
    let big = (r.width * r.height * r.bands.len() * 8) as u64 > 3_500_000_000;
    let (w, h) = (r.width, r.height);
    macro_rules! go {
        ($enc:ident) => {{
            let mut $enc = $enc.with_compression(Compression::Deflate(DeflateLevel::Balanced));
            let nb = r.bands.len();
            match (nb, &r.bands[0]) {
                (1, BandData::U8(v)) => write_tiff_image!($enc, colortype::Gray8, w, h, v, r),
                (1, BandData::U16(v)) => write_tiff_image!($enc, colortype::Gray16, w, h, v, r),
                (1, BandData::I16(v)) => write_tiff_image!($enc, colortype::GrayI16, w, h, v, r),
                (1, BandData::U32(v)) => write_tiff_image!($enc, colortype::Gray32, w, h, v, r),
                (1, BandData::I32(v)) => write_tiff_image!($enc, colortype::GrayI32, w, h, v, r),
                (1, BandData::F64(v)) => {
                    write_tiff_image!($enc, colortype::Gray64Float, w, h, v, r)
                }
                (1, b) => write_tiff_image!($enc, colortype::Gray32Float, w, h, &as_f32(b), r),
                (3 | 4, _)
                    if r.bands
                        .iter()
                        .take(nb)
                        .all(|b| matches!(b, BandData::U8(_))) =>
                {
                    let v: Vec<&Vec<u8>> = r
                        .bands
                        .iter()
                        .map(|b| {
                            if let BandData::U8(v) = b {
                                v
                            } else {
                                unreachable!()
                            }
                        })
                        .collect();
                    let data = interleave(&v);
                    if nb == 3 {
                        write_tiff_image!($enc, colortype::RGB8, w, h, &data, r)
                    } else {
                        write_tiff_image!($enc, colortype::RGBA8, w, h, &data, r)
                    }
                }
                (3 | 4, _) if r.bands.iter().all(|b| matches!(b, BandData::U16(_))) => {
                    let v: Vec<&Vec<u16>> = r
                        .bands
                        .iter()
                        .map(|b| {
                            if let BandData::U16(v) = b {
                                v
                            } else {
                                unreachable!()
                            }
                        })
                        .collect();
                    let data = interleave(&v);
                    if nb == 3 {
                        write_tiff_image!($enc, colortype::RGB16, w, h, &data, r)
                    } else {
                        write_tiff_image!($enc, colortype::RGBA16, w, h, &data, r)
                    }
                }
                (n, _) if n >= 3 => {
                    let f: Vec<Vec<f32>> = r.bands.iter().take(3).map(as_f32).collect();
                    let refs: Vec<&Vec<f32>> = f.iter().collect();
                    let data = interleave(&refs);
                    write_tiff_image!($enc, colortype::RGB32Float, w, h, &data, r)
                }
                (_, b) => write_tiff_image!($enc, colortype::Gray32Float, w, h, &as_f32(b), r),
            }
        }};
    }
    if big {
        let enc = TiffEncoder::new_big(file)?;
        go!(enc);
    } else {
        let enc = TiffEncoder::new(file)?;
        go!(enc);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// ESRI ASCII Grid
// ---------------------------------------------------------------------------

fn read_asc(path: &Path) -> Result<Raster> {
    let txt =
        fs::read_to_string(path).with_context(|| tr!("Nu pot citi {}", path.display()))?;
    let mut it = txt.split_whitespace().peekable();
    let (mut ncols, mut nrows) = (0usize, 0usize);
    let (mut xll, mut yll, mut cs, mut dx, mut dy) = (0.0, 0.0, 0.0, 0.0, 0.0);
    let (mut xcenter, mut ycenter) = (false, false);
    let mut nodata = None;
    while let Some(k) = it.peek() {
        if k.parse::<f64>().is_ok() {
            break;
        }
        let key = it.next().unwrap().to_lowercase();
        let val = it.next().ok_or_else(|| anyhow!(tr!("Antet ASC incomplet")))?;
        let v: f64 = val
            .parse()
            .map_err(|_| anyhow!("{}", tr!("Valoare invalidă în antet: {key} {val}")))?;
        match key.as_str() {
            "ncols" => ncols = v as usize,
            "nrows" => nrows = v as usize,
            "xllcorner" => xll = v,
            "yllcorner" => yll = v,
            "xllcenter" => {
                xll = v;
                xcenter = true
            }
            "yllcenter" => {
                yll = v;
                ycenter = true
            }
            "cellsize" => cs = v,
            "dx" => dx = v,
            "dy" => dy = v,
            "nodata_value" => nodata = Some(v),
            _ => {}
        }
    }
    if dx == 0.0 {
        dx = cs;
    }
    if dy == 0.0 {
        dy = cs;
    }
    if ncols == 0 || nrows == 0 || dx <= 0.0 {
        bail!(tr!("Antet ASC invalid"));
    }
    if xcenter {
        xll -= dx / 2.0;
    }
    if ycenter {
        yll -= dy / 2.0;
    }
    let mut data = Vec::with_capacity(ncols * nrows);
    for t in it.take(ncols * nrows) {
        data.push(t.parse::<f32>().unwrap_or(f32::NAN));
    }
    data.resize(ncols * nrows, nodata.unwrap_or(f64::NAN) as f32);
    Ok(Raster {
        name: String::new(),
        source: None,
        width: ncols,
        height: nrows,
        bands: vec![BandData::F32(data)],
        gt: [xll, dx, 0.0, yll + nrows as f64 * dy, 0.0, -dy],
        nodata,
        crs: read_prj_sidecar(path),
        full: None,
    })
}

fn write_asc(r: &Raster, path: &Path) -> Result<()> {
    if r.gt[2] != 0.0 || r.gt[4] != 0.0 {
        bail!(tr!("ASCII Grid nu suportă rastere rotite – reproiectați întâi"));
    }
    let mut w = BufWriter::new(
        fs::File::create(path).with_context(|| tr!("Nu pot scrie {}", path.display()))?,
    );
    let (dx, dy) = (r.gt[1], -r.gt[5]);
    writeln!(w, "ncols {}", r.width)?;
    writeln!(w, "nrows {}", r.height)?;
    writeln!(w, "xllcorner {:.10}", r.gt[0])?;
    writeln!(w, "yllcorner {:.10}", r.gt[3] - r.height as f64 * dy)?;
    if (dx - dy).abs() < 1e-9 * dx.abs().max(1.0) {
        writeln!(w, "cellsize {:.12}", dx)?;
    } else {
        writeln!(w, "dx {:.12}", dx)?;
        writeln!(w, "dy {:.12}", dy)?;
    }
    let nd = match r.nodata {
        Some(n) if !n.is_nan() => n,
        _ => -9999.0,
    };
    writeln!(w, "NODATA_value {}", fmt_real(nd).trim_end_matches(".0"))?;
    let b = &r.bands[0];
    for row in 0..r.height {
        let mut line = String::with_capacity(r.width * 8);
        for col in 0..r.width {
            let v = b.get(row * r.width + col);
            let v = if r.is_nodata(v) || !v.is_finite() {
                nd
            } else {
                v
            };
            if col > 0 {
                line.push(' ');
            }
            if b.is_float() {
                line.push_str(&format!("{}", v as f32));
            } else {
                line.push_str(&format!("{}", v as i64));
            }
        }
        writeln!(w, "{line}")?;
    }
    w.flush()?;
    if let Some(c) = &r.crs {
        write_prj_sidecar(path, c)?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Imagini (PNG/JPEG/BMP)
// ---------------------------------------------------------------------------

fn read_image(path: &Path) -> Result<Raster> {
    let img = image::open(path)
        .with_context(|| tr!("Nu pot deschide imaginea {}", path.display()))?;
    let (w, h) = (img.width() as usize, img.height() as usize);
    use image::DynamicImage::*;
    let bands: Vec<BandData> = match img {
        ImageLuma8(b) => vec![BandData::U8(b.into_raw())],
        ImageLuma16(b) => vec![BandData::U16(b.into_raw())],
        ImageLumaA8(b) => {
            let raw = b.into_raw();
            vec![
                BandData::U8(raw.iter().step_by(2).copied().collect()),
                BandData::U8(raw.iter().skip(1).step_by(2).copied().collect()),
            ]
        }
        ImageRgb16(b) => {
            let raw = b.into_raw();
            (0..3)
                .map(|c| BandData::U16(raw.iter().skip(c).step_by(3).copied().collect()))
                .collect()
        }
        ImageRgba8(b) => {
            let raw = b.into_raw();
            (0..4)
                .map(|c| BandData::U8(raw.iter().skip(c).step_by(4).copied().collect()))
                .collect()
        }
        other => {
            let raw = other.to_rgb8().into_raw();
            (0..3)
                .map(|c| BandData::U8(raw.iter().skip(c).step_by(3).copied().collect()))
                .collect()
        }
    };
    let gt = read_world_file(path).unwrap_or([0.0, 1.0, 0.0, h as f64, 0.0, -1.0]);
    Ok(Raster {
        name: String::new(),
        source: None,
        width: w,
        height: h,
        bands,
        gt,
        nodata: None,
        crs: read_prj_sidecar(path),
        full: None,
    })
}

/// Convertește o bandă la 8 biți (întindere min–max).
pub fn band_to_u8(r: &Raster, b: usize) -> Vec<u8> {
    match &r.bands[b] {
        BandData::U8(v) => v.clone(),
        band => {
            let st = r.stats(b);
            let (lo, hi) = (
                st.min,
                if st.max > st.min {
                    st.max
                } else {
                    st.min + 1.0
                },
            );
            (0..band.len())
                .map(|i| {
                    let v = band.get(i);
                    if r.is_nodata(v) || !v.is_finite() {
                        0
                    } else {
                        (((v - lo) / (hi - lo)) * 255.0).round().clamp(0.0, 255.0) as u8
                    }
                })
                .collect()
        }
    }
}

fn write_image(r: &Raster, path: &Path) -> Result<Vec<PathBuf>> {
    let (w, h) = (r.width as u32, r.height as u32);
    let jpeg = matches!(ext_of(path).as_str(), "jpg" | "jpeg");
    let nb = if r.bands.len() >= 3 {
        if r.bands.len() >= 4 && !jpeg { 4 } else { 3 }
    } else {
        1
    };
    let bands: Vec<Vec<u8>> = (0..nb).map(|b| band_to_u8(r, b)).collect();
    let img: image::DynamicImage = match nb {
        1 => {
            image::GrayImage::from_raw(w, h, bands[0].clone()).map(image::DynamicImage::ImageLuma8)
        }
        3 => image::RgbImage::from_raw(w, h, interleave(&bands.iter().collect::<Vec<_>>()))
            .map(image::DynamicImage::ImageRgb8),
        _ => image::RgbaImage::from_raw(w, h, interleave(&bands.iter().collect::<Vec<_>>()))
            .map(image::DynamicImage::ImageRgba8),
    }
    .ok_or_else(|| anyhow!(tr!("Dimensiuni invalide")))?;
    img.save(path)
        .with_context(|| tr!("Nu pot scrie {}", path.display()))?;
    let mut out = vec![path.to_path_buf(), write_world_file(path, &r.gt)?];
    if let Some(c) = &r.crs {
        write_prj_sidecar(path, c)?;
        out.push(path.with_extension("prj"));
        // GDAL/QGIS citesc sistemul imaginilor din .aux.xml
        if let Some(w) = c.to_wkt() {
            let mut aux = path.as_os_str().to_owned();
            aux.push(".aux.xml");
            let esc = w
                .replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;")
                .replace('"', "&quot;");
            fs::write(
                PathBuf::from(&aux),
                format!(
                    "<PAMDataset>\n  <SRS dataAxisToSRSAxisMapping=\"1,2\">{esc}</SRS>\n</PAMDataset>\n"
                ),
            )?;
            out.push(PathBuf::from(aux));
        }
    }
    Ok(out)
}
