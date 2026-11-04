//! Desenarea hărții și interacțiunea cu mouse-ul.

use super::layers::*;
use super::theme::{self, Icon, paint_icon};
use super::select::SelMode;
use super::{App, IdentifyResult, Tool};
use cadastru::crs::Transformer;
use cadastru::model::*;
use eframe::egui::{
    self, Align2, Color32, FontId, Mesh, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, Vec2,
};
use cadastru::i18n::tr;

const SEL_COLOR: Color32 = Color32::from_rgb(0, 230, 255);

/// Distanța geodezică pe elipsoidul WGS84 (formula Vincenty, invers).
pub fn vincenty(lon1: f64, lat1: f64, lon2: f64, lat2: f64) -> f64 {
    let a = 6378137.0f64;
    let f = 1.0 / 298.257223563;
    let b = a * (1.0 - f);
    let l = (lon2 - lon1).to_radians();
    let u1 = ((1.0 - f) * lat1.to_radians().tan()).atan();
    let u2 = ((1.0 - f) * lat2.to_radians().tan()).atan();
    let (su1, cu1, su2, cu2) = (u1.sin(), u1.cos(), u2.sin(), u2.cos());
    let mut lambda = l;
    for _ in 0..200 {
        let (sl, cl) = (lambda.sin(), lambda.cos());
        let ss = ((cu2 * sl).powi(2) + (cu1 * su2 - su1 * cu2 * cl).powi(2)).sqrt();
        if ss == 0.0 {
            return 0.0;
        }
        let cs = su1 * su2 + cu1 * cu2 * cl;
        let sigma = ss.atan2(cs);
        let sa = cu1 * cu2 * sl / ss;
        let c2a = 1.0 - sa * sa;
        let c2sm = if c2a != 0.0 {
            cs - 2.0 * su1 * su2 / c2a
        } else {
            0.0
        };
        let c = f / 16.0 * c2a * (4.0 + f * (4.0 - 3.0 * c2a));
        let prev = lambda;
        lambda = l
            + (1.0 - c) * f * sa * (sigma + c * ss * (c2sm + c * cs * (-1.0 + 2.0 * c2sm * c2sm)));
        if (lambda - prev).abs() < 1e-12 {
            let u2b = c2a * (a * a - b * b) / (b * b);
            let aa = 1.0 + u2b / 16384.0 * (4096.0 + u2b * (-768.0 + u2b * (320.0 - 175.0 * u2b)));
            let bb = u2b / 1024.0 * (256.0 + u2b * (-128.0 + u2b * (74.0 - 47.0 * u2b)));
            let ds = bb
                * ss
                * (c2sm
                    + bb / 4.0
                        * (cs * (-1.0 + 2.0 * c2sm * c2sm)
                            - bb / 6.0
                                * c2sm
                                * (-3.0 + 4.0 * ss * ss)
                                * (-3.0 + 4.0 * c2sm * c2sm)));
            return b * aa * (sigma - ds);
        }
    }
    // nu converge (puncte antipodale) – aproximare sferică
    let r = 6371008.8;
    let (p1, p2) = (lat1.to_radians(), lat2.to_radians());
    let h = ((p2 - p1) / 2.0).sin().powi(2) + p1.cos() * p2.cos() * (l / 2.0).sin().powi(2);
    2.0 * r * h.sqrt().asin()
}

/// Adaugă conturul unei polilinii (sau al unui inel închis) ca forme egui.
///
/// egui unește segmentele prin îmbinări în colț ascuțit (miter) fără limită: la o întoarcere
/// de aproape 180° colțul devine o linie lungă cât ecranul, iar trei puncte identice la rând
/// dau NaN. Datele detaliate au multe asemenea zigzaguri sub un pixel. De aceea: (1) punctele
/// mai apropiate de 0,5 px de precedentul sunt omise, (2) punctele invalide (NaN) întrerup
/// linia, (3) linia e despărțită în vârfurile cu unghi foarte ascuțit, unde capetele rămân drepte.
pub(crate) fn push_stroke(
    out: &mut Vec<Shape>,
    pts: impl Iterator<Item = Pos2>,
    closed: bool,
    stroke: Stroke,
) {
    const MIN_D2: f32 = 0.25; // (0,5 px)²
    // întoarcere mai ascuțită de ~120° între direcții: colțul ar depăși de 4 ori grosimea
    const SHARP_COS: f32 = -0.5;
    let mut runs: Vec<Vec<Pos2>> = vec![];
    let mut cur: Vec<Pos2> = vec![];
    let mut broken = false;
    for p in pts {
        if !(p.x.is_finite() && p.y.is_finite()) || p.x.abs() > 1e7 || p.y.abs() > 1e7 {
            if cur.len() >= 2 {
                runs.push(std::mem::take(&mut cur));
            } else {
                cur.clear();
            }
            broken = true;
            continue;
        }
        if let Some(&q) = cur.last()
            && (p - q).length_sq() < MIN_D2
        {
            continue;
        }
        cur.push(p);
    }
    if cur.len() >= 2 {
        runs.push(cur);
    }
    if runs.is_empty() {
        return;
    }
    let closed = closed && !broken && runs.len() == 1;
    let dir = |a: Pos2, b: Pos2| (b - a).normalized();
    for mut run in runs {
        if closed {
            // ultimul punct identic (sau aproape) cu primul: inelul se închide singur
            while run.len() > 2 && (run[run.len() - 1] - run[0]).length_sq() < MIN_D2 {
                run.pop();
            }
            if run.len() < 3 {
                out.push(Shape::line(run, stroke));
                continue;
            }
            let n = run.len();
            let sharp: Vec<usize> = (0..n)
                .filter(|&i| {
                    let a = run[(i + n - 1) % n];
                    let b = run[i];
                    let c = run[(i + 1) % n];
                    dir(a, b).dot(dir(b, c)) < SHARP_COS
                })
                .collect();
            if sharp.is_empty() {
                out.push(Shape::closed_line(run, stroke));
                continue;
            }
            // despărțit în vârfurile ascuțite: piese deschise de la un vârf la următorul
            for (k, &s0) in sharp.iter().enumerate() {
                let s1 = sharp[(k + 1) % sharp.len()];
                let mut piece = vec![run[s0]];
                let mut i = s0;
                loop {
                    i = (i + 1) % n;
                    piece.push(run[i]);
                    if i == s1 {
                        break;
                    }
                }
                out.push(Shape::line(piece, stroke));
            }
        } else {
            let mut piece = vec![run[0]];
            for i in 1..run.len() {
                piece.push(run[i]);
                if i + 1 < run.len() && dir(run[i - 1], run[i]).dot(dir(run[i], run[i + 1])) < SHARP_COS {
                    out.push(Shape::line(std::mem::replace(&mut piece, vec![run[i]]), stroke));
                }
            }
            if piece.len() >= 2 {
                out.push(Shape::line(piece, stroke));
            }
        }
    }
}

