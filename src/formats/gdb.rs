//! Esri File Geodatabase (.gdb) – citire.
//!
//! Implementare proprie a formatului deschis „OpenFileGDB” (descris public prin
//! reverse-engineering de proiectul GDAL): fișierele aNNNNNNNN.gdbtable (date) și
//! aNNNNNNNN.gdbtablx (indexul rândurilor). Suportă versiunile 9.x, 10.x și ArcGIS Pro
//! (inclusiv 3.2+ cu ObjectID pe 64 de biți), geometrii punct, multipunct, linie și poligon
//! cu Z/M, curbe (arce de cerc și Bézier, aproximate prin segmente) și toate tipurile
//! obișnuite de câmpuri. Geodatabase-urile comprimate (CDF) și multipatch nu sunt suportate.

use crate::crs::Crs;
use crate::model::*;
use anyhow::{Context, Result, bail};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use crate::i18n::tr;

// tipuri de câmpuri
const FT_INT16: u8 = 0;
const FT_INT32: u8 = 1;
const FT_FLOAT32: u8 = 2;
const FT_FLOAT64: u8 = 3;
const FT_STRING: u8 = 4;
const FT_DATETIME: u8 = 5;
const FT_OBJECTID: u8 = 6;
const FT_GEOMETRY: u8 = 7;
const FT_BINARY: u8 = 8;
const FT_RASTER: u8 = 9;
const FT_GUID: u8 = 10;
const FT_GLOBALID: u8 = 11;
const FT_XML: u8 = 12;
const FT_INT64: u8 = 13;
const FT_DATE: u8 = 14;
const FT_TIME: u8 = 15;
const FT_DATETIME_OFFSET: u8 = 16;

#[derive(Clone, Debug, Default)]
struct GeomDef {
    wkt: String,
    x_origin: f64,
    y_origin: f64,
    xy_scale: f64,
    z_origin: f64,
    z_scale: f64,
    m_origin: f64,
    m_scale: f64,
}

#[derive(Clone, Debug)]
struct FieldDef {
    name: String,
    ftype: u8,
    nullable: bool,
    width: i32,
    geom: Option<GeomDef>,
    /// raster gestionat (valoare int32) – altfel blob cu lungime
    raster_managed: bool,
}

struct Table {
    file: File,
    fields: Vec<FieldDef>,
    strings_utf8: bool,
    nullable_bytes: usize,
    /// (ObjectID, offset în .gdbtable)
    rows: Vec<(i64, u64)>,
    geom_has_z: bool,
}

fn rd_u16(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
fn rd_u32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
fn rd_i32(b: &[u8], o: usize) -> i32 {
    i32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
fn rd_u64(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}
fn rd_f64(b: &[u8], o: usize) -> f64 {
    f64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}

fn utf16(b: &[u8]) -> String {
    let v: Vec<u16> = b.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    String::from_utf16_lossy(&v)
}

/// Cursor peste un buffer, cu verificarea limitelor.
struct Cur<'a> {
    b: &'a [u8],
    p: usize,
}

impl<'a> Cur<'a> {
    fn new(b: &'a [u8]) -> Self {
        Cur { b, p: 0 }
    }
    fn need(&self, n: usize) -> Result<()> {
        if self.p + n > self.b.len() {
            bail!(tr!("date trunchiate"));
        }
        Ok(())
    }
    fn u8(&mut self) -> Result<u8> {
        self.need(1)?;
        self.p += 1;
        Ok(self.b[self.p - 1])
    }
    fn bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        self.need(n)?;
        self.p += n;
        Ok(&self.b[self.p - n..self.p])
    }
    fn f64(&mut self) -> Result<f64> {
        Ok(rd_f64(self.bytes(8)?, 0))
    }
    fn i16(&mut self) -> Result<i16> {
        let b = self.bytes(2)?;
        Ok(i16::from_le_bytes([b[0], b[1]]))
    }
    fn i32(&mut self) -> Result<i32> {
        Ok(rd_i32(self.bytes(4)?, 0))
    }
    fn varuint(&mut self) -> Result<u64> {
        let mut v: u64 = 0;
        let mut shift = 0;
        loop {
            let b = self.u8()?;
            if shift < 64 {
                v |= ((b & 0x7f) as u64) << shift;
            }
            if b & 0x80 == 0 {
                return Ok(v);
            }
            shift += 7;
            if shift > 70 {
                bail!(tr!("varuint invalid"));
            }
        }
    }
    /// varint cu semn în primul octet (bitul 6), 6 biți de valoare apoi grupe de 7.
    fn varint(&mut self) -> Result<i64> {
        let b = self.u8()?;
        let neg = b & 0x40 != 0;
        let mut v: u64 = (b & 0x3f) as u64;
        if b & 0x80 != 0 {
            let mut shift = 6;
            loop {
                let b = self.u8()?;
                if shift < 64 {
                    v |= ((b & 0x7f) as u64) << shift;
                }
                if b & 0x80 == 0 {
                    break;
                }
                shift += 7;
                if shift > 70 {
                    bail!(tr!("varint invalid"));
                }
            }
        }
        Ok(if neg { -(v as i64) } else { v as i64 })
    }
}

fn table_path(dir: &Path, id: i64, ext: &str) -> PathBuf {
    dir.join(format!("a{:08x}.{ext}", id))
}

