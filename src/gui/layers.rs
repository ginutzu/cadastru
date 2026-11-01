//! Straturile din hartă și pregătirea lor pentru afișare.

use cadastru::crs::{Crs, Transformer};
use cadastru::model::*;
use cadastru::rasterops::{RasterStyle, render_rgba};
use eframe::egui::{self, Color32, TextureHandle};
use serde::{Deserialize, Serialize};
use cadastru::i18n::tr;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VectorStyle {
    pub stroke: [u8; 4],
    pub width: f32,
    pub fill: [u8; 4],
    pub point_size: f32,
    pub label_field: Option<String>,
    pub label_size: f32,
    pub label_color: [u8; 4],
    /// poligoane fără umplere („No fill”): doar conturul
    #[serde(default)]
    pub no_fill: bool,
}

impl VectorStyle {
    pub fn for_index(i: usize) -> VectorStyle {
        const PAL: [[u8; 3]; 8] = [
            [220, 50, 47],
            [38, 139, 210],
            [133, 153, 0],
            [211, 54, 130],
            [181, 137, 0],
            [42, 161, 152],
            [108, 113, 196],
            [203, 75, 22],
        ];
        let c = PAL[i % PAL.len()];
        VectorStyle {
            stroke: [c[0], c[1], c[2], 255],
            width: 1.5,
            fill: [c[0], c[1], c[2], 60],
            point_size: 4.0,
            label_field: None,
            label_size: 12.0,
            label_color: [20, 20, 20, 255],
            no_fill: false,
        }
    }
    pub fn stroke_c(&self) -> Color32 {
        Color32::from_rgba_unmultiplied(
            self.stroke[0],
            self.stroke[1],
            self.stroke[2],
            self.stroke[3],
        )
    }
    /// Umplerea poligoanelor (transparentă pentru „No fill”).
    pub fn poly_fill(&self) -> [u8; 4] {
        if self.no_fill { [0, 0, 0, 0] } else { self.fill }
    }
    pub fn fill_c(&self) -> Color32 {
        Color32::from_rgba_unmultiplied(self.fill[0], self.fill[1], self.fill[2], self.fill[3])
    }
    pub fn label_c(&self) -> Color32 {
        Color32::from_rgba_unmultiplied(
            self.label_color[0],
            self.label_color[1],
            self.label_color[2],
            self.label_color[3],
        )
    }
}

/// O geometrie pregătită pentru desenare (coordonate în sistemul hărții).
pub struct DispGeom {
    pub feat: usize,
    pub bbox: Bounds,
    pub geom: Geometry,
    /// triunghiuri pentru umplerea poligoanelor: vârfuri + indici
    pub tri_verts: Vec<[f64; 2]>,
    pub tri_idx: Vec<u32>,
    pub label_pt: Option<Coord>,
}

pub struct VLayer {
    pub data: VectorLayer,
    pub style: VectorStyle,
    pub disp: Vec<DispGeom>,
    pub disp_bounds: Bounds,
    pub failed: usize,
    /// Entitățile selectate (indici în `data.features`)
    pub sel: std::collections::BTreeSet<usize>,
    /// Crește la fiecare modificare a selecției (pentru reîmprospătarea tabelului)
    pub sel_gen: u64,
}

pub struct RLayer {
    pub data: Raster,
    pub style: RasterStyle,
    pub map_bounds: Bounds,
    pub overview: Option<(TextureHandle, Bounds)>,
    pub detail: Option<(TextureHandle, Bounds)>,
    /// cheia ultimei vederi pentru care s-a randat detaliul
    pub detail_key: Option<[i64; 5]>,
    pub dirty: bool,
}

pub enum LayerKind {
    Vector(VLayer),
    Raster(RLayer),
}

pub struct LayerEntry {
    pub id: u64,
    pub name: String,
    pub visible: bool,
    pub opacity: f32,
    /// sistemul a fost presupus (fișierul nu avea proiecție)
    pub crs_assumed: bool,
    pub kind: LayerKind,
    /// identificatorul stratului în fișierul sursă (pentru GeoPackage etc.)
    pub source_layer: Option<String>,
    /// coloanele X/Y/Z folosite la importul CSV (pentru spațiul de lucru)
    pub csv_fields: [Option<String>; 3],
    /// numărul de ordine la deschidere
    pub seq: u64,
    /// stratul CGXML a fost încărcat cu X/Y inversate
    pub cgxml_swapped: bool,
}