pub(crate) fn nice_number(x: f64) -> f64 {
    let e = x.log10().floor();
    let f = x / 10f64.powf(e);
    let n = if f < 1.5 {
        1.0
    } else if f < 3.5 {
        2.0
    } else if f < 7.5 {
        5.0
    } else {
        10.0
    };
    n * 10f64.powf(e)
}

pub fn fmt_len(m: f64) -> String {
    if m >= 1000.0 {
        format!("{:.3} km", m / 1000.0)
    } else {
        format!("{:.2} m", m)
    }
}

pub fn fmt_area(m2: f64) -> String {
    if m2 >= 1_000_000.0 {
        format!("{:.4} km² ({:.2} ha)", m2 / 1e6, m2 / 1e4)
    } else if m2 >= 10_000.0 {
        format!("{:.2} m² ({:.4} ha)", m2, m2 / 1e4)
    } else {
        format!("{:.2} m²", m2)
    }
}

impl App {
    #[inline]
    pub fn w2s(&self, x: f64, y: f64) -> Pos2 {
        let c = self.map_rect.center();
        Pos2::new(
            c.x + ((x - self.view.cx) / self.view.scale) as f32,
            c.y - ((y - self.view.cy) / self.view.scale) as f32,
        )
    }

    #[inline]
    pub fn s2w(&self, p: Pos2) -> (f64, f64) {
        let c = self.map_rect.center();
        (
            self.view.cx + (p.x - c.x) as f64 * self.view.scale,
            self.view.cy - (p.y - c.y) as f64 * self.view.scale,
        )
    }

    pub fn view_bounds(&self) -> Bounds {
        let r = self.map_rect;
        let (x0, y1) = self.s2w(r.min);
        let (x1, y0) = self.s2w(r.max);
        Bounds::new(x0, y0, x1, y1)
    }

    pub fn zoom_to(&mut self, b: Bounds) {
        if !b.is_valid() {
            return;
        }
        let (w, h) = (
            self.map_rect.width().max(200.0) as f64,
            self.map_rect.height().max(200.0) as f64,
        );
        let (cx, cy) = b.center();
        let mut s = (b.width() / w).max(b.height() / h) * 1.18;
        if s <= 0.0 || !s.is_finite() {
            s = if self.map_crs.as_ref().is_some_and(|c| c.is_geographic()) {
                0.00001
            } else {
                1.0
            };
        }
        self.view.cx = cx;
        self.view.cy = cy;
        self.view.scale = s;
        self.view_initialized = self.map_rect.width() > 0.0;
    }

    /// Metri pe unitate de hartă în centrul vederii (măsurat geodezic, ține cont de deformarea proiecției).
    pub(crate) fn meters_per_unit(&self) -> f64 {
        let geo = self.map_crs.as_ref().is_some_and(|c| c.is_geographic());
        if let Some(t) = &self.to_wgs {
            let d = self.view.scale * 50.0;
            let (x0, y0) = (self.view.cx - d, self.view.cy);
            let (x1, y1) = (self.view.cx + d, self.view.cy);
            if let (Some(a), Some(b)) = (t.xy(x0, y0), t.xy(x1, y1)) {
                let m = vincenty(a.0, a.1, b.0, b.1);
                if m.is_finite() && m > 0.0 {
                    return m / (2.0 * d);
                }
            }
        } else if geo {
            return 111_320.0 * self.view.cy.to_radians().cos().abs().max(0.01);
        }
        if geo { 111_320.0 } else { 1.0 }
    }

    pub fn scale_denominator(&self) -> f64 {
        // 96 dpi
        self.view.scale * self.meters_per_unit() / 0.0254 * 96.0
    }

    fn measure_text(&self) -> Option<String> {
        if self.measure.len() < 2 {
            return None;
        }
        let geo = self.map_crs.as_ref().is_some_and(|c| c.is_geographic());
        let pts: Vec<Coord> = self
            .measure
            .iter()
            .map(|&(x, y)| Coord::new(x, y))
            .collect();
        let mut s = String::new();
        let planar = path_length(&pts);
        // geodezic
        let mut geod = 0.0;
        let mut ok = true;
        let wgs: Vec<Option<(f64, f64)>> = self
            .measure
            .iter()
            .map(|&(x, y)| {
                if geo {
                    Some((x, y))
                } else {
                    self.to_wgs.as_ref().and_then(|t| t.xy(x, y))
                }
            })
            .collect();
        for w in wgs.windows(2) {
            match (w[0], w[1]) {
                (Some(a), Some(b)) => geod += vincenty(a.0, a.1, b.0, b.1),
                _ => ok = false,
            }
        }
        if !geo {
            s.push_str(&tr!("Distanță (plan): {}\n", fmt_len(planar)));
        }
        if ok && self.to_wgs.is_some() || geo {
            s.push_str(&tr!("Distanță (elipsoid): {}\n", fmt_len(geod)));
        }
        if let Some(&(lx, ly)) = self.measure.last()
            && self.measure.len() >= 2
        {
            let (px, py) = self.measure[self.measure.len() - 2];
            let seg = if geo {
                wgs.last()
                    .copied()
                    .flatten()
                    .zip(wgs[wgs.len() - 2])
                    .map(|(b, a)| vincenty(a.0, a.1, b.0, b.1))
                    .unwrap_or(0.0)
            } else {
                ((lx - px).powi(2) + (ly - py).powi(2)).sqrt()
            };
            s.push_str(&tr!("Ultimul segment: {}\n", fmt_len(seg)));
        }
        if self.measure.len() >= 3 {
            let ring: Vec<Coord> = pts.clone();
            let a = ring_signed_area(&ring).abs();
            if geo {
                // arie aproximativă: proiecție locală echivalentă
                let lat0 =
                    self.measure.iter().map(|p| p.1).sum::<f64>() / self.measure.len() as f64;
                let k = 111_320.0f64;
                let a_m = a * k * k * lat0.to_radians().cos();
                s.push_str(&tr!("Arie (aprox.): {}\n", fmt_area(a_m)));
            } else {
                s.push_str(&tr!("Arie (plan): {}\n", fmt_area(a)));
                s.push_str(&tr!(
                    "Perimetru: {}\n",
                    fmt_len(
                        planar
                            + ((pts[0].x - pts[pts.len() - 1].x).powi(2)
                                + (pts[0].y - pts[pts.len() - 1].y).powi(2))
                            .sqrt()
                    )
                ));
            }
        }
        Some(s.trim_end().to_string())
    }

