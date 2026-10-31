//! Selecția entităților: instrumentul Selectare (click, Ctrl+click, dreptunghi), operațiile
//! pe selecție (tot, inversare, golire, zoom) și desenarea entităților selectate.

use super::App;
use super::layers::{LayerKind, VLayer};
use cadastru::i18n::tr;
use cadastru::model::*;
use eframe::egui::{self, Color32, Mesh, Pos2, Rect, Shape, Stroke};

/// Culoarea entităților selectate.
pub const SELECTED: Color32 = Color32::from_rgb(255, 214, 0);

/// Cum se combină entitățile alese cu selecția existentă.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelMode {
    /// înlocuiește selecția (click simplu)
    Replace,
    /// adaugă (Shift)
    Add,
    /// adaugă sau scoate (Ctrl + click)
    Toggle,
    /// scoate (Alt)
    Remove,
}

impl SelMode {
    /// Modul după tastele apăsate; `rect` = selecție prin dreptunghi (Ctrl adaugă).
    pub fn from_modifiers(m: &egui::Modifiers, rect: bool) -> SelMode {
        if m.alt {
            SelMode::Remove
        } else if m.command && !rect {
            SelMode::Toggle
        } else if m.shift || m.command {
            SelMode::Add
        } else {
            SelMode::Replace
        }
    }
}

impl VLayer {
    fn touch(&mut self) {
        self.sel_gen = self.sel_gen.wrapping_add(1);
    }
    /// Aplică modul de selecție pentru o mulțime de entități ale acestui strat.
    pub fn apply_sel(&mut self, feats: impl IntoIterator<Item = usize>, mode: SelMode) {
        let n = self.data.features.len();
        let feats = feats.into_iter().filter(|&i| i < n);
        match mode {
            SelMode::Replace => {
                self.sel.clear();
                self.sel.extend(feats);
            }
            SelMode::Add => self.sel.extend(feats),
            SelMode::Remove => {
                for f in feats {
                    self.sel.remove(&f);
                }
            }
            SelMode::Toggle => {
                for f in feats {
                    if !self.sel.remove(&f) {
                        self.sel.insert(f);
                    }
                }
            }
        }
        self.touch();
    }
    pub fn clear_sel(&mut self) {
        if !self.sel.is_empty() {
            self.sel.clear();
            self.touch();
        }
    }
    pub fn select_all(&mut self) {
        self.apply_sel(0..self.data.features.len(), SelMode::Replace);
    }
    pub fn invert_sel(&mut self) {
        let n = self.data.features.len();
        let new: Vec<usize> = (0..n).filter(|i| !self.sel.contains(i)).collect();
        self.apply_sel(new, SelMode::Replace);
    }
    /// Copia datelor stratului cu doar entitățile selectate.
    pub fn selected_data(&self) -> VectorLayer {
        VectorLayer {
            name: self.data.name.clone(),
            source: self.data.source.clone(),
            fields: self.data.fields.clone(),
            features: self
                .sel
                .iter()
                .filter_map(|&i| self.data.features.get(i).cloned())
                .collect(),
            crs: self.data.crs.clone(),
            has_z: self.data.has_z,
        }
    }
}

impl App {
    /// Straturile pe care lucrează instrumentul Selectare.
    fn selectable_layers(&self) -> Vec<u64> {
        self.layers
            .iter()
            .filter(|l| l.visible && l.is_vector())
            .filter(|l| !self.settings.select_active_only || self.selected_layer == Some(l.id))
            .map(|l| l.id)
            .collect()
    }

