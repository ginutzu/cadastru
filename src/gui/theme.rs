//! Teme (gri închis implicit / luminoasă), setări persistente și pictograme desenate vectorial.

use cadastru::formats::cgxml::CgxmlOptions;
use eframe::egui::{
    self, Color32, CornerRadius, Painter, Pos2, Rect, Shape, Stroke, Visuals, vec2,
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use cadastru::i18n::tr;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Theme {
    #[default]
    DarkGrey,
    Light,
}

impl Theme {
    pub fn label(&self) -> &'static str {
        match self {
            Theme::DarkGrey => tr!("Gri închis"),
            Theme::Light => tr!("Luminoasă"),
        }
    }
    pub fn is_dark(&self) -> bool {
        matches!(self, Theme::DarkGrey)
    }
}

/// Culorile folosite la desenarea hărții și a elementelor suprapuse.
#[derive(Clone, Copy)]
pub struct Palette {
    pub map_bg: Color32,
    pub map_fg: Color32,
    pub overlay_bg: Color32,
    pub overlay_hover: Color32,
    pub overlay_border: Color32,
    pub overlay_icon: Color32,
    pub accent: Color32,
    pub weak_text: Color32,
}

pub fn palette(t: Theme) -> Palette {
    match t {
        Theme::DarkGrey => Palette {
            map_bg: Color32::from_rgb(38, 39, 42),
            map_fg: Color32::from_rgb(225, 225, 225),
            overlay_bg: Color32::from_rgba_unmultiplied(58, 60, 64, 235),
            overlay_hover: Color32::from_rgba_unmultiplied(82, 86, 92, 245),
            overlay_border: Color32::from_rgb(92, 95, 100),
            overlay_icon: Color32::from_rgb(230, 230, 230),
            accent: Color32::from_rgb(64, 156, 255),
            weak_text: Color32::from_rgb(150, 150, 150),
        },
        Theme::Light => Palette {
            map_bg: Color32::WHITE,
            map_fg: Color32::from_rgb(20, 20, 20),
            overlay_bg: Color32::from_rgba_unmultiplied(255, 255, 255, 235),
            overlay_hover: Color32::from_rgba_unmultiplied(225, 232, 242, 245),
            overlay_border: Color32::from_rgb(170, 170, 170),
            overlay_icon: Color32::from_rgb(45, 45, 45),
            accent: Color32::from_rgb(0, 110, 210),
            weak_text: Color32::from_rgb(120, 120, 120),
        },
    }
}

/// Aplică tema asupra interfeței (independent de tema sistemului Windows).
pub fn apply(ctx: &egui::Context, t: Theme) {
    ctx.set_visuals_of(egui::Theme::Dark, visuals(Theme::DarkGrey));
    ctx.set_visuals_of(egui::Theme::Light, visuals(Theme::Light));
    ctx.set_theme(match t {
        Theme::DarkGrey => egui::ThemePreference::Dark,
        Theme::Light => egui::ThemePreference::Light,
    });
}