    fn identify_at(&mut self, x: f64, y: f64) {
        let tol = self.view.scale * 5.0;
        let mut results = vec![];
        let mut sel: Option<(u64, usize)> = None;
        for l in self.layers.iter().rev().filter(|l| l.visible) {
            match &l.kind {
                LayerKind::Vector(v) => {
                    if sel.is_some() {
                        continue;
                    }
                    let mut best: Option<(f64, usize, u8)> = None;
                    for d in &v.disp {
                        if !d.bbox.expanded(tol).contains(x, y) {
                            continue;
                        }
                        let dist = d.geom.distance_to(x, y);
                        if dist <= tol {
                            // prioritate: puncte, linii, apoi poligoane
                            let pr = match d.geom.kind() {
                                GeomKind::Point => 0,
                                GeomKind::Line => 1,
                                _ => 2,
                            };
                            if best.is_none_or(|(bd, _, bp)| (pr, dist) < (bp, bd)) {
                                best = Some((dist, d.feat, pr));
                            }
                        }
                    }
                    if let Some((_, fi, _)) = best {
                        let f = &v.data.features[fi];
                        let mut rows: Vec<(String, String)> = v
                            .data
                            .fields
                            .iter()
                            .zip(&f.attrs)
                            .map(|(fd, val)| (fd.name.clone(), val.to_text()))
                            .collect();
                        if let Some(g) = &f.geom {
                            rows.push((
                                tr!("‹geometrie›").into(),
                                format!(
                                    "{}{}",
                                    g.type_name(),
                                    if v.data.has_z { " Z" } else { "" }
                                ),
                            ));
                            rows.push((tr!("‹vârfuri›").into(), g.num_coords().to_string()));
                            let projected = v.data.crs.as_ref().is_some_and(|c| !c.is_geographic());
                            // strat în grade (ex. WGS84) pe o hartă proiectată: măsurăm în sistemul hărții
                            let map_proj = self.map_crs.as_ref().filter(|c| !c.is_geographic());
                            let measured = if projected {
                                Some((g, String::new()))
                            } else if let Some(mc) = map_proj {
                                v.disp
                                    .iter()
                                    .find(|d| d.feat == fi)
                                    .map(|d| (&d.geom, tr!(" (în {})", mc.short())))
                            } else {
                                None
                            };
                            if let Some((mg, note)) = measured
                                && matches!(mg.kind(), GeomKind::Line | GeomKind::Polygon)
                            {
                                match mg.kind() {
                                    GeomKind::Line => rows.push((
                                        tr!("‹lungime›").into(),
                                        format!("{}{note}", fmt_len(mg.length())),
                                    )),
                                    _ => {
                                        rows.push((tr!("‹arie›").into(), format!("{}{note}", fmt_area(mg.area()))));
                                        rows.push((
                                            tr!("‹perimetru›").into(),
                                            format!("{}{note}", fmt_len(mg.length())),
                                        ));
                                    }
                                }
                            }
                            if projected {
                                match g.kind() {
                                    GeomKind::Line | GeomKind::Polygon => {}
                                    _ => {
                                        if let Some(c) = g.points().first() {
                                            rows.push((
                                                "‹X, Y›".into(),
                                                format!("{:.3}, {:.3}", c.x, c.y),
                                            ));
                                        }
                                    }
                                }
                            }
                        }
                        results.push(IdentifyResult {
                            layer_id: l.id,
                            layer: l.name.clone(),
                            feat: Some(fi),
                            rows,
                        });
                        sel = Some((l.id, fi));
                    }
                }
                LayerKind::Raster(r) => {
                    let t = match (&self.map_crs, &r.data.crs) {
                        (Some(m), Some(s)) => Transformer::new(m, s).ok(),
                        _ => Some(Transformer::identity()),
                    };
                    let Some((sx, sy)) = t.and_then(|t| t.xy(x, y)) else {
                        continue;
                    };
                    let Some((col, row, vals)) = r.values_at(sx, sy) else { continue };
                    let mut rows = vec![(
                        "‹pixel›".to_string(),
                        tr!("coloana {}, rândul {}", col, row),
                    )];
                    for (b, v) in vals.into_iter().enumerate() {
                        rows.push((
                            tr!("Banda {}", b + 1),
                            if r.data.is_nodata(v) {
                                "NoData".into()
                            } else {
                                fmt_real(v)
                            },
                        ));
                    }
                    results.push(IdentifyResult {
                        layer_id: l.id,
                        layer: l.name.clone(),
                        feat: None,
                        rows,
                    });
                }
            }
        }
        if results.is_empty() {
            self.status = tr!("Nicio entitate la poziția indicată.").into();
        }
        self.selection = sel;
        self.identify = results;
        self.show_identify = !self.identify.is_empty();
        if let Some((lid, fi)) = sel {
            self.dlg.table.layer_id = Some(lid);
            self.dlg.table.scroll_to = Some(fi);
        }
    }

