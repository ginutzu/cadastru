//! Operații raster: reproiecție (warp), reeșantionare și randare pentru afișare.

use crate::crs::{Crs, Transformer};
use crate::model::*;
use anyhow::{Result, anyhow, bail};
use crate::i18n::tr;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum Resampling {
    #[default]
    Nearest,
    Bilinear,
}

/// Transformare aproximată pe grilă: pentru fiecare pixel de ieșire dă (col, row) în sursă.
struct PixelMapper<'a> {
    inv: &'a Transformer,
    src_inv_gt: [f64; 6],
    dst_gt: [f64; 6],
}

impl PixelMapper<'_> {
    #[inline]
    fn exact(&self, i: f64, j: f64) -> Option<(f64, f64)> {
        let g = &self.dst_gt;
        let x = g[0] + i * g[1] + j * g[2];
        let y = g[3] + i * g[4] + j * g[5];
        let (sx, sy) = self.inv.xy(x, y)?;
        let s = &self.src_inv_gt;
        Some((s[0] + sx * s[1] + sy * s[2], s[3] + sx * s[4] + sy * s[5]))
    }

    /// Calculează coordonatele sursă pentru un rând întreg, cu interpolare liniară între noduri.
    fn row(&self, j: usize, w: usize, step: usize, out: &mut Vec<(f64, f64)>) {
        out.clear();
        let jc = j as f64 + 0.5;
        if self.inv.is_identity() {
            for i in 0..w {
                out.push(
                    self.exact(i as f64 + 0.5, jc)
                        .unwrap_or((f64::NAN, f64::NAN)),
                );
            }
            return;
        }
        let mut i = 0;
        while i < w {
            let i1 = (i + step).min(w);
            let a = self.exact(i as f64 + 0.5, jc);
            let b = self.exact(i1 as f64 + 0.5, jc);
            match (a, b) {
                (Some(a), Some(b)) => {
                    let n = (i1 - i) as f64;
                    for k in i..i1 {
                        let t = (k - i) as f64 / n;
                        out.push((a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t));
                    }
                }
                _ => {
                    for k in i..i1 {
                        out.push(
                            self.exact(k as f64 + 0.5, jc)
                                .unwrap_or((f64::NAN, f64::NAN)),
                        );
                    }
                }
            }
            i = i1;
        }
    }
}

#[inline]
fn sample(src: &Raster, band: usize, c: f64, r: f64, rs: Resampling) -> Option<f64> {
    let (w, h) = (src.width as isize, src.height as isize);
    let b = &src.bands[band];
    match rs {
        Resampling::Nearest => {
            let (ci, ri) = (c.floor() as isize, r.floor() as isize);
            if ci < 0 || ri < 0 || ci >= w || ri >= h {
                return None;
            }
            let v = b.get((ri * w + ci) as usize);
            if src.is_nodata(v) { None } else { Some(v) }
        }
        Resampling::Bilinear => {
            let (cx, ry) = (c - 0.5, r - 0.5);
            let (c0, r0) = (cx.floor() as isize, ry.floor() as isize);
            let (fx, fy) = (cx - c0 as f64, ry - r0 as f64);
            let mut acc = 0.0;
            let mut wsum = 0.0;
            for (dc, dr, wt) in [
                (0, 0, (1.0 - fx) * (1.0 - fy)),
                (1, 0, fx * (1.0 - fy)),
                (0, 1, (1.0 - fx) * fy),
                (1, 1, fx * fy),
            ] {
                let (cc, rr) = ((c0 + dc).clamp(0, w - 1), (r0 + dr).clamp(0, h - 1));
                let v = b.get((rr * w + cc) as usize);
                if !src.is_nodata(v) && v.is_finite() {
                    acc += v * wt;
                    wsum += wt;
                }
            }
            if c < 0.0 || r < 0.0 || c >= w as f64 || r >= h as f64 || wsum < 1e-9 {
                return None;
            }
            Some(acc / wsum)
        }
    }
}

