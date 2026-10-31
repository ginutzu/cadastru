//! Tipărire și export PDF.
//!
//! Pagina (titlu, hartă, caroiaj, nord, legendă, scară, informații) este desenată pe CPU
//! la rezoluția de tipar (tiny-skia + ab_glyph), apoi scrisă ca PDF sau trimisă la
//! imprimantă (Windows, GDI). Harta de fundal se descarcă la nivelul de detaliu potrivit
//! rezoluției de tipar.

use super::App;
use super::basemap::{self, Source};
use super::layers::*;
use ab_glyph::{Font, FontVec, PxScale, ScaleFont};
use anyhow::{Context, Result, bail};
use cadastru::crs::{Crs, Transformer};
use cadastru::model::*;
use eframe::egui::{self, RichText};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, channel};
use tiny_skia::{
    FillRule, LineCap, LineJoin, Paint, PathBuilder, Pixmap, PixmapPaint, Stroke, Transform,
};
use cadastru::i18n::tr;

// ---------------------------------------------------------------------------
// Setări

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Paper {
    A4,
    A3,
    A2,
    A1,
    A0,
}

impl Paper {
    pub const ALL: [Paper; 5] = [Paper::A4, Paper::A3, Paper::A2, Paper::A1, Paper::A0];
    /// (lățime, înălțime) în mm, portret
    pub fn mm(&self) -> (f64, f64) {
        match self {
            Paper::A4 => (210.0, 297.0),
            Paper::A3 => (297.0, 420.0),
            Paper::A2 => (420.0, 594.0),
            Paper::A1 => (594.0, 841.0),
            Paper::A0 => (841.0, 1189.0),
        }
    }
    pub fn name(&self) -> &'static str {
        match self {
            Paper::A4 => "A4",
            Paper::A3 => "A3",
            Paper::A2 => "A2",
            Paper::A1 => "A1",
            Paper::A0 => "A0",
        }
    }
    /// codul DMPAPER din Windows
    #[allow(dead_code)]
    pub fn dm_code(&self) -> i16 {
        match self {
            Paper::A4 => 9,
            Paper::A3 => 8,
            Paper::A2 => 66,
            Paper::A1 => 0,
            Paper::A0 => 0,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Output {
    Pdf,
    Printer,
}

pub const SCALES: &[u32] = &[
    200, 250, 500, 1000, 2000, 2500, 5000, 10000, 25000, 50000, 100000, 200000, 500000, 1000000,
];

pub struct PrintDlg {
    pub open: bool,
    pub output: Output,
    pub title: String,
    pub note: String,
    pub paper: Paper,
    pub landscape: bool,
    /// None = vederea curentă
    pub scale: Option<u32>,
    pub custom_scale: String,
    pub dpi: u32,
    pub legend: bool,
    pub scalebar: bool,
    pub north: bool,
    pub grid: bool,
    pub basemap: bool,
    pub status: String,
    pub busy: bool,
    rx: Option<Receiver<PrintMsg>>,
    preview: Option<egui::TextureHandle>,
}

impl Default for PrintDlg {
    fn default() -> Self {
        PrintDlg {
            open: false,
            output: Output::Pdf,
            title: String::new(),
            note: String::new(),
            paper: Paper::A4,
            landscape: true,
            scale: None,
            custom_scale: String::new(),
            dpi: 200,
            legend: true,
            scalebar: true,
            north: true,
            grid: true,
            basemap: true,
            status: String::new(),
            busy: false,
            rx: None,
            preview: None,
        }
    }
}

enum PrintMsg {
    Progress(String),
    Done(String),
    Preview(egui::ColorImage),
    Error(String),
}

// ---------------------------------------------------------------------------
// Conținutul de tipărit (preluat din hartă pe firul interfeței)

enum PrintLayer {
    Vector {
        name: String,
        style: VectorStyle,
        opacity: f32,
        kind: GeomKind,
        geoms: Vec<(Geometry, Option<String>)>,
    },
    Raster {
        name: String,
        opacity: f32,
        /// imagine RGBA (nepremultiplicată) de dimensiunea cadrului hărții
        rgba: Vec<u8>,
    },
}

struct PageSpec {
    page_mm: (f64, f64),
    dpi: f64,
    title: String,
    note: String,
    legend: bool,
    scalebar: bool,
    north: bool,
    grid: bool,
    /// cadrul hărții în pixeli de pagină
    frame: [f64; 4],
    /// extinderea hărții (sistemul hărții) corespunzătoare cadrului
    extent: Bounds,
    /// unități de hartă pe pixel de pagină
    upp: f64,
    scale_n: f64,
    mpu: f64,
    geographic: bool,
    crs_name: String,
    map_crs: Option<Crs>,
    north_angle: f64,
    basemap: Option<(Source, f32)>,
    show_labels: bool,
}

const MARGIN: f64 = 10.0;

fn mm2px(mm: f64, dpi: f64) -> f64 {
    mm / 25.4 * dpi
}
fn pt2px(pt: f64, dpi: f64) -> f32 {
    (pt / 72.0 * dpi) as f32
}

/// Înălțimea benzii de jos (legendă + informații), în mm.
fn footer_mm(n_legend: usize, legend: bool, page_w: f64) -> f64 {
    let cols = (((page_w - 2.0 * MARGIN - 85.0) / 60.0).floor() as usize).max(1);
    let rows = if legend { n_legend.div_ceil(cols) } else { 0 };
    (rows as f64 * 5.0 + 9.0).max(30.0).min(80.0)
}

// ---------------------------------------------------------------------------
// Fonturi și text

struct Fonts {
    regular: FontVec,
    bold: FontVec,
}

fn load_fonts() -> Fonts {
    let try_load = |names: &[&str]| -> Option<FontVec> {
        let dir = std::env::var_os("WINDIR")
            .map(|w| PathBuf::from(w).join("Fonts"))
            .unwrap_or_else(|| PathBuf::from("C:\\Windows\\Fonts"));
        for n in names {
            if let Ok(b) = std::fs::read(dir.join(n))
                && let Ok(f) = FontVec::try_from_vec(b)
            {
                return Some(f);
            }
        }
        None
    };
    let fallback = || FontVec::try_from_vec(epaint_default_fonts::UBUNTU_LIGHT.to_vec()).unwrap();
    Fonts {
        regular: try_load(&["segoeui.ttf", "arial.ttf"]).unwrap_or_else(fallback),
        bold: try_load(&["segoeuib.ttf", "arialbd.ttf"]).unwrap_or_else(fallback),
    }
}

fn text_width(font: &FontVec, px: f32, s: &str) -> f32 {
    let sf = font.as_scaled(PxScale::from(px));
    let mut w = 0.0;
    let mut prev = None;
    for ch in s.chars() {
        let id = sf.glyph_id(ch);
        if let Some(p) = prev {
            w += sf.kern(p, id);
        }
        w += sf.h_advance(id);
        prev = Some(id);
    }
    w
}

/// Amestecă o culoare (nepremultiplicată) cu acoperirea `cov` în pixmap.
fn blend(pm: &mut Pixmap, x: i32, y: i32, c: [u8; 4], cov: f32) {
    if x < 0 || y < 0 || x >= pm.width() as i32 || y >= pm.height() as i32 {
        return;
    }
    let a = (c[3] as f32 / 255.0) * cov.clamp(0.0, 1.0);
    if a <= 0.0 {
        return;
    }
    let i = (y as usize * pm.width() as usize + x as usize) * 4;
    let d = pm.data_mut();
    for k in 0..3 {
        let src = c[k] as f32 * a;
        d[i + k] = (src + d[i + k] as f32 * (1.0 - a)).round().min(255.0) as u8;
    }
    d[i + 3] = ((a * 255.0) + d[i + 3] as f32 * (1.0 - a)).round().min(255.0) as u8;
}

/// Scrie text cu linia de bază la `y`. Întoarce lățimea.
fn draw_text(
    pm: &mut Pixmap,
    font: &FontVec,
    px: f32,
    x: f32,
    y: f32,
    c: [u8; 4],
    s: &str,
) -> f32 {
    let sf = font.as_scaled(PxScale::from(px));
    let mut pen = x;
    let mut prev = None;
    for ch in s.chars() {
        let id = sf.glyph_id(ch);
        if let Some(p) = prev {
            pen += sf.kern(p, id);
        }
        let g = id.with_scale_and_position(PxScale::from(px), ab_glyph::point(pen, y));
        if let Some(og) = font.outline_glyph(g) {
            let b = og.px_bounds();
            og.draw(|gx, gy, cov| {
                blend(pm, b.min.x as i32 + gx as i32, b.min.y as i32 + gy as i32, c, cov);
            });
        }
        pen += sf.h_advance(id);
        prev = Some(id);
    }
    pen - x
}

/// Text cu halou (contur deschis) pentru lizibilitate peste hartă.
#[allow(clippy::too_many_arguments)]
fn draw_text_halo(
    pm: &mut Pixmap,
    font: &FontVec,
    px: f32,
    x: f32,
    y: f32,
    c: [u8; 4],
    halo: [u8; 4],
    s: &str,
) {
    let r = (px / 9.0).max(1.0);
    for (dx, dy) in [
        (-r, 0.0),
        (r, 0.0),
        (0.0, -r),
        (0.0, r),
        (-r * 0.7, -r * 0.7),
        (r * 0.7, r * 0.7),
        (-r * 0.7, r * 0.7),
        (r * 0.7, -r * 0.7),
    ] {
        draw_text(pm, font, px, x + dx, y + dy, halo, s);
    }
    draw_text(pm, font, px, x, y, c, s);
}

fn paint(c: [u8; 4]) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color_rgba8(c[0], c[1], c[2], c[3]);
    p.anti_alias = true;
    p
}

fn stroke(w: f32) -> Stroke {
    Stroke {
        width: w.max(0.1),
        line_cap: LineCap::Round,
        line_join: LineJoin::Round,
        ..Default::default()
    }
}

fn rect_path(x: f32, y: f32, w: f32, h: f32) -> Option<tiny_skia::Path> {
    tiny_skia::Rect::from_xywh(x, y, w, h).map(PathBuilder::from_rect)
}

// ---------------------------------------------------------------------------
// Randarea paginii

fn fmt_scale(n: f64) -> String {
    let n = n.round() as u64;
    let s = n.to_string();
    // separatorul de mii: punct în română, virgulă în engleză
    let sep = if cadastru::i18n::is_en() { ',' } else { '.' };
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(sep);
        }
        out.push(ch);
    }
    out
}

