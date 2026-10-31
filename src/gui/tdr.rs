//! Fereastra TransDatRO: transformări punct cu punct și pe fișiere în formatul programului oficial.

use super::App;
use cadastru::crs::{self, Crs, Transformer};
use cadastru::model::*;
use cadastru::transdatro::{self as tdr, Direction, HeightModel, PointOut, Stereo};
use eframe::egui::{self, RichText};
use std::path::PathBuf;
use cadastru::i18n::tr;

pub struct TdrDlg {
    pub open: bool,
    pub dir: usize,
    pub a: String,
    pub b: String,
    pub h: String,
    pub result: String,
    pub list_in: String,
    pub list_out: String,
    pub points: Vec<PointOut>,
    pub points_dir: Option<Direction>,
    pub stats: String,
    pub pick_from_map: bool,
    pub source_name: String,
}

impl Default for TdrDlg {
    fn default() -> Self {
        TdrDlg {
            open: false,
            dir: 0,
            a: String::new(),
            b: String::new(),
            h: String::new(),
            result: String::new(),
            list_in: String::new(),
            list_out: String::new(),
            points: vec![],
            points_dir: None,
            stats: String::new(),
            pick_from_map: false,
            source_name: "TransDatRO".into(),
        }
    }
}

fn stereo_crs(s: Stereo) -> Crs {
    match s {
        Stereo::S70 => Crs::from_epsg(3844).expect("EPSG:3844"),
        Stereo::S30 => Crs::stereo30(),
    }
}

fn stereo_name(s: Stereo) -> &'static str {
    match s {
        Stereo::S70 => "Stereo 70",
        Stereo::S30 => "Stereo 30",
    }
}

/// Etichetele câmpurilor de intrare (x/y respectă convenția fiecărui sistem).
fn input_labels(d: Direction) -> (&'static str, &'static str, &'static str) {
    match d.input_stereo() {
        None => (
            tr!("Latitudine B (φ):"),
            tr!("Longitudine L (λ):"),
            tr!("Altitudine elipsoidală h [m]:"),
        ),
        Some(Stereo::S70) => (tr!("x (Nord) [m]:"), tr!("y (Est) [m]:"), tr!("Altitudine H MN75 [m]:")),
        Some(Stereo::S30) => (tr!("x (Est) [m]:"), tr!("y (Nord) [m]:"), tr!("Altitudine H MN75 [m]:")),
    }
}

impl TdrDlg {
    pub fn direction(&self) -> Direction {
        Direction::ALL[self.dir.min(Direction::ALL.len() - 1)]
    }

    fn compute(&mut self) {
        let d = self.direction();
        let geo = d.input_geographic();
        let parse = |s: &str| {
            if geo {
                tdr::parse_angle(s)
            } else {
                cadastru::formats::csv::parse_num(s)
            }
        };
        let (Some(a), Some(b)) = (parse(&self.a), parse(&self.b)) else {
            self.result = if self.a.trim().is_empty() && self.b.trim().is_empty() {
                String::new()
            } else {
                tr!("Coordonate invalide").into()
            };
            return;
        };
        let h = if d.has_height() && !self.h.trim().is_empty() {
            match cadastru::formats::csv::parse_num(&self.h) {
                Some(v) => Some(v),
                None => {
                    self.result = tr!("Altitudine invalidă").into();
                    return;
                }
            }
        } else {
            None
        };
        let model = crs::height_model();
        self.result = match tdr::transform_point(d, a, b, h, model) {
            Ok(p) => {
                let mut s = String::new();
                if d.output_geographic() {
                    s.push_str(&format!(
                        "B (φ) = {}   {:.10}°\nL (λ) = {}   {:.10}°\n",
                        tdr::fmt_dms(p.y, true),
                        p.y,
                        tdr::fmt_dms(p.x, false),
                        p.x
                    ));
                    if let Some(h) = p.h {
                        s.push_str(&format!("h ETRS89 = {h:.3} m\n"));
                    }
                } else {
                    let st = d.output_stereo().unwrap_or(Stereo::S70);
                    let (x, y) = tdr::stereo_en_to_xy(st, p.x, p.y);
                    let (lx, ly) = match st {
                        Stereo::S70 => (tr!("x (Nord)"), tr!("y (Est) ")),
                        Stereo::S30 => (tr!("x (Est) "), tr!("y (Nord)")),
                    };
                    s.push_str(&format!(
                        "{} {lx} = {x:.3} m\n{} {ly} = {y:.3} m\n",
                        stereo_name(st),
                        stereo_name(st)
                    ));
                    if let Some(h) = p.h {
                        s.push_str(&tr!("H Marea Neagră 1975 = {h:.3} m\n"));
                    }
                }
                // anomalia altitudinii folosită
                if h.is_some() {
                    let (lat, lon) = if d.output_geographic() {
                        (p.y, p.x)
                    } else {
                        (a, b)
                    };
                    if let Ok((z, g)) = tdr::zeta(lat, lon, model) {
                        s.push_str(&format!("ζ = {z:.3} m (grid {g})"));
                    }
                }
                s.trim_end().to_string()
            }
            Err(e) => tr!("Eroare: {e}"),
        };
    }

