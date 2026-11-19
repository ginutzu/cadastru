//! Tema dark grey cu accente neon + pictograme vectoriale desenate direct (din proiectul SIRUTA).

use super::data::Kind;
use eframe::egui::{
    self, Color32, CornerRadius, FontFamily, Painter, Pos2, Rect, Shape, Stroke, epaint::PathShape,
    pos2, vec2,
};

pub const BG: Color32 = Color32::from_rgb(0x16, 0x18, 0x1d);
pub const PANEL: Color32 = Color32::from_rgb(0x1d, 0x20, 0x27);
pub const PANEL2: Color32 = Color32::from_rgb(0x25, 0x29, 0x32);
pub const BORDER: Color32 = Color32::from_rgb(0x33, 0x39, 0x45);
pub const TEXT: Color32 = Color32::from_rgb(0xe6, 0xe9, 0xef);
pub const DIM: Color32 = Color32::from_rgb(0x8a, 0x93, 0xa5);
pub const CYAN: Color32 = Color32::from_rgb(0x00, 0xe5, 0xff);
pub const MAGENTA: Color32 = Color32::from_rgb(0xff, 0x2b, 0xd6);
pub const GREEN: Color32 = Color32::from_rgb(0x39, 0xff, 0x88);
pub const AMBER: Color32 = Color32::from_rgb(0xff, 0xb0, 0x20);
pub const GOLD: Color32 = Color32::from_rgb(0xff, 0xd8, 0x4a);
pub const VIOLET: Color32 = Color32::from_rgb(0xa9, 0x70, 0xff);
pub const SLATE: Color32 = Color32::from_rgb(0x8f, 0xa3, 0xc4);

pub fn kind_color(k: Kind) -> Color32 {
    match k {
        Kind::Capitala => GOLD,
        Kind::MunicipiuResedinta => MAGENTA,
        Kind::Municipiu => VIOLET,
        Kind::OrasResedinta | Kind::Oras => CYAN,
        Kind::Comuna => GREEN,
        Kind::Sector => AMBER,
        Kind::Localitate => SLATE,
    }
}

pub fn alpha(c: Color32, a: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), a)
}

/// Stilul ferestrei SIRUTA (tema dark grey / neon a proiectului SIRUTA), pornind de la stilul
/// curent al aplicației. Se aplică numai în fereastra SIRUTA; fonturile („bold”, „siruta_mono”)
/// sunt înregistrate de `setup_fonts` din Cadastru.
pub fn style(base: &egui::Style) -> egui::Style {
    let mut style = base.clone();
    style.spacing.item_spacing = vec2(8.0, 6.0);
    style.spacing.button_padding = vec2(10.0, 5.0);
    style.spacing.interact_size.y = 26.0;
    use egui::{FontId, TextStyle};
    style.text_styles = [
        (TextStyle::Heading, FontId::proportional(22.0)),
        (TextStyle::Body, FontId::proportional(14.5)),
        (TextStyle::Button, FontId::proportional(14.0)),
        (TextStyle::Small, FontId::proportional(12.0)),
        (TextStyle::Monospace, FontId::new(14.0, FontFamily::Name("siruta_mono".into()))),
    ]
    .into();

    let mut v = egui::Visuals::dark();
    v.override_text_color = Some(TEXT);
    v.panel_fill = BG;
    v.window_fill = PANEL;
    v.window_stroke = Stroke::new(1.0f32, BORDER);
    v.extreme_bg_color = Color32::from_rgb(0x12, 0x14, 0x18);
    v.faint_bg_color = Color32::from_rgb(0x1b, 0x1e, 0x24);
    v.code_bg_color = PANEL2;
    v.hyperlink_color = CYAN;
    v.selection.bg_fill = Color32::from_rgb(0x10, 0x3c, 0x48);
    v.selection.stroke = Stroke::new(1.0f32, CYAN);
    v.window_corner_radius = CornerRadius::same(10);
    v.menu_corner_radius = CornerRadius::same(8);
    let r = CornerRadius::same(6);
    v.widgets.noninteractive.bg_fill = PANEL;
    v.widgets.noninteractive.weak_bg_fill = PANEL;
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0f32, BORDER);
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0f32, TEXT);
    v.widgets.inactive.bg_fill = PANEL2;
    v.widgets.inactive.weak_bg_fill = PANEL2;
    v.widgets.inactive.bg_stroke = Stroke::new(1.0f32, BORDER);
    v.widgets.inactive.fg_stroke = Stroke::new(1.0f32, TEXT);
    v.widgets.inactive.corner_radius = r;
    v.widgets.hovered.bg_fill = Color32::from_rgb(0x2c, 0x31, 0x3c);
    v.widgets.hovered.weak_bg_fill = Color32::from_rgb(0x2c, 0x31, 0x3c);
    v.widgets.hovered.bg_stroke = Stroke::new(1.0f32, CYAN);
    v.widgets.hovered.fg_stroke = Stroke::new(1.5f32, CYAN);
    v.widgets.hovered.corner_radius = r;
    v.widgets.active.bg_fill = alpha(CYAN, 60);
    v.widgets.active.weak_bg_fill = alpha(CYAN, 60);
    v.widgets.active.bg_stroke = Stroke::new(1.5f32, CYAN);
    v.widgets.active.fg_stroke = Stroke::new(1.5f32, Color32::WHITE);
    v.widgets.active.corner_radius = r;
    v.widgets.open.bg_fill = PANEL2;
    v.widgets.open.weak_bg_fill = PANEL2;
    v.widgets.open.bg_stroke = Stroke::new(1.0f32, MAGENTA);
    v.widgets.open.corner_radius = r;
    style.visuals = v;
    style
}