fn fmt_coord(v: f64, geographic: bool) -> String {
    if geographic {
        cadastru::crs::to_dms(v, true)
            .trim_end_matches(['N', 'S', 'E', 'W'])
            .to_string()
    } else {
        let n = v.round() as i64;
        let s = n.abs().to_string();
        let mut out = String::new();
        for (i, ch) in s.chars().enumerate() {
            if i > 0 && (s.len() - i) % 3 == 0 {
                out.push(' ');
            }
            out.push(ch);
        }
        if n < 0 { format!("-{out}") } else { out }
    }
}

/// Desenează harta de fundal în pixmap-ul cadrului.
fn draw_basemap(
    map: &mut Pixmap,
    spec: &PageSpec,
    src: &Source,
    opacity: f32,
    progress: &(dyn Fn(String) + Sync),
) -> Result<()> {
    let Some(mc) = spec.map_crs.as_ref() else {
        return Ok(());
    };
    let merc = Crs::from_epsg(3857)?;
    let to_merc = Transformer::new(mc, &merc)?;
    let (w, h) = (map.width() as usize, map.height() as usize);
    let ext = spec.extent;
    // grilă de transformare (la 16 px)
    let step = 16usize;
    let gw = w.div_ceil(step) + 1;
    let gh = h.div_ceil(step) + 1;
    let mut grid = vec![[f64::NAN; 2]; gw * gh];
    let mut mb = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
    for j in 0..gh {
        for i in 0..gw {
            let x = ext.minx + (i * step) as f64 * spec.upp;
            let y = ext.maxy - (j * step) as f64 * spec.upp;
            if let Some((mx, my)) = to_merc.xy(x, y) {
                grid[j * gw + i] = [mx, my];
                mb = [mb[0].min(mx), mb[1].min(my), mb[2].max(mx), mb[3].max(my)];
            }
        }
    }
    if mb[0] > mb[2] {
        return Ok(());
    }
    const MERC: f64 = 20037508.342789244;
    // rezoluția Web Mercator pe pixel de pagină
    let res = ((mb[2] - mb[0]) / w as f64).max((mb[3] - mb[1]) / h as f64);
    let zf = (2.0 * MERC / 256.0 / res).log2();
    let mut z = (zf.ceil() as i32).clamp(0, src.max_zoom as i32) as u8;
    let clampm = |v: f64| v.clamp(-MERC, MERC - 1e-6);
    let mut keys;
    loop {
        let ts = 2.0 * MERC / (1u64 << z) as f64;
        let tx0 = ((clampm(mb[0]) + MERC) / ts).floor() as u32;
        let tx1 = ((clampm(mb[2]) + MERC) / ts).floor() as u32;
        let ty0 = ((MERC - clampm(mb[3])) / ts).floor() as u32;
        let ty1 = ((MERC - clampm(mb[1])) / ts).floor() as u32;
        let n = (tx1 - tx0 + 1) as usize * (ty1 - ty0 + 1) as usize;
        if n <= 900 || z == 0 {
            keys = vec![];
            for l in 0..src.layers.len().min(8) as u8 {
                for ty in ty0..=ty1 {
                    for tx in tx0..=tx1 {
                        keys.push((l, z, tx, ty));
                    }
                }
            }
            break;
        }
        z -= 1;
    }
    let total = keys.len();
    let tiles = basemap::fetch_tiles_blocking(src, &keys, &|n| {
        if n % 10 == 0 || n == total {
            progress(tr!("Harta de fundal: {n} / {total} tile-uri"));
        }
    });
    if tiles.is_empty() {
        bail!(tr!("tile-urile hărții de fundal nu au putut fi descărcate (verificați conexiunea)"));
    }
    let ts = 2.0 * MERC / (1u64 << z) as f64;
    let nl = src.layers.len().min(8) as u8;
    let a = opacity.clamp(0.0, 1.0);
    let data = map.data_mut();
    // eșantionare biliniară, pe benzi de rânduri în paralel
    let rows_per = h.div_ceil(8).max(1);
    std::thread::scope(|s| {
        for (bi, band) in data.chunks_mut(rows_per * w * 4).enumerate() {
            let grid = &grid;
            let tiles = &tiles;
            s.spawn(move || {
                let y0 = bi * rows_per;
                for (ry, row) in band.chunks_mut(w * 4).enumerate() {
                    let py = y0 + ry;
                    let gj = py / step;
                    let fy = (py % step) as f64 / step as f64;
                    for px in 0..w {
                        let gi = px / step;
                        let fx = (px % step) as f64 / step as f64;
                        let g = |i: usize, j: usize| grid[(j.min(gh - 1)) * gw + i.min(gw - 1)];
                        let (a00, a10, a01, a11) =
                            (g(gi, gj), g(gi + 1, gj), g(gi, gj + 1), g(gi + 1, gj + 1));
                        if [a00, a10, a01, a11].iter().any(|p| p[0].is_nan()) {
                            continue;
                        }
                        let mx = (a00[0] * (1.0 - fx) + a10[0] * fx) * (1.0 - fy)
                            + (a01[0] * (1.0 - fx) + a11[0] * fx) * fy;
                        let my = (a00[1] * (1.0 - fx) + a10[1] * fx) * (1.0 - fy)
                            + (a01[1] * (1.0 - fx) + a11[1] * fx) * fy;
                        let tfx = (clampm(mx) + MERC) / ts;
                        let tfy = (MERC - clampm(my)) / ts;
                        let (tx, ty) = (tfx.floor() as u32, tfy.floor() as u32);
                        let o = px * 4;
                        for l in 0..nl {
                            let Some(t) = tiles.get(&(l, z, tx, ty)) else {
                                continue;
                            };
                            let u = (tfx - tx as f64) * t.w as f64 - 0.5;
                            let v = (tfy - ty as f64) * t.h as f64 - 0.5;
                            let (u0, v0) = (u.floor().max(0.0), v.floor().max(0.0));
                            let (du, dv) = ((u - u0).clamp(0.0, 1.0), (v - v0).clamp(0.0, 1.0));
                            let (iu, iv) = (u0 as usize, v0 as usize);
                            let (iu1, iv1) = ((iu + 1).min(t.w - 1), (iv + 1).min(t.h - 1));
                            let (iu, iv) = (iu.min(t.w - 1), iv.min(t.h - 1));
                            let p = |x: usize, y: usize| t.px[y * t.w + x];
                            let (c00, c10, c01, c11) = (p(iu, iv), p(iu1, iv), p(iu, iv1), p(iu1, iv1));
                            let mut c = [0f64; 4];
                            for k in 0..4 {
                                c[k] = (c00[k] as f64 * (1.0 - du) + c10[k] as f64 * du) * (1.0 - dv)
                                    + (c01[k] as f64 * (1.0 - du) + c11[k] as f64 * du) * dv;
                            }
                            // compunere „source over” cu pixeli premultiplicați
                            let sa = c[3] / 255.0 * a as f64;
                            for k in 0..3 {
                                let src = c[k] * a as f64;
                                row[o + k] = (src + row[o + k] as f64 * (1.0 - sa)).round().min(255.0) as u8;
                            }
                            row[o + 3] = (sa * 255.0 + row[o + 3] as f64 * (1.0 - sa)).round().min(255.0) as u8;
                        }
                    }
                }
            });
        }
    });
    Ok(())
}