    /// Entitatea de sub punctul (x, y) al hărții: întâi straturile de deasupra, iar într-un
    /// strat întâi punctele, apoi liniile, apoi poligoanele.
    pub fn hit_test(&self, x: f64, y: f64, only: Option<&[u64]>) -> Option<(u64, usize)> {
        let tol = self.view.scale * 5.0;
        for l in self.layers.iter().rev().filter(|l| l.visible) {
            if only.is_some_and(|o| !o.contains(&l.id)) {
                continue;
            }
            let LayerKind::Vector(v) = &l.kind else { continue };
            let mut best: Option<(u8, f64, usize)> = None;
            for d in &v.disp {
                if !d.bbox.expanded(tol).contains(x, y) {
                    continue;
                }
                let dist = d.geom.distance_to(x, y);
                if dist <= tol {
                    let pr = match d.geom.kind() {
                        GeomKind::Point => 0,
                        GeomKind::Line => 1,
                        _ => 2,
                    };
                    if best.is_none_or(|(bp, bd, _)| (pr, dist) < (bp, bd)) {
                        best = Some((pr, dist, d.feat));
                    }
                }
            }
            if let Some((_, _, fi)) = best {
                return Some((l.id, fi));
            }
        }
        None
    }

    /// Click cu instrumentul Selectare.
    pub fn select_click(&mut self, x: f64, y: f64, mode: SelMode) {
        let ids = self.selectable_layers();
        let hit = self.hit_test(x, y, Some(&ids));
        if mode == SelMode::Replace {
            self.clear_selection_quiet();
        }
        if let Some((lid, fi)) = hit {
            if let Some(LayerKind::Vector(v)) = self.layer_mut(lid).map(|l| &mut l.kind) {
                v.apply_sel([fi], mode);
            }
            self.selection = Some((lid, fi));
            self.dlg.table.scroll_to = Some(fi);
            self.dlg.table.layer_id = Some(lid);
        } else {
            self.selection = None;
        }
        self.status = self.selection_status();
    }

    /// Selecție prin dreptunghi (coordonatele hărții).
    pub fn select_rect(&mut self, b: Bounds, mode: SelMode) {
        let ids = self.selectable_layers();
        if mode == SelMode::Replace {
            self.clear_selection_quiet();
        }
        for l in &mut self.layers {
            if !ids.contains(&l.id) {
                continue;
            }
            let LayerKind::Vector(v) = &mut l.kind else { continue };
            let hits: Vec<usize> = v
                .disp
                .iter()
                .filter(|d| d.bbox.intersects(&b) && d.geom.intersects_bounds(&b))
                .map(|d| d.feat)
                .collect();
            if !hits.is_empty() || mode == SelMode::Replace {
                v.apply_sel(hits, mode);
            }
        }
        self.selection = None;
        self.status = self.selection_status();
    }

    fn clear_selection_quiet(&mut self) {
        for l in &mut self.layers {
            if let LayerKind::Vector(v) = &mut l.kind {
                v.clear_sel();
            }
        }
    }

    /// Golește selecția din toate straturile.
    pub fn clear_selection(&mut self) {
        self.clear_selection_quiet();
        self.selection = None;
        self.status = tr!("Selecția a fost golită.").into();
    }

    /// Numărul total de entități selectate și numărul de straturi cu selecție.
    pub fn selection_totals(&self) -> (usize, usize) {
        let mut n = 0;
        let mut k = 0;
        for l in &self.layers {
            if let LayerKind::Vector(v) = &l.kind
                && !v.sel.is_empty()
            {
                n += v.sel.len();
                k += 1;
            }
        }
        (n, k)
    }

    pub fn selection_status(&self) -> String {
        match self.selection_totals() {
            (0, _) => tr!("Nicio entitate selectată.").into(),
            (1, _) => tr!("Selecție: o entitate").into(),
            (n, 1) => {
                let de = cadastru::i18n::de(n);
                tr!("Selecție: {n} {de}entități")
            }
            (n, k) => {
                let de = cadastru::i18n::de(n);
                tr!("Selecție: {n} {de}entități în {k} straturi")
            }
        }
    }

    /// Operații din meniuri: "all", "invert", "clear", "zoom" pe stratul `id`.
    pub fn selection_action(&mut self, id: u64, what: &str) {
        if what == "zoom" {
            self.zoom_to_selection(Some(id));
            return;
        }
        if let Some(LayerKind::Vector(v)) = self.layer_mut(id).map(|l| &mut l.kind) {
            match what {
                "all" => v.select_all(),
                "invert" => v.invert_sel(),
                "clear" => v.clear_sel(),
                _ => {}
            }
        }
        self.status = self.selection_status();
    }