pub fn bold(size: f32) -> egui::FontId {
    egui::FontId::new(size, FontFamily::Name("bold".into()))
}

/// Linie cu efect de stralucire neon.
pub fn glow_line(p: &Painter, pts: Vec<Pos2>, color: Color32, width: f32, closed: bool) {
    for (w, a) in [(width * 4.0, 25u8), (width * 2.2, 60u8)] {
        let s = Stroke::new(w, alpha(color, a));
        p.add(if closed {
            Shape::closed_line(pts.clone(), s)
        } else {
            Shape::line(pts.clone(), s)
        });
    }
    let s = Stroke::new(width, color);
    p.add(if closed {
        Shape::closed_line(pts, s)
    } else {
        Shape::line(pts, s)
    });
}

pub fn glow_rect(p: &Painter, rect: Rect, radius: u8, color: Color32, strength: f32) {
    for i in 1..=4 {
        let f = i as f32;
        let a = (40.0 * strength / f) as u8;
        p.rect_stroke(
            rect.expand(f * 1.5),
            CornerRadius::same(radius.saturating_add(i as u8)),
            Stroke::new(1.5f32, alpha(color, a)),
            egui::StrokeKind::Outside,
        );
    }
    p.rect_stroke(
        rect,
        CornerRadius::same(radius),
        Stroke::new(1.5f32, color),
        egui::StrokeKind::Outside,
    );
}

fn arc(c: Pos2, r: f32, a0: f32, a1: f32, n: usize) -> Vec<Pos2> {
    (0..=n)
        .map(|i| {
            let t = a0 + (a1 - a0) * i as f32 / n as f32;
            pos2(c.x + r * t.cos(), c.y + r * t.sin())
        })
        .collect()
}

/// Lupa (cautare).
pub fn icon_search(p: &Painter, rect: Rect, color: Color32) {
    let s = rect.width().min(rect.height());
    let c = rect.center() - vec2(s * 0.1, s * 0.1);
    let r = s * 0.28;
    glow_line(p, arc(c, r, 0.0, std::f32::consts::TAU, 32), color, 2.0, true);
    let d = std::f32::consts::FRAC_1_SQRT_2;
    let a = pos2(c.x + r * d, c.y + r * d);
    let b = pos2(c.x + s * 0.42, c.y + s * 0.42);
    glow_line(p, vec![a, b], color, 2.6, false);
}

