//! Fereastra modulului SIRUTA (meniul Nomenclatoare > SIRUTA), preluată din proiectul SIRUTA
//! cu toate facilitățile: căutare, filtre, tabel, detalii, copiere, actualizare automată,
//! funcționare offline, bara de stare și fereastra Despre.

use super::data::{self, Dataset, Entry};
use super::ui::{self, *};
use super::update::{self, Meta, Msg};
use cadastru::i18n::{is_en, tr, trs_static};
use chrono::{DateTime, Local, NaiveDate};
use eframe::egui::{self, Align, Color32, CornerRadius, Layout, RichText, Stroke, vec2};
use egui_extras::{Column, TableBuilder};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};

/// Versiunea modulului SIRUTA preluat (proiectul SIRUTA de sine stătător).
pub const SIRUTA_VERSION: &str = "1.0.0";

enum NetState {
    Checking,
    UpToDate,
    Updated,
    Offline(String),
}

pub struct SirutaApp {
    ds: Option<Dataset>,
    meta: Meta,
    from_seed: bool,
    query: String,
    judet: Option<u16>,
    include_loc: bool,
    results: Vec<usize>,
    dirty: bool,
    selected: Option<usize>,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    net: NetState,
    progress: String,
    logo: egui::TextureHandle,
    toast: Option<(String, f64)>,
    search_focused: bool,
    first_frame: bool,
    /// Rândul al cărui cod tocmai a fost copiat (SIRUTA, până când arătăm bifa).
    copied_flash: Option<(u32, f64)>,
    show_about: bool,
    /// Limba textului licentei in fereastra Despre (true = romana).
    license_ro: bool,
    /// Numarul de cadre desenate (pentru maximizarea de la pornire).
    frames: u32,
    /// Stilul ferestrei SIRUTA (calculat din stilul aplicației, la prima afișare).
    style: Option<Arc<egui::Style>>,
}

impl SirutaApp {
    pub fn new(ctx: &egui::Context) -> Self {
        let logo = ctx.load_texture(
            "siruta_logo",
            egui::ColorImage::from_rgba_unmultiplied([128, 128], super::ICON_RGBA),
            egui::TextureOptions::LINEAR,
        );
        let (tx, rx) = channel();
        let (ds, meta, from_seed) = match update::load_local() {
            Some((d, m, s)) => (Some(d), m, s),
            None => (None, update::load_meta(), false),
        };
        // La fiecare pornire: verificare si actualizare automata in fundal.
        update::spawn_update(tx.clone(), ctx.clone(), false);
        Self {
            ds,
            meta,
            from_seed,
            query: String::new(),
            judet: None,
            include_loc: false,
            results: Vec::new(),
            dirty: true,
            selected: None,
            tx,
            rx,
            net: NetState::Checking,
            progress: tr!("Se verifică actualizările…").into(),
            logo,
            toast: None,
            search_focused: false,
            first_frame: true,
            copied_flash: None,
            show_about: false,
            license_ro: true,
            frames: 0,
            style: None,
        }
    }

    /// La redeschiderea ferestrei: din nou maximizată, cu focus pe căutare, și o nouă
    /// verificare a actualizărilor (descarcă doar dacă INS a publicat o ediție nouă).
    pub fn reopen(&mut self, ctx: &egui::Context) {
        self.frames = 0;
        self.first_frame = true;
        self.start_update(ctx, false);
    }

    fn start_update(&mut self, ctx: &egui::Context, force: bool) {
        if matches!(self.net, NetState::Checking) {
            return;
        }
        self.net = NetState::Checking;
        self.progress = tr!("Se verifică actualizările…").into();
        update::spawn_update(self.tx.clone(), ctx.clone(), force);
    }