impl Table {
    fn open(path: &Path) -> Result<Table> {
        let mut file =
            File::open(path).with_context(|| tr!("Nu pot deschide {}", path.display()))?;
        let mut h = [0u8; 40];
        file.read_exact(&mut h).context(tr!("antet .gdbtable invalid"))?;
        let version = rd_i32(&h, 0);
        if version != 3 && version != 4 {
            bail!("{}", tr!("versiune .gdbtable nesuportată ({version})"));
        }
        let valid_rows = if version == 3 {
            rd_i32(&h, 4) as i64
        } else {
            rd_u64(&h, 16) as i64
        };
        let field_off = rd_u64(&h, 32);

        // ---- descrierea câmpurilor
        file.seek(SeekFrom::Start(field_off))?;
        let mut fh = [0u8; 14];
        file.read_exact(&mut fh)?;
        let desc_len = rd_u32(&fh, 0) as usize;
        if !(10..=10 * 1024 * 1024).contains(&desc_len) {
            bail!(tr!("secțiune de câmpuri invalidă"));
        }
        let strings_utf8 = fh[9] & 1 != 0;
        let geom_has_z = fh[11] & 0x80 != 0;
        let geom_has_m = fh[11] & 0x40 != 0;
        let nfields = rd_u16(&fh, 12) as usize;
        let mut buf = vec![0u8; desc_len - 10];
        file.read_exact(&mut buf)?;
        let mut c = Cur::new(&buf);
        let mut fields = Vec::with_capacity(nfields);
        for _ in 0..nfields {
            let n = c.u8()? as usize;
            let name = utf16(c.bytes(2 * n)?);
            let n = c.u8()? as usize;
            let _alias = utf16(c.bytes(2 * n)?);
            let ftype = c.u8()?;
            if ftype > FT_DATETIME_OFFSET {
                bail!("{}", tr!("tip de câmp necunoscut ({ftype}) la „{name}”"));
            }
            let mut fd = FieldDef {
                name,
                ftype,
                nullable: false,
                width: 0,
                geom: None,
                raster_managed: false,
            };
            if ftype != FT_GEOMETRY && ftype != FT_RASTER {
                let (flags, default_len) = match ftype {
                    FT_STRING => {
                        fd.width = c.i32()?;
                        let flags = c.u8()?;
                        (flags, c.varuint()? as usize)
                    }
                    FT_OBJECTID | FT_BINARY | FT_GUID | FT_GLOBALID | FT_XML => {
                        let b = c.bytes(2)?;
                        (b[1], 0)
                    }
                    _ => {
                        let b = c.bytes(3)?;
                        (b[1], b[2] as usize)
                    }
                };
                c.bytes(default_len)?; // valoare implicită (ignorată)
                fd.nullable = flags & 1 != 0;
            } else {
                let b = c.bytes(2)?;
                fd.nullable = b[1] & 1 != 0;
                if ftype == FT_RASTER {
                    let n = c.u8()? as usize;
                    c.bytes(2 * n)?;
                }
                let wkt_len = {
                    let b = c.bytes(2)?;
                    rd_u16(b, 0) as usize
                };
                let wkt = utf16(c.bytes(wkt_len)?);
                let gflags = c.u8()?;
                let has_m = gflags & 2 != 0;
                let has_z = gflags & 4 != 0;
                let mut g = GeomDef {
                    wkt,
                    z_scale: 1.0,
                    m_scale: 1.0,
                    ..Default::default()
                };
                if ftype == FT_GEOMETRY || gflags > 0 {
                    g.x_origin = c.f64()?;
                    g.y_origin = c.f64()?;
                    g.xy_scale = c.f64()?;
                    if has_m {
                        g.m_origin = c.f64()?;
                        g.m_scale = c.f64()?;
                    }
                    if has_z {
                        g.z_origin = c.f64()?;
                        g.z_scale = c.f64()?;
                    }
                    c.f64()?; // toleranță XY
                    if has_m {
                        c.f64()?;
                    }
                    if has_z {
                        c.f64()?;
                    }
                }
                if ftype == FT_RASTER {
                    fd.raster_managed = c.u8()? == 1;
                } else {
                    for _ in 0..4 {
                        c.f64()?; // extinderea stratului
                    }
                    if geom_has_z {
                        c.f64()?;
                        c.f64()?;
                    }
                    if geom_has_m {
                        c.f64()?;
                        c.f64()?;
                    }
                    c.u8()?;
                    let ngrid = c.i32()?;
                    if !(1..=3).contains(&ngrid) {
                        bail!(tr!("index spațial invalid în descrierea geometriei"));
                    }
                    for _ in 0..ngrid {
                        c.f64()?;
                    }
                    if g.xy_scale == 0.0 {
                        bail!(tr!("scară XY nulă"));
                    }
                    if g.z_scale == 0.0 {
                        g.z_scale = 1.0;
                    }
                    if g.m_scale == 0.0 {
                        g.m_scale = 1.0;
                    }
                    fd.geom = Some(g);
                }
            }
            fields.push(fd);
        }
        let n_nullable = fields.iter().filter(|f| f.nullable).count();

        // ---- indexul rândurilor (.gdbtablx)
        let mut rows = vec![];
        if valid_rows > 0 {
            let xpath = path.with_extension("gdbtablx");
            let x = std::fs::read(&xpath)
                .with_context(|| tr!("Lipsește {}", xpath.display()))?;
            rows = read_tablx(&x, version)?;
        }
        Ok(Table {
            file,
            fields,
            strings_utf8,
            nullable_bytes: n_nullable.div_ceil(8),
            rows,
            geom_has_z,
        })
    }

    /// Citește rândurile, în ordinea ObjectID. `f` primește (oid, blob).
    fn for_each_row(&mut self, mut f: impl FnMut(i64, &[u8]) -> Result<()>) -> Result<usize> {
        let mut order: Vec<usize> = (0..self.rows.len()).collect();
        order.sort_by_key(|&i| self.rows[i].1);
        let mut blobs: Vec<(i64, Vec<u8>)> = Vec::with_capacity(self.rows.len());
        let mut skipped = 0;
        for i in order {
            let (oid, off) = self.rows[i];
            self.file.seek(SeekFrom::Start(off))?;
            let mut lb = [0u8; 4];
            if self.file.read_exact(&mut lb).is_err() {
                skipped += 1;
                continue;
            }
            let len = u32::from_le_bytes(lb);
            if len >> 31 != 0 || (len as usize) < self.nullable_bytes || len > 512 * 1024 * 1024 {
                skipped += 1;
                continue;
            }
            let mut b = vec![0u8; len as usize];
            if self.file.read_exact(&mut b).is_err() {
                skipped += 1;
                continue;
            }
            blobs.push((oid, b));
        }
        blobs.sort_by_key(|(o, _)| *o);
        for (oid, b) in &blobs {
            f(*oid, b)?;
        }
        Ok(skipped)
    }
}