/// Sageti circulare (actualizare). `angle` permite animatia de rotatie.
pub fn icon_refresh(p: &Painter, rect: Rect, color: Color32, angle: f32) {
    let s = rect.width().min(rect.height());
    let c = rect.center();
    let r = s * 0.3;
    let a0 = angle + 0.9;
    let a1 = angle + 0.9 + 4.4;
    glow_line(p, arc(c, r, a0, a1, 36), color, 2.2, false);
    // varful sagetii la capatul arcului
    let end = pos2(c.x + r * a1.cos(), c.y + r * a1.sin());
    let tangent = vec2(-a1.sin(), a1.cos());
    let normal = vec2(a1.cos(), a1.sin());
    let h = s * 0.26;
    let tip = end + tangent * h;
    let l = end + normal * h * 0.8 - tangent * h * 0.15;
    let rr = end - normal * h * 0.8 - tangent * h * 0.15;
    p.add(Shape::convex_polygon(vec![tip, l, rr], color, Stroke::NONE));
}

/// Nor (sincronizare). `ok`: bifa; altfel bara oblica (offline).
pub fn icon_cloud(p: &Painter, rect: Rect, color: Color32, ok: bool) {
    let s = rect.width().min(rect.height());
    let c = rect.center();
    let base_y = c.y + s * 0.18;
    p.circle_filled(pos2(c.x - s * 0.18, base_y - s * 0.08), s * 0.17, color);
    p.circle_filled(pos2(c.x + s * 0.02, base_y - s * 0.2), s * 0.22, color);
    p.circle_filled(pos2(c.x + s * 0.22, base_y - s * 0.07), s * 0.16, color);
    p.rect_filled(
        Rect::from_min_max(pos2(c.x - s * 0.35, base_y - s * 0.1), pos2(c.x + s * 0.38, base_y + s * 0.06)),
        CornerRadius::same((s * 0.08) as u8),
        color,
    );
    let dark = BG;
    if ok {
        p.add(Shape::line(
            vec![
                pos2(c.x - s * 0.13, base_y - s * 0.08),
                pos2(c.x - s * 0.03, base_y + 0.01 * s),
                pos2(c.x + s * 0.15, base_y - s * 0.2),
            ],
            Stroke::new(s * 0.08, dark),
        ));
    } else {
        p.line_segment(
            [pos2(c.x - s * 0.1, base_y - s * 0.24), pos2(c.x + s * 0.12, base_y + s * 0.0)],
            Stroke::new(s * 0.08, dark),
        );
        p.line_segment(
            [pos2(c.x - s * 0.1, base_y + s * 0.0), pos2(c.x + s * 0.12, base_y - s * 0.24)],
            Stroke::new(s * 0.08, dark),
        );
    }
}

/// Ceas (ultima actualizare).
pub fn icon_clock(p: &Painter, rect: Rect, color: Color32) {
    let s = rect.width().min(rect.height());
    let c = rect.center();
    let r = s * 0.36;
    p.circle_stroke(c, r, Stroke::new(1.8f32, color));
    p.line_segment([c, c + vec2(0.0, -r * 0.62)], Stroke::new(1.8f32, color));
    p.line_segment([c, c + vec2(r * 0.48, 0.0)], Stroke::new(1.8f32, color));
}

/// Teanc de date (baza de date / sursa).
pub fn icon_database(p: &Painter, rect: Rect, color: Color32) {
    let s = rect.width().min(rect.height());
    let c = rect.center();
    let w = s * 0.34;
    let h = s * 0.1;
    let ell = |y: f32| -> Vec<Pos2> {
        (0..=24)
            .map(|i| {
                let t = std::f32::consts::TAU * i as f32 / 24.0;
                pos2(c.x + w * t.cos(), y + h * t.sin())
            })
            .collect()
    };
    let top = c.y - s * 0.26;
    let bot = c.y + s * 0.26;
    p.add(Shape::closed_line(ell(top), Stroke::new(1.6f32, color)));
    for y in [c.y - s * 0.04, bot] {
        let half: Vec<Pos2> = (0..=12)
            .map(|i| {
                let t = std::f32::consts::PI * i as f32 / 12.0;
                pos2(c.x + w * t.cos(), y + h * t.sin())
            })
            .collect();
        p.add(Shape::line(half, Stroke::new(1.6f32, color)));
    }
    p.line_segment([pos2(c.x - w, top), pos2(c.x - w, bot)], Stroke::new(1.6f32, color));
    p.line_segment([pos2(c.x + w, top), pos2(c.x + w, bot)], Stroke::new(1.6f32, color));
}