impl LayerEntry {
    pub fn crs(&self) -> Option<&Crs> {
        match &self.kind {
            LayerKind::Vector(v) => v.data.crs.as_ref(),
            LayerKind::Raster(r) => r.data.crs.as_ref(),
        }
    }
    pub fn set_crs(&mut self, c: Option<Crs>) {
        match &mut self.kind {
            LayerKind::Vector(v) => v.data.crs = c,
            LayerKind::Raster(r) => r.data.crs = c,
        }
    }
    pub fn source(&self) -> Option<&std::path::Path> {
        match &self.kind {
            LayerKind::Vector(v) => v.data.source.as_deref(),
            LayerKind::Raster(r) => r.data.source.as_deref(),
        }
    }
    pub fn map_bounds(&self) -> Bounds {
        match &self.kind {
            LayerKind::Vector(v) => v.disp_bounds,
            LayerKind::Raster(r) => r.map_bounds,
        }
    }
    pub fn is_vector(&self) -> bool {
        matches!(self.kind, LayerKind::Vector(_))
    }
    pub fn summary(&self) -> String {
        match &self.kind {
            LayerKind::Vector(v) => tr!(
                "{} entități, {}",
                v.data.features.len(),
                v.data.geom_kind().label()
            ),
            LayerKind::Raster(r) => tr!(
                "{} x {} px, {} benzi",
                r.data.width,
                r.data.height,
                r.data.bands.len()
            ),
        }
    }

    /// Recalculează afișarea pentru sistemul hărții.
    pub fn rebuild(&mut self, map_crs: Option<&Crs>) {
        let t = match (self.crs(), map_crs) {
            (Some(a), Some(b)) => {
                Transformer::new(a, b).unwrap_or_else(|_| Transformer::identity())
            }
            _ => Transformer::identity(),
        };
        match &mut self.kind {
            LayerKind::Vector(v) => v.rebuild(&t),
            LayerKind::Raster(r) => {
                r.map_bounds = if t.is_identity() {
                    r.data.bounds()
                } else {
                    // margini densificate în spațiul pixelilor
                    let mut b = Bounds::empty();
                    let n = 32;
                    for k in 0..=n {
                        let s = k as f64 / n as f64;
                        let (w, h) = (r.data.width as f64, r.data.height as f64);
                        for (c, rr) in [(s * w, 0.0), (s * w, h), (0.0, s * h), (w, s * h)] {
                            let (x, y) = r.data.pixel_to_world(c, rr);
                            if let Some((tx, ty)) = t.xy(x, y) {
                                b.extend(tx, ty);
                            }
                        }
                    }
                    b
                };
                r.overview = None;
                r.detail = None;
                r.detail_key = None;
                r.dirty = true;
            }
        }
    }
}

impl VLayer {
    pub fn new(data: VectorLayer, idx: usize) -> VLayer {
        let mut style = VectorStyle::for_index(idx);
        if data.geom_kind() == GeomKind::Point {
            style.fill = [style.stroke[0], style.stroke[1], style.stroke[2], 200];
        }
        VLayer {
            data,
            style,
            disp: vec![],
            disp_bounds: Bounds::empty(),
            failed: 0,
            sel: Default::default(),
            sel_gen: 0,
        }
    }

    pub fn rebuild(&mut self, t: &Transformer) {
        let p = build_disp(&self.data, t);
        self.disp = p.disp;
        self.disp_bounds = p.bounds;
        self.failed = p.failed;
    }

    /// Folosește geometria de afișare pregătită în alt fir, dacă e pentru același sistem al hărții.
    pub fn install(&mut self, p: Prebuilt) {
        self.disp = p.disp;
        self.disp_bounds = p.bounds;
        self.failed = p.failed;
    }
}

/// Geometria de afișare calculată dinainte (reproiectare și triangulare), de exemplu în
/// firul care deschide fișierele în masă.
pub struct Prebuilt {
    pub map_crs: Option<Crs>,
    pub disp: Vec<DispGeom>,
    pub bounds: Bounds,
    pub failed: usize,
}