    fn poll(&mut self) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Msg::Progress(p) => self.progress = p,
                Msg::Done(Ok(ok)) => {
                    self.meta = ok.meta;
                    self.from_seed = false;
                    if let Some(ds) = ok.dataset {
                        self.ds = Some(ds);
                        self.dirty = true;
                        self.selected = None;
                        self.net = NetState::Updated;
                    } else {
                        self.net = NetState::UpToDate;
                    }
                }
                Msg::Done(Err(e)) => self.net = NetState::Offline(e),
            }
        }
    }

    fn toast(&mut self, ctx: &egui::Context, msg: String) {
        self.toast = Some((msg, ctx.input(|i| i.time) + 2.2));
    }

    fn copy(&mut self, ctx: &egui::Context, text: String, label: &str) {
        ctx.copy_text(text.clone());
        self.toast(ctx, tr!("Copiat {label}: {text}"));
    }

    // ---------------------------------------------------------------- header
    fn header(&mut self, ui: &mut egui::Ui) {
        let ctx = &ui.ctx().clone();
        egui::Panel::top("siruta_header")
            .frame(
                egui::Frame::new()
                    .fill(PANEL)
                    .inner_margin(egui::Margin::symmetric(18, 12))
                    .stroke(Stroke::new(1.0f32, BORDER)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.add(egui::Image::new(&self.logo).fit_to_exact_size(vec2(46.0, 46.0)));
                    ui.add_space(6.0);
                    ui.vertical(|ui| {
                        ui.add_space(2.0);
                        ui.label(RichText::new("SIRUTA").font(bold(26.0)).color(CYAN));
                        ui.label(
                            RichText::new(
                                tr!("Registrul unităților administrativ-teritoriale ale României · date oficiale INS"),
                            )
                            .color(DIM)
                            .size(12.5),
                        );
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let checking = matches!(self.net, NetState::Checking);
                        let angle = if checking {
                            ctx.request_repaint();
                            (ctx.input(|i| i.time) * 5.0) as f32
                        } else {
                            0.0
                        };
                        let r = ui::icon_button(ui, 40.0, CYAN, |p, rect, c| {
                            ui::icon_refresh(p, rect, c, angle)
                        })
                        .on_hover_text(tr!("Actualizează acum lista SIRUTA de la INS (data.gov.ro)"));
                        if r.clicked() {
                            self.start_update(ctx, true);
                        }
                        ui.add_space(4.0);
                        let a = ui::icon_button(ui, 40.0, MAGENTA, ui::icon_info)
                            .on_hover_text(tr!("Despre SIRUTA (autor, licență)"));
                        if a.clicked() {
                            self.show_about = true;
                        }
                    });
                });
            });
    }

    // ---------------------------------------------------------------- status bar
    fn status_bar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::bottom("siruta_status")
            .frame(
                egui::Frame::new()
                    .fill(PANEL)
                    .inner_margin(egui::Margin::symmetric(14, 8))
                    .stroke(Stroke::new(1.0f32, BORDER)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        match &self.net {
                            NetState::Checking => {
                                ui.add(egui::Spinner::new().size(16.0).color(CYAN));
                                ui.label(RichText::new(&self.progress).color(CYAN));
                            }
                            NetState::UpToDate => {
                                let (r, _) = ui.allocate_exact_size(vec2(28.0, 22.0), egui::Sense::hover());
                                ui::icon_cloud(ui.painter(), r, GREEN, true);
                                ui.label(RichText::new(tr!("Online · datele sunt la zi")).color(GREEN));
                            }
                            NetState::Updated => {
                                let (r, _) = ui.allocate_exact_size(vec2(28.0, 22.0), egui::Sense::hover());
                                ui::icon_cloud(ui.painter(), r, GREEN, true);
                                ui.label(RichText::new(tr!("Online · lista a fost actualizată")).color(GREEN));
                            }
                            NetState::Offline(err) => {
                                let err = err.clone();
                                let (r, _) = ui.allocate_exact_size(vec2(28.0, 22.0), egui::Sense::hover());
                                ui::icon_cloud(ui.painter(), r, AMBER, false);
                                let txt = if self.ds.is_some() {
                                    tr!("Offline · se folosesc datele salvate local")
                                } else {
                                    tr!("Offline · nu există date locale")
                                };
                                ui.label(RichText::new(txt).color(AMBER)).on_hover_text(err);
                            }
                        }
                        ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                        // ultima actualizare reusita
                        let (r, _) = ui.allocate_exact_size(vec2(18.0, 18.0), egui::Sense::hover());
                        ui::icon_clock(ui.painter(), r, CYAN);
                        let last = match self.meta.last_success_local() {
                            Some(dt) => RichText::new(tr!(
                                "Ultima actualizare reușită: {} ({})",
                                dt.format("%d.%m.%Y, %H:%M"),
                                ago(dt)
                            ))
                            .color(TEXT),
                            None => RichText::new(tr!("Ultima actualizare reușită: niciuna încă")).color(AMBER),
                        };
                        ui.add(egui::Label::new(last).truncate());

                        if !self.meta.resource_name.is_empty() {
                            ui.add_space(10.0);
                            let (r, _) = ui.allocate_exact_size(vec2(18.0, 18.0), egui::Sense::hover());
                            ui::icon_database(ui.painter(), r, VIOLET);
                            let mut src = tr!("Sursa: INS · {}", self.meta.resource_name);
                            if let Some(d) = fmt_date(&self.meta.resource_modified) {
                                src.push_str(&tr!(" (publicat {d})"));
                            }
                            ui.add(egui::Label::new(RichText::new(src).color(DIM)).truncate())
                                .on_hover_text(&self.meta.resource_url);
                        }
                        if let Some(ds) = &self.ds {
                            ui.add_space(6.0);
                            ui.add(
                                egui::Label::new(
                                    RichText::new(tr!(
                                        "· {} UAT · {} localități",
                                        fmt_int(ds.uat_count),
                                        fmt_int(ds.loc_count)
                                    ))
                                    .color(DIM),
                                )
                                .truncate(),
                            );
                        }

                        });
                    });
                });
            });
    }

    // ---------------------------------------------------------------- search
    fn search_bar(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let stroke_col = if self.search_focused { CYAN } else { BORDER };
        let frame = egui::Frame::new()
            .fill(Color32::from_rgb(0x12, 0x14, 0x18))
            .corner_radius(CornerRadius::same(10))
            .stroke(Stroke::new(1.5f32, stroke_col))
            .inner_margin(egui::Margin::symmetric(12, 8));
        let resp = frame.show(ui, |ui| {
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(vec2(24.0, 24.0), egui::Sense::hover());
                ui::icon_search(ui.painter(), r, if self.search_focused { CYAN } else { DIM });
                ui.add_space(4.0);
                let clear_w = if self.query.is_empty() { 0.0 } else { 34.0 };
                let te = egui::TextEdit::singleline(&mut self.query)
                    .frame(egui::Frame::NONE)
                    .font(egui::FontId::proportional(19.0))
                    .hint_text(
                        RichText::new(tr!("Caută UAT după denumire sau cod SIRUTA (ex: Cluj-Napoca, Bucium, 54975)"))
                            .color(alpha(DIM, 160)),
                    )
                    .desired_width(ui.available_width() - clear_w);
                let out = ui.add(te);
                if self.first_frame {
                    out.request_focus();
                }
                if out.changed() {
                    self.dirty = true;
                }
                self.search_focused = out.has_focus();
                if out.has_focus() && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                    self.query.clear();
                    self.dirty = true;
                }
                if out.lost_focus() && ctx.input(|i| i.key_pressed(egui::Key::Enter)) {
                    self.selected = self.results.first().copied();
                }
                if !self.query.is_empty() {
                    let b = ui::close_button(ui, 26.0).on_hover_text(tr!("Șterge căutarea (Esc)"));
                    if b.clicked() {
                        self.query.clear();
                        self.dirty = true;
                        out.request_focus();
                    }
                }
            });
        });
        if self.search_focused {
            ui::glow_rect(ui.painter(), resp.response.rect, 10, CYAN, 0.8);
        }
    }

    fn filters(&mut self, ui: &mut egui::Ui) {
        let Some(ds) = &self.ds else { return };
        ui.horizontal(|ui| {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let n = self.results.len();
                let txt = format!(
                    "{} {}",
                    fmt_int(n),
                    if n == 1 { tr!("rezultat") } else { tr!("rezultate") }
                );
                let galley = ui.painter().layout_no_wrap(txt, egui::FontId::proportional(13.0), MAGENTA);
                let (rect, _) = ui.allocate_exact_size(galley.size() + vec2(22.0, 8.0), egui::Sense::hover());
                ui.painter().rect_filled(rect, CornerRadius::same(12), alpha(MAGENTA, 28));
                ui.painter().rect_stroke(rect, CornerRadius::same(12), Stroke::new(1.0f32, MAGENTA), egui::StrokeKind::Inside);
                ui.painter().galley(rect.center() - galley.size() / 2.0, galley, MAGENTA);
                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                ui.label(RichText::new(tr!("Județ:")).color(DIM));
                let sel = match self.judet {
                    None => tr!("Toate județele").to_string(),
                    Some(j) => ds
                        .judete
                        .iter()
                        .find(|x| x.0 == j)
                        .map(|x| x.1.clone())
                        .unwrap_or_default(),
                };
                let before = self.judet;
                egui::ComboBox::from_id_salt("siruta_judet")
                    .selected_text(sel)
                    .width(210.0)
                    .height(420.0)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.judet, None, tr!("Toate județele"));
                        for (code, name) in &ds.judete {
                            ui.selectable_value(&mut self.judet, Some(*code), name);
                        }
                    });
                if before != self.judet {
                    self.dirty = true;
                }
                ui.add_space(12.0);
                if ui
                    .checkbox(
                        &mut self.include_loc,
                        tr!("Include și localitățile (sate, localități componente)"),
                    )
                    .changed()
                {
                    self.dirty = true;
                }

                });
            });
        });
    }

    // ---------------------------------------------------------------- table
    fn table(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let Some(ds) = &self.ds else { return };
        let show_parent = self.include_loc;
        let mut clicked: Option<usize> = None;
        let mut copy_req: Option<(String, &'static str)> = None;
        let mut flash_req: Option<u32> = None;
        let now = ctx.input(|i| i.time);
        let flash = self.copied_flash.filter(|(_, until)| now < *until).map(|(s, _)| s);

        let mut tb = TableBuilder::new(ui)
            .striped(true)
            .resizable(true)
            .sense(egui::Sense::click())
            .cell_layout(Layout::left_to_right(Align::Center))
            .column(Column::exact(96.0))
            .column(Column::initial(260.0).at_least(140.0).clip(true))
            .column(Column::initial(150.0).at_least(90.0).clip(true));
        if show_parent {
            tb = tb
                .column(Column::initial(370.0).at_least(220.0).clip(true))
                .column(Column::remainder().at_least(140.0).clip(true));
        } else {
            tb = tb.column(Column::remainder().at_least(300.0).clip(true));
        }

        let head = |ui: &mut egui::Ui, t: &str| {
            ui.label(RichText::new(t).font(bold(13.5)).color(CYAN));
        };
        tb.header(30.0, |mut h| {
            h.col(|ui| head(ui, tr!("COD SIRUTA")));
            h.col(|ui| head(ui, tr!("DENUMIRE")));
            h.col(|ui| head(ui, tr!("JUDEȚ")));
            h.col(|ui| head(ui, tr!("CLASIFICARE")));
            if show_parent {
                h.col(|ui| head(ui, tr!("UAT DE APARTENENȚĂ")));
            }
        })
        .body(|body| {
            body.rows(28.0, self.results.len(), |mut row| {
                let idx = self.results[row.index()];
                let e: &Entry = &ds.entries[idx];
                row.set_selected(self.selected == Some(idx));
                row.col(|ui| {
                    ui.label(RichText::new(&e.siruta_str).monospace().color(CYAN));
                });
                row.col(|ui| {
                    ui.label(RichText::new(&e.name).font(bold(14.5)).color(TEXT));
                });
                row.col(|ui| {
                    ui.label(RichText::new(&e.judet).color(TEXT));
                });
                row.col(|ui| {
                    badge(ui, trs_static(e.clasificare), kind_color(e.kind));
                    ui.add_space(6.0);
                    let r = if flash == Some(e.siruta) {
                        ui::icon_button(ui, 26.0, GREEN, ui::icon_check)
                            .on_hover_text(tr!("Copiat în clipboard"))
                    } else {
                        ui::icon_button(ui, 26.0, CYAN, ui::icon_copy)
                            .on_hover_text(tr!("Copiază codul SIRUTA {} în clipboard", e.siruta))
                    };
                    if r.clicked() {
                        copy_req = Some((e.siruta_str.clone(), tr!("codul SIRUTA")));
                        flash_req = Some(e.siruta);
                    }
                });
                if show_parent {
                    row.col(|ui| {
                        if !e.parent.is_empty() {
                            ui.label(RichText::new(&e.parent).color(DIM));
                        }
                    });
                }
                let r = row.response();
                if r.clicked() {
                    clicked = Some(idx);
                }
                if r.double_clicked() {
                    copy_req = Some((e.siruta_str.clone(), tr!("codul SIRUTA")));
                }
                r.context_menu(|ui| {
                    if ui.button(tr!("Copiază codul SIRUTA")).clicked() {
                        copy_req = Some((e.siruta_str.clone(), tr!("codul SIRUTA")));
                        ui.close();
                    }
                    if ui.button(tr!("Copiază denumirea")).clicked() {
                        copy_req = Some((e.name.clone(), tr!("denumirea")));
                        ui.close();
                    }
                    if ui.button(tr!("Copiază rândul (separat cu tab)")).clicked() {
                        copy_req = Some((
                            format!("{}\t{}\t{}\t{}", e.siruta, e.name, e.judet, e.clasificare),
                            tr!("rândul"),
                        ));
                        ui.close();
                    }
                });
            });
        });
        if let Some(i) = clicked {
            self.selected = Some(i);
        }
        if let Some(s) = flash_req {
            self.copied_flash = Some((s, now + 1.5));
        }
        if flash.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(200));
        }
        if let Some((t, l)) = copy_req {
            self.copy(&ctx, t, l);
        }
    }

    // ---------------------------------------------------------------- details
    fn details(&mut self, ui: &mut egui::Ui) {
        let ctx = &ui.ctx().clone();
        let Some(idx) = self.selected else { return };
        let Some(ds) = &self.ds else { return };
        let Some(e) = ds.entries.get(idx).cloned() else { return };
        let mut close = false;
        let mut copy_req: Option<(String, &'static str)> = None;
        egui::Panel::right("siruta_details")
            .resizable(false)
            .exact_size(300.0)
            .frame(
                egui::Frame::new()
                    .fill(PANEL)
                    .inner_margin(egui::Margin::same(16))
                    .stroke(Stroke::new(1.0f32, BORDER)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(vec2(28.0, 28.0), egui::Sense::hover());
                    ui::icon_pin(ui.painter(), r, kind_color(e.kind));
                    ui.label(RichText::new(tr!("Detalii")).font(bold(17.0)).color(TEXT));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui::close_button(ui, 24.0).on_hover_text(tr!("Închide")).clicked()
                        {
                            close = true;
                        }
                    });
                });
                ui.add_space(8.0);
                ui.label(RichText::new(&e.name).font(bold(22.0)).color(kind_color(e.kind)));
                ui.add_space(4.0);
                badge(ui, trs_static(e.clasificare), kind_color(e.kind));
                ui.add_space(12.0);
                let mut field = |ui: &mut egui::Ui, label: &str, value: String, copy: Option<&'static str>| {
                    ui.label(RichText::new(label).color(DIM).size(12.0));
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(&value).color(TEXT).size(15.0));
                        if let Some(l) = copy {
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if ui::icon_button(ui, 24.0, CYAN, ui::icon_copy)
                                    .on_hover_text(tr!("Copiază"))
                                    .clicked()
                                {
                                    copy_req = Some((value.clone(), l));
                                }
                            });
                        }
                    });
                    ui.add_space(6.0);
                };
                field(ui, tr!("COD SIRUTA"), e.siruta_str.clone(), Some(tr!("codul SIRUTA")));
                field(ui, tr!("DENUMIRE OFICIALĂ (DENLOC)"), e.denloc.clone(), Some(tr!("denumirea")));
                field(ui, tr!("JUDEȚ"), tr!("{} (cod JUD {})", e.judet, e.jud_code), None);
                if !e.parent.is_empty() {
                    field(ui, tr!("UAT DE APARTENENȚĂ"), e.parent.clone(), None);
                }
                field(ui, tr!("COD SIRUTA SUPERIOR (SIRSUP)"), e.sirsup.to_string(), Some(tr!("codul SIRSUP")));
                field(
                    ui,
                    tr!("NIVEL / TIP (legenda INS)"),
                    format!("NIV {} · TIP {}", e.niv, e.tip),
                    None,
                );
            });
        if close {
            self.selected = None;
        }
        if let Some((t, l)) = copy_req {
            self.copy(ctx, t, l);
        }
    }
}