    fn draw_vector(&self, painter: &egui::Painter, l: &LayerEntry, v: &VLayer, vb: &Bounds) {
        let alpha = l.opacity;
        let st = &v.style;
        let mul = |c: Color32| c.gamma_multiply(alpha);
        let stroke = Stroke::new(st.width, mul(st.stroke_c()));
        let fill = mul(st.fill_c());
        // umplerea poligoanelor („No fill” = doar contur); punctele își păstrează umplerea
        let pfill = if st.no_fill { Color32::TRANSPARENT } else { fill };
        let mut mesh = Mesh::default();
        let mut shapes: Vec<Shape> = vec![];
        let tiny = self.view.scale * 1.5;
        let mut drawn = 0usize;
        let mut labels: Vec<(Pos2, String)> = vec![];
        let label_idx = st.label_field.as_ref().and_then(|n| v.data.field_index(n));
        for d in &v.disp {
            if !d.bbox.intersects(vb) {
                continue;
            }
            drawn += 1;
            if d.bbox.width() < tiny && d.bbox.height() < tiny && d.geom.kind() != GeomKind::Point {
                // prea mic: un singur pixel
                let p = self.w2s(d.bbox.minx, d.bbox.maxy);
                shapes.push(Shape::rect_filled(
                    Rect::from_min_size(p, Vec2::splat(1.5)),
                    0.0,
                    if d.geom.kind() == GeomKind::Polygon && pfill.a() > 0 {
                        pfill
                    } else {
                        stroke.color
                    },
                ));
                continue;
            }
            // umplere
            if pfill.a() > 0 && !d.tri_idx.is_empty() {
                let base = mesh.vertices.len() as u32;
                for v in &d.tri_verts {
                    mesh.colored_vertex(self.w2s(v[0], v[1]), pfill);
                }
                mesh.indices.extend(d.tri_idx.iter().map(|i| base + i));
            }
            if st.width > 0.0 {
                for line in d.geom.lines() {
                    push_stroke(&mut shapes, line.iter().map(|c| self.w2s(c.x, c.y)), false, stroke);
                }
                for poly in d.geom.polygons() {
                    for ring in poly {
                        push_stroke(&mut shapes, ring.iter().map(|c| self.w2s(c.x, c.y)), true, stroke);
                    }
                }
            }
            for p in d.geom.points() {
                let sp = self.w2s(p.x, p.y);
                shapes.push(Shape::circle_filled(
                    sp,
                    st.point_size,
                    fill.gamma_multiply(1.0).to_opaque().gamma_multiply(alpha),
                ));
                if st.width > 0.0 {
                    shapes.push(Shape::circle_stroke(
                        sp,
                        st.point_size,
                        Stroke::new(st.width.min(2.0), mul(st.stroke_c())),
                    ));
                }
            }
            if self.show_labels
                && let Some(li) = label_idx
                && let Some(lp) = d.label_pt
            {
                let t = v.data.features[d.feat].attrs[li].to_text();
                if !t.is_empty() {
                    labels.push((self.w2s(lp.x, lp.y), t));
                }
            }
        }
        if !mesh.is_empty() {
            painter.add(Shape::mesh(mesh));
        }
        painter.extend(shapes);
        if !labels.is_empty() && labels.len() <= 3000 {
            let font = FontId::proportional(st.label_size);
            let mut placed: Vec<Rect> = vec![];
            for (p, t) in labels {
                let galley = painter.layout_no_wrap(t, font.clone(), st.label_c());
                let off = if v.data.geom_kind() == GeomKind::Point {
                    Vec2::new(st.point_size + 2.0, -galley.size().y / 2.0)
                } else {
                    -galley.size() / 2.0
                };
                let r = Rect::from_min_size(p + off, galley.size());
                let hw = if st.halo == HaloMode::Off { 0.0 } else { st.halo_width.max(0.0) };
                if placed.iter().any(|q| q.intersects(r.expand(hw))) {
                    continue;
                }
                draw_label_halo(painter, st, r, &galley);
                painter.galley(r.min, galley, st.label_c());
                placed.push(r);
            }
        }
        let _ = drawn;
    }

    fn draw_selection(&self, painter: &egui::Painter) {
        let Some((lid, fi)) = self.selection else {
            return;
        };
        let Some(l) = self.layer(lid) else { return };
        let LayerKind::Vector(v) = &l.kind else {
            return;
        };
        let Some(d) = v.disp.iter().find(|d| d.feat == fi) else {
            return;
        };
        let stroke = Stroke::new(3.0, SEL_COLOR);
        let mut shapes = vec![];
        for line in d.geom.lines() {
            push_stroke(&mut shapes, line.iter().map(|c| self.w2s(c.x, c.y)), false, stroke);
        }
        for poly in d.geom.polygons() {
            for ring in poly {
                push_stroke(&mut shapes, ring.iter().map(|c| self.w2s(c.x, c.y)), true, stroke);
            }
        }
        painter.extend(shapes);
        for p in d.geom.points() {
            painter.circle_stroke(self.w2s(p.x, p.y), v.style.point_size + 4.0, stroke);
        }
    }

    fn draw_scale_bar(&self, painter: &egui::Painter, rect: Rect) {
        let mpu = self.meters_per_unit();
        let target_px = 120.0;
        let meters = nice_number(target_px as f64 * self.view.scale * mpu);
        let px = (meters / mpu / self.view.scale) as f32;
        if !px.is_finite() || px <= 0.0 {
            return;
        }
        let pal = theme::palette(self.settings.theme);
        let o = Pos2::new(rect.left() + 16.0, rect.bottom() - 22.0);
        let bg = Rect::from_min_max(o - Vec2::new(6.0, 18.0), o + Vec2::new(px + 6.0, 10.0));
        painter.rect_filled(bg, 3.0, pal.overlay_bg);
        let s = Stroke::new(2.0, pal.map_fg);
        painter.line_segment([o, o + Vec2::new(px, 0.0)], s);
        painter.line_segment([o, o - Vec2::new(0.0, 6.0)], s);
        painter.line_segment([o + Vec2::new(px, 0.0), o + Vec2::new(px, -6.0)], s);
        painter.text(
            o + Vec2::new(px / 2.0, -4.0),
            Align2::CENTER_BOTTOM,
            fmt_len(meters).replace(".00 ", " ").replace(".000 ", " "),
            FontId::proportional(11.0),
            pal.map_fg,
        );
    }