/// Randează pagina completă. Întoarce pixmap-ul paginii.
fn render_page(
    spec: &PageSpec,
    layers: &[PrintLayer],
    progress: &(dyn Fn(String) + Sync),
) -> Result<(Pixmap, Vec<String>)> {
    let dpi = spec.dpi;
    let (pw, ph) = (
        mm2px(spec.page_mm.0, dpi).round() as u32,
        mm2px(spec.page_mm.1, dpi).round() as u32,
    );
    let mut page = Pixmap::new(pw, ph).context(tr!("pagină prea mare"))?;
    page.fill(tiny_skia::Color::WHITE);
    let fonts = load_fonts();
    let k = (dpi / 96.0) as f32; // pixeli ecran -> pixeli de tipar
    let [fx, fy, fw, fh] = spec.frame;
    let (mw, mh) = (fw.round() as u32, fh.round() as u32);
    let mut map = Pixmap::new(mw.max(1), mh.max(1)).context(tr!("cadru prea mare"))?;
    map.fill(tiny_skia::Color::WHITE);
    let mut warnings = vec![];

    // 1. harta de fundal
    let mut basemap_drawn = false;
    if let Some((src, op)) = &spec.basemap {
        progress(tr!("Se descarcă harta de fundal…").into());
        match draw_basemap(&mut map, spec, src, *op, progress) {
            Ok(()) => basemap_drawn = true,
            Err(e) => warnings.push(format!("{e:#}")),
        }
    }
    progress(tr!("Se desenează straturile…").into());
    let ext = spec.extent;
    let to_px = |x: f64, y: f64| -> (f32, f32) {
        (
            ((x - ext.minx) / spec.upp) as f32,
            ((ext.maxy - y) / spec.upp) as f32,
        )
    };
    // 2. straturile
    let mut label_boxes: Vec<[f32; 4]> = vec![];
    for l in layers {
        match l {
            PrintLayer::Raster { rgba, opacity, .. } => {
                let mut px = Pixmap::new(mw.max(1), mh.max(1)).unwrap();
                for (d, s) in px.data_mut().chunks_exact_mut(4).zip(rgba.chunks_exact(4)) {
                    let a = s[3] as u16;
                    d[0] = (s[0] as u16 * a / 255) as u8;
                    d[1] = (s[1] as u16 * a / 255) as u8;
                    d[2] = (s[2] as u16 * a / 255) as u8;
                    d[3] = s[3];
                }
                map.draw_pixmap(
                    0,
                    0,
                    px.as_ref(),
                    &PixmapPaint {
                        opacity: *opacity,
                        ..Default::default()
                    },
                    Transform::identity(),
                    None,
                );
            }
            PrintLayer::Vector {
                style,
                opacity,
                geoms,
                kind,
                ..
            } => {
                let mul = |c: [u8; 4]| [c[0], c[1], c[2], (c[3] as f32 * opacity) as u8];
                let fillp = paint(mul(style.fill));
                let strokep = paint(mul(style.stroke));
                let sw = stroke(style.width * k);
                for (g, _) in geoms {
                    // poligoane: umplere (par-impar pentru goluri) + contur
                    for poly in g.polygons() {
                        let mut pb = PathBuilder::new();
                        for ring in poly {
                            for (i, c) in ring.iter().enumerate() {
                                let (x, y) = to_px(c.x, c.y);
                                if i == 0 {
                                    pb.move_to(x, y)
                                } else {
                                    pb.line_to(x, y)
                                }
                            }
                            pb.close();
                        }
                        if let Some(path) = pb.finish() {
                            if style.fill[3] > 0 {
                                map.fill_path(&path, &fillp, FillRule::EvenOdd, Transform::identity(), None);
                            }
                            if style.width > 0.0 {
                                map.stroke_path(&path, &strokep, &sw, Transform::identity(), None);
                            }
                        }
                    }
                    if style.width > 0.0 {
                        for line in g.lines() {
                            let mut pb = PathBuilder::new();
                            for (i, c) in line.iter().enumerate() {
                                let (x, y) = to_px(c.x, c.y);
                                if i == 0 {
                                    pb.move_to(x, y)
                                } else {
                                    pb.line_to(x, y)
                                }
                            }
                            if let Some(path) = pb.finish() {
                                map.stroke_path(&path, &strokep, &sw, Transform::identity(), None);
                            }
                        }
                    }
                    for p in g.points() {
                        let (x, y) = to_px(p.x, p.y);
                        if let Some(c) = PathBuilder::from_circle(x, y, style.point_size * k) {
                            let f = [style.fill[0], style.fill[1], style.fill[2], 255];
                            map.fill_path(&c, &paint(mul(f)), FillRule::Winding, Transform::identity(), None);
                            if style.width > 0.0 {
                                map.stroke_path(
                                    &c,
                                    &strokep,
                                    &stroke(style.width.min(2.0) * k),
                                    Transform::identity(),
                                    None,
                                );
                            }
                        }
                    }
                }
                // etichete (fără suprapuneri)
                if spec.show_labels {
                    let fpx = style.label_size * k;
                    let lc = style.label_color;
                    let lum = 0.299 * lc[0] as f32 + 0.587 * lc[1] as f32 + 0.114 * lc[2] as f32;
                    let halo = if lum > 140.0 {
                        [0, 0, 0, 170]
                    } else {
                        [255, 255, 255, 220]
                    };
                    for (g, label) in geoms {
                        let (Some(t), Some(lp)) = (label, g.label_point()) else {
                            continue;
                        };
                        let (x, y) = to_px(lp.x, lp.y);
                        let tw = text_width(&fonts.regular, fpx, t);
                        let (bx, by) = if *kind == GeomKind::Point {
                            (x + style.point_size * k + 2.0 * k, y + fpx * 0.35)
                        } else {
                            (x - tw / 2.0, y + fpx * 0.35)
                        };
                        let bb = [bx, by - fpx, bx + tw, by + fpx * 0.3];
                        if bb[2] < 0.0 || bb[0] > mw as f32 || bb[3] < 0.0 || bb[1] > mh as f32 {
                            continue;
                        }
                        if label_boxes
                            .iter()
                            .any(|o| !(bb[2] < o[0] || bb[0] > o[2] || bb[3] < o[1] || bb[1] > o[3]))
                        {
                            continue;
                        }
                        label_boxes.push(bb);
                        draw_text_halo(&mut map, &fonts.regular, fpx, bx, by, lc, halo, t);
                    }
                }
            }
        }
    }

    // 3. caroiaj de coordonate (cruci + etichete pe margini)
    if spec.grid {
        let target = (fw.max(fh) / 6.0) * spec.upp;
        let iv = super::map::nice_number(target);
        if iv.is_finite() && iv > 0.0 {
            let gc = [40, 40, 40, 220];
            let gp = paint(gc);
            let gs = stroke(0.6 * k);
            let lpx = pt2px(6.5, dpi);
            let cross = 3.0 * mm2px(1.0, dpi) as f32;
            let x0 = (ext.minx / iv).ceil() as i64;
            let x1 = (ext.maxx / iv).floor() as i64;
            let y0 = (ext.miny / iv).ceil() as i64;
            let y1 = (ext.maxy / iv).floor() as i64;
            if (x1 - x0) < 60 && (y1 - y0) < 60 {
                for ix in x0..=x1 {
                    for iy in y0..=y1 {
                        let (x, y) = to_px(ix as f64 * iv, iy as f64 * iv);
                        let mut pb = PathBuilder::new();
                        pb.move_to(x - cross, y);
                        pb.line_to(x + cross, y);
                        pb.move_to(x, y - cross);
                        pb.line_to(x, y + cross);
                        if let Some(p) = pb.finish() {
                            map.stroke_path(&p, &gp, &gs, Transform::identity(), None);
                        }
                    }
                }
                let white = [255, 255, 255, 230];
                // etichetele care nu încap întregi în cadru (prea aproape de margine) sunt omise,
                // iar cele verticale nu intră în benzile de sus/jos ale etichetelor orizontale
                let pad = lpx * 0.4;
                let strip = lpx * 1.8;
                for ix in x0..=x1 {
                    let v = ix as f64 * iv;
                    let (x, _) = to_px(v, ext.maxy);
                    let s = fmt_coord(v, spec.geographic);
                    let tw = text_width(&fonts.regular, lpx, &s);
                    if x - tw / 2.0 < pad || x + tw / 2.0 > mw as f32 - pad {
                        continue;
                    }
                    draw_text_halo(&mut map, &fonts.regular, lpx, x - tw / 2.0, lpx * 1.2, gc, white, &s);
                    draw_text_halo(&mut map, &fonts.regular, lpx, x - tw / 2.0, mh as f32 - lpx * 0.4, gc, white, &s);
                }
                for iy in y0..=y1 {
                    let v = iy as f64 * iv;
                    let (_, y) = to_px(ext.minx, v);
                    if y - lpx < strip || y > mh as f32 - strip {
                        continue;
                    }
                    let s = fmt_coord(v, spec.geographic);
                    let tw = text_width(&fonts.regular, lpx, &s);
                    draw_text_halo(&mut map, &fonts.regular, lpx, pad, y - lpx * 0.3, gc, white, &s);
                    draw_text_halo(&mut map, &fonts.regular, lpx, mw as f32 - tw - pad, y - lpx * 0.3, gc, white, &s);
                }
            }
        }
    }

    // 4. săgeata nordului
    if spec.north {
        let s = mm2px(12.0, dpi) as f32;
        let (cx, cy) = (mw as f32 - s * 0.9, s * 1.1);
        let ang = spec.north_angle as f32;
        let rot = |dx: f32, dy: f32| -> (f32, f32) {
            let (c, si) = (ang.cos(), ang.sin());
            (cx + dx * c - dy * si, cy + dx * si + dy * c)
        };
        if let Some(p) = PathBuilder::from_circle(cx, cy, s * 0.62) {
            map.fill_path(&p, &paint([255, 255, 255, 200]), FillRule::Winding, Transform::identity(), None);
            map.stroke_path(&p, &paint([60, 60, 60, 255]), &stroke(0.5 * k), Transform::identity(), None);
        }
        let mut left = PathBuilder::new();
        let (a, b, c) = (rot(0.0, -s * 0.5), rot(-s * 0.22, s * 0.35), rot(0.0, s * 0.18));
        left.move_to(a.0, a.1);
        left.line_to(b.0, b.1);
        left.line_to(c.0, c.1);
        left.close();
        let mut right = PathBuilder::new();
        let d = rot(s * 0.22, s * 0.35);
        right.move_to(a.0, a.1);
        right.line_to(d.0, d.1);
        right.line_to(c.0, c.1);
        right.close();
        if let Some(p) = left.finish() {
            map.fill_path(&p, &paint([30, 30, 30, 255]), FillRule::Winding, Transform::identity(), None);
        }
        if let Some(p) = right.finish() {
            map.fill_path(&p, &paint([255, 255, 255, 255]), FillRule::Winding, Transform::identity(), None);
            map.stroke_path(&p, &paint([30, 30, 30, 255]), &stroke(0.6 * k), Transform::identity(), None);
        }
        let npx = pt2px(9.0, dpi);
        let tw = text_width(&fonts.bold, npx, "N");
        let n = rot(0.0, -s * 0.5 - npx * 0.25);
        draw_text_halo(&mut map, &fonts.bold, npx, n.0 - tw / 2.0, n.1, [20, 20, 20, 255], [255, 255, 255, 220], "N");
    }

    // cadrul hărții pe pagină
    page.draw_pixmap(
        fx.round() as i32,
        fy.round() as i32,
        map.as_ref(),
        &PixmapPaint::default(),
        Transform::identity(),
        None,
    );
    drop(map);
    if let Some(p) = rect_path(fx as f32, fy as f32, fw as f32, fh as f32) {
        page.stroke_path(&p, &paint([0, 0, 0, 255]), &stroke(1.0 * k), Transform::identity(), None);
    }

    // 5. titlu
    let black = [15, 15, 15, 255];
    let grey = [90, 90, 90, 255];
    if !spec.title.trim().is_empty() {
        let tpx = pt2px(18.0, dpi);
        let tw = text_width(&fonts.bold, tpx, spec.title.trim());
        draw_text(
            &mut page,
            &fonts.bold,
            tpx,
            (pw as f32 - tw) / 2.0,
            mm2px(MARGIN + 8.0, dpi) as f32,
            black,
            spec.title.trim(),
        );
    }

    // 6. banda de jos: legendă (stânga) și informații (dreapta)
    let foot_top = (fy + fh) as f32 + mm2px(3.0, dpi) as f32;
    let left = mm2px(MARGIN, dpi) as f32;
    let right = pw as f32 - mm2px(MARGIN, dpi) as f32;
    let info_w = mm2px(80.0, dpi) as f32;
    let info_x = right - info_w;
    let fpx = pt2px(8.0, dpi);
    let line_h = mm2px(5.0, dpi) as f32;
    if spec.legend {
        let lpx = pt2px(8.5, dpi);
        let mut y = foot_top + lpx;
        draw_text(&mut page, &fonts.bold, lpx, left, y, black, tr!("Legendă"));
        y += line_h * 0.9;
        let col_w = mm2px(60.0, dpi) as f32;
        let cols = (((info_x - left - mm2px(5.0, dpi) as f32) / col_w).floor() as usize).max(1);
        let sym_w = mm2px(7.0, dpi) as f32;
        let sym_h = mm2px(3.6, dpi) as f32;
        let items: Vec<&PrintLayer> = layers.iter().rev().collect();
        for (i, l) in items.iter().enumerate() {
            let col = i % cols;
            let row = i / cols;
            let x = left + col as f32 * col_w;
            let yy = y + row as f32 * line_h;
            let sy = yy - sym_h * 0.85;
            let name = match l {
                PrintLayer::Vector { name, style, kind, .. } => {
                    match kind {
                        GeomKind::Point => {
                            if let Some(c) = PathBuilder::from_circle(x + sym_w / 2.0, sy + sym_h / 2.0, (style.point_size * k).min(sym_h / 2.0)) {
                                page.fill_path(&c, &paint([style.fill[0], style.fill[1], style.fill[2], 255]), FillRule::Winding, Transform::identity(), None);
                                page.stroke_path(&c, &paint(style.stroke), &stroke(style.width.min(2.0) * k), Transform::identity(), None);
                            }
                        }
                        GeomKind::Line => {
                            let mut pb = PathBuilder::new();
                            pb.move_to(x, sy + sym_h * 0.75);
                            pb.line_to(x + sym_w * 0.4, sy + sym_h * 0.25);
                            pb.line_to(x + sym_w, sy + sym_h * 0.6);
                            if let Some(p) = pb.finish() {
                                page.stroke_path(&p, &paint(style.stroke), &stroke(style.width.max(1.0) * k), Transform::identity(), None);
                            }
                        }
                        _ => {
                            if let Some(p) = rect_path(x, sy, sym_w, sym_h) {
                                page.fill_path(&p, &paint(style.fill), FillRule::Winding, Transform::identity(), None);
                                page.stroke_path(&p, &paint(style.stroke), &stroke(style.width.max(0.8) * k), Transform::identity(), None);
                            }
                        }
                    }
                    name
                }
                PrintLayer::Raster { name, .. } => {
                    for s in 0..4 {
                        let v = 70 + s * 50;
                        if let Some(p) = rect_path(x + s as f32 * sym_w / 4.0, sy, sym_w / 4.0, sym_h) {
                            page.fill_path(&p, &paint([v as u8, (v + 20).min(255) as u8, v as u8, 255]), FillRule::Winding, Transform::identity(), None);
                        }
                    }
                    name
                }
            };
            let mut n = name.clone();
            let max_w = col_w - sym_w - mm2px(4.0, dpi) as f32;
            while n.chars().count() > 3 && text_width(&fonts.regular, lpx, &n) > max_w {
                n = n.chars().take(n.chars().count() - 2).collect::<String>();
                n.push('…');
            }
            draw_text(&mut page, &fonts.regular, lpx, x + sym_w + mm2px(2.0, dpi) as f32, yy, black, &n);
        }
    }
    // informații
    let mut y = foot_top + fpx;
    let scale_txt = tr!("Scara 1:{}", fmt_scale(spec.scale_n));
    draw_text(&mut page, &fonts.bold, pt2px(9.5, dpi), info_x, y + 2.0, black, &scale_txt);
    y += line_h;
    if spec.scalebar {
        // scara grafică: ~45 mm, în segmente alternante
        let meters = super::map::nice_number(mm2px(45.0, dpi) * spec.upp * spec.mpu);
        let len_px = (meters / spec.mpu / spec.upp) as f32;
        if len_px.is_finite() && len_px > 10.0 {
            let bh = mm2px(1.8, dpi) as f32;
            let segs = 4;
            for s in 0..segs {
                let c = if s % 2 == 0 { [20, 20, 20, 255] } else { [255, 255, 255, 255] };
                if let Some(p) = rect_path(info_x + s as f32 * len_px / segs as f32, y - bh, len_px / segs as f32, bh) {
                    page.fill_path(&p, &paint(c), FillRule::Winding, Transform::identity(), None);
                }
            }
            if let Some(p) = rect_path(info_x, y - bh, len_px, bh) {
                page.stroke_path(&p, &paint([20, 20, 20, 255]), &stroke(0.6 * k), Transform::identity(), None);
            }
            let lab = if meters >= 1000.0 {
                format!("{} km", meters / 1000.0)
            } else {
                format!("{meters} m")
            };
            let spx = pt2px(7.0, dpi);
            draw_text(&mut page, &fonts.regular, spx, info_x, y + spx * 1.2, black, "0");
            let tw = text_width(&fonts.regular, spx, &lab);
            draw_text(&mut page, &fonts.regular, spx, info_x + len_px - tw / 2.0, y + spx * 1.2, black, &lab);
            y += line_h * 1.2;
        }
    }
    let info_lines = [
        tr!("Sistem de coordonate: {}", spec.crs_name),
        tr!("Data: {}", today()),
    ];
    for l in info_lines.iter().chain(std::iter::once(&spec.note)) {
        if l.trim().is_empty() {
            continue;
        }
        draw_text(&mut page, &fonts.regular, fpx, info_x, y, black, l.trim());
        y += line_h * 0.85;
    }
    if let Some((src, _)) = spec.basemap.as_ref().filter(|_| basemap_drawn) {
        let apx = pt2px(6.0, dpi);
        let mut t = tr!("Fundal: {}", if src.attribution.is_empty() { &src.name } else { &src.attribution });
        while t.chars().count() > 10 && text_width(&fonts.regular, apx, &t) > info_w {
            t = t.chars().take(t.chars().count() - 2).collect::<String>();
            t.push('…');
        }
        draw_text(&mut page, &fonts.regular, apx, info_x, y, grey, &t);
    }
    let cpx = pt2px(6.0, dpi);
    let credit = tr!("Realizat cu Cadastru");
    let tw = text_width(&fonts.regular, cpx, credit);
    draw_text(&mut page, &fonts.regular, cpx, right - tw, ph as f32 - mm2px(MARGIN * 0.5, dpi) as f32, grey, credit);
    Ok((page, warnings))
}