impl SirutaApp {
    /// Desenează fereastra SIRUTA. Tema neon se aplică numai pe durata acestei ferestre;
    /// la final se revine la stilul aplicației Cadastru.
    pub fn ui(&mut self, root: &mut egui::Ui) {
        let ctx = &root.ctx().clone();
        let theme = ctx.theme();
        let prev = ctx.style_of(theme);
        let style = self
            .style
            .get_or_insert_with(|| Arc::new(ui::style(&prev)))
            .clone();
        ctx.set_style_of(theme, style.clone());
        root.set_style(style);
        self.frame(ctx, root);
        ctx.set_style_of(theme, prev);
    }

    fn frame(&mut self, ctx: &egui::Context, root: &mut egui::Ui) {
        // Pornire maximizata: Windows ignora maximizarea ceruta cat timp fereastra
        // este inca ascunsa (eframe o afiseaza abia dupa primul cadru), asa ca o
        // cerem din nou in primele cadre, dupa ce fereastra a devenit vizibila.
        if self.frames < 3 {
            ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(true));
            ctx.request_repaint();
        }
        self.frames = self.frames.saturating_add(1);
        self.poll();
        if self.dirty {
            if let Some(ds) = &self.ds {
                self.results = data::search(ds, &self.query, self.judet, self.include_loc);
            }
            self.dirty = false;
        }