    /// Încadrarea entităților selectate (în sistemul hărții), dintr-un strat sau din toate.
    pub fn selection_bounds(&self, only: Option<u64>) -> Option<Bounds> {
        let mut b = Bounds::empty();
        for l in &self.layers {
            if only.is_some_and(|o| o != l.id) {
                continue;
            }
            if let LayerKind::Vector(v) = &l.kind {
                for d in v.disp.iter().filter(|d| v.sel.contains(&d.feat)) {
                    b.merge(&d.bbox);
                }
            }
        }
        b.is_valid().then_some(b)
    }

    pub fn zoom_to_selection(&mut self, only: Option<u64>) {
        match self.selection_bounds(only) {
            Some(b) => self.zoom_to_padded(b),
            None => self.status = tr!("Nicio entitate selectată.").into(),
        }
    }

    /// Zoom pe o încadrare, cu o margine; punctele izolate primesc o fereastră minimă.
    pub fn zoom_to_padded(&mut self, b: Bounds) {
        let geo = self.map_crs.as_ref().is_some_and(|c| c.is_geographic());
        let min = if geo { 0.002 } else { 200.0 };
        let (cx, cy) = b.center();
        let w = b.width().max(min) * 1.15;
        let h = b.height().max(min) * 1.15;
        self.zoom_to(Bounds::new(cx - w / 2.0, cy - h / 2.0, cx + w / 2.0, cy + h / 2.0));
    }

    /// Desenează entitățile selectate peste straturi.
    pub fn draw_selected(&self, painter: &egui::Painter, vb: &Bounds) {
        let fill = Color32::from_rgba_unmultiplied(255, 214, 0, 70);
        let stroke = Stroke::new(2.4, SELECTED);
        for l in self.layers.iter().filter(|l| l.visible) {
            let LayerKind::Vector(v) = &l.kind else { continue };
            if v.sel.is_empty() {
                continue;
            }
            let mut mesh = Mesh::default();
            let mut shapes: Vec<Shape> = vec![];
            for d in &v.disp {
                if !v.sel.contains(&d.feat) || !d.bbox.intersects(vb) {
                    continue;
                }
                if !d.tri_idx.is_empty() {
                    let base = mesh.vertices.len() as u32;
                    for t in &d.tri_verts {
                        mesh.colored_vertex(self.w2s(t[0], t[1]), fill);
                    }
                    mesh.indices.extend(d.tri_idx.iter().map(|i| base + i));
                }
                for line in d.geom.lines() {
                    super::map::push_stroke(&mut shapes, line.iter().map(|c| self.w2s(c.x, c.y)), false, stroke);
                }
                for poly in d.geom.polygons() {
                    for ring in poly {
                        super::map::push_stroke(&mut shapes, ring.iter().map(|c| self.w2s(c.x, c.y)), true, stroke);
                    }
                }
                for p in d.geom.points() {
                    let sp = self.w2s(p.x, p.y);
                    shapes.push(Shape::circle_filled(sp, v.style.point_size + 1.5, SELECTED));
                    shapes.push(Shape::circle_stroke(
                        sp,
                        v.style.point_size + 1.5,
                        Stroke::new(1.2, Color32::from_rgb(90, 70, 0)),
                    ));
                }
            }
            if !mesh.is_empty() {
                painter.add(Shape::mesh(mesh));
            }
            painter.extend(shapes);
        }
    }

    /// Dreptunghiul de selecție în curs de trasare.
    pub fn draw_select_box(&self, painter: &egui::Painter, a: Pos2, b: Pos2) {
        let r = Rect::from_two_pos(a, b);
        painter.rect_filled(r, 0.0, Color32::from_rgba_unmultiplied(255, 214, 0, 40));
        painter.rect_stroke(r, 0.0, Stroke::new(1.5, SELECTED), egui::StrokeKind::Middle);
    }
}