/// Lista (ObjectID, offset) din .gdbtablx (variantele V3 și V4, cu hartă de blocuri rară).
fn read_tablx(x: &[u8], version: i32) -> Result<Vec<(i64, u64)>> {
    if x.len() < 16 {
        bail!(tr!(".gdbtablx trunchiat"));
    }
    if rd_i32(x, 0) != version {
        bail!(tr!(".gdbtablx are altă versiune decât .gdbtable"));
    }
    let (blocks, mut total, osize) = if version == 3 {
        (rd_u32(x, 4) as u64, rd_i32(x, 8) as i64, rd_u32(x, 12) as usize)
    } else {
        (rd_u64(x, 4), 0i64, rd_u32(x, 12) as usize)
    };
    if !(4..=6).contains(&osize) {
        bail!(tr!("dimensiune de offset invalidă în .gdbtablx"));
    }
    let trailer = 16 + osize as u64 * 1024 * blocks;
    let mut bitmap: Option<Vec<u8>> = None;
    if blocks > 0 {
        let t = trailer as usize;
        if version == 3 {
            if x.len() < t + 16 {
                bail!(tr!(".gdbtablx trunchiat"));
            }
            let words = rd_u32(x, t);
            let bits = rd_u32(x, t + 4) as usize;
            if words != 0 {
                let n = bits.div_ceil(8);
                if x.len() < t + 16 + n {
                    bail!(tr!(".gdbtablx trunchiat"));
                }
                bitmap = Some(x[t + 16..t + 16 + n].to_vec());
            }
        } else {
            if x.len() < t + 12 {
                bail!(tr!(".gdbtablx trunchiat"));
            }
            total = rd_u64(x, t) as i64;
            let sz = rd_u32(x, t + 8) as usize;
            if sz == 22 + 32768 + 52 && x.len() >= t + 12 + sz {
                bitmap = Some(x[t + 12 + 22..t + 12 + 22 + 32768].to_vec());
            } else if sz != 0 {
                // hartă necunoscută: ObjectID-urile pot fi aproximative
                total = 1024 * blocks as i64;
            }
        }
    }
    let read_off = |pos: usize| -> u64 {
        let mut v = [0u8; 8];
        v[..osize].copy_from_slice(&x[pos..pos + osize]);
        u64::from_le_bytes(v)
    };
    let mut out = Vec::new();
    let mut blocks_before: u64 = 0;
    let mut last_block: i64 = -1;
    for row in 0..total.max(0) {
        let block = (row / 1024) as usize;
        let idx = match &bitmap {
            Some(bm) => {
                if bm.get(block / 8).is_none_or(|b| b & (1 << (block % 8)) == 0) {
                    continue;
                }
                if block as i64 != last_block {
                    blocks_before = (0..block)
                        .filter(|&i| bm.get(i / 8).is_some_and(|b| b & (1 << (i % 8)) != 0))
                        .count() as u64;
                    last_block = block as i64;
                }
                blocks_before * 1024 + (row as u64 % 1024)
            }
            None => row as u64,
        };
        let pos = 16 + osize * idx as usize;
        if pos + osize > trailer as usize || pos + osize > x.len() {
            break;
        }
        let off = read_off(pos);
        if off != 0 {
            out.push((row + 1, off));
        }
    }
    Ok(out)
}

/// Date ESRI (zile de la 1899-12-30) -> (an, lună, zi, h, min, s) – secunde întregi
fn esri_date(days: f64) -> Option<(i64, u32, u32, u32, u32, u32)> {
    esri_date_ms(days).map(|(y, m, d, h, mi, s, _)| (y, m, d, h, mi, s))
}

/// Ca `esri_date`, cu milisecunde.
fn esri_date_ms(days: f64) -> Option<(i64, u32, u32, u32, u32, u32, u32)> {
    if !days.is_finite() {
        return None;
    }
    let ms_total = ((days - 25569.0) * 86400000.0).round() as i64;
    let ms = ms_total.rem_euclid(1000) as u32;
    let secs = ms_total.div_euclid(1000);
    let d = secs.div_euclid(86400);
    let s = secs.rem_euclid(86400);
    // algoritmul „civil from days” (H. Hinnant)
    let z = d + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = if m <= 2 { y + 1 } else { y };
    Some((y, m, day, (s / 3600) as u32, ((s % 3600) / 60) as u32, (s % 60) as u32, ms))
}

fn guid(b: &[u8]) -> String {
    format!(
        "{{{:02X}{:02X}{:02X}{:02X}-{:02X}{:02X}-{:02X}{:02X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}}}",
        b[3], b[2], b[1], b[0], b[5], b[4], b[7], b[6], b[8], b[9], b[10], b[11], b[12], b[13],
        b[14], b[15]
    )
}

/// Statistici despre geometriile citite.
#[derive(Default)]
struct GeomStats {
    curves: usize,
    multipatch: usize,
    errors: usize,
}