fn today() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let z = secs.div_euclid(86400) + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    format!("{d:02}.{m:02}.{y}")
}

// ---------------------------------------------------------------------------
// PDF

/// Pagina ca imagine RGB (fără alfa, pe fond alb).
fn page_rgb(page: &Pixmap) -> Vec<u8> {
    let mut v = Vec::with_capacity((page.width() * page.height() * 3) as usize);
    for p in page.data().chunks_exact(4) {
        let a = p[3] as u16;
        for k in 0..3 {
            v.push((p[k] as u16 + (255 - a)).min(255) as u8);
        }
    }
    v
}

fn write_pdf(path: &std::path::Path, page: &Pixmap, page_mm: (f64, f64), title: &str) -> Result<()> {
    let (w, h) = (page.width(), page.height());
    let rgb = page_rgb(page);
    let mut jpeg = vec![];
    {
        let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 92);
        enc.encode(&rgb, w, h, image::ExtendedColorType::Rgb8)
            .context(tr!("codare JPEG"))?;
    }
    let pw = page_mm.0 / 25.4 * 72.0;
    let ph = page_mm.1 / 25.4 * 72.0;
    let mut out: Vec<u8> = b"%PDF-1.4\n%\xe2\xe3\xcf\xd3\n".to_vec();
    let mut offs = vec![];
    let pdf_str = |s: &str| -> String {
        // text Unicode: UTF-16BE cu BOM, în hex
        let mut hex = String::from("<FEFF");
        for u in s.encode_utf16() {
            hex.push_str(&format!("{u:04X}"));
        }
        hex.push('>');
        hex
    };
    let content = format!("q\n{pw:.3} 0 0 {ph:.3} 0 0 cm\n/Im0 Do\nQ\n");
    let objs: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {pw:.3} {ph:.3}] /Resources << /XObject << /Im0 4 0 R >> >> /Contents 5 0 R >>"
        )
        .into_bytes(),
        {
            let mut v = format!(
                "<< /Type /XObject /Subtype /Image /Width {w} /Height {h} /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /DCTDecode /Length {} >>\nstream\n",
                jpeg.len()
            )
            .into_bytes();
            v.extend_from_slice(&jpeg);
            v.extend_from_slice(b"\nendstream");
            v
        },
        format!("<< /Length {} >>\nstream\n{content}endstream", content.len()).into_bytes(),
        format!(
            "<< /Title {} /Producer (Cadastru) /Creator (Cadastru) >>",
            pdf_str(if title.trim().is_empty() { tr!("Hartă") } else { title.trim() })
        )
        .into_bytes(),
    ];
    for (i, o) in objs.iter().enumerate() {
        offs.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(o);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offs {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R /Info 6 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objs.len() + 1
        )
        .as_bytes(),
    );
    std::fs::write(path, out).with_context(|| tr!("Nu pot scrie {}", path.display()))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Tipărire Windows (GDI)