/// Pin de harta (UAT selectat).
pub fn icon_pin(p: &Painter, rect: Rect, color: Color32) {
    let s = rect.width().min(rect.height());
    let c = rect.center() - vec2(0.0, s * 0.12);
    let r = s * 0.26;
    let mut pts = vec![pos2(c.x, c.y + s * 0.48)];
    pts.extend(arc(c, r, std::f32::consts::PI * 0.82, std::f32::consts::PI * 2.18, 28));
    p.add(Shape::Path(PathShape::convex_polygon(pts, color, Stroke::NONE)));
    p.circle_filled(c, r * 0.42, BG);
}

/// Copiere (doua foi suprapuse).
pub fn icon_copy(p: &Painter, rect: Rect, color: Color32) {
    let s = rect.width().min(rect.height());
    let c = rect.center();
    let a = Rect::from_center_size(c + vec2(-s * 0.08, -s * 0.08), vec2(s * 0.45, s * 0.52));
    let b = a.translate(vec2(s * 0.18, s * 0.18));
    p.rect_stroke(a, CornerRadius::same(2), Stroke::new(1.5f32, alpha(color, 150)), egui::StrokeKind::Middle);
    p.rect_filled(b, CornerRadius::same(2), PANEL2);
    p.rect_stroke(b, CornerRadius::same(2), Stroke::new(1.5f32, color), egui::StrokeKind::Middle);
}

/// Buton-pictograma cu fundal rotunjit si stralucire la hover.
pub fn icon_button(
    ui: &mut egui::Ui,
    size: f32,
    color: Color32,
    paint: impl FnOnce(&Painter, Rect, Color32),
) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(size, size), egui::Sense::click());
    let p = ui.painter();
    let hovered = resp.hovered();
    p.rect_filled(rect, CornerRadius::same(8), if hovered { Color32::from_rgb(0x2c, 0x31, 0x3c) } else { PANEL2 });
    if hovered {
        glow_rect(p, rect, 8, color, 1.0);
    } else {
        p.rect_stroke(rect, CornerRadius::same(8), Stroke::new(1.0f32, BORDER), egui::StrokeKind::Inside);
    }
    paint(p, rect.shrink(size * 0.18), color);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// X (inchidere / stergere).
pub fn icon_close(p: &Painter, rect: Rect, color: Color32) {
    let s = rect.width().min(rect.height()) * 0.22;
    let c = rect.center();
    let st = Stroke::new(2.0f32, color);
    p.line_segment([c + vec2(-s, -s), c + vec2(s, s)], st);
    p.line_segment([c + vec2(-s, s), c + vec2(s, -s)], st);
}

/// Buton mic, fara fundal, cu pictograma X.
pub fn close_button(ui: &mut egui::Ui, size: f32) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(size, size), egui::Sense::click());
    let col = if resp.hovered() { MAGENTA } else { DIM };
    if resp.hovered() {
        ui.painter().circle_filled(rect.center(), size * 0.48, alpha(MAGENTA, 30));
    }
    icon_close(ui.painter(), rect, col);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Bifa (confirmare: copiat).
pub fn icon_check(p: &Painter, rect: Rect, color: Color32) {
    let s = rect.width().min(rect.height());
    let c = rect.center();
    glow_line(
        p,
        vec![
            c + vec2(-s * 0.3, 0.0),
            c + vec2(-s * 0.08, s * 0.22),
            c + vec2(s * 0.32, -s * 0.22),
        ],
        color,
        2.2,
        false,
    );
}

/// "i" in cerc (Despre).
pub fn icon_info(p: &Painter, rect: Rect, color: Color32) {
    let s = rect.width().min(rect.height());
    let c = rect.center();
    let r = s * 0.42;
    glow_line(p, arc(c, r, 0.0, std::f32::consts::TAU, 40), color, 2.0, true);
    p.circle_filled(c + vec2(0.0, -r * 0.48), s * 0.065, color);
    p.line_segment(
        [c + vec2(0.0, -r * 0.12), c + vec2(0.0, r * 0.55)],
        Stroke::new(s * 0.11, color),
    );
}