/// Decodează o geometrie (shape buffer comprimat).
fn parse_geometry(b: &[u8], g: &GeomDef, st: &mut GeomStats) -> Result<Option<Geometry>> {
    let mut c = Cur::new(b);
    let gtype = c.varuint()? as u32;
    let base = gtype & 0xff;
    let mut has_z = gtype & 0x8000_0000 != 0;
    let mut has_m = gtype & 0x4000_0000 != 0;
    let has_curves = gtype & 0x2000_0000 != 0;
    match base {
        9 | 11 | 18 | 20 | 10 | 13 | 19 | 15 => has_z = true,
        _ => {}
    }
    match gtype {
        21 | 11 | 28 | 18 | 23 | 13 | 25 | 15 => has_m = true,
        _ => {}
    }
    match base {
        0 => Ok(None),
        1 | 9 | 11 | 21 | 52 => {
            let x = c.varuint()?;
            let y = c.varuint()?;
            if x == 0 || y == 0 {
                return Ok(None);
            }
            let mut p = Coord::new(
                (x - 1) as f64 / g.xy_scale + g.x_origin,
                (y - 1) as f64 / g.xy_scale + g.y_origin,
            );
            if has_z && let Ok(z) = c.varuint() {
                p.z = if z == 0 {
                    0.0
                } else {
                    (z - 1) as f64 / g.z_scale + g.z_origin
                };
            }
            Ok(Some(Geometry::Point(p)))
        }
        8 | 18 | 20 | 28 | 53 => {
            let n = c.varuint()? as usize;
            if n == 0 {
                return Ok(None);
            }
            for _ in 0..4 {
                c.varuint()?;
            }
            let pts = read_xy(&mut c, n, g)?;
            let mut pts = pts;
            if has_z {
                read_z(&mut c, &mut pts, g)?;
            }
            Ok(Some(Geometry::MultiPoint(pts)))
        }
        3 | 10 | 13 | 23 | 50 | 5 | 15 | 19 | 25 | 51 => {
            let is_poly = matches!(base, 5 | 15 | 19 | 25 | 51);
            let npts = c.varuint()? as usize;
            if npts == 0 {
                return Ok(None);
            }
            if npts > b.len() {
                bail!(tr!("număr de puncte invalid"));
            }
            let nparts = c.varuint()? as usize;
            let ncurves = if has_curves { c.varuint()? as usize } else { 0 };
            if nparts == 0 || nparts > npts {
                return Ok(None);
            }
            for _ in 0..4 {
                c.varuint()?; // încadrare
            }
            let mut counts = Vec::with_capacity(nparts);
            let mut sum = 0usize;
            for _ in 0..nparts - 1 {
                let k = c.varuint()? as usize;
                sum += k;
                counts.push(k);
            }
            if sum > npts {
                bail!(tr!("părți invalide"));
            }
            counts.push(npts - sum);
            let mut pts = read_xy(&mut c, npts, g)?;
            if has_z {
                read_z(&mut c, &mut pts, g)?;
            }
            if has_m && c.p < c.b.len() {
                if c.b[c.p] == 0x42 {
                    c.p += 1;
                } else if c.p + npts <= c.b.len() {
                    for _ in 0..npts {
                        c.varint()?;
                    }
                }
            }
            // curbe: segmente înlocuite prin arce / Bézier densificate
            let mut extra: Vec<(usize, Vec<Coord>)> = vec![];
            for _ in 0..ncurves {
                let start = c.varuint()? as usize;
                let ctype = c.varuint()?;
                match ctype {
                    1 => {
                        let v1 = c.f64()?;
                        let v2 = c.f64()?;
                        let bits = c.i32()?;
                        if start + 1 < pts.len() {
                            let a = pts[start];
                            let e = pts[start + 1];
                            let mid = if bits & 0x80 != 0 && bits & 0x20 == 0 {
                                let m = Coord::new(v1, v2);
                                if a.x == e.x && a.y == e.y {
                                    full_circle(a, m)
                                } else {
                                    arc_three_points(a, m, e)
                                }
                            } else if bits & (0x1 | 0x20 | 0x40) == 0 {
                                arc_center(a, Coord::new(v1, v2), e, bits & 0x8 != 0)
                            } else {
                                vec![]
                            };
                            if !mid.is_empty() {
                                extra.push((start, mid));
                            }
                        }
                    }
                    4 => {
                        let c1 = Coord::new(c.f64()?, c.f64()?);
                        let c2 = Coord::new(c.f64()?, c.f64()?);
                        if start + 1 < pts.len() {
                            extra.push((start, bezier(pts[start], c1, c2, pts[start + 1])));
                        }
                    }
                    5 => {
                        let cx = c.f64()?;
                        let cy = c.f64()?;
                        let rot = c.f64()?;
                        let major = c.f64()?;
                        let ratio = c.f64()?;
                        let bits = c.i32()?;
                        // doar forma „după centru” (fără CenterTo / CenterFrom)
                        if bits & (0x200 | 0x400) == 0 && start + 1 < pts.len() {
                            let mid = ellipse_arc(
                                pts[start],
                                pts[start + 1],
                                Coord::new(cx, cy),
                                rot,
                                major,
                                major * ratio,
                                bits,
                            );
                            if !mid.is_empty() {
                                extra.push((start, mid));
                            }
                        }
                    }
                    _ => break,
                }
            }
            if !extra.is_empty() {
                st.curves += 1;
            }
            // reconstruiește părțile, inserând punctele intermediare ale curbelor
            extra.sort_by_key(|e| e.0);
            let mut parts: Vec<Vec<Coord>> = Vec::with_capacity(nparts);
            let mut idx = 0usize;
            let mut ei = 0usize;
            for &k in &counts {
                let mut part = Vec::with_capacity(k);
                for j in 0..k {
                    let gi = idx + j;
                    part.push(pts[gi]);
                    while ei < extra.len() && extra[ei].0 < gi {
                        ei += 1;
                    }
                    if j + 1 < k && ei < extra.len() && extra[ei].0 == gi {
                        part.extend_from_slice(&extra[ei].1);
                        ei += 1;
                    }
                }
                idx += k;
                parts.push(part);
            }
            if is_poly {
                Ok(crate::formats::shp::assemble_rings(parts))
            } else {
                let parts: Vec<Vec<Coord>> = parts.into_iter().filter(|p| p.len() >= 2).collect();
                Ok(match parts.len() {
                    0 => None,
                    1 => Some(Geometry::LineString(parts.into_iter().next().unwrap())),
                    _ => Some(Geometry::MultiLineString(parts)),
                })
            }
        }
        31 | 32 | 54 => {
            st.multipatch += 1;
            Ok(None)
        }
        _ => {
            st.errors += 1;
            Ok(None)
        }
    }
}

fn read_xy(c: &mut Cur, n: usize, g: &GeomDef) -> Result<Vec<Coord>> {
    let (mut dx, mut dy) = (0i64, 0i64);
    let mut v = Vec::with_capacity(n);
    for _ in 0..n {
        dx = dx.wrapping_add(c.varint()?);
        dy = dy.wrapping_add(c.varint()?);
        v.push(Coord::new(
            dx as f64 / g.xy_scale + g.x_origin,
            dy as f64 / g.xy_scale + g.y_origin,
        ));
    }
    Ok(v)
}