    /// Controlul de navigație desenat peste hartă: deplasare în 4 direcții, extindere completă, zoom +/−.
    /// Întoarce true dacă vederea s-a modificat.
    fn nav_controls(&mut self, ui: &mut egui::Ui, rect: Rect, now: f64) -> bool {
        let pal = theme::palette(self.settings.theme);
        let painter = ui.painter_at(rect);
        let r = 34.0f32;
        let c = rect.left_top() + Vec2::new(14.0 + r, 14.0 + r);
        let mut changed = false;
        // --- pad de deplasare
        let pad_rect = Rect::from_center_size(c, Vec2::splat(2.0 * r));
        let pad = ui.interact(
            pad_rect,
            ui.scope_id().with("nav_pad"),
            Sense::click_and_drag(),
        );
        let region = |p: Pos2| -> Option<Vec2> {
            let d = p - c;
            if d.length() > r + 1.0 {
                return None;
            }
            if d.length() < 12.0 {
                return Some(Vec2::ZERO);
            }
            Some(if d.x.abs() > d.y.abs() {
                Vec2::new(d.x.signum(), 0.0)
            } else {
                Vec2::new(0.0, -d.y.signum())
            })
        };
        let hov = pad.hover_pos().and_then(region);
        painter.circle_filled(c, r, pal.overlay_bg);
        painter.circle_stroke(c, r, Stroke::new(1.0, pal.overlay_border));
        for d in [
            Vec2::new(0.0, 1.0),
            Vec2::new(0.0, -1.0),
            Vec2::new(-1.0, 0.0),
            Vec2::new(1.0, 0.0),
        ] {
            let sd = Vec2::new(d.x, -d.y); // direcție pe ecran
            let tip = c + sd * (r - 7.0);
            let base = c + sd * (r - 17.0);
            let n = Vec2::new(-sd.y, sd.x) * 7.0;
            let active = hov == Some(d);
            if active {
                painter.circle_filled(c + sd * (r - 12.0), 10.5, pal.overlay_hover);
            }
            let col = if active { pal.accent } else { pal.overlay_icon };
            painter.add(Shape::convex_polygon(
                vec![tip, base + n, base - n],
                col,
                Stroke::NONE,
            ));
        }
        let home_active = hov == Some(Vec2::ZERO);
        painter.circle_filled(
            c,
            11.0,
            if home_active {
                pal.overlay_hover
            } else {
                pal.overlay_bg
            },
        );
        painter.circle_stroke(c, 11.0, Stroke::new(1.0, pal.overlay_border));
        paint_icon(
            &painter,
            Rect::from_center_size(c, Vec2::splat(15.0)),
            Icon::FullExtent,
            if home_active {
                pal.accent
            } else {
                pal.overlay_icon
            },
        );
        let pad =
            pad.on_hover_text(tr!("Deplasare (click sau ține apăsat) · centru = extindere completă"));
        let press_dir = if pad.is_pointer_button_down_on() {
            pad.interact_pointer_pos().and_then(region)
        } else {
            None
        };
        match (press_dir, self.nav_press) {
            (Some(d), None) => self.nav_press = Some((d, now)),
            (Some(d), Some((pd, t0))) if d == pd && d != Vec2::ZERO && now - t0 > 0.35 => {
                // deplasare continuă cât timp butonul e ținut apăsat
                let dt = ui.input(|i| i.stable_dt).min(0.1) as f64;
                self.pan_fraction(d.x as f64 * dt * 0.7, d.y as f64 * dt * 0.7);
                changed = true;
                ui.ctx().request_repaint();
            }
            (Some(d), Some((pd, _))) if d != pd => self.nav_press = Some((d, now)),
            (None, Some(_)) => {}
            _ => {}
        }
        if pad.clicked() {
            let held = self.nav_press.map(|(_, t0)| now - t0).unwrap_or(0.0);
            match pad.interact_pointer_pos().and_then(region) {
                Some(d) if d == Vec2::ZERO => {
                    self.zoom_full();
                    changed = true;
                }
                Some(d) if held <= 0.35 => {
                    self.pan_fraction(d.x as f64 * 0.25, d.y as f64 * 0.25);
                    changed = true;
                }
                _ => {}
            }
        }
        if press_dir.is_none() {
            self.nav_press = None;
        }
        // --- butoane zoom
        let bsz = Vec2::new(30.0, 30.0);
        let zin = Rect::from_center_size(Pos2::new(c.x, c.y + r + 24.0), bsz);
        let zout = Rect::from_center_size(Pos2::new(c.x, zin.bottom() + 1.0 + bsz.y / 2.0), bsz);
        let group = zin.union(zout);
        painter.rect_filled(group, 6.0, pal.overlay_bg);
        painter.rect_stroke(
            group,
            6.0,
            Stroke::new(1.0, pal.overlay_border),
            StrokeKind::Inside,
        );
        painter.line_segment(
            [
                Pos2::new(group.left() + 5.0, zin.bottom()),
                Pos2::new(group.right() - 5.0, zin.bottom()),
            ],
            Stroke::new(1.0, pal.overlay_border),
        );
        for (br, icon, f, tip) in [
            (zin, Icon::Plus, 2.0, tr!("Mărire (+)")),
            (zout, Icon::Minus, 0.5, tr!("Micșorare (-)")),
        ] {
            let b = ui
                .interact(br, ui.scope_id().with(("nav_zoom", tip)), Sense::click())
                .on_hover_text(tip);
            if b.hovered() {
                painter.rect_filled(br.shrink(2.0), 4.0, pal.overlay_hover);
            }
            paint_icon(
                &painter,
                br.shrink(7.0),
                icon,
                if b.hovered() {
                    pal.accent
                } else {
                    pal.overlay_icon
                },
            );
            if b.clicked() {
                self.zoom_step(f);
                changed = true;
            }
        }
        changed
    }