        self.header(root);
        self.status_bar(root);
        self.about(ctx);
        self.details(root);

        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(BG).inner_margin(egui::Margin::same(16)))
            .show(root, |ui| {
                if self.ds.is_none() {
                    self.empty_state(ui);
                    return;
                }
                self.search_bar(ui);
                ui.add_space(10.0);
                self.filters(ui);
                if self.from_seed {
                    ui.label(
                        RichText::new(tr!("Se folosește fișierul siruta.csv de lângă aplicație până la prima actualizare reușită."))
                            .color(AMBER)
                            .size(12.0),
                    );
                }
                ui.add_space(6.0);
                egui::Frame::new()
                    .fill(PANEL)
                    .corner_radius(CornerRadius::same(10))
                    .stroke(Stroke::new(1.0f32, BORDER))
                    .inner_margin(egui::Margin::symmetric(10, 6))
                    .show(ui, |ui| {
                        if self.results.is_empty() {
                            ui.add_space(30.0);
                            ui.vertical_centered(|ui| {
                                ui.label(
                                    RichText::new(tr!("Niciun rezultat pentru căutarea curentă"))
                                        .color(DIM)
                                        .size(16.0),
                                );
                                if !self.include_loc {
                                    ui.label(
                                        RichText::new(tr!("Sfat: bifați „Include și localitățile” pentru a căuta și satele."))
                                            .color(alpha(DIM, 170))
                                            .size(13.0),
                                    );
                                }
                            });
                            ui.add_space(ui.available_height());
                        } else {
                            self.table(ui);
                        }
                    });
            });

        // notificare scurta (ex. "Copiat")
        if let Some((msg, until)) = self.toast.clone() {
            let now = ctx.input(|i| i.time);
            if now > until {
                self.toast = None;
            } else {
                ctx.request_repaint();
                egui::Area::new(egui::Id::unique("siruta_toast"))
                    .anchor(egui::Align2::CENTER_BOTTOM, vec2(0.0, -56.0))
                    .order(egui::Order::Foreground)
                    .show(ctx, |ui| {
                        egui::Frame::new()
                            .fill(PANEL2)
                            .corner_radius(CornerRadius::same(18))
                            .stroke(Stroke::new(1.0f32, GREEN))
                            .inner_margin(egui::Margin::symmetric(16, 8))
                            .show(ui, |ui| {
                                ui.label(RichText::new(msg).color(GREEN));
                            });
                    });
            }
        }
        self.first_frame = false;
    }
}