fn visuals(t: Theme) -> Visuals {
    match t {
        Theme::DarkGrey => {
            let mut v = Visuals::dark();
            let bg = Color32::from_rgb(45, 46, 50);
            let panel = Color32::from_rgb(50, 51, 55);
            v.panel_fill = panel;
            v.window_fill = Color32::from_rgb(54, 55, 60);
            v.extreme_bg_color = Color32::from_rgb(32, 33, 36);
            v.faint_bg_color = Color32::from_rgb(58, 59, 64);
            v.code_bg_color = bg;
            v.window_stroke = Stroke::new(1.0, Color32::from_rgb(78, 80, 86));
            v.selection.bg_fill = Color32::from_rgb(38, 94, 160);
            v.selection.stroke = Stroke::new(1.0, Color32::from_rgb(200, 225, 255));
            v.hyperlink_color = Color32::from_rgb(110, 175, 255);
            v.warn_fg_color = Color32::from_rgb(255, 176, 64);
            v.error_fg_color = Color32::from_rgb(255, 110, 100);
            v.override_text_color = Some(Color32::from_rgb(222, 222, 222));
            let w = &mut v.widgets;
            w.noninteractive.bg_fill = panel;
            w.noninteractive.weak_bg_fill = panel;
            w.noninteractive.bg_stroke = Stroke::new(1.0, Color32::from_rgb(70, 72, 77));
            w.noninteractive.fg_stroke = Stroke::new(1.0, Color32::from_rgb(210, 210, 210));
            w.inactive.bg_fill = Color32::from_rgb(64, 66, 71);
            w.inactive.weak_bg_fill = Color32::from_rgb(62, 64, 69);
            w.inactive.bg_stroke = Stroke::new(1.0, Color32::from_rgb(76, 78, 84));
            w.inactive.fg_stroke = Stroke::new(1.0, Color32::from_rgb(220, 220, 220));
            w.hovered.bg_fill = Color32::from_rgb(78, 81, 88);
            w.hovered.weak_bg_fill = Color32::from_rgb(76, 79, 86);
            w.hovered.bg_stroke = Stroke::new(1.0, Color32::from_rgb(110, 114, 122));
            w.hovered.fg_stroke = Stroke::new(1.5, Color32::WHITE);
            w.active.bg_fill = Color32::from_rgb(92, 96, 104);
            w.active.weak_bg_fill = Color32::from_rgb(92, 96, 104);
            w.active.fg_stroke = Stroke::new(2.0, Color32::WHITE);
            w.open.weak_bg_fill = Color32::from_rgb(70, 72, 78);
            v
        }
        Theme::Light => Visuals::light(),
    }
}

// ---------------------------------------------------------------------------
// Setări persistente
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub theme: Theme,
    pub cgxml: CgxmlOptions,
    pub show_labels: bool,
    pub show_nav: bool,
    /// TransDatRO pentru Stereo 70
    pub transdatro: bool,
    /// Altitudini doar cu EGG97
    pub height_egg97_only: bool,
    /// Opacitatea hărții de fundal (0..1)
    pub basemap_opacity: f32,
    /// Ultimul șablon XYZ personalizat
    pub basemap_custom: String,
    pub basemap_custom_zoom: u8,
    /// Limba interfeței (implicit română)
    pub lang: cadastru::i18n::Lang,
    /// Instrumentul Selectare lucrează doar pe stratul selectat în listă
    pub select_active_only: bool,
    /// Căutarea locurilor doar în România
    pub geocode_ro_only: bool,
    /// La deschidere: doar entitățile din zonă (implicit încadrarea României, EPSG:3844)
    pub load_area_filter: bool,
    /// Filtrul păstrează și entitățile care doar ating zona
    pub load_area_partial: bool,
    /// Zona filtrului, în EPSG:3844: minX, minY, maxX, maxY
    pub load_area_bounds: [f64; 4],
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            theme: Theme::DarkGrey,
            cgxml: CgxmlOptions::default(),
            show_labels: true,
            show_nav: true,
            transdatro: true,
            height_egg97_only: false,
            basemap_opacity: 1.0,
            basemap_custom: String::new(),
            basemap_custom_zoom: 19,
            lang: cadastru::i18n::Lang::Ro,
            select_active_only: false,
            geocode_ro_only: true,
            load_area_filter: false,
            load_area_partial: false,
            load_area_bounds: cadastru::area::ROMANIA_3844,
        }
    }
}

fn config_base() -> Option<PathBuf> {
    std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
}

fn settings_path() -> Option<PathBuf> {
    Some(config_base()?.join("Cadastru").join("setari.json"))
}