#[cfg(windows)]
mod winprint {
    use anyhow::{Result, bail};
    use cadastru::i18n::tr;
    use std::mem::{size_of, zeroed};
    use windows_sys::Win32::Foundation::GlobalFree;
    use windows_sys::Win32::Graphics::Gdi::*;
    use windows_sys::Win32::Storage::Xps::*;
    use windows_sys::Win32::System::Memory::*;
    use windows_sys::Win32::UI::Controls::Dialogs::*;

    pub struct Printer {
        hdc: HDC,
        pub dpi: (i32, i32),
        pub phys: (i32, i32),
        pub off: (i32, i32),
    }
    unsafe impl Send for Printer {}

    impl Printer {
        /// Dimensiunea hârtiei în mm.
        pub fn page_mm(&self) -> (f64, f64) {
            (
                self.phys.0 as f64 / self.dpi.0.max(1) as f64 * 25.4,
                self.phys.1 as f64 / self.dpi.1.max(1) as f64 * 25.4,
            )
        }
    }

    impl Drop for Printer {
        fn drop(&mut self) {
            unsafe {
                DeleteDC(self.hdc);
            }
        }
    }

    /// Dialogul standard Windows de alegere a imprimantei.
    pub fn choose(landscape: bool, paper: i16) -> Option<Printer> {
        unsafe {
            let hdm = GlobalAlloc(GHND, size_of::<DEVMODEW>());
            if !hdm.is_null() {
                let dm = GlobalLock(hdm) as *mut DEVMODEW;
                if !dm.is_null() {
                    (*dm).dmSize = size_of::<DEVMODEW>() as u16;
                    (*dm).dmSpecVersion = 0x0401;
                    (*dm).dmFields = DM_ORIENTATION;
                    (*dm).Anonymous1.Anonymous1.dmOrientation = if landscape { 2 } else { 1 };
                    if paper > 0 {
                        (*dm).dmFields |= DM_PAPERSIZE;
                        (*dm).Anonymous1.Anonymous1.dmPaperSize = paper;
                    }
                    GlobalUnlock(hdm);
                }
            }
            let mut pd: PRINTDLGW = zeroed();
            pd.lStructSize = size_of::<PRINTDLGW>() as u32;
            pd.hDevMode = hdm;
            pd.Flags = PD_RETURNDC | PD_NOPAGENUMS | PD_NOSELECTION | PD_USEDEVMODECOPIESANDCOLLATE;
            pd.nCopies = 1;
            let ok = PrintDlgW(&mut pd) != 0;
            if !pd.hDevMode.is_null() {
                GlobalFree(pd.hDevMode);
            }
            if !pd.hDevNames.is_null() {
                GlobalFree(pd.hDevNames);
            }
            if !ok || pd.hDC.is_null() {
                return None;
            }
            let hdc = pd.hDC;
            Some(Printer {
                hdc,
                dpi: (GetDeviceCaps(hdc, LOGPIXELSX as i32), GetDeviceCaps(hdc, LOGPIXELSY as i32)),
                phys: (GetDeviceCaps(hdc, PHYSICALWIDTH as i32), GetDeviceCaps(hdc, PHYSICALHEIGHT as i32)),
                off: (GetDeviceCaps(hdc, PHYSICALOFFSETX as i32), GetDeviceCaps(hdc, PHYSICALOFFSETY as i32)),
            })
        }
    }