impl SirutaApp {
    fn empty_state(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        ui.vertical_centered(|ui| {
            ui.add_space(ui.available_height() * 0.25);
            let (r, _) = ui.allocate_exact_size(vec2(90.0, 70.0), egui::Sense::hover());
            match &self.net {
                NetState::Checking => {
                    ui::icon_cloud(ui.painter(), r, CYAN, true);
                    ui.add_space(10.0);
                    ui.label(RichText::new(tr!("Se descarcă lista SIRUTA de la INS…")).size(18.0).color(CYAN));
                    ui.add(egui::Spinner::new().size(26.0).color(CYAN));
                }
                NetState::Offline(err) => {
                    ui::icon_cloud(ui.painter(), r, AMBER, false);
                    ui.add_space(10.0);
                    ui.label(
                        RichText::new(tr!("Nu există încă date salvate local și nu s-a putut contacta INS."))
                            .size(17.0)
                            .color(AMBER),
                    );
                    ui.label(RichText::new(err).color(DIM));
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new(tr!("Conectați-vă la internet o singură dată; după aceea aplicația funcționează și offline."))
                            .color(DIM),
                    );
                    ui.add_space(10.0);
                    if ui.button(RichText::new(tr!("  Reîncearcă  ")).size(15.0)).clicked() {
                        self.start_update(&ctx, true);
                    }
                }
                _ => {}
            }
        });
    }
}

