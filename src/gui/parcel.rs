//! Editare → Parcelare / detașare: împărțirea unui poligon în loturi de
//! suprafețe date, cu previzualizare pe hartă și crearea unui strat nou.

use super::App;
use super::layers::*;
use cadastru::i18n::tr;
use cadastru::model::*;
use cadastru::parcel::{self, Lot, Method, Sizes};
use eframe::egui::{self, Color32, FontId, Pos2, RichText, Stroke, Vec2};
use geo::MultiPolygon;
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SizeMode {
    /// o singură suprafață, restul rămâne
    #[default]
    Detach,
    /// n loturi egale
    Equal,
    /// o listă de suprafețe
    List,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum CutMode {
    #[default]
    ParallelSide,
    PerpSide,
    Azimuth,
    Pivot,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Pick {
    #[default]
    None,
    Side,
    Vertex,
}

pub struct ParcelDlg {
    pub open: bool,
    /// stratul și entitatea parcelate
    pub src: Option<(u64, usize)>,
    pub src_name: String,
    /// originea coordonatelor locale (în sistemul hărții)
    origin: (f64, f64),
    poly: Option<MultiPolygon<f64>>,
    sides: Vec<((f64, f64), (f64, f64))>,
    ccw: bool,
    total: f64,
    pub size_mode: SizeMode,
    pub detach_area: f64,
    pub n_lots: usize,
    pub areas_text: String,
    pub rest: bool,
    pub cut: CutMode,
    pub side: usize,
    pub azimuth: f64,
    pub vertex: usize,
    pub reverse: bool,
    pub pick: Pick,
    result: Option<Result<Vec<Lot>, String>>,
    key: String,
    /// de ce nu se poate parcela (selecția nu e un singur poligon)
    sel_msg: String,
}

impl Default for ParcelDlg {
    fn default() -> Self {
        ParcelDlg {
            open: false,
            src: None,
            src_name: String::new(),
            origin: (0.0, 0.0),
            poly: None,
            sides: vec![],
            ccw: true,
            total: 0.0,
            size_mode: SizeMode::Detach,
            detach_area: 500.0,
            n_lots: 4,
            areas_text: String::new(),
            rest: true,
            cut: CutMode::ParallelSide,
            side: 0,
            azimuth: 0.0,
            vertex: 0,
            reverse: false,
            pick: Pick::None,
            result: None,
            key: String::new(),
            sel_msg: String::new(),
        }
    }
}

/// Culorile loturilor în previzualizare.
const PAL: [[u8; 3]; 8] = [
    [255, 179, 0],
    [0, 188, 212],
    [156, 204, 101],
    [240, 98, 146],
    [121, 134, 203],
    [255, 112, 67],
    [77, 182, 172],
    [186, 104, 200],
];

/// „500; 750,5\n600” → [500, 750.5, 600] (virgula e separator zecimal).
pub fn parse_areas(s: &str) -> Result<Vec<f64>, String> {
    s.split(|c: char| c == ';' || c == '\n' || c == '\r' || c == '\t' || c == ' ')
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(|t| {
            t.replace(',', ".")
                .parse::<f64>()
                .map_err(|_| tr!("„{}” nu este un număr.", t))
        })
        .collect()
}

impl ParcelDlg {
    fn method(&self) -> Option<Method> {
        let n = self.sides.len();
        if n == 0 {
            return None;
        }
        let (a, b) = self.sides[self.side.min(n - 1)];
        let len = (b.0 - a.0).hypot(b.1 - a.1).max(1e-12);
        let d = ((b.0 - a.0) / len, (b.1 - a.1) / len);
        let ang = d.1.atan2(d.0);
        // normala spre interior: la stânga laturii pentru un inel trigonometric
        let inward = if self.ccw { (-d.1, d.0) } else { (d.1, -d.0) };
        let flip = |v: (f64, f64)| if self.reverse { (-v.0, -v.1) } else { v };
        Some(match self.cut {
            CutMode::ParallelSide => Method::Parallel { angle: ang, normal: flip(inward) },
            CutMode::PerpSide => Method::Parallel { angle: ang + std::f64::consts::FRAC_PI_2, normal: flip(d) },
            CutMode::Azimuth => {
                let a = parcel::angle_from_azimuth(self.azimuth);
                // normala la dreapta direcției: pentru linii N–S primul lot e la vest
                Method::Parallel { angle: a, normal: flip((a.sin(), -a.cos())) }
            }
            CutMode::Pivot => {
                let v = self.vertex.min(n - 1);
                let p = self.sides[v].0;
                if !self.reverse {
                    let q = self.sides[v].1;
                    Method::Pivot { pivot: p, angle0: (q.1 - p.1).atan2(q.0 - p.0), ccw: self.ccw }
                } else {
                    let q = self.sides[(v + n - 1) % n].0;
                    Method::Pivot { pivot: p, angle0: (q.1 - p.1).atan2(q.0 - p.0), ccw: !self.ccw }
                }
            }
        })
    }

    fn sizes(&self) -> Result<Sizes, String> {
        Ok(match self.size_mode {
            SizeMode::Detach => Sizes::Areas { areas: vec![self.detach_area], rest: true },
            SizeMode::Equal => Sizes::Equal(self.n_lots),
            SizeMode::List => Sizes::Areas { areas: parse_areas(&self.areas_text)?, rest: self.rest },
        })
    }

    fn params_key(&self) -> String {
        format!(
            "{:?}|{:?}|{}|{}|{}|{}|{:?}|{}|{}|{}|{}",
            self.src, self.size_mode, self.detach_area, self.n_lots, self.areas_text, self.rest,
            self.cut, self.side, self.azimuth, self.vertex, self.reverse
        )
    }

    /// Recalculează loturile dacă s-au schimbat parametrii.
    fn recompute(&mut self) {
        let k = self.params_key();
        if k == self.key {
            return;
        }
        self.key = k;
        let Some(poly) = &self.poly else {
            self.result = None;
            return;
        };
        let r = match (self.method(), self.sizes()) {
            (Some(m), Ok(s)) => parcel::divide(poly, m, &s).map_err(|e| e.to_string()),
            (_, Err(e)) => Err(e),
            (None, _) => Err(tr!("Poligonul nu are laturi.").into()),
        };
        self.result = Some(r);
    }

    fn lots(&self) -> Option<&Vec<Lot>> {
        self.result.as_ref().and_then(|r| r.as_ref().ok())
    }
}

impl App {
    /// Poligonul pe care se face parcelarea: strict entitatea selectată în
    /// prealabil, una singură, de tip poligon, cu harta într-un sistem proiectat.
    pub fn parcel_selection(&self) -> Result<(u64, usize), String> {
        if self.map_crs.as_ref().is_none_or(|c| c.is_geographic()) {
            return Err(tr!("Parcelarea se face într-un sistem de coordonate proiectat (în metri). Schimbați sistemul hărții, de exemplu în Stereo 70 (EPSG:3844).").into());
        }
        let mut one = None;
        let mut count = 0;
        for l in &self.layers {
            if let LayerKind::Vector(v) = &l.kind {
                count += v.sel.len();
                if let Some(&fi) = v.sel.iter().next() {
                    one = Some((l.id, fi));
                }
            }
        }
        match (count, one) {
            (0, _) | (_, None) => Err(tr!("Selectați întâi poligonul de parcelat: instrumentul Selectare, tabelul de atribute sau selectarea după atribute.").into()),
            (1, Some((lid, fi))) => {
                let is_poly = self.layer(lid).is_some_and(|l| match &l.kind {
                    LayerKind::Vector(v) => v
                        .data
                        .features
                        .get(fi)
                        .and_then(|f| f.geom.as_ref())
                        .is_some_and(|g| !g.polygons().is_empty()),
                    _ => false,
                });
                if is_poly {
                    Ok((lid, fi))
                } else {
                    Err(tr!("Entitatea selectată nu este un poligon.").into())
                }
            }
            (n, _) => Err(tr!("Sunt selectate {} entități: parcelarea se face pe un singur poligon selectat.", n)),
        }
    }

    /// Editare → Parcelare / detașare: doar pe poligonul selectat în prealabil.
    pub fn open_parcel(&mut self) {
        match self.parcel_selection() {
            Ok((lid, fi)) => {
                if self.parcel_set_source(lid, fi) {
                    self.dlg.parcel.open = true;
                }
            }
            Err(e) => self.status = e,
        }
    }

    /// Ține fereastra legată de selecție: alt poligon selectat îl înlocuiește,
    /// iar fără un singur poligon selectat loturile dispar.
    fn parcel_follow_selection(&mut self) {
        match self.parcel_selection() {
            Ok(s) => {
                self.dlg.parcel.sel_msg.clear();
                if self.dlg.parcel.src != Some(s) {
                    self.parcel_set_source(s.0, s.1);
                }
            }
            Err(e) => {
                let p = &mut self.dlg.parcel;
                p.sel_msg = e;
                p.poly = None;
                p.src = None;
                p.sides.clear();
                p.result = None;
                p.key.clear();
                p.pick = Pick::None;
            }
        }
    }

    /// Alege poligonul de parcelat (geometria lui în sistemul hărții).
    fn parcel_set_source(&mut self, lid: u64, fi: usize) -> bool {
        let Some(l) = self.layer(lid) else { return false };
        let LayerKind::Vector(v) = &l.kind else { return false };
        let Some(d) = v.disp.iter().find(|d| d.feat == fi) else { return false };
        if d.geom.polygons().is_empty() {
            self.status = tr!("Entitatea aleasă nu este un poligon.").into();
            return false;
        }
        let b = d.bbox;
        let origin = (((b.minx + b.maxx) / 2.0).round(), ((b.miny + b.maxy) / 2.0).round());
        let Some(poly) = parcel::to_geo(&d.geom, origin) else { return false };
        let name = format!("{} #{}", l.name, fi + 1);
        let p = &mut self.dlg.parcel;
        p.sides = parcel::sides(&poly);
        p.ccw = parcel::is_ccw(&poly);
        p.total = parcel::area(&poly);
        p.poly = Some(poly);
        p.origin = origin;
        p.src = Some((lid, fi));
        p.src_name = name;
        p.side = 0;
        p.vertex = 0;
        p.key.clear();
        p.pick = Pick::None;
        if p.detach_area >= p.total {
            p.detach_area = (p.total / 2.0).round().max(1.0);
        }
        self.status = tr!("Parcelare: {} – {:.2} mp.", self.dlg.parcel.src_name, self.dlg.parcel.total);
        true
    }

    /// Click pe hartă în timpul alegerii (poligon, latură, vârf). Întoarce true dacă l-a folosit.
    pub fn parcel_click(&mut self, x: f64, y: f64) -> bool {
        let pick = self.dlg.parcel.pick;
        if !self.dlg.parcel.open || pick == Pick::None {
            return false;
        }
        match pick {
            Pick::Side | Pick::Vertex => {
                let p = &mut self.dlg.parcel;
                let (lx, ly) = (x - p.origin.0, y - p.origin.1);
                if pick == Pick::Side {
                    let best = p
                        .sides
                        .iter()
                        .enumerate()
                        .map(|(i, (a, b))| (i, seg_dist((lx, ly), *a, *b)))
                        .min_by(|a, b| a.1.total_cmp(&b.1));
                    if let Some((i, _)) = best {
                        p.side = i;
                    }
                } else {
                    let best = p
                        .sides
                        .iter()
                        .enumerate()
                        .map(|(i, (a, _))| (i, (a.0 - lx).hypot(a.1 - ly)))
                        .min_by(|a, b| a.1.total_cmp(&b.1));
                    if let Some((i, _)) = best {
                        p.vertex = i;
                    }
                }
                p.pick = Pick::None;
            }
            Pick::None => {}
        }
        true
    }

    /// Previzualizarea pe hartă: conturul, numerotarea vârfurilor, latura / vârful ales și loturile.
    pub fn draw_parcel(&self, painter: &egui::Painter) {
        let p = &self.dlg.parcel;
        if !p.open || p.poly.is_none() || p.src.is_some_and(|(lid, _)| self.layer(lid).is_none()) {
            return;
        }
        let o = p.origin;
        let s = |q: (f64, f64)| self.w2s(q.0 + o.0, q.1 + o.1);
        // loturile
        if let Some(lots) = p.lots() {
            for (i, lot) in lots.iter().enumerate() {
                let c = PAL[i % PAL.len()];
                let fill = Color32::from_rgba_unmultiplied(c[0], c[1], c[2], 90);
                let g = parcel::from_geo(&lot.geom, (0.0, 0.0));
                let mut shapes = vec![];
                for poly in g.polygons() {
                    // umplere prin triangulare (poligoane concave, goluri)
                    let mut flat = vec![];
                    let mut holes = vec![];
                    for (k, r) in poly.iter().enumerate() {
                        if k > 0 {
                            holes.push(flat.len() / 2);
                        }
                        for c in r.iter().take(r.len().saturating_sub(1)) {
                            flat.push(c.x);
                            flat.push(c.y);
                        }
                    }
                    if let Ok(tris) = earcutr::earcut(&flat, &holes, 2) {
                        let mut mesh = egui::Mesh::default();
                        for k in 0..flat.len() / 2 {
                            mesh.colored_vertex(s((flat[2 * k], flat[2 * k + 1])), fill);
                        }
                        mesh.indices = tris.into_iter().map(|t| t as u32).collect();
                        shapes.push(egui::Shape::mesh(mesh));
                    }
                    for r in poly {
                        let pts: Vec<Pos2> = r.iter().map(|c| s((c.x, c.y))).collect();
                        shapes.push(egui::Shape::line(pts, Stroke::new(1.6, Color32::from_rgb(c[0], c[1], c[2]))));
                    }
                }
                painter.extend(shapes);
            }
            for (i, lot) in lots.iter().enumerate() {
                use geo::InteriorPoint;
                if let Some(ip) = lot.geom.interior_point() {
                    let txt = format!("{} {}\n{:.2} mp", tr!("Lot"), i + 1, lot.area);
                    let galley = painter.layout(txt, FontId::proportional(12.0), Color32::BLACK, 200.0);
                    let c = s((ip.x(), ip.y()));
                    let r = egui::Rect::from_center_size(c, galley.size() + Vec2::new(8.0, 4.0));
                    painter.rect_filled(r, 3.0, Color32::from_white_alpha(225));
                    painter.galley(r.min + Vec2::new(4.0, 2.0), galley, Color32::BLACK);
                }
            }
        }
        // conturul și vârfurile
        let edge = Color32::from_rgb(0, 229, 255);
        for (a, b) in &p.sides {
            painter.line_segment([s(*a), s(*b)], Stroke::new(1.4, edge));
        }
        let n = p.sides.len();
        if n > 0 {
            let uses_side = matches!(p.cut, CutMode::ParallelSide | CutMode::PerpSide);
            if uses_side {
                let (a, b) = p.sides[p.side.min(n - 1)];
                painter.line_segment([s(a), s(b)], Stroke::new(4.0, Color32::from_rgb(255, 61, 0)));
                // săgeata sensului: de la începutul laturii
                painter.circle_filled(s(a), 5.0, Color32::from_rgb(255, 61, 0));
            }
            if p.cut == CutMode::Pivot {
                let v = p.sides[p.vertex.min(n - 1)].0;
                painter.circle(s(v), 7.0, Color32::from_rgb(255, 61, 0), Stroke::new(2.0, Color32::WHITE));
            }
            if n <= 200 {
                for (i, (a, _)) in p.sides.iter().enumerate() {
                    let q = s(*a);
                    painter.circle_filled(q, 2.5, edge);
                    let galley = painter.layout_no_wrap(format!("{}", i + 1), FontId::proportional(11.0), Color32::BLACK);
                    let r = egui::Rect::from_min_size(q + Vec2::new(4.0, -4.0 - galley.size().y - 2.0), galley.size() + Vec2::new(6.0, 2.0));
                    painter.rect_filled(r, 2.0, Color32::from_rgba_unmultiplied(0, 229, 255, 220));
                    painter.galley(r.min + Vec2::new(3.0, 1.0), galley, Color32::BLACK);
                }
            }
        }
    }

    pub fn parcel_window(&mut self, ctx: &egui::Context) {
        if !self.dlg.parcel.open {
            return;
        }
        self.parcel_follow_selection();
        self.dlg.parcel.recompute();
        let mut open = true;
        let mut create = false;
        let mut copy_inv = false;
        let mut zoom = false;
        egui::Window::new(tr!("Parcelare / detașare"))
            .id(egui::Id::unique("win_parcel"))
            .open(&mut open)
            .default_width(460.0)
            .resizable(true)
            .show(ctx, |ui| {
                let p = &mut self.dlg.parcel;
                // terenul: strict poligonul selectat în prealabil
                ui.horizontal(|ui| {
                    ui.label(tr!("Poligonul selectat:"));
                    if p.poly.is_some() {
                        ui.label(RichText::new(&p.src_name).strong());
                        ui.label(format!("{:.2} mp", p.total));
                        if ui.small_button(tr!("Zoom la teren")).clicked() {
                            zoom = true;
                        }
                    } else {
                        ui.label(RichText::new(tr!("(niciunul)")).weak());
                    }
                });
                if !p.sel_msg.is_empty() {
                    ui.label(RichText::new(&p.sel_msg).color(Color32::from_rgb(230, 140, 60)));
                }
                if p.poly.is_none() {
                    return;
                }
                ui.separator();
                // ce se împarte
                ui.horizontal(|ui| {
                    ui.label(tr!("Operațiune:"));
                    ui.radio_value(&mut p.size_mode, SizeMode::Detach, tr!("Detașare"));
                    ui.radio_value(&mut p.size_mode, SizeMode::Equal, tr!("Loturi egale"));
                    ui.radio_value(&mut p.size_mode, SizeMode::List, tr!("Suprafețe date"));
                });
                match p.size_mode {
                    SizeMode::Detach => {
                        ui.horizontal(|ui| {
                            ui.label(tr!("Suprafața de detașat:"));
                            ui.add(
                                egui::DragValue::new(&mut p.detach_area)
                                    .range(0.01..=p.total.max(0.01))
                                    .speed(1.0)
                                    .max_decimals(2)
                                    .suffix(" mp"),
                            );
                            ui.label(RichText::new(tr!("rest: {:.2} mp", (p.total - p.detach_area).max(0.0))).weak());
                        });
                    }
                    SizeMode::Equal => {
                        ui.horizontal(|ui| {
                            ui.label(tr!("Număr de loturi:"));
                            ui.add(egui::DragValue::new(&mut p.n_lots).range(2..=500));
                            ui.label(RichText::new(tr!("≈ {:.2} mp / lot", p.total / p.n_lots.max(1) as f64)).weak());
                        });
                    }
                    SizeMode::List => {
                        ui.label(tr!("Suprafețele loturilor, în ordine (mp), separate prin „;” sau pe rânduri:"));
                        ui.add(
                            egui::TextEdit::multiline(&mut p.areas_text)
                                .desired_rows(2)
                                .desired_width(f32::INFINITY)
                                .hint_text("500; 500; 750,5"),
                        );
                        ui.checkbox(&mut p.rest, tr!("Restul terenului rămâne lot separat"))
                            .on_hover_text(tr!("Debifat: restul se adaugă la ultimul lot"));
                    }
                }
                ui.separator();
                // cum se taie
                ui.label(tr!("Liniile de tăiere:"));
                ui.horizontal_wrapped(|ui| {
                    ui.radio_value(&mut p.cut, CutMode::ParallelSide, tr!("paralele cu o latură"));
                    ui.radio_value(&mut p.cut, CutMode::PerpSide, tr!("perpendiculare pe o latură"));
                    ui.radio_value(&mut p.cut, CutMode::Azimuth, tr!("după o orientare"));
                    ui.radio_value(&mut p.cut, CutMode::Pivot, tr!("printr-un vârf (evantai)"));
                });
                let n = p.sides.len();
                let side_label = |i: usize, sides: &[((f64, f64), (f64, f64))]| {
                    let (a, b) = sides[i];
                    format!(
                        "{} {}–{}  ·  {:.2} m  ·  {:.4}°",
                        tr!("Latura"),
                        i + 1,
                        (i + 1) % sides.len() + 1,
                        (b.0 - a.0).hypot(b.1 - a.1),
                        parcel::azimuth_deg(b.0 - a.0, b.1 - a.1)
                    )
                };
                match p.cut {
                    CutMode::ParallelSide | CutMode::PerpSide => {
                        ui.horizontal(|ui| {
                            egui::ComboBox::from_id_salt("parcel_side")
                                .selected_text(side_label(p.side.min(n - 1), &p.sides))
                                .width(280.0)
                                .show_ui(ui, |ui| {
                                    for i in 0..n {
                                        ui.selectable_value(&mut p.side, i, side_label(i, &p.sides));
                                    }
                                });
                            let picking = p.pick == Pick::Side;
                            if ui.selectable_label(picking, tr!("pe hartă")).on_hover_text(tr!("Click lângă latura dorită")).clicked() {
                                p.pick = if picking { Pick::None } else { Pick::Side };
                            }
                        });
                        ui.label(
                            RichText::new(if p.cut == CutMode::ParallelSide {
                                tr!("Primul lot se ia de lângă latura aleasă (roșie pe hartă).")
                            } else {
                                tr!("Primul lot se ia de la capătul laturii marcat cu punct roșu.")
                            })
                            .weak()
                            .small(),
                        );
                    }
                    CutMode::Azimuth => {
                        ui.horizontal(|ui| {
                            ui.label(tr!("Orientarea liniilor:"));
                            ui.add(
                                egui::DragValue::new(&mut p.azimuth)
                                    .range(0.0..=360.0)
                                    .speed(0.5)
                                    .max_decimals(4)
                                    .suffix("°"),
                            )
                            .on_hover_text(tr!("Unghiul față de Nord, în sens orar (0° = Nord, 90° = Est)"));
                        });
                    }
                    CutMode::Pivot => {
                        ui.horizontal(|ui| {
                            ui.label(tr!("Vârful:"));
                            egui::ComboBox::from_id_salt("parcel_vertex")
                                .selected_text(format!("{} {}", tr!("Punctul"), p.vertex.min(n - 1) + 1))
                                .show_ui(ui, |ui| {
                                    for i in 0..n {
                                        ui.selectable_value(&mut p.vertex, i, format!("{} {}", tr!("Punctul"), i + 1));
                                    }
                                });
                            let picking = p.pick == Pick::Vertex;
                            if ui.selectable_label(picking, tr!("pe hartă")).on_hover_text(tr!("Click lângă vârful dorit")).clicked() {
                                p.pick = if picking { Pick::None } else { Pick::Vertex };
                            }
                        });
                        ui.label(
                            RichText::new(tr!("Liniile pleacă din vârf; primul lot se ia lângă latura care urmează vârfului."))
                                .weak()
                                .small(),
                        );
                    }
                }
                ui.checkbox(&mut p.reverse, tr!("Sens invers (primul lot din partea opusă)"));
                ui.separator();
                // rezultatul
                match &p.result {
                    Some(Ok(lots)) => {
                        egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
                            egui::Grid::new("parcel_lots").striped(true).num_columns(5).show(ui, |ui| {
                                ui.strong(tr!("Lot"));
                                ui.strong(tr!("Suprafață (mp)"));
                                ui.strong(tr!("Cerută"));
                                ui.strong(tr!("Diferență"));
                                ui.strong(tr!("Perimetru (m)"));
                                ui.end_row();
                                for (i, l) in lots.iter().enumerate() {
                                    let c = PAL[i % PAL.len()];
                                    ui.label(RichText::new(format!("{}", i + 1)).strong().color(Color32::from_rgb(c[0], c[1], c[2])));
                                    ui.label(format!("{:.2}", l.area));
                                    match l.target {
                                        Some(t) => {
                                            ui.label(format!("{t:.2}"));
                                            ui.label(format!("{:+.4}", l.area - t));
                                        }
                                        None => {
                                            ui.label(tr!("rest"));
                                            ui.label("");
                                        }
                                    }
                                    let parts = l.geom.0.len();
                                    let per = format!("{:.2}", parcel::perimeter(&l.geom));
                                    if parts > 1 {
                                        ui.label(RichText::new(tr!("{} ({} părți)", per, parts)).color(Color32::from_rgb(230, 140, 60)))
                                            .on_hover_text(tr!("Linia de tăiere împarte lotul în mai multe bucăți (teren concav): alegeți altă direcție sau alt vârf."));
                                    } else {
                                        ui.label(per);
                                    }
                                    ui.end_row();
                                }
                            });
                        });
                        let sum: f64 = lots.iter().map(|l| l.area).sum();
                        ui.label(RichText::new(tr!("Total: {:.2} mp din {:.2} mp", sum, p.total)).weak());
                    }
                    Some(Err(e)) => {
                        ui.label(RichText::new(e).color(Color32::from_rgb(230, 110, 90)));
                    }
                    None => {}
                }
                ui.separator();
                ui.horizontal(|ui| {
                    let ok = p.lots().is_some();
                    if ui.add_enabled(ok, egui::Button::new(tr!("Creează stratul cu loturile"))).clicked() {
                        create = true;
                    }
                    if ui
                        .add_enabled(ok, egui::Button::new(tr!("Copiază inventarul de coordonate")))
                        .on_hover_text(tr!("Coordonatele vârfurilor fiecărui lot, ca text (se poate lipi în Excel)"))
                        .clicked()
                    {
                        copy_inv = true;
                    }
                });
            });
        if zoom && let Some(lid) = self.dlg.parcel.src.map(|s| s.0) {
            let fi = self.dlg.parcel.src.map(|s| s.1).unwrap_or(0);
            self.zoom_to_feature(lid, fi);
        }
        if copy_inv && let Some(t) = self.parcel_inventory() {
            ctx.copy_text(t);
            self.status = tr!("Inventarul de coordonate a fost copiat.").into();
        }
        if create {
            self.parcel_create_layer();
        }
        if !open {
            self.dlg.parcel.open = false;
            self.dlg.parcel.pick = Pick::None;
        }
    }

    /// Coordonatele vârfurilor loturilor, ca text cu tab-uri. În Stereo 70 / 30
    /// X este Nordul și Y Estul (convenția cadastrală).
    fn parcel_inventory(&self) -> Option<String> {
        let p = &self.dlg.parcel;
        let lots = p.lots()?;
        let north_first = self
            .map_crs
            .as_ref()
            .is_some_and(|c| matches!(c.epsg, Some(3844) | Some(31700) | Some(3843)) || c.short().contains("Stereo"));
        let mut out = String::new();
        out.push_str(&tr!("Inventar de coordonate – {} ({})\n", p.src_name, self.map_crs.as_ref().map(|c| c.short()).unwrap_or_default()));
        for (i, lot) in lots.iter().enumerate() {
            out.push_str(&format!("\n{} {}\t{:.2} mp\n", tr!("Lot"), i + 1, lot.area));
            out.push_str(if north_first { "Pct\tX (N)\tY (E)\n" } else { "Pct\tE\tN\n" });
            let g = parcel::from_geo(&lot.geom, p.origin);
            let mut k = 0;
            for poly in g.polygons() {
                for ring in poly {
                    for c in ring.iter().take(ring.len().saturating_sub(1)) {
                        k += 1;
                        let (a, b) = if north_first { (c.y, c.x) } else { (c.x, c.y) };
                        out.push_str(&format!("{k}\t{a:.3}\t{b:.3}\n"));
                    }
                }
            }
        }
        Some(out)
    }

    /// Stratul nou cu loturile (în sistemul hărții).
    fn parcel_create_layer(&mut self) {
        let p = &self.dlg.parcel;
        let Some(lots) = p.lots() else { return };
        let fields = vec![
            Field::new("LOT", FieldType::Integer),
            Field { precision: 2, ..Field::new("SUPRAFATA", FieldType::Real) },
            Field { precision: 2, ..Field::new("SUPR_CERUTA", FieldType::Real) },
            Field { precision: 2, ..Field::new("PERIMETRU", FieldType::Real) },
            Field::new("ETICHETA", FieldType::Text),
            Field::new("SURSA", FieldType::Text),
        ];
        let features = lots
            .iter()
            .enumerate()
            .map(|(i, l)| Feature {
                geom: Some(parcel::from_geo(&l.geom, p.origin)),
                attrs: vec![
                    Value::Int(i as i64 + 1),
                    Value::Real((l.area * 100.0).round() / 100.0),
                    l.target.map(|t| Value::Real((t * 100.0).round() / 100.0)).unwrap_or(Value::Null),
                    Value::Real((parcel::perimeter(&l.geom) * 100.0).round() / 100.0),
                    Value::Text(format!("Lot {} – {:.2} mp", i + 1, l.area)),
                    Value::Text(p.src_name.clone()),
                ],
            })
            .collect();
        let n_lots = lots.len();
        let name = tr!("Parcelare {}", p.src_name);
        let layer = VectorLayer {
            name: name.clone(),
            source: None,
            fields,
            features,
            crs: self.map_crs.clone(),
            has_z: false,
        };
        let seq = self.next_seq();
        let ids = self.add_datasets(Path::new(""), vec![Dataset::Vector(layer)], seq);
        if let Some(id) = ids.first().copied()
            && let Some(LayerKind::Vector(v)) = self.layer_mut(id).map(|l| &mut l.kind)
        {
            v.style.label_field = Some("ETICHETA".into());
            v.style.fill = [255, 179, 0, 70];
            v.style.stroke = [230, 81, 0, 255];
            v.style.width = 1.8;
        }
        self.status = tr!("Strat nou: {} ({} loturi). Exportați-l (de ex. în SHP sau DXF) ca să-l păstrați.", name, n_lots);
    }
}

/// Distanța de la punctul p la segmentul ab.
fn seg_dist(p: (f64, f64), a: (f64, f64), b: (f64, f64)) -> f64 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let l2 = dx * dx + dy * dy;
    let t = if l2 > 0.0 { (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / l2).clamp(0.0, 1.0) } else { 0.0 };
    (p.0 - a.0 - t * dx).hypot(p.1 - a.1 - t * dy)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_area_lists() {
        assert_eq!(parse_areas("500; 750,5\n600").unwrap(), vec![500.0, 750.5, 600.0]);
        assert_eq!(parse_areas(" 1000 ").unwrap(), vec![1000.0]);
        assert!(parse_areas("500; abc").is_err());
    }
}