impl Settings {
    pub fn load() -> Settings {
        // la prima pornire preia setările versiunii anterioare (Cartograf)
        let old = config_base().map(|b| b.join("Cartograf").join("setari.json"));
        settings_path()
            .filter(|p| p.exists())
            .or(old)
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }
    pub fn save(&self) {
        if let Some(p) = settings_path() {
            if let Some(d) = p.parent() {
                let _ = std::fs::create_dir_all(d);
            }
            if let Ok(s) = serde_json::to_string_pretty(self) {
                let _ = std::fs::write(p, s);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Pictograme desenate (nu depind de fonturi)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    Open,
    Pan,
    ZoomIn,
    ZoomOut,
    Identify,
    Measure,
    FullExtent,
    Back,
    Forward,
    Table,
    Convert,
    Globe,
    Plus,
    Minus,
    Select,
    Search,
    SelectAttr,
    Redraw,
}

fn arrow_head(p: &Painter, tip: Pos2, dir: egui::Vec2, size: f32, c: Color32) {
    let d = dir.normalized();
    let n = vec2(-d.y, d.x);
    p.add(Shape::convex_polygon(
        vec![
            tip,
            tip - d * size + n * size * 0.6,
            tip - d * size - n * size * 0.6,
        ],
        c,
        Stroke::NONE,
    ));
}

/// Desenează pictograma în dreptunghiul dat.
pub fn paint_icon(p: &Painter, r: Rect, icon: Icon, c: Color32) {
    let s = Stroke::new(1.6, c);
    let ctr = r.center();
    let u = r.width().min(r.height()) / 16.0;
    let pt = |x: f32, y: f32| Pos2::new(ctr.x + x * u, ctr.y + y * u);
    match icon {
        Icon::Open => {
            p.add(Shape::closed_line(
                vec![
                    pt(-7.0, -5.0),
                    pt(-2.0, -5.0),
                    pt(0.0, -3.0),
                    pt(7.0, -3.0),
                    pt(7.0, 6.0),
                    pt(-7.0, 6.0),
                ],
                s,
            ));
            p.line_segment([pt(-7.0, -1.0), pt(7.0, -1.0)], s);
        }
        Icon::Pan => {
            for d in [
                vec2(1.0, 0.0),
                vec2(-1.0, 0.0),
                vec2(0.0, 1.0),
                vec2(0.0, -1.0),
            ] {
                p.line_segment([ctr, ctr + d * 5.0 * u], s);
                arrow_head(p, ctr + d * 7.5 * u, d, 3.0 * u, c);
            }
        }
        Icon::ZoomIn | Icon::ZoomOut => {
            let lc = pt(-1.5, -1.5);
            p.circle_stroke(lc, 4.8 * u, s);
            p.line_segment([pt(2.0, 2.0), pt(6.5, 6.5)], Stroke::new(2.4, c));
            p.line_segment([lc - vec2(2.6 * u, 0.0), lc + vec2(2.6 * u, 0.0)], s);
            if icon == Icon::ZoomIn {
                p.line_segment([lc - vec2(0.0, 2.6 * u), lc + vec2(0.0, 2.6 * u)], s);
            }
        }
        Icon::Identify => {
            p.circle_stroke(ctr, 6.8 * u, s);
            p.circle_filled(pt(0.0, -3.5), 1.1 * u, c);
            p.line_segment([pt(0.0, -1.0), pt(0.0, 4.2)], Stroke::new(1.9, c));
        }
        Icon::Measure => {
            let a = pt(-7.0, 3.0);
            let b = pt(7.0, -3.0);
            let d = (b - a).normalized();
            let n = vec2(-d.y, d.x) * 2.6 * u;
            p.add(Shape::closed_line(vec![a - n, b - n, b + n, a + n], s));
            for i in 1..6 {
                let q = a + (b - a) * (i as f32 / 6.0);
                let len = if i % 2 == 0 { 1.0 } else { 0.5 };
                p.line_segment([q - n, q - n + n * len], Stroke::new(1.2, c));
            }
        }
        Icon::FullExtent => {
            for (sx, sy) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
                let corner = pt(7.0 * sx, 6.0 * sy);
                p.line_segment([corner, corner - vec2(3.5 * u * sx, 0.0)], s);
                p.line_segment([corner, corner - vec2(0.0, 3.5 * u * sy)], s);
            }
            p.rect_filled(Rect::from_center_size(ctr, vec2(5.0 * u, 4.0 * u)), 0.0, c);
        }
        Icon::Back | Icon::Forward => {
            let sgn = if icon == Icon::Back { -1.0 } else { 1.0 };
            p.line_segment(
                [pt(-5.5 * sgn, 0.0), pt(4.0 * sgn, 0.0)],
                Stroke::new(2.0, c),
            );
            arrow_head(p, pt(7.0 * sgn, 0.0), vec2(sgn, 0.0), 4.5 * u, c);
        }
        Icon::Table => {
            let rr = Rect::from_center_size(ctr, vec2(14.0 * u, 11.0 * u));
            p.rect_stroke(rr, CornerRadius::same(1), s, egui::StrokeKind::Middle);
            p.line_segment([pt(-7.0, -2.0), pt(7.0, -2.0)], s);
            p.line_segment([pt(-7.0, 1.5), pt(7.0, 1.5)], Stroke::new(1.0, c));
            p.line_segment([pt(-2.0, -5.5), pt(-2.0, 5.5)], Stroke::new(1.0, c));
        }
        Icon::Convert => {
            p.line_segment([pt(-6.0, -2.5), pt(5.0, -2.5)], s);
            arrow_head(p, pt(7.0, -2.5), vec2(1.0, 0.0), 3.0 * u, c);
            p.line_segment([pt(6.0, 2.5), pt(-5.0, 2.5)], s);
            arrow_head(p, pt(-7.0, 2.5), vec2(-1.0, 0.0), 3.0 * u, c);
        }
        Icon::Globe => {
            p.circle_stroke(ctr, 7.0 * u, s);
            p.line_segment([pt(-7.0, 0.0), pt(7.0, 0.0)], Stroke::new(1.1, c));
            p.add(Shape::ellipse_stroke(
                ctr,
                vec2(3.2 * u, 7.0 * u),
                Stroke::new(1.1, c),
            ));
        }
        Icon::Plus => {
            p.line_segment([pt(-5.5, 0.0), pt(5.5, 0.0)], Stroke::new(2.2, c));
            p.line_segment([pt(0.0, -5.5), pt(0.0, 5.5)], Stroke::new(2.2, c));
        }
        Icon::Minus => {
            p.line_segment([pt(-5.5, 0.0), pt(5.5, 0.0)], Stroke::new(2.2, c));
        }
        Icon::Select => {
            // dreptunghi punctat + săgeata cursorului
            let thin = Stroke::new(1.2, c);
            let (x0, y0, x1, y1) = (-7.0f32, -6.0f32, 4.0f32, 3.0f32);
            let mut t = x0;
            while t < x1 {
                let e = (t + 2.0).min(x1);
                p.line_segment([pt(t, y0), pt(e, y0)], thin);
                p.line_segment([pt(t, y1), pt(e, y1)], thin);
                t += 3.5;
            }
            let mut t = y0;
            while t < y1 {
                let e = (t + 2.0).min(y1);
                p.line_segment([pt(x0, t), pt(x0, e)], thin);
                p.line_segment([pt(x1, t), pt(x1, e)], thin);
                t += 3.5;
            }
            p.add(Shape::convex_polygon(
                vec![pt(0.5, -0.5), pt(0.5, 7.5), pt(2.6, 5.4), pt(5.6, 5.6)],
                c,
                Stroke::NONE,
            ));
            p.line_segment([pt(2.8, 5.0), pt(4.6, 8.0)], Stroke::new(1.6, c));
        }
        Icon::SelectAttr => {
            // tabel cu un rând selectat
            let rr = Rect::from_center_size(ctr, vec2(15.0 * u, 12.0 * u));
            let hl = Rect::from_min_max(pt(-7.5, -1.0), pt(7.5, 2.5));
            p.rect_filled(hl, CornerRadius::ZERO, Color32::from_rgb(255, 214, 0));
            p.rect_stroke(rr, CornerRadius::same(1), s, egui::StrokeKind::Middle);
            p.line_segment([pt(-7.5, -3.0), pt(7.5, -3.0)], s);
            p.line_segment([pt(-2.5, -6.0), pt(-2.5, 6.0)], Stroke::new(1.0, c));
            p.line_segment([pt(-7.5, 4.0), pt(7.5, 4.0)], Stroke::new(1.0, c));
        }
        Icon::Redraw => {
            // săgeată circulară (reîmprospătare)
            let r = 6.0 * u;
            let n = 20;
            let (a0, a1) = (0.35_f32, 0.35 + std::f32::consts::TAU * 0.80);
            let pts: Vec<Pos2> = (0..=n)
                .map(|i| {
                    let a = a0 + (a1 - a0) * i as f32 / n as f32;
                    ctr + vec2(a.cos(), -a.sin()) * r
                })
                .collect();
            p.add(Shape::line(pts, Stroke::new(1.8, c)));
            let tip = ctr + vec2(a1.cos(), -a1.sin()) * r;
            let dir = vec2(-a1.sin(), -a1.cos());
            arrow_head(p, tip + dir * 2.0 * u, dir, 4.5 * u, c);
        }
        Icon::Search => {
            p.circle_stroke(pt(-1.5, -1.5), 5.0 * u, Stroke::new(1.8, c));
            p.line_segment([pt(2.2, 2.2), pt(7.0, 7.0)], Stroke::new(2.6, c));
        }
    }
}

/// Buton de bară de instrumente cu pictogramă și text.
pub fn icon_button(
    ui: &mut egui::Ui,
    icon: Icon,
    text: &str,
    selected: bool,
    tip: &str,
) -> egui::Response {
    let font = egui::TextStyle::Button.resolve(ui.style());
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_string(), font, Color32::PLACEHOLDER);
    let pad = vec2(6.0, 3.0);
    let icon_sz = 18.0;
    let gap = if text.is_empty() { 0.0 } else { 5.0 };
    let size = vec2(
        pad.x * 2.0 + icon_sz + gap + galley.size().x,
        (icon_sz + pad.y * 2.0).max(galley.size().y + pad.y * 2.0),
    );
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
    let resp = if tip.is_empty() {
        resp
    } else {
        resp.on_hover_text(tip)
    };
    if ui.is_rect_visible(rect) {
        let v = ui.visuals();
        let (bg, stroke) = if selected {
            (
                v.selection.bg_fill,
                Stroke::new(1.0, v.selection.stroke.color.gamma_multiply(0.6)),
            )
        } else if resp.hovered() {
            (v.widgets.hovered.weak_bg_fill, v.widgets.hovered.bg_stroke)
        } else {
            (Color32::TRANSPARENT, Stroke::NONE)
        };
        let p = ui.painter();
        p.rect(
            rect,
            CornerRadius::same(4),
            bg,
            stroke,
            egui::StrokeKind::Inside,
        );
        let fg = if selected {
            Color32::WHITE
        } else {
            v.text_color()
        };
        let ir = Rect::from_min_size(
            rect.min + pad + vec2(0.0, (rect.height() - pad.y * 2.0 - icon_sz) / 2.0),
            vec2(icon_sz, icon_sz),
        );
        paint_icon(p, ir, icon, fg);
        if !text.is_empty() {
            let tp = Pos2::new(ir.right() + gap, rect.center().y - galley.size().y / 2.0);
            p.galley(tp, galley, fg);
        }
    }
    resp
}