const MIT_LICENSE_RO: &str = "Licența MIT

Drepturi de autor (c) 2026 Ginu Popescu

Se acordă prin prezenta permisiunea, în mod gratuit, oricărei persoane care obține o copie a acestui software și a fișierelor de documentație asociate (denumite în continuare „Software-ul”), de a folosi Software-ul fără restricții, inclusiv, fără a se limita la, drepturile de a utiliza, copia, modifica, îmbina, publica, distribui, sublicenția și/sau vinde copii ale Software-ului, precum și de a permite persoanelor cărora le este furnizat Software-ul să facă același lucru, cu respectarea următoarelor condiții:

Notificarea privind drepturile de autor de mai sus și prezenta notificare de permisiune trebuie incluse în toate copiile sau în toate părțile substanțiale ale Software-ului.

SOFTWARE-UL ESTE FURNIZAT „CA ATARE”, FĂRĂ NICIO GARANȚIE DE NICIUN FEL, EXPRESĂ SAU IMPLICITĂ, INCLUSIV, DAR FĂRĂ A SE LIMITA LA, GARANȚIILE DE VANDABILITATE, DE ADECVARE PENTRU UN ANUMIT SCOP ȘI DE NEÎNCĂLCARE A DREPTURILOR. ÎN NICIUN CAZ AUTORII SAU DEȚINĂTORII DREPTURILOR DE AUTOR NU VOR FI RĂSPUNZĂTORI PENTRU NICIO PRETENȚIE, DAUNĂ SAU ALTĂ RĂSPUNDERE, FIE ÎNTR-O ACȚIUNE CONTRACTUALĂ, DELICTUALĂ SAU DE ALTĂ NATURĂ, CARE DECURGE DIN SAU ESTE ÎN LEGĂTURĂ CU SOFTWARE-UL ORI CU UTILIZAREA SAU ALTE OPERAȚIUNI CU SOFTWARE-UL.";