fn read_z(c: &mut Cur, pts: &mut [Coord], g: &GeomDef) -> Result<()> {
    let mut dz = 0i64;
    for p in pts.iter_mut() {
        dz = dz.wrapping_add(c.varint()?);
        p.z = dz as f64 / g.z_scale + g.z_origin;
    }
    Ok(())
}

/// Puncte intermediare pe arcul de cerc a -> m -> e (fără capete).
fn arc_three_points(a: Coord, m: Coord, e: Coord) -> Vec<Coord> {
    let d = 2.0 * (a.x * (m.y - e.y) + m.x * (e.y - a.y) + e.x * (a.y - m.y));
    if d.abs() < 1e-12 {
        return vec![];
    }
    let a2 = a.x * a.x + a.y * a.y;
    let m2 = m.x * m.x + m.y * m.y;
    let e2 = e.x * e.x + e.y * e.y;
    let cx = (a2 * (m.y - e.y) + m2 * (e.y - a.y) + e2 * (a.y - m.y)) / d;
    let cy = (a2 * (e.x - m.x) + m2 * (a.x - e.x) + e2 * (m.x - a.x)) / d;
    let ang = |p: Coord| (p.y - cy).atan2(p.x - cx);
    let (t0, tm, t1) = (ang(a), ang(m), ang(e));
    // sensul: cel care trece prin m
    let tau = std::f64::consts::TAU;
    let norm = |x: f64| x.rem_euclid(tau);
    let ccw = norm(tm - t0) < norm(t1 - t0);
    let sweep = if ccw { norm(t1 - t0) } else { -norm(t0 - t1) };
    densify_arc(cx, cy, ((a.x - cx).powi(2) + (a.y - cy).powi(2)).sqrt(), t0, sweep, a.z, e.z)
}

fn arc_center(a: Coord, c: Coord, e: Coord, ccw: bool) -> Vec<Coord> {
    // ca în GDAL: unghiurile capetelor față de centru, raza capătului final, apoi arcul
    // prin capete și punctul de la mijlocul unghiului (definiția poate să nu fie un cerc exact)
    let t0 = (a.y - c.y).atan2(a.x - c.x);
    let mut t1 = (e.y - c.y).atan2(e.x - c.x);
    if ccw {
        if t0 >= t1 {
            t1 += std::f64::consts::TAU;
        }
    } else if t0 <= t1 {
        t1 -= std::f64::consts::TAU;
    }
    let r = ((e.x - c.x).powi(2) + (e.y - c.y).powi(2)).sqrt();
    let tm = (t0 + t1) / 2.0;
    let m = Coord::xyz(c.x + r * tm.cos(), c.y + r * tm.sin(), a.z);
    if a.x == e.x && a.y == e.y {
        return full_circle(a, Coord::new(2.0 * c.x - a.x, 2.0 * c.y - a.y));
    }
    arc_three_points(a, m, e)
}

fn densify_arc(cx: f64, cy: f64, r: f64, t0: f64, sweep: f64, z0: f64, z1: f64) -> Vec<Coord> {
    if !(r.is_finite() && sweep.is_finite()) || r <= 0.0 {
        return vec![];
    }
    // pas: max. 2° sau o eroare de coardă de ~1 mm
    let by_err = if r > 0.001 {
        2.0 * (1.0 - 0.001 / r).clamp(-1.0, 1.0).acos()
    } else {
        f64::INFINITY
    };
    let step = (2f64.to_radians()).min(by_err.max(0.2f64.to_radians()));
    let n = ((sweep.abs() / step).ceil() as usize).clamp(2, 720);
    (1..n)
        .map(|i| {
            let f = i as f64 / n as f64;
            let t = t0 + sweep * f;
            Coord::xyz(cx + r * t.cos(), cy + r * t.sin(), z0 + (z1 - z0) * f)
        })
        .collect()
}

/// Cerc complet care începe și se termină în `a`, cu punctul diametral opus `m`
/// (două semicercuri, ca în GDAL).
fn full_circle(a: Coord, m: Coord) -> Vec<Coord> {
    if a.x == m.x && a.y == m.y {
        return vec![];
    }
    let (cx, cy) = ((a.x + m.x) / 2.0, (a.y + m.y) / 2.0);
    let i1 = Coord::xyz(cx - (m.y - cy), cy + (a.x - cx), a.z);
    let i2 = Coord::xyz(cx + (m.y - cy), cy - (a.x - cx), a.z);
    let mut v = arc_three_points(a, i1, m);
    v.push(Coord::xyz(m.x, m.y, a.z));
    v.extend(arc_three_points(m, i2, a));
    v
}

/// Arc de elipsă dat prin centru (rotația în radiani, sens trigonometric).
fn ellipse_arc(a: Coord, e: Coord, c: Coord, rot: f64, major: f64, minor: f64, bits: i32) -> Vec<Coord> {
    use std::f64::consts::{PI, TAU};
    if !(major > 0.0 && minor > 0.0 && major.is_finite() && minor.is_finite()) {
        return vec![];
    }
    let (cr, sr) = (rot.cos(), rot.sin());
    let param = |p: Coord| {
        let (dx, dy) = (p.x - c.x, p.y - c.y);
        let u = (dx * cr + dy * sr) / major;
        let v = (-dx * sr + dy * cr) / minor;
        v.atan2(u)
    };
    let (p0, p1) = (param(a), param(e));
    let ccw = bits & 0x800 != 0;
    let sweep = if bits & 0x2000 != 0 {
        if ccw { TAU } else { -TAU }
    } else {
        let mut d = (p1 - p0).rem_euclid(TAU); // sens trigonometric, în [0, 2π)
        let minor_arc = bits & 0x1000 != 0;
        if minor_arc {
            if d > PI {
                d -= TAU;
            }
        } else if d < PI {
            d -= TAU;
        }
        d
    };
    let n = ((sweep.abs() / 2f64.to_radians()).ceil() as usize).clamp(2, 720);
    (1..n)
        .map(|i| {
            let f = i as f64 / n as f64;
            let t = p0 + sweep * f;
            let (ex, ey) = (major * t.cos(), minor * t.sin());
            Coord::xyz(c.x + ex * cr - ey * sr, c.y + ex * sr + ey * cr, a.z + (e.z - a.z) * f)
        })
        .collect()
}