    pub fn map_ui(&mut self, ui: &mut egui::Ui) {
        let rect = ui.available_rect_before_wrap();
        let resp = ui.allocate_rect(rect, Sense::click_and_drag());
        let resized = self.map_rect != rect;
        self.map_rect = rect;
        if (!self.view_initialized || resized && self.layers.is_empty()) && !self.layers.is_empty()
        {
            self.zoom_full();
        } else if self.layers.is_empty() && !self.view_initialized && rect.width() > 0.0 {
            self.zoom_home();
        }
        let ctx = ui.ctx().clone();
        let now = ctx.input(|i| i.time);
        let painter = ui.painter_at(rect);
        let pal = theme::palette(self.settings.theme);
        painter.rect_filled(rect, 0.0, pal.map_bg);

        // ---- interacțiune
        let hover = resp.hover_pos();
        self.cursor_map = hover.map(|p| self.s2w(p));
        let mut interacted = false;
        if let Some(hp) = hover {
            let (scroll, zoom) = ui.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta()));
            let mut factor = 1.0f64;
            if scroll != 0.0 {
                factor *= (scroll as f64 * 0.0025).exp();
            }
            if zoom != 1.0 {
                factor *= zoom as f64;
            }
            if factor != 1.0 {
                let (wx, wy) = self.s2w(hp);
                self.view.scale /= factor;
                let (nx, ny) = self.s2w(hp);
                self.view.cx += wx - nx;
                self.view.cy += wy - ny;
                interacted = true;
            }
        }
        let pan_drag = resp.dragged_by(egui::PointerButton::Middle)
            || (resp.dragged_by(egui::PointerButton::Primary)
                && matches!(self.tool, Tool::Pan | Tool::Identify | Tool::Measure));
        if pan_drag {
            let d = resp.drag_delta();
            self.view.cx -= d.x as f64 * self.view.scale;
            self.view.cy += d.y as f64 * self.view.scale;
            interacted = true;
        }
        if self.tool == Tool::Select {
            let mods = ui.input(|i| i.modifiers);
            if resp.drag_started_by(egui::PointerButton::Primary) {
                self.zoom_box_start = resp.interact_pointer_pos();
            }
            if resp.drag_stopped_by(egui::PointerButton::Primary)
                && let (Some(a), Some(b)) = (
                    self.zoom_box_start.take(),
                    hover.or(resp.interact_pointer_pos()),
                )
                && (a - b).length() > 4.0
            {
                let (x0, y0) = self.s2w(a);
                let (x1, y1) = self.s2w(b);
                let bx = Bounds::new(x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1));
                self.select_rect(bx, SelMode::from_modifiers(&mods, true));
            }
            if resp.clicked()
                && let Some(p) = resp.interact_pointer_pos()
            {
                let (x, y) = self.s2w(p);
                self.select_click(x, y, SelMode::from_modifiers(&mods, false));
            }
        }
        if matches!(self.tool, Tool::ZoomIn | Tool::ZoomOut) {
            let zin = self.tool == Tool::ZoomIn;
            if resp.drag_started_by(egui::PointerButton::Primary) {
                self.zoom_box_start = resp.interact_pointer_pos();
            }
            if resp.drag_stopped_by(egui::PointerButton::Primary)
                && let (Some(a), Some(b)) = (
                    self.zoom_box_start.take(),
                    hover.or(resp.interact_pointer_pos()),
                )
            {
                if (a - b).length() > 5.0 {
                    let (x0, y0) = self.s2w(a);
                    let (x1, y1) = self.s2w(b);
                    let bx = Bounds::new(x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1));
                    if zin {
                        self.zoom_to(bx);
                        self.view.scale /= 1.08;
                    } else {
                        // vederea curentă se „strânge” în dreptunghiul trasat
                        let f = (rect.width() / (a.x - b.x).abs().max(1.0))
                            .max(rect.height() / (a.y - b.y).abs().max(1.0))
                            as f64;
                        let (cx, cy) = bx.center();
                        self.view.cx = cx;
                        self.view.cy = cy;
                        self.view.scale *= f;
                    }
                }
                interacted = true;
            }
            if resp.clicked()
                && let Some(p) = resp.interact_pointer_pos()
            {
                let (x, y) = self.s2w(p);
                self.view.cx = x;
                self.view.cy = y;
                if zin {
                    self.view.scale /= 2.0;
                } else {
                    self.view.scale *= 2.0;
                }
                interacted = true;
            }
        }
        if resp.clicked()
            && let Some(p) = resp.interact_pointer_pos()
        {
            let (x, y) = self.s2w(p);
            let used = self.parcel_click(x, y);
            match self.tool {
                _ if used => {}
                Tool::Identify => self.identify_at(x, y),
                Tool::Measure => {
                    if self.measure_done {
                        self.measure.clear();
                        self.measure_done = false;
                    }
                    self.measure.push((x, y));
                }
                _ => {}
            }
            if self.dlg.converter.pick_from_map {
                self.dlg.converter.set_from_map(x, y, self.map_crs.clone());
            }
            if self.dlg.tdr.open && self.dlg.tdr.pick_from_map {
                self.dlg.tdr.set_from_map(x, y, self.map_crs.clone());
            }
        }
        if resp.double_clicked() {
            match self.tool {
                Tool::Measure => {
                    self.measure_done = true;
                    self.measure.dedup();
                }
                Tool::Pan => {
                    if let Some(p) = resp.interact_pointer_pos() {
                        let (x, y) = self.s2w(p);
                        self.view.cx = x;
                        self.view.cy = y;
                        self.view.scale /= 2.0;
                        interacted = true;
                    }
                }
                _ => {}
            }
        }
        if resp.secondary_clicked() && self.tool == Tool::Measure && !self.measure.is_empty() {
            self.measure.clear();
            self.measure_done = false;
        }
        let ctx_pos = resp.interact_pointer_pos();
        resp.context_menu(|ui| {
            if let Some(p) = ctx_pos.or(hover) {
                let (x, y) = self.s2w(p);
                let geo = self.map_crs.as_ref().is_some_and(|c| c.is_geographic());
                let txt = if geo {
                    format!("{:.8} {:.8}", x, y)
                } else {
                    format!("{:.3} {:.3}", x, y)
                };
                if ui.button(tr!("Copiază coordonatele ({txt})")).clicked() {
                    ui.ctx().copy_text(txt.clone());
                    ui.close();
                }
                if let Some((lon, lat)) = self.to_wgs.as_ref().and_then(|t| t.xy(x, y)) {
                    let w = format!("{:.8}, {:.8}", lat, lon);
                    if ui.button(tr!("Copiază lat, lon WGS84 ({w})")).clicked() {
                        ui.ctx().copy_text(w);
                        ui.close();
                    }
                }
                if ui.button(tr!("Centrează aici")).clicked() {
                    self.view.cx = x;
                    self.view.cy = y;
                    ui.close();
                }
                if ui.button(tr!("Identifică aici")).clicked() {
                    self.identify_at(x, y);
                    ui.close();
                }
            }
            if ui.button(tr!("Extindere completă")).clicked() {
                self.zoom_full();
                ui.close();
            }
        });
        if interacted || pan_drag {
            self.last_interaction = now;
            self.view_initialized = true;
        }
        let cursor = match self.tool {
            Tool::Pan if pan_drag => egui::CursorIcon::Grabbing,
            Tool::Pan => egui::CursorIcon::Grab,
            Tool::ZoomIn => egui::CursorIcon::ZoomIn,
            Tool::ZoomOut => egui::CursorIcon::ZoomOut,
            _ => egui::CursorIcon::Crosshair,
        };
        if hover.is_some() {
            ctx.set_cursor_icon(cursor);
        }

        // ---- desenare
        let vb = self.view_bounds();
        let idle = now - self.last_interaction > 0.25;
        let map_crs = self.map_crs.clone();
        let ppp = ctx.pixels_per_point();
        let mut need_repaint = false;
        // harta de fundal, sub toate straturile
        {
            let vp = super::basemap::ViewParams {
                rect,
                cx: self.view.cx,
                cy: self.view.cy,
                scale: self.view.scale,
                ppp,
                now,
            };
            if self.basemap.draw(&ctx, &painter, map_crs.as_ref(), &vp) {
                ctx.request_repaint_after(std::time::Duration::from_millis(250));
            }
        }
        // randări raster (necesită &mut)
        for i in 0..self.layers.len() {
            let id = self.layers[i].id;
            let visible = self.layers[i].visible;
            if let LayerKind::Raster(r) = &mut self.layers[i].kind {
                if !visible || !r.map_bounds.intersects(&vb) {
                    continue;
                }
                r.ensure_overview(&ctx, map_crs.as_ref(), id);
                let ov_res = r
                    .overview
                    .as_ref()
                    .map(|(t, b)| b.width() / t.size()[0] as f64)
                    .unwrap_or(f64::INFINITY);
                if let Some(ib) = r.map_bounds.intersection(&vb) {
                    if self.view.scale / ppp as f64 * 1.2 < ov_res {
                        let key = [
                            (ib.minx / self.view.scale).round() as i64,
                            (ib.maxy / self.view.scale).round() as i64,
                            (ib.width() / self.view.scale).round() as i64,
                            (ib.height() / self.view.scale).round() as i64,
                            (self.view.scale.ln() * 1000.0).round() as i64,
                        ];
                        if r.detail_key != Some(key) {
                            if idle {
                                let w = ((ib.width() / self.view.scale) * ppp as f64)
                                    .round()
                                    .clamp(1.0, 4096.0)
                                    as usize;
                                let h = ((ib.height() / self.view.scale) * ppp as f64)
                                    .round()
                                    .clamp(1.0, 4096.0)
                                    as usize;
                                r.detail = r
                                    .render(&ctx, map_crs.as_ref(), &ib, w, h, &format!("det{id}"), true)
                                    .map(|t| (t, ib));
                                r.detail_key = Some(key);
                            } else {
                                need_repaint = true;
                            }
                        }
                    } else {
                        r.detail = None;
                        r.detail_key = None;
                    }
                }
            }
        }
        for l in &self.layers {
            if !l.visible {
                continue;
            }
            match &l.kind {
                LayerKind::Raster(r) => {
                    let tint = Color32::from_white_alpha((l.opacity * 255.0) as u8);
                    let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
                    if let Some((t, b)) = &r.overview {
                        painter.image(
                            t.id(),
                            Rect::from_min_max(self.w2s(b.minx, b.maxy), self.w2s(b.maxx, b.miny)),
                            uv,
                            tint,
                        );
                    }
                    if let Some((t, b)) = &r.detail {
                        painter.image(
                            t.id(),
                            Rect::from_min_max(self.w2s(b.minx, b.maxy), self.w2s(b.maxx, b.miny)),
                            uv,
                            tint,
                        );
                    }
                }
                LayerKind::Vector(v) => self.draw_vector(&painter, l, v, &vb),
            }
        }
        self.draw_selected(&painter, &vb);
        self.draw_selection(&painter);
        self.draw_search_marker(&painter);
        self.draw_parcel(&painter);

        // măsurare
        if !self.measure.is_empty() {
            let mut pts: Vec<Pos2> = self.measure.iter().map(|&(x, y)| self.w2s(x, y)).collect();
            if !self.measure_done
                && let Some(h) = hover
                && self.tool == Tool::Measure
            {
                pts.push(h);
            }
            let mut shapes = vec![];
            push_stroke(
                &mut shapes,
                pts.iter().copied(),
                false,
                Stroke::new(2.5, Color32::from_rgb(255, 120, 0)),
            );
            painter.extend(shapes);
            for p in &pts {
                painter.circle_filled(*p, 3.5, Color32::from_rgb(255, 120, 0));
            }
            if let Some(t) = self.measure_text() {
                let galley = painter.layout(t, FontId::monospace(12.0), pal.map_fg, 360.0);
                let r = Rect::from_min_size(
                    Pos2::new(rect.right() - galley.size().x - 20.0, rect.top() + 10.0),
                    galley.size() + Vec2::splat(10.0),
                );
                painter.rect_filled(r, 4.0, pal.overlay_bg);
                painter.rect_stroke(
                    r,
                    4.0,
                    Stroke::new(1.0, pal.overlay_border),
                    StrokeKind::Inside,
                );
                painter.galley(r.min + Vec2::splat(5.0), galley, pal.map_fg);
            }
        }
        // dreptunghiul de zoom
        if let (Some(a), Some(b)) = (self.zoom_box_start, hover)
            && self.tool == Tool::Select
        {
            self.draw_select_box(&painter, a, b);
        } else if let (Some(a), Some(b)) = (self.zoom_box_start, hover) {
            let r = Rect::from_two_pos(a, b);
            painter.rect_filled(r, 0.0, pal.accent.gamma_multiply(0.18));
            painter.rect_stroke(r, 0.0, Stroke::new(1.5, pal.accent), StrokeKind::Inside);
        }
        self.draw_scale_bar(&painter, rect);
        if let Some(t) = self.basemap.status_text() {
            let galley = painter.layout(t, FontId::proportional(11.0), pal.map_fg, rect.width() * 0.7);
            let r = Rect::from_min_size(
                Pos2::new(
                    rect.right() - galley.size().x - 10.0,
                    rect.bottom() - galley.size().y - 6.0,
                ),
                galley.size() + Vec2::new(8.0, 4.0),
            );
            painter.rect_filled(r, 3.0, pal.overlay_bg.gamma_multiply(0.85));
            painter.galley(r.min + Vec2::new(4.0, 2.0), galley, pal.map_fg);
        }
        if self.layers.is_empty() && self.basemap.source.is_none() {
            painter.text(
                rect.center(),
                Align2::CENTER_CENTER,
                tr!("Trageți aici fișiere GIS sau folosiți Fișier > Deschide (Ctrl+O)"),
                FontId::proportional(18.0),
                pal.weak_text,
            );
        }
        if self.settings.show_nav && self.nav_controls(ui, rect, now) {
            self.last_interaction = now;
            self.view_initialized = true;
        }
        // istoricul vederilor: se înregistrează când harta stă pe loc
        if self.view_initialized
            && now - self.last_interaction > 0.6
            && self.zoom_box_start.is_none()
        {
            self.commit_history();
        } else if self.view_initialized {
            ctx.request_repaint_after(std::time::Duration::from_millis(650));
        }
        if need_repaint {
            ctx.request_repaint_after(std::time::Duration::from_millis(260));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::epaint::{TessellationOptions, Tessellator};

    /// Extinderea maximă a triunghiurilor rezultate față de punctele de intrare.
    fn max_excursion(shapes: Vec<Shape>, pts: &[Pos2]) -> f32 {
        let mut t = Tessellator::new(1.0, TessellationOptions::default(), [64, 64], vec![]);
        let mut mesh = egui::epaint::Mesh::default();
        for s in shapes {
            t.tessellate_shape(s, &mut mesh);
        }
        let mut worst = 0f32;
        for v in &mesh.vertices {
            let d = pts
                .iter()
                .map(|p| (v.pos - *p).length())
                .fold(f32::INFINITY, f32::min);
            worst = worst.max(if d.is_finite() { d } else { f32::INFINITY });
        }
        worst
    }

    fn ring() -> Vec<Pos2> {
        // pătrat cu un vârf întors aproape la 180° și zigzaguri sub un pixel
        let mut p = vec![Pos2::new(100.0, 100.0), Pos2::new(200.0, 100.0)];
        p.push(Pos2::new(260.0, 100.2)); // vârf (spike)
        p.push(Pos2::new(200.0, 100.4));
        for i in 0..20 {
            let j = if i % 2 == 0 { 0.13 } else { -0.11 };
            p.push(Pos2::new(200.0 + j, 150.0 + i as f32 * 0.05));
        }
        p.push(Pos2::new(200.0, 150.0)); // triplicat
        p.push(Pos2::new(200.0, 150.0));
        p.push(Pos2::new(200.0, 200.0));
        p.push(Pos2::new(100.0, 200.0));
        p
    }

    #[test]
    fn egui_miter_explodes_without_guard() {
        let pts = ring();
        let raw = vec![Shape::closed_line(pts.clone(), Stroke::new(1.5, Color32::RED))];
        assert!(max_excursion(raw, &pts) > 50.0, "testul nu reproduce problema");
    }

    #[test]
    fn push_stroke_stays_on_geometry() {
        let pts = ring();
        let mut shapes = vec![];
        push_stroke(&mut shapes, pts.iter().copied(), true, Stroke::new(1.5, Color32::RED));
        assert!(max_excursion(shapes, &pts) < 6.0);
        let mut shapes = vec![];
        push_stroke(&mut shapes, pts.iter().copied(), false, Stroke::new(1.5, Color32::RED));
        assert!(max_excursion(shapes, &pts) < 6.0);
        // NaN în mijloc: linia se întrerupe, fără puncte invalide
        let mut with_nan = pts.clone();
        with_nan.insert(5, Pos2::new(f32::NAN, f32::NAN));
        let mut shapes = vec![];
        push_stroke(&mut shapes, with_nan.iter().copied(), true, Stroke::new(1.5, Color32::RED));
        assert!(max_excursion(shapes, &pts) < 6.0);
    }
}