const MIT_LICENSE: &str = "MIT License

Copyright (c) 2026 Ginu Popescu

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the \"Software\"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED \"AS IS\", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.";

impl SirutaApp {
    // ---------------------------------------------------------------- despre
    fn about(&mut self, ctx: &egui::Context) {
        if !self.show_about {
            return;
        }
        let modal = egui::Modal::new(egui::Id::unique("siruta_despre"))
            .frame(
                egui::Frame::new()
                    .fill(PANEL)
                    .corner_radius(CornerRadius::same(14))
                    .stroke(Stroke::new(1.5f32, CYAN))
                    .inner_margin(egui::Margin::same(22)),
            )
            .show(ctx, |ui| {
                ui.set_width(560.0);
                ui.horizontal(|ui| {
                    ui.add(egui::Image::new(&self.logo).fit_to_exact_size(vec2(64.0, 64.0)));
                    ui.add_space(8.0);
                    ui.vertical(|ui| {
                        ui.label(RichText::new("SIRUTA").font(bold(28.0)).color(CYAN));
                        ui.label(
                            RichText::new(tr!(
                                "Versiunea {} · modul integrat în Cadastru {}",
                                SIRUTA_VERSION,
                                env!("CARGO_PKG_VERSION")
                            ))
                            .color(DIM),
                        );
                    });
                });
                ui.add_space(8.0);
                ui.label(
                    RichText::new(tr!(
                        "Căutare în Registrul unităților administrativ-teritoriale ale României (SIRUTA), pe baza datelor oficiale publicate de Institutul Național de Statistică."
                    ))
                    .color(TEXT),
                );
                ui.add_space(12.0);
                egui::Grid::new("siruta_despre_grid")
                    .num_columns(2)
                    .spacing(vec2(14.0, 8.0))
                    .show(ui, |ui| {
                        ui.label(RichText::new(tr!("Autor")).color(DIM));
                        ui.label(RichText::new("Ginu Popescu").font(bold(15.5)).color(TEXT));
                        ui.end_row();
                        ui.label(RichText::new(tr!("E-mail")).color(DIM));
                        ui.hyperlink_to(
                            RichText::new("scriemiaici@protonmail.me").color(CYAN),
                            "mailto:scriemiaici@protonmail.me",
                        );
                        ui.end_row();
                        ui.label(RichText::new(tr!("Licență")).color(DIM));
                        ui.label(RichText::new("MIT").font(bold(15.5)).color(MAGENTA));
                        ui.end_row();
                        ui.label(RichText::new(tr!("Date")).color(DIM));
                        ui.hyperlink_to(
                            RichText::new("INS · data.gov.ro").color(CYAN),
                            "https://data.gov.ro/organization/institutul-national-de-statistica",
                        );
                        ui.end_row();
                    });
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new(tr!("Textul licenței:")).color(DIM));
                    ui.selectable_value(&mut self.license_ro, true, "Română");
                    ui.selectable_value(&mut self.license_ro, false, "English (original)");
                });
                ui.add_space(4.0);
                egui::Frame::new()
                    .fill(Color32::from_rgb(0x12, 0x14, 0x18))
                    .corner_radius(CornerRadius::same(8))
                    .stroke(Stroke::new(1.0f32, BORDER))
                    .inner_margin(egui::Margin::same(10))
                    .show(ui, |ui| {
                        egui::ScrollArea::vertical().max_height(240.0).show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            let src = if self.license_ro { MIT_LICENSE_RO } else { MIT_LICENSE };
                            let text = src
                                .split("\n\n")
                                .map(|p| p.replace('\n', " "))
                                .collect::<Vec<_>>()
                                .join("\n\n");
                            ui.label(RichText::new(text).size(12.5).color(DIM));
                        });
                    });
                if self.license_ro {
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new(
                            tr!("Traducere cu caracter informativ; în caz de neconcordanță, prevalează textul original în limba engleză."),
                        )
                        .italics()
                        .size(11.5)
                        .color(alpha(DIM, 200)),
                    );
                }
                ui.add_space(14.0);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui
                        .add(egui::Button::new(RichText::new(tr!("   Închide   ")).color(CYAN)))
                        .clicked()
                    {
                        self.show_about = false;
                    }
                });
            });
        if modal.should_close() {
            self.show_about = false;
        }
    }
}