    /// Preia un punct de pe hartă (din sistemul hărții în sistemul de intrare).
    pub fn set_from_map(&mut self, x: f64, y: f64, map_crs: Option<Crs>) {
        let Some(m) = map_crs else {
            return;
        };
        let d = self.direction();
        match d.input_stereo() {
            None => {
                let etrs = Crs::from_epsg(4258).expect("EPSG:4258");
                if let Some((lon, lat)) = Transformer::new(&m, &etrs)
                    .ok()
                    .and_then(|t| t.xy(x, y))
                {
                    self.a = tdr::fmt_dms(lat, true);
                    self.b = tdr::fmt_dms(lon, false);
                }
            }
            Some(s) => {
                if let Some((e, n)) = Transformer::new(&m, &stereo_crs(s))
                    .ok()
                    .and_then(|t| t.xy(x, y))
                {
                    let (a, b) = tdr::stereo_en_to_xy(s, e, n);
                    self.a = format!("{a:.3}");
                    self.b = format!("{b:.3}");
                }
            }
        }
        self.compute();
    }

    fn run_list(&mut self) {
        let d = self.direction();
        let r = tdr::process_text(&self.list_in, d, crs::height_model());
        self.list_out = r.output;
        self.points = r.points;
        self.points_dir = Some(d);
        self.stats = tr!("{} puncte transformate, {} erori", r.ok, r.errors);
    }

    /// Stratul de puncte cu rezultatele (în sistemul de ieșire).
    fn result_layer(&self) -> Option<VectorLayer> {
        let d = self.points_dir?;
        if self.points.is_empty() {
            return None;
        }
        let crs = match d.output_stereo() {
            Some(s) => stereo_crs(s),
            None => Crs::from_epsg(4258).ok()?,
        };
        let mut l = VectorLayer::new(format!("{} ({})", self.source_name, d.key()));
        l.crs = Some(crs);
        let geo = d.output_geographic();
        let (fx, fy) = match d.output_stereo() {
            None => ("B", "L"),
            Some(Stereo::S70) => ("X_NORD", "Y_EST"),
            Some(Stereo::S30) => ("X_EST", "Y_NORD"),
        };
        let mut fields = vec![
            Field::new("COD", FieldType::Text),
            Field::new(fx, FieldType::Real),
            Field::new(fy, FieldType::Real),
        ];
        if d.has_height() {
            fields.push(Field::new(if geo { "H_ELIPS" } else { "H_MN75" }, FieldType::Real));
        }
        for f in &mut fields {
            if f.ftype == FieldType::Real {
                f.width = 18;
                f.precision = if geo { 10 } else { 3 };
            }
        }
        l.fields = fields;
        for p in &self.points {
            let (a, b) = match d.output_stereo() {
                Some(s) => tdr::stereo_en_to_xy(s, p.x, p.y),
                None => (p.y, p.x),
            };
            let mut attrs = vec![Value::Text(p.id.clone()), Value::Real(a), Value::Real(b)];
            if d.has_height() {
                attrs.push(p.h.map(Value::Real).unwrap_or(Value::Null));
            }
            l.features.push(Feature {
                geom: Some(Geometry::Point(Coord::new(p.x, p.y))),
                attrs,
            });
        }
        Some(l)
    }
}

fn read_text(p: &std::path::Path) -> std::io::Result<String> {
    let b = std::fs::read(p)?;
    Ok(match String::from_utf8(b.clone()) {
        Ok(s) => s,
        Err(_) => encoding_rs::WINDOWS_1250.decode(&b).0.into_owned(),
    })
}

impl App {
    pub fn open_transdatro(&mut self) {
        self.dlg.tdr.open = true;
    }