#[cfg(test)]
mod real_data {
    use super::*;
    use egui::epaint::{TessellationOptions, Tessellator};

    /// CADASTRU_STROKE_FILE=fișier.geojson cargo test --release --bin cadastru real_data -- --ignored --nocapture
    #[test]
    #[ignore]
    fn file_strokes() {
        let Ok(path) = std::env::var("CADASTRU_STROKE_FILE") else { return };
        let mut l = cadastru::formats::geojson::read(std::path::Path::new(&path)).unwrap();
        cadastru::crs::reproject_layer(&mut l, &cadastru::crs::Crs::from_epsg(3844).unwrap()).unwrap();
        for denom in [10_000_000.0, 2_500_000.0, 500_000.0, 100_000.0] {
            let upp = denom * 0.000_264_583; // m / pixel la 96 dpi
            let b = l.bounds();
            let w2s = |x: f64, y: f64| Pos2::new(((x - b.minx) / upp) as f32, ((b.maxy - y) / upp) as f32);
            let (mut raw, mut ours) = (0f32, 0f32);
            for f in &l.features {
                let Some(g) = &f.geom else { continue };
                for poly in g.polygons() {
                    for ring in poly {
                        let pts: Vec<Pos2> = ring.iter().map(|c| w2s(c.x, c.y)).collect();
                        let ex = |shapes: Vec<Shape>| {
                            let mut t = Tessellator::new(1.0, TessellationOptions::default(), [64, 64], vec![]);
                            let mut m = egui::epaint::Mesh::default();
                            for s in shapes { t.tessellate_shape(s, &mut m); }
                            // depășirea dreptunghiului inelului
                            let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
                            for p in &pts { x0 = x0.min(p.x); y0 = y0.min(p.y); x1 = x1.max(p.x); y1 = y1.max(p.y); }
                            m.vertices.iter().map(|v| {
                                let dx = (x0 - v.pos.x).max(v.pos.x - x1).max(0.0);
                                let dy = (y0 - v.pos.y).max(v.pos.y - y1).max(0.0);
                                if v.pos.x.is_finite() && v.pos.y.is_finite() { dx.max(dy) } else { f32::INFINITY }
                            }).fold(0f32, f32::max)
                        };
                        raw = raw.max(ex(vec![Shape::closed_line(pts.clone(), Stroke::new(1.5, Color32::RED))]));
                        let mut s = vec![];
                        push_stroke(&mut s, pts.iter().copied(), true, Stroke::new(1.5, Color32::RED));
                        ours = ours.max(ex(s));
                    }
                }
            }
            println!("1:{denom}: egui direct depășește cu {raw:.0} px, push_stroke cu {ours:.1} px");
        }
    }
}