fn badge(ui: &mut egui::Ui, text: &str, color: Color32) {
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_string(), egui::FontId::proportional(12.5), color);
    let size = galley.size() + vec2(16.0, 6.0);
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, CornerRadius::same(10), alpha(color, 26));
    p.rect_stroke(rect, CornerRadius::same(10), Stroke::new(1.0f32, alpha(color, 170)), egui::StrokeKind::Inside);
    p.galley(rect.center() - galley.size() / 2.0, galley, color);
}

fn ago(dt: DateTime<Local>) -> String {
    let s = (Local::now() - dt).num_seconds().max(0);
    match s {
        0..=59 => tr!("chiar acum").into(),
        60..=3599 => tr!("acum {} min", s / 60),
        3600..=86399 => {
            let h = s / 3600;
            if h == 1 { tr!("acum o oră").into() } else { tr!("acum {h} ore") }
        }
        _ => {
            let d = s / 86400;
            if d == 1 { tr!("acum o zi").into() } else { tr!("acum {d} zile") }
        }
    }
}

fn fmt_date(iso: &str) -> Option<String> {
    let d = NaiveDate::parse_from_str(iso.get(..10)?, "%Y-%m-%d").ok()?;
    Some(d.format("%d.%m.%Y").to_string())
}

fn fmt_int(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(if is_en() { ',' } else { '.' });
        }
        out.push(ch);
    }
    out
}