    /// Trimite pagina (RGBA premultiplicat, pe fond alb) la imprimantă, în benzi.
    pub fn print(p: &Printer, rgba: &[u8], w: u32, h: u32, name: &str) -> Result<()> {
        unsafe {
            let name_w: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
            let di = DOCINFOW {
                cbSize: size_of::<DOCINFOW>() as i32,
                lpszDocName: name_w.as_ptr(),
                lpszOutput: std::ptr::null(),
                lpszDatatype: std::ptr::null(),
                fwType: 0,
            };
            if StartDocW(p.hdc, &di) <= 0 {
                bail!(tr!("imprimanta a refuzat documentul"));
            }
            if StartPage(p.hdc) <= 0 {
                EndDoc(p.hdc);
                bail!(tr!("imprimanta a refuzat pagina"));
            }
            SetStretchBltMode(p.hdc, HALFTONE);
            let sx = p.phys.0 as f64 / w as f64;
            let sy = p.phys.1 as f64 / h as f64;
            let band = 256u32;
            let mut y = 0u32;
            while y < h {
                let bh = band.min(h - y);
                // BGRA, pe fond alb
                let mut buf = Vec::with_capacity((w * bh * 4) as usize);
                let start = (y * w * 4) as usize;
                for px in rgba[start..start + (w * bh * 4) as usize].chunks_exact(4) {
                    let a = px[3] as u16;
                    let c = |v: u8| (v as u16 + (255 - a)).min(255) as u8;
                    buf.extend_from_slice(&[c(px[2]), c(px[1]), c(px[0]), 0]);
                }
                let mut bmi: BITMAPINFO = zeroed();
                bmi.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
                bmi.bmiHeader.biWidth = w as i32;
                bmi.bmiHeader.biHeight = -(bh as i32);
                bmi.bmiHeader.biPlanes = 1;
                bmi.bmiHeader.biBitCount = 32;
                bmi.bmiHeader.biCompression = BI_RGB;
                let dy0 = (y as f64 * sy).round() as i32 - p.off.1;
                let dy1 = ((y + bh) as f64 * sy).round() as i32 - p.off.1;
                StretchDIBits(
                    p.hdc,
                    -p.off.0,
                    dy0,
                    (w as f64 * sx).round() as i32,
                    dy1 - dy0,
                    0,
                    0,
                    w as i32,
                    bh as i32,
                    buf.as_ptr() as *const _,
                    &bmi,
                    DIB_RGB_COLORS,
                    SRCCOPY,
                );
                y += bh;
            }
            EndPage(p.hdc);
            if EndDoc(p.hdc) <= 0 {
                bail!(tr!("eroare la finalizarea tipăririi"));
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Dialogul

impl App {
    pub fn open_print(&mut self, output: Output) {
        let d = &mut self.dlg.print;
        d.open = true;
        d.output = output;
        d.status.clear();
        d.preview = None;
    }

    /// Calculează extinderea și scara pentru o pagină dată.
    #[allow(clippy::type_complexity)]
    fn print_layout(&self, d: &PrintDlg, page_mm: (f64, f64), dpi: f64) -> PageSpec {
        let n_legend = self.layers.iter().filter(|l| l.visible).count();
        let title_mm = if d.title.trim().is_empty() { 0.0 } else { 12.0 };
        let foot = footer_mm(n_legend, d.legend, page_mm.0);
        let fx = mm2px(MARGIN, dpi);
        let fy = mm2px(MARGIN + title_mm, dpi);
        let fw = mm2px(page_mm.0 - 2.0 * MARGIN, dpi);
        let fh = mm2px(page_mm.1 - 2.0 * MARGIN - title_mm - foot - 3.0, dpi).max(10.0);
        let mpu = self.meters_per_unit();
        let m_per_px = 0.0254 / dpi; // metri de hârtie pe pixel
        let upp = match d.scale {
            Some(n) => n as f64 * m_per_px / mpu,
            None => {
                let vb = self.view_bounds();
                (vb.width() / fw).max(vb.height() / fh)
            }
        };
        let scale_n = upp * mpu / m_per_px;
        let (cx, cy) = (self.view.cx, self.view.cy);
        let extent = Bounds::new(
            cx - fw / 2.0 * upp,
            cy - fh / 2.0 * upp,
            cx + fw / 2.0 * upp,
            cy + fh / 2.0 * upp,
        );
        // unghiul nordului geografic față de nordul caroiajului
        let mut north_angle = 0.0;
        if let (Some(m), Some(tw)) = (&self.map_crs, &self.to_wgs)
            && let Ok(fw_t) = Transformer::new(&Crs::wgs84(), m)
            && let Some((lon, lat)) = tw.xy(cx, cy)
            && let Some((x2, y2)) = fw_t.xy(lon, lat + 0.01)
        {
            north_angle = (x2 - cx).atan2(y2 - cy);
        }
        PageSpec {
            page_mm,
            dpi,
            title: d.title.clone(),
            note: d.note.clone(),
            legend: d.legend,
            scalebar: d.scalebar,
            north: d.north,
            grid: d.grid,
            frame: [fx, fy, fw, fh],
            extent,
            upp,
            scale_n,
            mpu,
            geographic: self.map_crs.as_ref().is_some_and(|c| c.is_geographic()),
            crs_name: self
                .map_crs
                .as_ref()
                .map(|c| c.to_string())
                .unwrap_or_else(|| tr!("nedefinit").into()),
            map_crs: self.map_crs.clone(),
            north_angle,
            basemap: if d.basemap {
                self.basemap.source.clone().map(|s| (s, self.basemap.opacity))
            } else {
                None
            },
            show_labels: self.show_labels,
        }
    }

    /// Preia conținutul straturilor vizibile pentru extinderea paginii.
    fn print_content(&self, spec: &PageSpec) -> Vec<PrintLayer> {
        let mut out = vec![];
        let (mw, mh) = (spec.frame[2].round() as usize, spec.frame[3].round() as usize);
        for l in &self.layers {
            if !l.visible {
                continue;
            }
            match &l.kind {
                LayerKind::Vector(v) => {
                    let li = v.style.label_field.as_ref().and_then(|n| v.data.field_index(n));
                    let geoms = v
                        .disp
                        .iter()
                        .filter(|d| d.bbox.intersects(&spec.extent))
                        .map(|d| {
                            let lab = li
                                .map(|i| v.data.features[d.feat].attrs[i].to_text())
                                .filter(|s| !s.is_empty());
                            (d.geom.clone(), lab)
                        })
                        .collect();
                    out.push(PrintLayer::Vector {
                        name: l.name.clone(),
                        style: v.style.clone(),
                        opacity: l.opacity,
                        kind: v.data.geom_kind(),
                        geoms,
                    });
                }
                LayerKind::Raster(r) => {
                    if !r.map_bounds.intersects(&spec.extent) {
                        continue;
                    }
                    let inv = match (&self.map_crs, r.data.crs.as_ref()) {
                        (Some(m), Some(s)) => {
                            Transformer::new(m, s).unwrap_or_else(|_| Transformer::identity())
                        }
                        _ => Transformer::identity(),
                    };
                    let rgba = cadastru::rasterops::render_rgba(
                        &r.data,
                        &r.style,
                        &inv,
                        &spec.extent,
                        mw.max(1),
                        mh.max(1),
                    );
                    out.push(PrintLayer::Raster {
                        name: l.name.clone(),
                        opacity: l.opacity,
                        rgba,
                    });
                }
            }
        }
        out
    }

    fn start_print_job(&mut self, ctx: &egui::Context, preview: bool) {
        let d = &self.dlg.print;
        let (mut pw, mut ph) = d.paper.mm();
        if d.landscape {
            std::mem::swap(&mut pw, &mut ph);
        }
        let mut dpi = if preview { 50.0 } else { d.dpi as f64 };
        let mut target: Option<PathBuf> = None;
        #[cfg(windows)]
        let mut printer: Option<winprint::Printer> = None;
        if !preview {
            match d.output {
                Output::Pdf => {
                    let name = if d.title.trim().is_empty() {
                        "harta.pdf".to_string()
                    } else {
                        format!("{}.pdf", d.title.trim().replace(['/', '\\', ':', '*', '?', '"', '<', '>', '|'], "_"))
                    };
                    let Some(p) = rfd::FileDialog::new()
                        .set_title(tr!("Salvează ca PDF"))
                        .add_filter("PDF", &["pdf"])
                        .set_file_name(name)
                        .save_file()
                    else {
                        return;
                    };
                    target = Some(if p.extension().is_none() { p.with_extension("pdf") } else { p });
                }
                Output::Printer => {
                    #[cfg(windows)]
                    {
                        match winprint::choose(d.landscape, d.paper.dm_code()) {
                            Some(p) => {
                                (pw, ph) = p.page_mm();
                                dpi = dpi.min(p.dpi.0.max(72) as f64);
                                printer = Some(p);
                            }
                            None => return,
                        }
                    }
                    #[cfg(not(windows))]
                    {
                        self.dlg.print.status =
                            tr!("Tipărirea directă este disponibilă doar pe Windows. Folosiți „Salvează ca PDF”.")
                                .into();
                        return;
                    }
                }
            }
        }
        // limită de memorie: maxim ~120 megapixeli pe pagină
        let mp = mm2px(pw, dpi) * mm2px(ph, dpi) / 1e6;
        let mut note = String::new();
        if mp > 120.0 {
            let nd = (dpi * (120.0 / mp).sqrt()).floor();
            note = tr!(" (rezoluție redusă la {nd} DPI pentru formatul ales)");
            dpi = nd;
        }
        let spec = self.print_layout(&self.dlg.print, (pw, ph), dpi);
        let content = self.print_content(&spec);
        let (tx, rx) = channel();
        let ctx = ctx.clone();
        let title = self.dlg.print.title.clone();
        #[cfg(windows)]
        let printer = printer;
        std::thread::spawn(move || {
            let prog = {
                let tx = tx.clone();
                let ctx = ctx.clone();
                move |s: String| {
                    let _ = tx.send(PrintMsg::Progress(s));
                    ctx.request_repaint();
                }
            };
            let res = (|| -> Result<String> {
                let (page, warns) = render_page(&spec, &content, &prog)?;
                let w_note = if warns.is_empty() {
                    String::new()
                } else {
                    tr!(" — atenție: {}", warns.join("; "))
                };
                if preview {
                    let img = egui::ColorImage::from_rgba_premultiplied(
                        [page.width() as usize, page.height() as usize],
                        page.data(),
                    );
                    let _ = tx.send(PrintMsg::Preview(img));
                    return Ok(tr!("Previzualizare: scara 1:{}{w_note}", fmt_scale(spec.scale_n)));
                }
                if let Some(p) = &target {
                    prog(tr!("Se scrie PDF-ul…").into());
                    write_pdf(p, &page, spec.page_mm, &title)?;
                    return Ok(tr!("PDF salvat: {} (scara 1:{}){note}{w_note}", p.display(), fmt_scale(spec.scale_n)));
                }
                #[cfg(windows)]
                if let Some(pr) = &printer {
                    prog(tr!("Se trimite la imprimantă…").into());
                    winprint::print(pr, page.data(), page.width(), page.height(), if title.trim().is_empty() { "Cadastru" } else { title.trim() })?;
                    return Ok(tr!("Pagina a fost trimisă la imprimantă (scara 1:{}){note}{w_note}", fmt_scale(spec.scale_n)));
                }
                bail!(tr!("nicio destinație"))
            })();
            let _ = tx.send(match res {
                Ok(s) => PrintMsg::Done(s),
                Err(e) => PrintMsg::Error(format!("{e:#}")),
            });
            ctx.request_repaint();
        });
        let d = &mut self.dlg.print;
        d.rx = Some(rx);
        d.busy = true;
        d.status = if preview { tr!("Se pregătește previzualizarea…").into() } else { tr!("Se pregătește pagina…").into() };
    }

    pub fn print_window(&mut self, ctx: &egui::Context) {
        if !self.dlg.print.open {
            return;
        }
        // mesajele lucrării în curs
        if let Some(rx) = &self.dlg.print.rx {
            let mut done = false;
            while let Ok(m) = rx.try_recv() {
                match m {
                    PrintMsg::Progress(s) => self.dlg.print.status = s,
                    PrintMsg::Preview(img) => {
                        self.dlg.print.preview = Some(ctx.load_texture("print-preview", img, egui::TextureOptions::LINEAR));
                    }
                    PrintMsg::Done(s) => {
                        self.status = s.clone();
                        self.dlg.print.status = s;
                        done = true;
                    }
                    PrintMsg::Error(e) => {
                        self.dlg.print.status = tr!("Eroare: {e}");
                        done = true;
                    }
                }
            }
            if done {
                self.dlg.print.rx = None;
                self.dlg.print.busy = false;
            }
        }
        let mut open = true;
        let mut go: Option<bool> = None; // Some(preview)
        let title = match self.dlg.print.output {
            Output::Pdf => tr!("Salvează harta ca PDF"),
            Output::Printer => tr!("Tipărire hartă"),
        };
        let cur_scale = {
            let (mut pw, mut ph) = self.dlg.print.paper.mm();
            if self.dlg.print.landscape {
                std::mem::swap(&mut pw, &mut ph);
            }
            let mut tmp = PrintDlg { scale: None, ..Default::default() };
            tmp.title = self.dlg.print.title.clone();
            tmp.legend = self.dlg.print.legend;
            self.print_layout(&tmp, (pw, ph), 96.0).scale_n
        };
        let has_basemap = self.basemap.source.is_some();
        let d = &mut self.dlg.print;
        egui::Window::new(title).id(egui::Id::unique("win_print_0"))
            .open(&mut open)
            .default_width(520.0)
            .collapsible(false)
            .show(ctx, |ui| {
                egui::Grid::new("printg").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
                    ui.label(tr!("Titlu:"));
                    ui.add(egui::TextEdit::singleline(&mut d.title).desired_width(320.0).hint_text(tr!("ex. Plan de încadrare în zonă")));
                    ui.end_row();
                    ui.label(tr!("Notă (în colț):"));
                    ui.add(egui::TextEdit::singleline(&mut d.note).desired_width(320.0).hint_text(tr!("ex. Întocmit: …")));
                    ui.end_row();
                    ui.label(tr!("Hârtie:"));
                    ui.horizontal(|ui| {
                        egui::ComboBox::from_id_salt("paper").selected_text(d.paper.name()).show_ui(ui, |ui| {
                            for p in Paper::ALL {
                                ui.selectable_value(&mut d.paper, p, p.name());
                            }
                        });
                        ui.radio_value(&mut d.landscape, true, tr!("Peisaj"));
                        ui.radio_value(&mut d.landscape, false, tr!("Portret"));
                    });
                    ui.end_row();
                    ui.label(tr!("Scara:"));
                    ui.horizontal(|ui| {
                        let txt = match d.scale {
                            None => tr!("Vederea curentă (≈1:{})", fmt_scale(cur_scale)),
                            Some(n) => format!("1:{}", fmt_scale(n as f64)),
                        };
                        egui::ComboBox::from_id_salt("scale").selected_text(txt).width(210.0).show_ui(ui, |ui| {
                            ui.selectable_value(&mut d.scale, None, tr!("Vederea curentă (≈1:{})", fmt_scale(cur_scale)));
                            for &n in SCALES {
                                ui.selectable_value(&mut d.scale, Some(n), format!("1:{}", fmt_scale(n as f64)));
                            }
                        });
                        ui.label("1:");
                        let r = ui.add(egui::TextEdit::singleline(&mut d.custom_scale).desired_width(70.0).hint_text(tr!("altă")));
                        if r.changed()
                            && let Ok(n) = d.custom_scale.replace(['.', ' '], "").parse::<u32>()
                            && n > 0
                        {
                            d.scale = Some(n);
                        }
                    });
                    ui.end_row();
                    ui.label(tr!("Rezoluție:"));
                    ui.horizontal(|ui| {
                        for v in [150u32, 200, 300] {
                            ui.radio_value(&mut d.dpi, v, format!("{v} DPI"));
                        }
                    });
                    ui.end_row();
                    ui.label(tr!("Elemente:"));
                    ui.vertical(|ui| {
                        ui.horizontal(|ui| {
                            ui.checkbox(&mut d.legend, tr!("Legendă"));
                            ui.checkbox(&mut d.scalebar, tr!("Scară grafică"));
                            ui.checkbox(&mut d.north, tr!("Nord"));
                        });
                        ui.horizontal(|ui| {
                            ui.checkbox(&mut d.grid, tr!("Caroiaj de coordonate"));
                            ui.add_enabled(has_basemap, egui::Checkbox::new(&mut d.basemap, tr!("Harta de fundal")));
                        });
                    });
                    ui.end_row();
                });
                ui.label(RichText::new(tr!("Harta este centrată pe vederea curentă; elementele vizibile sunt cele din hartă (straturi, stiluri, etichete).")).small().weak());
                ui.separator();
                if let Some(t) = &d.preview {
                    let max = egui::vec2(ui.available_width().min(480.0), 340.0);
                    let s = t.size_vec2();
                    let f = (max.x / s.x).min(max.y / s.y);
                    ui.vertical_centered(|ui| {
                        ui.add(egui::Image::new((t.id(), s * f)).bg_fill(egui::Color32::WHITE));
                    });
                    ui.separator();
                }
                ui.horizontal(|ui| {
                    ui.add_enabled_ui(!d.busy, |ui| {
                        if ui.button(tr!("Previzualizare")).clicked() {
                            go = Some(true);
                        }
                        let main = match d.output {
                            Output::Pdf => tr!("Salvează PDF…"),
                            Output::Printer => tr!("Tipărește…"),
                        };
                        if ui.button(RichText::new(main).strong()).clicked() {
                            go = Some(false);
                        }
                    });
                    if d.busy {
                        ui.spinner();
                    }
                });
                if !d.status.is_empty() {
                    ui.label(RichText::new(&d.status).small());
                }
            });
        if !open {
            self.dlg.print.open = false;
        }
        if let Some(p) = go {
            self.start_print_job(ctx, p);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pagină de probă cu datele din `CADASTRU_SAMPLE_DIR` (rulare manuală).
    #[test]
    #[ignore]
    fn sample_page() {
        let dir = PathBuf::from(std::env::var("CADASTRU_SAMPLE_DIR").unwrap());
        let out = PathBuf::from(std::env::var("CADASTRU_SAMPLE_OUT").unwrap());
        let s70 = Crs::from_epsg(3844).unwrap();
        let mut layers = vec![];
        let mut ext = Bounds::empty();
        for (i, f) in ["orto.tif", "parcele.shp", "drumuri.shp", "puncte3d.shp"].iter().enumerate() {
            let ds = cadastru::formats::open(&dir.join(f), &Default::default()).unwrap();
            for d in ds {
                match d {
                    Dataset::Vector(v) => {
                        ext.merge(&v.bounds());
                        let mut st = VectorStyle::for_index(i);
                        st.label_field = v.fields.first().map(|f| f.name.clone());
                        layers.push(PrintLayer::Vector {
                            name: v.name.clone(),
                            style: st,
                            opacity: 1.0,
                            kind: v.geom_kind(),
                            geoms: v.features.iter().filter_map(|f| f.geom.clone().map(|g| (g, f.attrs.first().map(|a| a.to_text())))).collect(),
                        });
                    }
                    Dataset::Raster(_) => {}
                }
            }
        }
        let dpi = 150.0;
        let page_mm = (297.0, 210.0);
        let fw = mm2px(277.0, dpi);
        let fh = mm2px(210.0 - 20.0 - 12.0 - 30.0 - 3.0, dpi);
        let (cx, cy) = ext.center();
        let upp = (ext.width() / fw).max(ext.height() / fh) * 1.15;
        let spec = PageSpec {
            page_mm,
            dpi,
            title: "Plan de situație – test tipărire".into(),
            note: "Întocmit: Ginu Popescu".into(),
            legend: true,
            scalebar: true,
            north: true,
            grid: true,
            frame: [mm2px(10.0, dpi), mm2px(22.0, dpi), fw, fh],
            extent: Bounds::new(cx - fw / 2.0 * upp, cy - fh / 2.0 * upp, cx + fw / 2.0 * upp, cy + fh / 2.0 * upp),
            upp,
            scale_n: upp / (0.0254 / dpi),
            mpu: 1.0,
            geographic: false,
            crs_name: s70.to_string(),
            map_crs: Some(s70),
            north_angle: 0.02,
            basemap: std::env::var("CADASTRU_SAMPLE_TILES").ok().and_then(|t| Source::custom(&t, 19)).map(|s| (s, 0.9)),
            show_labels: true,
        };
        let (page, warns) = render_page(&spec, &layers, &|s| eprintln!("{s}")).unwrap();
        eprintln!("avertismente: {warns:?}");
        page.save_png(out.with_extension("png")).unwrap();
        write_pdf(&out.with_extension("pdf"), &page, page_mm, &spec.title).unwrap();
    }
}