fn bezier(p0: Coord, p1: Coord, p2: Coord, p3: Coord) -> Vec<Coord> {
    // numărul de pași ca în GDAL: după unghiul dintre tangente și lungimea relativă a lor
    let step = 4f64.to_radians();
    let mut a0 = (p1.y - p0.y).atan2(p1.x - p0.x);
    let mut a1 = (p3.y - p2.y).atan2(p3.x - p2.x);
    if a0 + std::f64::consts::PI < a1 {
        a0 += std::f64::consts::TAU;
    } else if a1 + std::f64::consts::PI < a0 {
        a1 += std::f64::consts::TAU;
    }
    let lt0 = (p1.x - p0.x).powi(2) + (p1.y - p0.y).powi(2);
    let lt1 = (p3.x - p2.x).powi(2) + (p3.y - p2.y).powi(2);
    let l = (p3.x - p0.x).powi(2) + (p3.y - p0.y).powi(2);
    let n = if l < 1e-9 {
        1
    } else {
        ((((a1 - a0).abs() / step).max(2.0) * (5.0 * (lt0 + lt1) / l).max(1.0)).ceil())
            .min(1000.0) as usize
    };
    (1..n)
        .map(|i| {
            let t = i as f64 / n as f64;
            let u = 1.0 - t;
            let (a, b, c, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
            Coord::xyz(
                a * p0.x + b * p1.x + c * p2.x + d * p3.x,
                a * p0.y + b * p1.y + c * p2.y + d * p3.y,
                p0.z + (p3.z - p0.z) * t,
            )
        })
        .collect()
}

/// Dosarul .gdb pentru o cale (dosarul însuși sau un fișier din el).
pub fn gdb_root(path: &Path) -> Option<PathBuf> {
    let is_root = |p: &Path| p.is_dir() && p.join("a00000001.gdbtable").exists();
    if is_root(path) {
        return Some(path.to_path_buf());
    }
    let parent = path.parent()?;
    if is_root(parent) {
        return Some(parent.to_path_buf());
    }
    None
}

/// Citește toate clasele de entități (feature classes) dintr-un File Geodatabase.
pub fn read(path: &Path) -> Result<(Vec<VectorLayer>, Vec<String>)> {
    let root = gdb_root(path)
        .with_context(|| tr!("{} nu este un File Geodatabase (.gdb)", path.display()))?;
    let mut info = vec![];
    // catalogul de sistem: ID (ObjectID), Name, FileFormat
    let mut cat = Table::open(&root.join("a00000001.gdbtable"))
        .context(tr!("Catalogul geodatabase-ului (a00000001.gdbtable) nu poate fi citit"))?;
    let name_idx = cat.fields.iter().position(|f| f.name.eq_ignore_ascii_case("Name"));
    let fmt_idx = cat
        .fields
        .iter()
        .position(|f| f.name.eq_ignore_ascii_case("FileFormat"));
    let Some(name_idx) = name_idx else {
        bail!(tr!("Catalog fără coloana Name"));
    };
    let cat_fields = cat.fields.clone();
    let cat_utf8 = cat.strings_utf8;
    let cat_nb = cat.nullable_bytes;
    let mut entries: Vec<(i64, String, i64)> = vec![];
    cat.for_each_row(|oid, blob| {
        let vals = decode_row(blob, &cat_fields, cat_nb, cat_utf8)?;
        let name = match &vals[name_idx] {
            Some(RawVal::Text(s)) => s.clone(),
            _ => return Ok(()),
        };
        let fmt = fmt_idx
            .and_then(|i| match &vals[i] {
                Some(RawVal::Int(v)) => Some(*v),
                _ => None,
            })
            .unwrap_or(0);
        entries.push((oid, name, fmt));
        Ok(())
    })?;

    // câmpurile dată-oră „HighPrecision” (din definițiile XML din GDB_Items)
    let high_precision = entries
        .iter()
        .find(|(_, n, f)| n.eq_ignore_ascii_case("GDB_Items") && *f == 0)
        .and_then(|(id, _, _)| read_high_precision(&table_path(&root, *id, "gdbtable")).ok())
        .unwrap_or_default();
    let mut layers = vec![];
    let mut tables_no_geom = vec![];
    for (id, name, fmt) in entries {
        if name.starts_with("GDB_") || name.is_empty() {
            continue;
        }
        let tpath = table_path(&root, id, "gdbtable");
        if !tpath.exists() {
            continue;
        }
        if fmt != 0 {
            info.push(tr!(
                "„{name}”: format comprimat (CDF) – nesuportat, decomprimați-l în ArcGIS"
            ));
            continue;
        }
        let hp = high_precision
            .iter()
            .filter(|(t, _)| t.eq_ignore_ascii_case(&name))
            .map(|(_, f)| f.to_lowercase())
            .collect::<Vec<_>>();
        match read_layer(&tpath, &name, &hp) {
            Ok(Some((mut l, notes))) => {
                l.source = Some(root.clone());
                info.extend(notes);
                layers.push(l);
            }
            Ok(None) => tables_no_geom.push(name),
            Err(e) => info.push(format!("„{name}”: {e:#}")),
        }
    }
    if !tables_no_geom.is_empty() {
        info.push(tr!(
            "Tabele fără geometrie (neîncărcate): {}",
            tables_no_geom.join(", ")
        ));
    }
    if layers.is_empty() {
        bail!(
            "{}", tr!("{} nu conține clase de entități cu geometrie{}",
            root.display(),
            if info.is_empty() {
                String::new()
            } else {
                format!(" ({})", info.join("; "))
            }
        ));
    }
    layers.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    Ok((layers, info))
}

#[derive(Clone, Debug)]
enum RawVal {
    Int(i64),
    Real(f64),
    Text(String),
    Date(f64),
    /// dată și oră locală + decalaj față de UTC (minute)
    DateOff(f64, i16),
    Geom(Vec<u8>),
}

fn decode_row(
    blob: &[u8],
    fields: &[FieldDef],
    nullable_bytes: usize,
    utf8: bool,
) -> Result<Vec<Option<RawVal>>> {
    let mut out = Vec::with_capacity(fields.len());
    let nulls = &blob[..nullable_bytes];
    let mut c = Cur::new(blob);
    c.p = nullable_bytes;
    let mut inull = 0usize;
    for f in fields {
        if f.nullable {
            let is_null = nulls[inull / 8] & (1 << (inull % 8)) != 0;
            inull += 1;
            if is_null {
                out.push(None);
                continue;
            }
        }
        let v = match f.ftype {
            FT_OBJECTID => None,
            FT_INT16 => Some(RawVal::Int(c.i16()? as i64)),
            FT_INT32 => Some(RawVal::Int(c.i32()? as i64)),
            FT_INT64 => Some(RawVal::Int(rd_u64(c.bytes(8)?, 0) as i64)),
            FT_FLOAT32 => {
                let b = c.bytes(4)?;
                Some(RawVal::Real(f32::from_le_bytes(b.try_into().unwrap()) as f64))
            }
            FT_FLOAT64 => Some(RawVal::Real(c.f64()?)),
            FT_DATETIME | FT_DATE => Some(RawVal::Date(c.f64()?)),
            FT_TIME => {
                let d = c.f64()?;
                let s = (d * 86400.0).round() as i64;
                Some(RawVal::Text(format!(
                    "{:02}:{:02}:{:02}",
                    s / 3600,
                    (s % 3600) / 60,
                    s % 60
                )))
            }
            FT_DATETIME_OFFSET => {
                let d = c.f64()?;
                let off = c.i16()?;
                Some(RawVal::DateOff(d, off))
            }
            FT_STRING | FT_XML => {
                let n = c.varuint()? as usize;
                let b = c.bytes(n)?;
                Some(RawVal::Text(if utf8 || f.ftype == FT_XML {
                    String::from_utf8_lossy(b).into_owned()
                } else {
                    utf16(b)
                }))
            }
            FT_GUID | FT_GLOBALID => Some(RawVal::Text(guid(c.bytes(16)?))),
            FT_GEOMETRY => {
                let n = c.varuint()? as usize;
                Some(RawVal::Geom(c.bytes(n)?.to_vec()))
            }
            FT_BINARY => {
                let n = c.varuint()? as usize;
                c.bytes(n)?;
                None
            }
            FT_RASTER => {
                if f.raster_managed {
                    c.bytes(4)?;
                } else {
                    let n = c.varuint()? as usize;
                    c.bytes(n)?;
                }
                None
            }
            _ => bail!(tr!("tip de câmp necunoscut")),
        };
        out.push(v);
    }
    Ok(out)
}

/// Citește o clasă de entități. None = tabel fără geometrie.
/// (tabel, câmp) pentru câmpurile dată-oră cu precizie de milisecunde.
fn read_high_precision(path: &Path) -> Result<Vec<(String, String)>> {
    let mut t = Table::open(path)?;
    let fields = t.fields.clone();
    let idx = |n: &str| fields.iter().position(|f| f.name.eq_ignore_ascii_case(n));
    let (Some(ni), Some(di)) = (idx("Name"), idx("Definition")) else {
        return Ok(vec![]);
    };
    let pi = idx("PhysicalName");
    let (utf8, nb) = (t.strings_utf8, t.nullable_bytes);
    let mut out = vec![];
    t.for_each_row(|_, blob| {
        let Ok(v) = decode_row(blob, &fields, nb, utf8) else {
            return Ok(());
        };
        let Some(RawVal::Text(def)) = &v[di] else {
            return Ok(());
        };
        if !def.contains("HighPrecision") {
            return Ok(());
        }
        let mut names = vec![];
        if let Some(RawVal::Text(n)) = &v[ni] {
            // numele poate fi calificat (set de date.clasă)
            names.push(n.rsplit(['.', '\\']).next().unwrap_or(n).to_string());
        }
        if let Some(Some(RawVal::Text(n))) = pi.map(|i| &v[i]) {
            names.push(n.rsplit(['.', '\\']).next().unwrap_or(n).to_string());
        }
        // blocurile <GPFieldInfoEx>…</GPFieldInfoEx> cu <HighPrecision>true</HighPrecision>
        for block in def.split("<GPFieldInfoEx").skip(1) {
            let block = block.split("</GPFieldInfoEx>").next().unwrap_or(block);
            if !block.contains("<HighPrecision>true</HighPrecision>") {
                continue;
            }
            if let Some(n) = block
                .split("<Name>")
                .nth(1)
                .and_then(|x| x.split("</Name>").next())
            {
                for t in &names {
                    out.push((t.clone(), n.to_string()));
                }
            }
        }
        Ok(())
    })?;
    Ok(out)
}

fn read_layer(
    path: &Path,
    name: &str,
    high_precision: &[String],
) -> Result<Option<(VectorLayer, Vec<String>)>> {
    let mut t = Table::open(path)?;
    let Some(gi) = t.fields.iter().position(|f| f.ftype == FT_GEOMETRY) else {
        return Ok(None);
    };
    let gdef = t.fields[gi].geom.clone().unwrap_or_default();
    let mut layer = VectorLayer::new(name);
    layer.has_z = t.geom_has_z;
    if !gdef.wkt.trim().is_empty() {
        layer.crs = Crs::from_wkt(&gdef.wkt).ok();
    }
    // câmpurile de atribute (ObjectID primul; fără geometrie, binare, raster)
    let mut map: Vec<(usize, usize)> = vec![]; // (index câmp gdb, index câmp strat)
    let mut oid_field: Option<usize> = None;
    for (i, f) in t.fields.iter().enumerate() {
        let ft = match f.ftype {
            FT_OBJECTID => {
                oid_field = Some(layer.fields.len());
                FieldType::Integer
            }
            FT_INT16 | FT_INT32 | FT_INT64 => FieldType::Integer,
            FT_FLOAT32 | FT_FLOAT64 => FieldType::Real,
            FT_STRING | FT_GUID | FT_GLOBALID | FT_XML | FT_TIME => FieldType::Text,
            FT_DATETIME | FT_DATE => FieldType::Date,
            FT_DATETIME_OFFSET => FieldType::Text,
            _ => continue,
        };
        let mut fld = Field::new(f.name.clone(), ft);
        if f.ftype == FT_STRING && f.width > 0 {
            fld.width = f.width.min(254) as usize;
        }
        if matches!(f.ftype, FT_GUID | FT_GLOBALID) {
            fld.width = 38;
        }
        map.push((i, layer.fields.len()));
        layer.fields.push(fld);
    }
    let fields = t.fields.clone();
    let utf8 = t.strings_utf8;
    let nb = t.nullable_bytes;
    let nf = layer.fields.len();
    let mut st = GeomStats::default();
    let mut bad_rows = 0usize;
    let mut dates: Vec<(usize, usize, f64)> = vec![]; // (entitate, câmp, zile)
    // câmpurile dată-oră obișnuite au precizie de o secundă (ca în ArcGIS)
    let round_to_s: Vec<bool> = layer
        .fields
        .iter()
        .map(|f| !high_precision.contains(&f.name.to_lowercase()))
        .collect();
    let mut feats: Vec<Feature> = Vec::with_capacity(t.rows.len());
    let skipped = t.for_each_row(|oid, blob| {
        let vals = match decode_row(blob, &fields, nb, utf8) {
            Ok(v) => v,
            Err(_) => {
                bad_rows += 1;
                return Ok(());
            }
        };
        let mut attrs = vec![Value::Null; nf];
        for &(gi2, li) in &map {
            attrs[li] = match &vals[gi2] {
                None => Value::Null,
                Some(RawVal::Int(v)) => Value::Int(*v),
                Some(RawVal::Real(v)) => Value::Real(*v),
                Some(RawVal::Text(s)) => Value::Text(s.clone()),
                Some(RawVal::Date(d)) => {
                    let d = if round_to_s[li] {
                        ((d - 25569.0) * 86400.0).round() / 86400.0 + 25569.0
                    } else {
                        *d
                    };
                    dates.push((feats.len(), li, d));
                    Value::Null
                }
                Some(RawVal::DateOff(d, off)) => match esri_date(*d) {
                    Some((y, m, dd, h, mi, s)) => {
                        let sign = if *off < 0 { '-' } else { '+' };
                        let a = off.unsigned_abs();
                        Value::Text(format!(
                            "{y:04}-{m:02}-{dd:02} {h:02}:{mi:02}:{s:02}{sign}{:02}:{:02}",
                            a / 60,
                            a % 60
                        ))
                    }
                    None => Value::Null,
                },
                Some(RawVal::Geom(_)) => Value::Null,
            };
        }
        if let Some(oi) = oid_field {
            attrs[oi] = Value::Int(oid);
        }
        let geom = match &vals[gi] {
            Some(RawVal::Geom(b)) => match parse_geometry(b, &gdef, &mut st) {
                Ok(g) => g,
                Err(_) => {
                    st.errors += 1;
                    None
                }
            },
            _ => None,
        };
        feats.push(Feature { geom, attrs });
        Ok(())
    })?;
    // datele: doar data dacă toate orele sunt 00:00:00, altfel dată și oră
    let all_midnight = dates.iter().all(|(_, _, d)| {
        esri_date_ms(*d).is_some_and(|(_, _, _, h, mi, s, ms)| h == 0 && mi == 0 && s == 0 && ms == 0)
    });
    for (fi, li, d) in dates {
        if let Some((y, m, dd, h, mi, s, ms)) = esri_date_ms(d) {
            feats[fi].attrs[li] = Value::Text(if all_midnight && ms == 0 {
                format!("{y:04}-{m:02}-{dd:02}")
            } else if ms != 0 {
                format!("{y:04}-{m:02}-{dd:02} {h:02}:{mi:02}:{s:02}.{ms:03}")
            } else {
                format!("{y:04}-{m:02}-{dd:02} {h:02}:{mi:02}:{s:02}")
            });
        }
    }
    if !all_midnight {
        for f in &mut layer.fields {
            if f.ftype == FieldType::Date {
                f.ftype = FieldType::Text;
                f.width = 19;
            }
        }
    }
    layer.features = feats;
    let mut notes = vec![];
    if st.curves > 0 {
        notes.push(tr!(
            "„{name}”: {} entități cu curbe (arce/Bézier) aproximate prin segmente",
            st.curves
        ));
    }
    if st.multipatch > 0 {
        notes.push(tr!(
            "„{name}”: {} entități multipatch (3D) ignorate",
            st.multipatch
        ));
    }
    if st.errors + bad_rows + skipped > 0 {
        notes.push(tr!(
            "„{name}”: {} înregistrări nu au putut fi citite complet",
            st.errors + bad_rows + skipped
        ));
    }
    Ok(Some((layer, notes)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn varints() {
        let mut c = Cur::new(&[0x81, 0x01]);
        assert_eq!(c.varuint().unwrap(), 129);
        // varint cu semn: 0x41 = -1 ; 0x80 0x01 = 64
        let mut c = Cur::new(&[0x41, 0x80, 0x01]);
        assert_eq!(c.varint().unwrap(), -1);
        assert_eq!(c.varint().unwrap(), 64);
    }

    #[test]
    fn dates() {
        assert_eq!(esri_date(25569.0), Some((1970, 1, 1, 0, 0, 0)));
        assert_eq!(esri_date(45658.5), Some((2025, 1, 1, 12, 0, 0)));
    }

    #[test]
    fn arcs() {
        // semicerc prin (0,1): de la (-1,0) la (1,0)
        let p = arc_three_points(Coord::new(-1.0, 0.0), Coord::new(0.0, 1.0), Coord::new(1.0, 0.0));
        assert!(p.iter().all(|c| c.y >= -1e-9 && ((c.x * c.x + c.y * c.y).sqrt() - 1.0).abs() < 1e-9));
        let p = arc_three_points(Coord::new(-1.0, 0.0), Coord::new(0.0, -1.0), Coord::new(1.0, 0.0));
        assert!(p.iter().all(|c| c.y <= 1e-9));
    }
}