    pub fn transdatro_window(&mut self, ctx: &egui::Context) {
        if !self.dlg.tdr.open {
            return;
        }
        let mut open = true;
        let mut add_layer: Option<VectorLayer> = None;
        let mut settings_changed = false;
        let mut tdr_on = crs::transdatro_enabled();
        let mut egg_only = crs::height_model() == HeightModel::Egg97;
        let t = &mut self.dlg.tdr;
        egui::Window::new(tr!("TransDatRO — transformări oficiale ANCPI")).id(egui::Id::unique("win_tdr_0"))
            .open(&mut open)
            .default_width(820.0)
            .show(ctx, |ui| {
                let mut changed = false;
                egui::ComboBox::from_id_salt("tdr_dir")
                    .width(560.0)
                    .selected_text(t.direction().label())
                    .show_ui(ui, |ui| {
                        for (i, d) in Direction::ALL.iter().enumerate() {
                            if ui.selectable_value(&mut t.dir, i, d.label()).changed() {
                                changed = true;
                            }
                        }
                    });
                if changed {
                    t.a.clear();
                    t.b.clear();
                    t.result.clear();
                }
                let d = t.direction();
                let (la, lb, lh) = input_labels(d);
                ui.add_space(4.0);
                egui::Grid::new("tdr_in")
                    .num_columns(2)
                    .spacing([10.0, 6.0])
                    .show(ui, |ui| {
                        let hint = if d.input_geographic() {
                            tr!("44 38 43.52664  sau  44.645424")
                        } else {
                            ""
                        };
                        ui.label(la);
                        if ui
                            .add(
                                egui::TextEdit::singleline(&mut t.a)
                                    .desired_width(240.0)
                                    .hint_text(hint),
                            )
                            .changed()
                        {
                            t.compute();
                        }
                        ui.end_row();
                        ui.label(lb);
                        if ui
                            .add(egui::TextEdit::singleline(&mut t.b).desired_width(240.0))
                            .changed()
                        {
                            t.compute();
                        }
                        ui.end_row();
                        if d.has_height() {
                            ui.label(lh);
                            if ui
                                .add(
                                    egui::TextEdit::singleline(&mut t.h)
                                        .desired_width(120.0)
                                        .hint_text(tr!("opțional")),
                                )
                                .changed()
                            {
                                t.compute();
                            }
                            ui.end_row();
                        }
                    });
                ui.checkbox(&mut t.pick_from_map, tr!("Preia punctul printr-un click pe hartă"));
                ui.separator();
                ui.horizontal(|ui| {
                    ui.add(
                        egui::Label::new(RichText::new(&t.result).monospace().size(14.0))
                            .selectable(true),
                    );
                    if !t.result.is_empty() && ui.small_button(tr!("Copiază")).clicked() {
                        ui.ctx().copy_text(t.result.clone());
                    }
                });
                ui.separator();
                egui::CollapsingHeader::new(tr!("Transformare listă / fișier (format TransDatRO)"))
                    .default_open(true)
                    .show(ui, |ui| {
                        ui.label(
                            RichText::new(
                                tr!("Primul rând: titlu (opțional). Apoi „cod, coord1, coord2[, h]” pe rând; ENDF la final (opțional)."),
                            )
                            .weak(),
                        );
                        ui.horizontal(|ui| {
                            if ui.button(tr!("Deschide fișier…")).clicked()
                                && let Some(p) = rfd::FileDialog::new()
                                    .add_filter("Text", &["txt", "csv", "dat"])
                                    .add_filter(tr!("Toate"), &["*"])
                                    .pick_file()
                            {
                                match read_text(&p) {
                                    Ok(s) => {
                                        t.list_in = s;
                                        t.source_name = p
                                            .file_stem()
                                            .map(|s| s.to_string_lossy().to_string())
                                            .unwrap_or_else(|| "TransDatRO".into());
                                        t.run_list();
                                    }
                                    Err(e) => t.stats = tr!("Eroare la citire: {e}"),
                                }
                            }
                            if ui.button(tr!("Transformă")).clicked() {
                                t.run_list();
                            }
                            if ui
                                .add_enabled(!t.list_out.is_empty(), egui::Button::new(tr!("Salvează rezultatul…")))
                                .clicked()
                                && let Some(p) = rfd::FileDialog::new()
                                    .set_file_name(tr!("REZULTAT.txt"))
                                    .add_filter("Text", &["txt"])
                                    .save_file()
                            {
                                let p: PathBuf = p;
                                t.stats = match std::fs::write(&p, t.list_out.as_bytes()) {
                                    Ok(_) => tr!("Salvat: {}", p.display()),
                                    Err(e) => tr!("Eroare la salvare: {e}"),
                                };
                            }
                            if ui
                                .add_enabled(!t.points.is_empty(), egui::Button::new(tr!("Adaugă pe hartă")))
                                .on_hover_text(tr!("Creează un strat de puncte cu rezultatele"))
                                .clicked()
                            {
                                add_layer = t.result_layer();
                            }
                            if ui.button(tr!("Copiază")).clicked() {
                                ui.ctx().copy_text(t.list_out.clone());
                            }
                        });
                        if !t.stats.is_empty() {
                            ui.label(&t.stats);
                        }
                        ui.columns(2, |cols| {
                            egui::ScrollArea::vertical()
                                .id_salt("tdr_lin")
                                .max_height(220.0)
                                .show(&mut cols[0], |ui| {
                                    ui.add(
                                        egui::TextEdit::multiline(&mut t.list_in)
                                            .desired_rows(10)
                                            .desired_width(f32::INFINITY)
                                            .font(egui::TextStyle::Monospace)
                                            .hint_text("PROIECT: exemplu\nP1, 47 42 56.40000, 22 28 32.00000, 162.000\nENDF"),
                                    );
                                });
                            egui::ScrollArea::vertical()
                                .id_salt("tdr_lout")
                                .max_height(220.0)
                                .show(&mut cols[1], |ui| {
                                    ui.add(
                                        egui::TextEdit::multiline(&mut t.list_out.as_str())
                                            .desired_rows(10)
                                            .desired_width(f32::INFINITY)
                                            .font(egui::TextStyle::Monospace),
                                    );
                                });
                        });
                    });
                ui.separator();
                egui::CollapsingHeader::new(tr!("Setări și griduri")).show(ui, |ui| {
                    if ui
                        .checkbox(
                            &mut tdr_on,
                            tr!("Folosește TransDatRO la reproiecția datelor din/în Stereo 70"),
                        )
                        .on_hover_text(tr!("Altfel se folosesc parametrii Helmert cu 7 parametri (EPSG:15994), diferențe de ordinul 1 m"))
                        .changed()
                    {
                        settings_changed = true;
                    }
                    if ui
                        .checkbox(&mut egg_only, tr!("Altitudini doar cu cvasigeoidul EGG97 (fără gridul București-Ilfov)"))
                        .changed()
                    {
                        settings_changed = true;
                    }
                    let g = tdr::grids();
                    for gr in [&g.st70, &g.st30, &g.egg97, &g.bucx] {
                        ui.label(
                            RichText::new(format!("{} — {}", gr.name, gr.source))
                                .small()
                                .weak(),
                        );
                    }
                    ui.label(
                        RichText::new(
                            tr!("Griduri mai noi publicate de ANCPI se pun în dosarul „grile” de lângă Cadastru.exe."),
                        )
                        .small()
                        .weak(),
                    );
                });
            });
        if settings_changed {
            crs::set_transdatro(tdr_on);
            crs::set_height_model(if egg_only {
                HeightModel::Egg97
            } else {
                HeightModel::Auto
            });
            self.settings.transdatro = tdr_on;
            self.settings.height_egg97_only = egg_only;
            self.settings.save();
            self.dlg.tdr.compute();
            self.refresh_projections();
        }
        if !open {
            self.dlg.tdr.open = false;
            self.dlg.tdr.pick_from_map = false;
        }
        if let Some(l) = add_layer {
            let seq = self.next_seq();
            let n = l.features.len();
            let ids = self.add_datasets(
                std::path::Path::new("transdatro.txt"),
                vec![Dataset::Vector(l)],
                seq,
            );
            if let Some(b) = ids
                .first()
                .and_then(|id| self.layer(*id))
                .map(|l| l.map_bounds())
                .filter(|b| b.is_valid())
            {
                self.zoom_to(b);
            }
            self.status = tr!("Strat TransDatRO adăugat ({n} puncte)");
        }
    }

    /// Recalculează afișarea tuturor straturilor (după schimbarea metodei de transformare).
    pub fn refresh_projections(&mut self) {
        let mc = self.map_crs.clone();
        self.to_wgs = mc
            .as_ref()
            .and_then(|m| Transformer::new(m, &Crs::wgs84()).ok());
        for l in &mut self.layers {
            l.rebuild(mc.as_ref());
        }
    }
}