/// Reeșantionează `src` pe grila `dst_gt` (w x h) dintr-un sistem în altul.
pub fn resample(
    src: &Raster,
    inv: &Transformer,
    dst_gt: [f64; 6],
    w: usize,
    h: usize,
    rs: Resampling,
    nodata_out: f64,
) -> Result<Vec<BandData>> {
    let src_inv_gt = src
        .inv_gt()
        .ok_or_else(|| anyhow!(tr!("Geotransformare invalidă")))?;
    let mapper = PixelMapper {
        inv,
        src_inv_gt,
        dst_gt,
    };
    let nb = src.bands.len();
    let mut out: Vec<BandData> = src
        .bands
        .iter()
        .map(|b| b.new_like(w * h, nodata_out))
        .collect();
    // RGB(A) pe 8 biți: interpolare biliniară doar pe culori
    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(2)
        .min(16);
    let rows_per = h.div_ceil(threads).max(1);
    // fiecare fir produce valorile pentru un bloc de rânduri
    let results: Vec<(usize, Vec<Vec<f64>>)> = std::thread::scope(|s| {
        let mut handles = vec![];
        for t in 0..threads {
            let j0 = t * rows_per;
            if j0 >= h {
                break;
            }
            let j1 = (j0 + rows_per).min(h);
            let mapper = &mapper;
            handles.push(s.spawn(move || {
                let mut vals: Vec<Vec<f64>> = vec![Vec::with_capacity((j1 - j0) * w); nb];
                let mut rowbuf = Vec::with_capacity(w);
                for j in j0..j1 {
                    mapper.row(j, w, 16, &mut rowbuf);
                    for &(c, r) in &rowbuf {
                        for (b, vb) in vals.iter_mut().enumerate() {
                            let v = if c.is_finite() {
                                sample(src, b, c, r, rs).unwrap_or(nodata_out)
                            } else {
                                nodata_out
                            };
                            vb.push(v);
                        }
                    }
                }
                (j0, vals)
            }));
        }
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    for (j0, vals) in results {
        for (b, vb) in vals.into_iter().enumerate() {
            let off = j0 * w;
            for (k, v) in vb.into_iter().enumerate() {
                out[b].set(off + k, v);
            }
        }
    }
    Ok(out)
}

/// Reproiectează un raster în alt sistem de coordonate.
pub fn warp(src: &Raster, dst: &Crs, res: Option<f64>, rs: Resampling) -> Result<Raster> {
    let scrs = src.crs.clone().ok_or_else(|| {
        anyhow!(
            "{}", tr!("Rasterul „{}” nu are sistem de coordonate definit",
            src.name
        ))
    })?;
    let fwd = Transformer::new(&scrs, dst)?;
    let inv = Transformer::new(dst, &scrs)?;
    // extinderea în sistemul destinație (margini densificate în spațiul pixelilor)
    let mut db = Bounds::empty();
    let n = 40;
    for k in 0..=n {
        let t = k as f64 / n as f64;
        for (c, r) in [
            (t * src.width as f64, 0.0),
            (t * src.width as f64, src.height as f64),
            (0.0, t * src.height as f64),
            (src.width as f64, t * src.height as f64),
        ] {
            let (x, y) = src.pixel_to_world(c, r);
            if let Some((tx, ty)) = fwd.xy(x, y) {
                db.extend(tx, ty);
            }
        }
    }
    if !db.is_valid() {
        bail!(tr!("Rasterul nu poate fi transformat în sistemul destinație"));
    }
    // rezoluția: păstrează aproximativ numărul de pixeli pe diagonală
    let px = match res {
        Some(r) if r > 0.0 => r,
        _ => {
            let (cx, cy) = src.pixel_to_world(src.width as f64 / 2.0, src.height as f64 / 2.0);
            let (cx2, cy2) =
                src.pixel_to_world(src.width as f64 / 2.0 + 1.0, src.height as f64 / 2.0 + 1.0);
            match (fwd.xy(cx, cy), fwd.xy(cx2, cy2)) {
                (Some(a), Some(b)) => (((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt()
                    / std::f64::consts::SQRT_2)
                    .max(1e-12),
                _ => db.width() / src.width as f64,
            }
        }
    };
    let w = (db.width() / px).ceil().max(1.0) as usize;
    let h = (db.height() / px).ceil().max(1.0) as usize;
    if w * h > 1_000_000_000 {
        bail!("{}", tr!("Rasterul rezultat ar fi prea mare ({w} x {h}); alegeți o rezoluție mai mare"));
    }
    let gt = [db.minx, px, 0.0, db.maxy, 0.0, -px];
    let nodata = src.nodata.unwrap_or(if src.bands[0].is_float() {
        f64::NAN
    } else {
        0.0
    });
    let bands = resample(src, &inv, gt, w, h, rs, nodata)?;
    Ok(Raster {
        name: src.name.clone(),
        source: None,
        width: w,
        height: h,
        bands,
        gt,
        nodata: Some(nodata),
        crs: Some(dst.clone()),
    })
}

/// Decupează/reeșantionează la o nouă rezoluție în același sistem.
pub fn resize(src: &Raster, res: f64, rs: Resampling) -> Result<Raster> {
    let b = src.bounds();
    let w = (b.width() / res).ceil().max(1.0) as usize;
    let h = (b.height() / res).ceil().max(1.0) as usize;
    let gt = [b.minx, res, 0.0, b.maxy, 0.0, -res];
    let nodata = src.nodata.unwrap_or(if src.bands[0].is_float() {
        f64::NAN
    } else {
        0.0
    });
    let bands = resample(src, &Transformer::identity(), gt, w, h, rs, nodata)?;
    Ok(Raster {
        name: src.name.clone(),
        source: None,
        width: w,
        height: h,
        bands,
        gt,
        nodata: src.nodata.or(Some(nodata)),
        crs: src.crs.clone(),
    })
}

// ---------------------------------------------------------------------------
// Randare pentru afișare
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Ramp {
    Gray,
    Terrain,
    Spectral,
    Viridis,
}

impl Ramp {
    pub const ALL: [Ramp; 4] = [Ramp::Terrain, Ramp::Gray, Ramp::Spectral, Ramp::Viridis];
    pub fn label(&self) -> &'static str {
        match self {
            Ramp::Gray => tr!("Tonuri de gri"),
            Ramp::Terrain => tr!("Relief"),
            Ramp::Spectral => "Spectral",
            Ramp::Viridis => "Viridis",
        }
    }
    fn stops(&self) -> &'static [(f32, [u8; 3])] {
        match self {
            Ramp::Gray => &[(0.0, [0, 0, 0]), (1.0, [255, 255, 255])],
            Ramp::Terrain => &[
                (0.0, [38, 115, 77]),
                (0.2, [84, 166, 76]),
                (0.4, [222, 214, 125]),
                (0.6, [189, 140, 79]),
                (0.8, [140, 99, 84]),
                (1.0, [250, 250, 250]),
            ],
            Ramp::Spectral => &[
                (0.0, [43, 131, 186]),
                (0.25, [171, 221, 164]),
                (0.5, [255, 255, 191]),
                (0.75, [253, 174, 97]),
                (1.0, [215, 25, 28]),
            ],
            Ramp::Viridis => &[
                (0.0, [68, 1, 84]),
                (0.25, [59, 82, 139]),
                (0.5, [33, 145, 140]),
                (0.75, [94, 201, 98]),
                (1.0, [253, 231, 37]),
            ],
        }
    }
    pub fn color(&self, t: f32) -> [u8; 3] {
        let s = self.stops();
        let t = t.clamp(0.0, 1.0);
        for w in s.windows(2) {
            if t <= w[1].0 {
                let f = (t - w[0].0) / (w[1].0 - w[0].0);
                let mix = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * f) as u8;
                return [
                    mix(w[0].1[0], w[1].1[0]),
                    mix(w[0].1[1], w[1].1[1]),
                    mix(w[0].1[2], w[1].1[2]),
                ];
            }
        }
        s[s.len() - 1].1
    }
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RasterStyle {
    /// true = compunere RGB din 3 benzi; false = o singură bandă cu rampă
    pub rgb: bool,
    pub bands: [usize; 3],
    pub min: [f64; 3],
    pub max: [f64; 3],
    pub ramp: Ramp,
    pub hillshade: bool,
    /// valoare transparentă suplimentară (ex. fundal alb al hărților scanate)
    pub transparent_color: Option<[u8; 3]>,
}

impl RasterStyle {
    pub fn auto(r: &Raster) -> RasterStyle {
        let rgb = r.bands.len() >= 3;
        let mut min = [0.0; 3];
        let mut max = [255.0; 3];
        let n = if rgb { 3 } else { 1 };
        for b in 0..n {
            match &r.bands[b] {
                BandData::U8(_) => {}
                _ => {
                    let st = r.stats(b);
                    if rgb || !r.bands[b].is_float() && !matches!(r.bands[b], BandData::I16(_)) {
                        min[b] = st.p2;
                        max[b] = st.p98;
                    } else {
                        min[b] = st.min;
                        max[b] = st.max;
                    }
                }
            }
        }
        let single_u8 = !rgb && matches!(r.bands[0], BandData::U8(_));
        RasterStyle {
            rgb,
            bands: [0, 1.min(r.bands.len() - 1), 2.min(r.bands.len() - 1)],
            min,
            max,
            ramp: if single_u8 || matches!(r.bands[0], BandData::U16(_)) {
                Ramp::Gray
            } else {
                Ramp::Terrain
            },
            hillshade: !rgb && (r.bands[0].is_float() || matches!(r.bands[0], BandData::I16(_))),
            transparent_color: None,
        }
    }
}

/// Randează zona `view` (în sistemul hărții) într-o imagine RGBA de w x h pixeli.
pub fn render_rgba(
    src: &Raster,
    style: &RasterStyle,
    inv: &Transformer,
    view: &Bounds,
    w: usize,
    h: usize,
) -> Vec<u8> {
    let mut out = vec![0u8; w * h * 4];
    let Some(src_inv_gt) = src.inv_gt() else {
        return out;
    };
    let dst_gt = [
        view.minx,
        view.width() / w as f64,
        0.0,
        view.maxy,
        0.0,
        -view.height() / h as f64,
    ];
    let mapper = PixelMapper {
        inv,
        src_inv_gt,
        dst_gt,
    };
    let (sw, sh) = (src.width as isize, src.height as isize);
    let (pxs, pys) = src.pixel_size();
    let b0 = style.bands[0].min(src.bands.len() - 1);
    let range = |k: usize| {
        let d = style.max[k] - style.min[k];
        if d.abs() < 1e-12 { 1.0 } else { d }
    };
    let get = |b: usize, c: isize, r: isize| -> Option<f64> {
        if c < 0 || r < 0 || c >= sw || r >= sh {
            return None;
        }
        let v = src.bands[b].get((r * sw + c) as usize);
        if src.is_nodata(v) || !v.is_finite() {
            None
        } else {
            Some(v)
        }
    };
    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(2)
        .min(16);
    let rows_per = h.div_ceil(threads).max(1);
    std::thread::scope(|s| {
        for (t, chunk) in out.chunks_mut(rows_per * w * 4).enumerate() {
            let mapper = &mapper;
            let get = &get;
            let range = &range;
            s.spawn(move || {
                let mut rowbuf = Vec::with_capacity(w);
                let rows = chunk.len() / (w * 4);
                for jj in 0..rows {
                    let j = t * rows_per + jj;
                    mapper.row(j, w, 8, &mut rowbuf);
                    for (i, &(c, r)) in rowbuf.iter().enumerate() {
                        if !c.is_finite() {
                            continue;
                        }
                        let (ci, ri) = (c.floor() as isize, r.floor() as isize);
                        let o = (jj * w + i) * 4;
                        let px: Option<[u8; 3]> = if style.rgb {
                            let mut rgb = [0u8; 3];
                            let mut ok = true;
                            for k in 0..3 {
                                match get(style.bands[k].min(src.bands.len() - 1), ci, ri) {
                                    Some(v) => {
                                        rgb[k] = (((v - style.min[k]) / range(k)) * 255.0)
                                            .clamp(0.0, 255.0)
                                            as u8
                                    }
                                    None => ok = false,
                                }
                            }
                            // alfa din a patra bandă
                            if ok
                                && src.bands.len() >= 4
                                && matches!(src.bands[3], BandData::U8(_))
                                && get(3, ci, ri) == Some(0.0)
                            {
                                ok = false;
                            }
                            if ok { Some(rgb) } else { None }
                        } else {
                            get(b0, ci, ri).map(|v| {
                                let t = ((v - style.min[0]) / range(0)) as f32;
                                let mut col = style.ramp.color(t);
                                if style.hillshade {
                                    let z = |dc: isize, dr: isize| {
                                        get(b0, ci + dc, ri + dr).unwrap_or(v)
                                    };
                                    let dzdx = ((z(1, -1) + 2.0 * z(1, 0) + z(1, 1))
                                        - (z(-1, -1) + 2.0 * z(-1, 0) + z(-1, 1)))
                                        / (8.0 * pxs.max(1e-12));
                                    let dzdy = ((z(-1, 1) + 2.0 * z(0, 1) + z(1, 1))
                                        - (z(-1, -1) + 2.0 * z(0, -1) + z(1, -1)))
                                        / (8.0 * pys.max(1e-12));
                                    // pentru grade, scara verticală aproximativă
                                    let zf = if pxs < 0.01 { 1.0 / 111_320.0 } else { 1.0 };
                                    let (dzdx, dzdy) = (dzdx * zf, dzdy * zf);
                                    let slope = (dzdx * dzdx + dzdy * dzdy).sqrt().atan();
                                    let aspect = dzdy.atan2(-dzdx);
                                    let (alt, az) = (
                                        45f64.to_radians(),
                                        (360.0f64 - 315.0 + 90.0).to_radians(),
                                    );
                                    let shade = (alt.sin() * slope.cos()
                                        + alt.cos() * slope.sin() * (az - aspect).cos())
                                    .clamp(0.0, 1.0);
                                    let f = (0.35 + 0.75 * shade) as f32;
                                    col = [
                                        (col[0] as f32 * f).min(255.0) as u8,
                                        (col[1] as f32 * f).min(255.0) as u8,
                                        (col[2] as f32 * f).min(255.0) as u8,
                                    ];
                                }
                                col
                            })
                        };
                        if let Some(p) = px {
                            if style.transparent_color == Some(p) {
                                continue;
                            }
                            chunk[o] = p[0];
                            chunk[o + 1] = p[1];
                            chunk[o + 2] = p[2];
                            chunk[o + 3] = 255;
                        }
                    }
                }
            });
        }
    });
    out
}