/// Halo-ul unei etichete: contur care urmează literele sau dreptunghi,
/// în culoarea și cu transparența alese pe strat.
fn draw_label_halo(
    painter: &egui::Painter,
    st: &VectorStyle,
    r: Rect,
    galley: &std::sync::Arc<egui::Galley>,
) {
    let Some(h) = st.halo_rgba() else { return };
    let w = st.halo_width.max(0.0);
    match st.halo {
        HaloMode::Off => {}
        HaloMode::Box => {
            let c = Color32::from_rgba_unmultiplied(h[0], h[1], h[2], h[3]);
            painter.rect_filled(r.expand(w.max(0.5)), 2.0, c);
        }
        HaloMode::Outline => {
            if w <= 0.0 {
                return;
            }
            // copii ale textului deplasate pe un cerc (și pe unul interior, la
            // grosimi mari); copiile se suprapun, așa că opacitatea fiecăreia se
            // micșorează ca halo-ul să aibă aproximativ transparența cerută
            let rings: &[f32] = if w > 1.6 { &[1.0, 0.5] } else { &[1.0] };
            let n = 8 * rings.len();
            let a = h[3] as f32 / 255.0;
            let k = if n > 8 { 5.0 } else { 3.0 };
            let each = if a >= 0.999 { 1.0 } else { 1.0 - (1.0 - a).powf(1.0 / k) };
            let c = Color32::from_rgba_unmultiplied(h[0], h[1], h[2], (each * 255.0).round() as u8);
            for f in rings {
                let d = w * f;
                for i in 0..8 {
                    let ang = i as f32 * std::f32::consts::FRAC_PI_4;
                    let o = Vec2::new(ang.cos() * d, ang.sin() * d);
                    painter.galley_with_override_text_color(r.min + o, galley.clone(), c);
                }
            }
        }
    }
}