/// Reproiectează și triangulează entitățile unui strat pentru afișare.
pub fn build_disp(data: &VectorLayer, t: &Transformer) -> Prebuilt {
    let mut disp = vec![];
    let mut bounds = Bounds::empty();
    let mut failed = 0;
    for (i, f) in data.features.iter().enumerate() {
        let Some(g) = &f.geom else { continue };
        let mut g = g.clone();
        if !t.geometry(&mut g) {
            failed += 1;
        }
        let bbox = g.bounds();
        if !bbox.is_valid() {
            continue;
        }
        bounds.merge(&bbox);
        let (tri_verts, tri_idx) = triangulate(&g);
        let label_pt = g.label_point();
        disp.push(DispGeom {
            feat: i,
            bbox,
            geom: g,
            tri_verts,
            tri_idx,
            label_pt,
        });
    }
    Prebuilt {
        map_crs: None,
        disp,
        bounds,
        failed,
    }
}

/// Ca `LayerEntry::rebuild`, dar fără strat: pentru sistemul datelor și sistemul hărții date.
pub fn prebuild(data: &VectorLayer, map_crs: Option<&Crs>) -> Prebuilt {
    let t = match (data.crs.as_ref(), map_crs) {
        (Some(a), Some(b)) => Transformer::new(a, b).unwrap_or_else(|_| Transformer::identity()),
        _ => Transformer::identity(),
    };
    let mut p = build_disp(data, &t);
    p.map_crs = map_crs.cloned();
    p
}

/// Triangulează poligoanele (earcut) pentru umplere.
pub fn triangulate(g: &Geometry) -> (Vec<[f64; 2]>, Vec<u32>) {
    let mut verts: Vec<[f64; 2]> = vec![];
    let mut idx: Vec<u32> = vec![];
    for p in g.polygons() {
        let mut flat: Vec<f64> = vec![];
        let mut holes: Vec<usize> = vec![];
        let base = verts.len() as u32;
        for (ri, r) in p.iter().enumerate() {
            let mut pts: &[Coord] = r;
            if pts.len() > 3 && pts.first() == pts.last() {
                pts = &pts[..pts.len() - 1];
            }
            if pts.len() < 3 {
                if ri == 0 {
                    break;
                }
                continue;
            }
            if ri > 0 {
                holes.push(flat.len() / 2);
            }
            for c in pts {
                flat.push(c.x);
                flat.push(c.y);
                verts.push([c.x, c.y]);
            }
        }
        if flat.len() < 6 {
            continue;
        }
        if let Ok(tris) = earcutr::earcut(&flat, &holes, 2) {
            idx.extend(tris.into_iter().map(|t| base + t as u32));
        }
    }
    (verts, idx)
}

impl RLayer {
    pub fn new(data: Raster) -> RLayer {
        let style = RasterStyle::auto(&data);
        RLayer {
            data,
            style,
            map_bounds: Bounds::empty(),
            overview: None,
            detail: None,
            detail_key: None,
            dirty: true,
        }
    }

    /// Randează o textură pentru zona `view` (în sistemul hărții).
    pub fn render(
        &self,
        ctx: &egui::Context,
        map_crs: Option<&Crs>,
        view: &Bounds,
        w: usize,
        h: usize,
        name: &str,
    ) -> Option<TextureHandle> {
        if w == 0 || h == 0 || !view.is_valid() {
            return None;
        }
        let inv = match (map_crs, self.data.crs.as_ref()) {
            (Some(m), Some(s)) => Transformer::new(m, s).ok()?,
            _ => Transformer::identity(),
        };
        let rgba = render_rgba(&self.data, &self.style, &inv, view, w, h);
        let img = egui::ColorImage::from_rgba_unmultiplied([w, h], &rgba);
        Some(ctx.load_texture(name, img, egui::TextureOptions::LINEAR))
    }

    /// Imaginea de ansamblu (toată extinderea, max. 2048 px).
    pub fn ensure_overview(&mut self, ctx: &egui::Context, map_crs: Option<&Crs>, id: u64) {
        if self.overview.is_some() && !self.dirty {
            return;
        }
        let b = self.map_bounds;
        if !b.is_valid() || b.width() <= 0.0 || b.height() <= 0.0 {
            return;
        }
        let max = 2048.0f64.min(self.data.width.max(self.data.height) as f64);
        let (w, h) = if b.width() >= b.height() {
            (max, (max * b.height() / b.width()).max(1.0))
        } else {
            ((max * b.width() / b.height()).max(1.0), max)
        };
        self.overview = self
            .render(ctx, map_crs, &b, w as usize, h as usize, &format!("ov{id}"))
            .map(|t| (t, b));
        self.detail = None;
        self.detail_key = None;
        self.dirty = false;
    }
}
