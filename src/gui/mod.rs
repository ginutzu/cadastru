//! Interfața grafică Cadastru (egui/eframe).

mod attrsel;
mod dialogs;
mod ecw;
mod layers;
mod basemap;
mod bulk;
mod manual;
mod loadfilter;
mod parcel;
mod map;
mod print;
mod search;
mod select;
mod split;
mod tdr;
pub mod theme;
mod google;
mod workspace;

use cadastru::crs::{Crs, Transformer};
use cadastru::formats::{self, ReadOptions};
use cadastru::model::*;
use dialogs::*;
use eframe::egui::{self, Color32, Key, RichText};
use layers::*;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};
use theme::{Icon, Settings, Theme, icon_button, menu_item, small_icon_button, tool_dropdown};
use cadastru::i18n::tr;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Pan,
    ZoomIn,
    ZoomOut,
    Identify,
    Measure,
    Select,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    pub cx: f64,
    pub cy: f64,
    /// unități de hartă pe pixel
    pub scale: f64,
}

pub struct IdentifyResult {
    pub layer_id: u64,
    pub layer: String,
    pub feat: Option<usize>,
    pub rows: Vec<(String, String)>,
}

pub enum JobResult {
    Opened {
        path: PathBuf,
        result: anyhow::Result<(Vec<Dataset>, Vec<String>)>,
        opts: ReadOptions,
        seq: u64,
    },
    Exported(anyhow::Result<Vec<PathBuf>>),
    /// un fișier din deschiderea în masă
    BulkItem {
        path: PathBuf,
        rel: PathBuf,
        result: anyhow::Result<Vec<bulk::Prepared>>,
        seq: u64,
        rel_names: bool,
    },
    /// sfârșitul deschiderii în masă
    BulkDone,
    Log(String),
    Done(String),
}

pub struct Job {
    pub label: String,
    pub rx: Receiver<JobResult>,
}

/// Lățimea benzii rămase când panoul Straturi e ascuns.
const LAYERS_STRIP: f32 = 26.0;

pub struct App {
    pub layers: Vec<LayerEntry>,
    pub next_id: u64,
    pub selected_layer: Option<u64>,
    pub map_crs: Option<Crs>,
    pub view: View,
    pub view_initialized: bool,
    pub tool: Tool,
    pub status: String,
    pub cursor_map: Option<(f64, f64)>,
    pub selection: Option<(u64, usize)>,
    pub identify: Vec<IdentifyResult>,
    pub show_identify: bool,
    /// cerere de afișare / ascundere a panoului Straturi, aplicată după desenarea panoului
    pub layers_toggle: Option<bool>,
    pub measure: Vec<(f64, f64)>,
    pub measure_done: bool,
    pub zoom_box_start: Option<egui::Pos2>,
    /// deschiderea în masă în curs
    pub bulk: Option<bulk::BulkRun>,
    /// marcajul locului găsit cu Căutare
    pub search_marker: Option<search::Marker>,
    pub jobs: Vec<Job>,
    pub last_interaction: f64,
    pub map_rect: egui::Rect,
    pub to_wgs: Option<Transformer>,
    pub workspace_path: Option<PathBuf>,
    pub dlg: Dialogs,
    pub errors: Vec<String>,
    pub show_labels: bool,
    pub pending_workspace: Option<PathBuf>,
    pub open_seq: u64,
    pub settings: Settings,
    pub show_cgxml_opts: bool,
    /// istoricul vederilor (înapoi / înainte)
    pub history: Vec<View>,
    pub history_pos: usize,
    /// direcția și momentul apăsării pe controlul de deplasare
    pub nav_press: Option<(egui::Vec2, f64)>,
    /// harta de fundal
    pub basemap: basemap::Basemap,
    /// fereastra pentru șablonul XYZ personalizat
    pub show_bm_custom: bool,
    pub bm_custom_text: String,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, files: Vec<PathBuf>) -> App {
        let ctx = &cc.egui_ctx;
        let settings = Settings::load();
        cadastru::i18n::set_lang(settings.lang);
        {
            let p = settings.gdal_path.trim();
            cadastru::formats::gdalrt::set_hint((!p.is_empty()).then(|| PathBuf::from(p)));
        }
        let bm_opacity = settings.basemap_opacity;
        let bm_custom = settings.basemap_custom.clone();
        cadastru::crs::set_transdatro(settings.transdatro);
        cadastru::crs::set_height_model(if settings.height_egg97_only {
            cadastru::transdatro::HeightModel::Egg97
        } else {
            cadastru::transdatro::HeightModel::Auto
        });
        theme::apply(ctx, settings.theme);
        setup_fonts(ctx);
        ctx.global_style_mut(|s| {
            s.spacing.item_spacing = egui::vec2(6.0, 4.0);
        });
        let mut app = App {
            layers: vec![],
            next_id: 1,
            selected_layer: None,
            map_crs: None,
            view: View {
                cx: 500000.0,
                cy: 500000.0,
                scale: 1000.0,
            },
            view_initialized: false,
            tool: Tool::Pan,
            status: tr!("Gata. Deschideți fișiere din meniul Fișier sau trageți-le în fereastră.")
                .into(),
            cursor_map: None,
            selection: None,
            identify: vec![],
            show_identify: false,
            layers_toggle: None,
            measure: vec![],
            measure_done: false,
            zoom_box_start: None,
            search_marker: None,
            bulk: None,
            jobs: vec![],
            last_interaction: 0.0,
            map_rect: egui::Rect::NOTHING,
            to_wgs: None,
            workspace_path: None,
            dlg: Dialogs::default(),
            errors: vec![],
            show_labels: settings.show_labels,
            pending_workspace: None,
            open_seq: 0,
            settings,
            show_cgxml_opts: false,
            history: vec![],
            history_pos: 0,
            nav_press: None,
            basemap: basemap::Basemap::new(
                basemap::Source::from_id("osm"),
                bm_opacity,
            ),
            show_bm_custom: false,
            bm_custom_text: bm_custom,
        };
        // la pornire: Stereo 70, vederea pe extinderea României (aplicată la primul cadru)
        app.map_crs = Crs::from_epsg(3844).ok();
        app.to_wgs = app
            .map_crs
            .as_ref()
            .and_then(|m| Transformer::new(m, &Crs::wgs84()).ok());
        for f in files {
            if formats::ext_of(&f) == "cgw" {
                app.load_workspace(&f, ctx);
            } else {
                app.open_path(f, ReadOptions::default());
            }
        }
        app
    }

    pub fn layer(&self, id: u64) -> Option<&LayerEntry> {
        self.layers.iter().find(|l| l.id == id)
    }
    pub fn layer_mut(&mut self, id: u64) -> Option<&mut LayerEntry> {
        self.layers.iter_mut().find(|l| l.id == id)
    }

    /// „Redraw” (ca REDRAWALL / REGEN din programele CAD): șterge grafica temporară
    /// (măsurătoarea, dreptunghiul de zoom, marcajul căutării, evidențierea de la identificare),
    /// regenerează afișarea tuturor straturilor în sistemul hărții, reîncearcă tile-urile
    /// eșuate ale hărții de fundal și redesenează harta. Selecțiile și vederea rămân.
    pub fn redraw_all(&mut self, ctx: &egui::Context) {
        let t0 = std::time::Instant::now();
        self.measure.clear();
        self.measure_done = false;
        self.zoom_box_start = None;
        self.nav_press = None;
        self.search_marker = None;
        self.selection = None;
        self.refresh_projections();
        let retried = self.basemap.redraw();
        ctx.request_repaint();
        let n = self.layers.len();
        let mut msg = tr!(
            "Redraw: harta a fost curățată și redesenată; straturi regenerate: {} ({:.2} s).",
            n,
            t0.elapsed().as_secs_f64()
        );
        if retried > 0 {
            msg.push(' ');
            msg.push_str(&tr!("{} tile-uri de fundal se reîncarcă.", retried));
        }
        self.status = msg;
    }

    pub fn set_map_crs(&mut self, c: Option<Crs>) {
        // păstrează centrul vederii
        let center = match (&self.map_crs, &c) {
            (Some(a), Some(b)) => Transformer::new(a, b).ok().and_then(|t| {
                let (x, y) = t.xy(self.view.cx, self.view.cy)?;
                let (x2, _) = t.xy(self.view.cx + self.view.scale * 100.0, self.view.cy)?;
                Some((x, y, ((x2 - x).abs() / 100.0).max(1e-12)))
            }),
            _ => None,
        };
        self.map_crs = c;
        self.to_wgs = self
            .map_crs
            .as_ref()
            .and_then(|m| Transformer::new(m, &Crs::wgs84()).ok());
        let mc = self.map_crs.clone();
        for l in &mut self.layers {
            l.rebuild(mc.as_ref());
        }
        if let Some((x, y, s)) = center {
            self.view = View {
                cx: x,
                cy: y,
                scale: s,
            };
        } else {
            self.zoom_full();
        }
        self.measure.clear();
    }

    /// Deschide un fișier în fundal.
    pub fn open_path(&mut self, path: PathBuf, opts: ReadOptions) {
        if formats::ext_of(&path) == "csv" && opts.x_field.is_none() && opts.crs.is_none() {
            // dialog de import pentru fișiere text
            match CsvImport::new(&path) {
                Ok(d) => {
                    self.dlg.csv = Some(d);
                    return;
                }
                Err(e) => {
                    self.errors.push(format!("{}: {e:#}", path.display()));
                    return;
                }
            }
        }
        let mut opts = opts;
        if !opts.cgxml_custom {
            opts.cgxml = self.settings.cgxml.clone();
        }
        if opts.area.is_none() {
            opts.area = self.load_filter();
        }
        let (tx, rx) = channel();
        let p = path.clone();
        let o = opts.clone();
        let seq = self.next_seq();
        std::thread::spawn(move || {
            let r = formats::open_ex(&p, &o);
            let _ = tx.send(JobResult::Opened {
                path: p,
                result: r,
                opts: o,
                seq,
            });
        });
        self.jobs.push(Job {
            label: tr!(
                "Se deschide {}",
                path.file_name()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_default()
            ),
            rx,
        });
    }

    pub fn next_seq(&mut self) -> u64 {
        self.open_seq += 1;
        self.open_seq
    }

    pub fn open_dialog(&mut self) {
        let exts = formats::all_read_exts();
        let mut d = rfd::FileDialog::new()
            .set_title(tr!("Deschide date"))
            .add_filter(tr!("Toate formatele suportate"), &exts);
        for f in formats::FORMATS.iter().filter(|f| f.read) {
            d = d.add_filter(f.label(), f.exts);
        }
        d = d
            .add_filter(tr!("Spațiu de lucru Cadastru"), &["cgw"])
            .add_filter(tr!("Toate fișierele"), &["*"]);
        if let Some(files) = d.pick_files() {
            for f in files {
                if formats::ext_of(&f) == "cgw" {
                    self.pending_workspace = Some(f);
                } else {
                    self.open_path(f, ReadOptions::default());
                }
            }
        }
    }

    /// Adaugă straturile încărcate în hartă.
    pub fn add_datasets(&mut self, path: &Path, ds: Vec<Dataset>, seq: u64) -> Vec<u64> {
        self.add_prepared(path, ds.into_iter().map(|d| (d, None, None)).collect(), seq)
    }

    /// Ca `add_datasets`, pentru seturi pregătite în alt fir: sistemul presupus deja stabilit
    /// (`Some(presupus)`) și, opțional, geometria de afișare calculată.
    pub fn add_prepared(
        &mut self,
        path: &Path,
        ds: Vec<(Dataset, Option<bool>, Option<Prebuilt>)>,
        seq: u64,
    ) -> Vec<u64> {
        let mut ids = vec![];
        for (d, pre_assumed, prebuilt) in ds {
            let id = self.next_id;
            self.next_id += 1;
            let idx = (seq as usize).saturating_sub(1) + ids.len();
            let name = d.name().to_string();
            let mut assumed = pre_assumed.unwrap_or(false);
            let mut d = d;
            if pre_assumed.is_none() && d.crs().is_none() {
                let b = d.bounds();
                if let Some(g) = cadastru::crs::guess_crs(b.minx, b.miny, b.maxx, b.maxy)
                    .or_else(|| self.map_crs.clone())
                {
                    assumed = true;
                    match &mut d {
                        Dataset::Vector(v) => v.crs = Some(g),
                        Dataset::Raster(r) => r.crs = Some(g),
                    }
                }
            }
            let source_layer = Some(name.clone());
            let is_cgxml = formats::gdb::gdb_root(path).is_none()
                && (path.is_dir() || matches!(formats::ext_of(path).as_str(), "cgxml" | "zip"));
            let name = if is_cgxml {
                let stem = path
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_default();
                format!("{stem}: {name}")
            } else {
                name
            };
            let dark = self.settings.theme.is_dark();
            let kind = match d {
                Dataset::Vector(v) => {
                    let mut vl = VLayer::new(v, idx);
                    if dark {
                        vl.style.label_color = [235, 235, 235, 255];
                    }
                    if is_cgxml {
                        style_cgxml(&mut vl);
                    }
                    LayerKind::Vector(vl)
                }
                Dataset::Raster(r) => LayerKind::Raster(RLayer::new(r)),
            };
            let mut e = LayerEntry {
                id,
                name,
                visible: true,
                opacity: 1.0,
                crs_assumed: assumed,
                kind,
                source_layer,
                csv_fields: Default::default(),
                seq,
                cgxml_swapped: false,
            };
            if self.map_crs.is_none()
                && let Some(c) = e.crs().cloned()
            {
                self.map_crs = Some(c);
                self.to_wgs = self
                    .map_crs
                    .as_ref()
                    .and_then(|m| Transformer::new(m, &Crs::wgs84()).ok());
                for l in &mut self.layers {
                    l.rebuild(self.map_crs.as_ref());
                }
            }
            match (prebuilt, &mut e.kind) {
                (Some(p), LayerKind::Vector(v)) if p.map_crs == self.map_crs => v.install(p),
                _ => e.rebuild(self.map_crs.as_ref()),
            }
            if assumed {
                self.status = tr!(
                    "„{}” nu are proiecție – presupus {}. Verificați din meniul contextual al stratului.",
                    e.name,
                    e.crs().map(|c| c.short()).unwrap_or_default()
                );
            } else {
                self.status = tr!("Încărcat: {} ({})", e.name, e.summary());
            }
            if let LayerKind::Vector(v) = &e.kind
                && v.failed > 0
            {
                self.errors.push(tr!(
                    "{}: {} entități nu au putut fi transformate în sistemul hărții",
                    e.name, v.failed
                ));
            }
            ids.push(id);
            // ordinea de deschidere: straturile deschise mai târziu stau deasupra
            let pos = self
                .layers
                .iter()
                .position(|l| l.seq > seq)
                .unwrap_or(self.layers.len());
            self.layers.insert(pos, e);
            self.selected_layer = Some(id);
        }
        let _ = path;
        if !self.view_initialized || self.layers.len() == ids.len() {
            self.zoom_full();
        }
        ids
    }

    pub fn remove_layer(&mut self, id: u64) {
        self.layers.retain(|l| l.id != id);
        if self.selected_layer == Some(id) {
            self.selected_layer = self.layers.last().map(|l| l.id);
        }
        if self.selection.is_some_and(|(l, _)| l == id) {
            self.selection = None;
        }
        self.identify.retain(|r| r.layer_id != id);
    }

    pub fn full_bounds(&self) -> Bounds {
        let mut b = Bounds::empty();
        for l in &self.layers {
            b.merge(&l.map_bounds());
        }
        b
    }

    pub fn zoom_full(&mut self) {
        let b = self.full_bounds();
        if b.is_valid() {
            self.zoom_to(b);
        } else {
            self.zoom_home();
        }
    }

    /// Extinderea României în sistemul hărții.
    pub fn romania_bounds(&self) -> Option<Bounds> {
        let m = self.map_crs.as_ref()?;
        let t = Transformer::new(&Crs::wgs84(), m).ok()?;
        let b = t.bounds(&Bounds::new(20.22, 43.62, 29.76, 48.27));
        b.is_valid().then_some(b)
    }

    /// Vederea inițială: toată România.
    pub fn zoom_home(&mut self) {
        if let Some(b) = self.romania_bounds() {
            self.zoom_to(b);
        }
    }

    fn poll_jobs(&mut self, ctx: &egui::Context) {
        let mut finished = vec![];
        let mut results = vec![];
        for (i, j) in self.jobs.iter().enumerate() {
            loop {
                match j.rx.try_recv() {
                    Ok(r) => {
                        let done = !matches!(r, JobResult::Log(_) | JobResult::BulkItem { .. });
                        let bulk = matches!(r, JobResult::BulkItem { .. });
                        results.push(r);
                        if done {
                            finished.push(i);
                            break;
                        }
                        // câteva fișiere pe cadru, ca interfața să rămână fluidă
                        if bulk && results.len() >= 24 {
                            break;
                        }
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => break,
                    Err(_) => {
                        finished.push(i);
                        break;
                    }
                }
            }
        }
        for i in finished.into_iter().rev() {
            self.jobs.remove(i);
        }
        for r in results {
            match r {
                JobResult::Opened {
                    path,
                    result,
                    opts,
                    seq,
                } => match result {
                    Ok((ds, info)) => {
                        let ids = self.add_datasets(&path, ds, seq);
                        if let Some(first) = info.first() {
                            self.status = first.clone();
                        }
                        for m in info.iter().skip(1) {
                            self.errors.push(m.clone());
                        }
                        for id in &ids {
                            if let Some(l) = self.layer_mut(*id) {
                                l.csv_fields = [
                                    opts.x_field.clone(),
                                    opts.y_field.clone(),
                                    opts.z_field.clone(),
                                ];
                                l.cgxml_swapped = opts.cgxml.swap_xy;
                            }
                        }
                        // setări din spațiul de lucru
                        if let Some(pending) = self.dlg.pending_settings.remove(&path) {
                            self.apply_layer_settings(&ids, pending);
                        }
                        let _ = opts;
                    }
                    Err(e) => self.errors.push(format!("{}: {e:#}", path.display())),
                },
                JobResult::Exported(r) => match r {
                    Ok(files) => {
                        self.status = tr!(
                            "Export reușit: {}",
                            files
                                .iter()
                                .map(|p| p.display().to_string())
                                .collect::<Vec<_>>()
                                .join(", ")
                        )
                    }
                    Err(e) => self.errors.push(tr!("Export eșuat: {e:#}")),
                },
                JobResult::Log(l) => {
                    if let Some(b) = self.dlg.batch.as_mut() {
                        b.log.push(l);
                    }
                }
                JobResult::BulkItem {
                    path,
                    rel,
                    result,
                    seq,
                    rel_names,
                } => self.bulk_item(path, rel, result, seq, rel_names),
                JobResult::BulkDone => self.bulk_done(),
                JobResult::Done(msg) => {
                    if let Some(b) = self.dlg.batch.as_mut() {
                        b.running = false;
                        b.log.push(msg.clone());
                    }
                    self.status = msg;
                }
            }
        }
        if !self.jobs.is_empty() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
    }

    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        struct K {
            ctrl_o: bool,
            ctrl_s: bool,
            ctrl_p: bool,
            f1: bool,
            home: bool,
            esc: bool,
            i: bool,
            m: bool,
            p: bool,
            z: bool,
            x: bool,
            plus: bool,
            minus: bool,
            t: bool,
            s: bool,
            ctrl_f: bool,
            f5: bool,
            ctrl_l: bool,
            deselect: bool,
            attr_sel: bool,
            back: bool,
            fwd: bool,
            pan: egui::Vec2,
        }
        let k = ctx.input(|i| {
            let cmd = i.modifiers.command;
            let alt = i.modifiers.alt;
            let mut pan = egui::Vec2::ZERO;
            if !alt {
                if i.key_pressed(Key::ArrowLeft) {
                    pan.x -= 1.0;
                }
                if i.key_pressed(Key::ArrowRight) {
                    pan.x += 1.0;
                }
                if i.key_pressed(Key::ArrowUp) {
                    pan.y += 1.0;
                }
                if i.key_pressed(Key::ArrowDown) {
                    pan.y -= 1.0;
                }
            }
            K {
                ctrl_o: cmd && i.key_pressed(Key::O),
                ctrl_s: cmd && i.key_pressed(Key::S),
                ctrl_p: cmd && i.key_pressed(Key::P),
                f1: i.key_pressed(Key::F1),
                f5: i.key_pressed(Key::F5),
                ctrl_l: cmd && i.key_pressed(Key::L),
                home: i.key_pressed(Key::Home),
                esc: i.key_pressed(Key::Escape),
                i: !cmd && i.key_pressed(Key::I),
                m: !cmd && i.key_pressed(Key::M),
                p: !cmd && i.key_pressed(Key::P),
                z: !cmd && i.key_pressed(Key::Z),
                x: !cmd && i.key_pressed(Key::X),
                plus: i.key_pressed(Key::Plus) || i.key_pressed(Key::Equals),
                minus: i.key_pressed(Key::Minus),
                t: !cmd && i.key_pressed(Key::T),
                s: !cmd && i.key_pressed(Key::S),
                ctrl_f: cmd && !i.modifiers.shift && i.key_pressed(Key::F),
                attr_sel: cmd && i.modifiers.shift && i.key_pressed(Key::F),
                deselect: cmd && i.modifiers.shift && i.key_pressed(Key::A),
                back: i.key_pressed(Key::Backspace) || (alt && i.key_pressed(Key::ArrowLeft)),
                fwd: alt && i.key_pressed(Key::ArrowRight),
                pan,
            }
        });
        if k.ctrl_o {
            self.open_dialog();
        }
        if k.ctrl_s {
            self.save_workspace_dialog(false);
        }
        if k.ctrl_p {
            self.open_print(print::Output::Printer);
        }
        if k.f1 {
            self.open_manual();
        }
        if k.f5 {
            self.redraw_all(ctx);
        }
        if k.ctrl_l {
            self.layers_toggle = Some(!self.settings.layers_panel);
        }
        if k.home {
            self.zoom_full();
        }
        if k.esc {
            self.dlg.parcel.pick = parcel::Pick::None;
            // Esc cu instrumentul Selectare (fără altă operație în curs) golește selecția
            if self.tool == Tool::Select && self.zoom_box_start.is_none() && self.measure.is_empty() {
                self.clear_selection();
            }
            self.measure.clear();
            self.measure_done = false;
            self.selection = None;
            self.zoom_box_start = None;
        }
        if k.s {
            self.tool = Tool::Select;
        }
        if k.ctrl_f {
            self.dlg.search.open = true;
            self.dlg.search.focus = true;
        }
        if k.deselect {
            self.clear_selection();
        }
        if k.attr_sel {
            self.open_attr_select(None);
        }
        if k.i {
            self.tool = Tool::Identify;
        }
        if k.m {
            self.tool = Tool::Measure;
        }
        if k.p {
            self.tool = Tool::Pan;
        }
        if k.z {
            self.tool = Tool::ZoomIn;
        }
        if k.x {
            self.tool = Tool::ZoomOut;
        }
        if k.t {
            self.open_table();
        }
        if k.plus {
            self.zoom_step(2.0);
        }
        if k.minus {
            self.zoom_step(0.5);
        }
        if k.back {
            self.history_back();
        }
        if k.fwd {
            self.history_forward();
        }
        if k.pan != egui::Vec2::ZERO {
            self.pan_fraction(k.pan.x as f64 * 0.25, k.pan.y as f64 * 0.25);
        }
    }

    pub fn open_table(&mut self) {
        self.dlg.table.open = true;
        if self.dlg.table.layer_id.is_none() {
            self.dlg.table.layer_id = self.selected_layer;
        }
    }

    pub fn open_converter(&mut self) {
        self.dlg.converter.open = true;
        if self.dlg.converter.src.is_none() {
            self.dlg.converter.src = self.map_crs.clone().or_else(|| Crs::from_epsg(3844).ok());
            self.dlg.converter.dst = Some(Crs::wgs84());
        }
    }

    /// Mărire (f > 1) sau micșorare (f < 1) în jurul centrului.
    pub fn zoom_step(&mut self, f: f64) {
        self.view.scale /= f;
        self.view_initialized = true;
    }

    /// Deplasare cu o fracțiune din lățimea / înălțimea vederii.
    pub fn pan_fraction(&mut self, fx: f64, fy: f64) {
        let w = self.map_rect.width().max(100.0) as f64 * self.view.scale;
        let h = self.map_rect.height().max(100.0) as f64 * self.view.scale;
        self.view.cx += fx * w;
        self.view.cy += fy * h;
        self.view_initialized = true;
    }

    /// Înregistrează vederea curentă în istoric (apelat când harta stă pe loc).
    pub fn commit_history(&mut self) {
        if self.history.get(self.history_pos) == Some(&self.view) {
            return;
        }
        if !self.history.is_empty() {
            self.history.truncate(self.history_pos + 1);
        }
        self.history.push(self.view);
        if self.history.len() > 100 {
            self.history.remove(0);
        }
        self.history_pos = self.history.len() - 1;
    }

    pub fn history_back(&mut self) {
        self.commit_history();
        if self.history_pos > 0 {
            self.history_pos -= 1;
            self.view = self.history[self.history_pos];
        }
    }

    pub fn history_forward(&mut self) {
        if self.history_pos + 1 < self.history.len() {
            self.history_pos += 1;
            self.view = self.history[self.history_pos];
        }
    }

    /// Schimbă limba interfeței și o memorează în setări.
    pub fn set_lang(&mut self, l: cadastru::i18n::Lang) {
        if cadastru::i18n::lang() == l {
            return;
        }
        cadastru::i18n::set_lang(l);
        self.settings.lang = l;
        self.settings.save();
        self.status = tr!("Limba interfeței: română").into();
    }

    pub fn set_theme(&mut self, ctx: &egui::Context, t: Theme) {
        if self.settings.theme == t {
            return;
        }
        self.settings.theme = t;
        theme::apply(ctx, t);
        // etichetele implicite urmează tema
        for l in &mut self.layers {
            if let LayerKind::Vector(v) = &mut l.kind {
                if t.is_dark() && v.style.label_color == [20, 20, 20, 255] {
                    v.style.label_color = [235, 235, 235, 255];
                } else if !t.is_dark() && v.style.label_color == [235, 235, 235, 255] {
                    v.style.label_color = [20, 20, 20, 255];
                }
            }
        }
        self.settings.save();
    }

    /// Reîncarcă straturile CGXML dintr-o sursă cu alte opțiuni.
    pub fn reload_cgxml(&mut self, src: PathBuf, swap: bool) {
        let ids: Vec<u64> = self
            .layers
            .iter()
            .filter(|l| l.source() == Some(src.as_path()))
            .map(|l| l.id)
            .collect();
        for id in ids {
            self.remove_layer(id);
        }
        let mut c = self.settings.cgxml.clone();
        c.swap_xy = swap;
        let o = ReadOptions {
            cgxml: c,
            cgxml_custom: true,
            ..Default::default()
        };
        self.open_path(src, o);
    }

    /// Scrie straturile CGXML ca teren.shp / constructii.shp într-un dosar (ca CGXML2SHP).
    pub fn save_cgxml_shp(&mut self, src: PathBuf) {
        let Some(dir) = rfd::FileDialog::new()
            .set_title(tr!("Dosar pentru teren.shp / constructii.shp"))
            .pick_folder()
        else {
            return;
        };
        let layers: Vec<VectorLayer> = self
            .layers
            .iter()
            .filter(|l| l.source() == Some(src.as_path()))
            .filter_map(|l| match &l.kind {
                LayerKind::Vector(v) => Some(v.data.clone()),
                _ => None,
            })
            .collect();
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let mut files = vec![];
            let mut err = None;
            for l in &layers {
                let p = dir.join(format!("{}.shp", l.name));
                match formats::write_vector(
                    &[l],
                    &p,
                    &formats::WriteOptions {
                        overwrite: true,
                        ..Default::default()
                    },
                ) {
                    Ok(f) => files.extend(f),
                    Err(e) => err = Some(e),
                }
            }
            let _ = tx.send(JobResult::Exported(match err {
                Some(e) => Err(e),
                None => Ok(files),
            }));
        });
        self.jobs.push(Job {
            label: tr!("Salvare shapefile CGXML").into(),
            rx,
        });
    }

    fn open_manual(&mut self) {
        match manual::open_manual() {
            Ok(p) => self.status = tr!("Manualul de utilizare: {}", p.display()),
            Err(e) => self.errors.push(tr!("Manualul de utilizare nu s-a putut deschide: {e:#}")),
        }
    }

    fn menu_bar(&mut self, ui: &mut egui::Ui) {
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button(tr!("Fișier"), |ui| {
                if ui.button(tr!("Deschide date…   Ctrl+O")).clicked() {
                    ui.close();
                    self.open_dialog();
                }
                if ui.button(tr!("Deschide File Geodatabase (.gdb)…")).clicked() {
                    ui.close();
                    if let Some(d) = rfd::FileDialog::new()
                        .set_title(tr!("Dosarul File Geodatabase (.gdb)"))
                        .pick_folder()
                    {
                        if formats::gdb::gdb_root(&d).is_some() {
                            self.open_path(d, ReadOptions::default());
                        } else {
                            self.errors.push(tr!(
                                "{} nu este un File Geodatabase (alegeți dosarul cu extensia .gdb)",
                                d.display()
                            ));
                        }
                    }
                }
                if ui.button(tr!("Deschide toate fișierele de un tip dintr-un dosar…")).clicked() {
                    ui.close();
                    self.open_bulk_dialog();
                }
                if ui.button(tr!("Deschide dosar cu fișiere CGXML…")).clicked() {
                    ui.close();
                    if let Some(d) = rfd::FileDialog::new()
                        .set_title(tr!("Dosar cu fișiere CGXML (inclusiv arhive ZIP)"))
                        .pick_folder()
                    {
                        self.open_path(d, ReadOptions::default());
                    }
                }
                self.load_filter_checkbox(ui);
                if ui.button(tr!("Opțiunile filtrului de încărcare…")).clicked() {
                    ui.close();
                    self.open_load_filter_dialog();
                }
                ui.separator();
                if ui.button(tr!("Deschide spațiu de lucru…")).clicked() {
                    ui.close();
                    if let Some(p) = rfd::FileDialog::new()
                        .add_filter(tr!("Spațiu de lucru Cadastru"), &["cgw"])
                        .pick_file()
                    {
                        self.pending_workspace = Some(p);
                    }
                }
                if ui.button(tr!("Salvează spațiu de lucru   Ctrl+S")).clicked() {
                    ui.close();
                    self.save_workspace_dialog(false);
                }
                if ui.button(tr!("Salvează spațiu de lucru ca…")).clicked() {
                    ui.close();
                    self.save_workspace_dialog(true);
                }
                ui.separator();
                if ui
                    .add_enabled(
                        self.selected_layer.is_some(),
                        egui::Button::new(tr!("Export strat selectat…")),
                    )
                    .clicked()
                {
                    ui.close();
                    if let Some(id) = self.selected_layer {
                        self.dlg.export = Some(ExportDlg::new(self, id));
                    }
                }
                if ui
                    .add_enabled(
                        self.layers.iter().any(|l| l.is_vector()),
                        egui::Button::new(tr!("Export toate straturile vectoriale în GeoPackage…")),
                    )
                    .clicked()
                {
                    ui.close();
                    self.export_all_gpkg();
                }
                if ui
                    .add_enabled(
                        self.layers.iter().any(|l| l.is_vector()),
                        egui::Button::new(tr!("Export pe tipuri de geometrie (DXF » SHP puncte / linii / poligoane)…")),
                    )
                    .clicked()
                {
                    ui.close();
                    self.open_split_dialog(None);
                }
                ui.separator();
                if ui.button(tr!("Închide toate straturile")).clicked() {
                    ui.close();
                    self.layers.clear();
                    self.selection = None;
                    self.identify.clear();
                    self.selected_layer = None;
                    self.set_map_crs(Crs::from_epsg(3844).ok());
                    self.view_initialized = false;
                    self.workspace_path = None;
                }
                if ui.button(tr!("Tipărire…   Ctrl+P")).clicked() {
                    ui.close();
                    self.open_print(print::Output::Printer);
                }
                if ui.button(tr!("Salvează ca PDF…")).clicked() {
                    ui.close();
                    self.open_print(print::Output::Pdf);
                }
                ui.separator();
                if ui.button(tr!("Ieșire")).clicked() {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
            ui.menu_button(tr!("Editare"), |ui| {
                let can_parcel = self.parcel_selection();
                if ui
                    .add_enabled(can_parcel.is_ok(), egui::Button::new(tr!("Parcelare / detașare…")))
                    .on_hover_text(tr!("Împarte poligonul selectat în loturi: detașarea unei suprafețe, loturi egale sau suprafețe date, cu linii paralele, perpendiculare, după o orientare sau printr-un vârf"))
                    .on_disabled_hover_text(can_parcel.err().unwrap_or_default())
                    .clicked()
                {
                    ui.close();
                    self.open_parcel();
                }
                ui.separator();
                let lid = self.selected_layer;
                let nsel = lid
                    .and_then(|id| self.layer(id))
                    .map(|l| match &l.kind {
                        LayerKind::Vector(v) => v.sel.len(),
                        _ => 0,
                    })
                    .unwrap_or(0);
                if ui
                    .add_enabled(nsel > 0, egui::Button::new(tr!("Strat nou din obiectele selectate (Make Layer with selected objects)")))
                    .on_hover_text(tr!("Pentru stratul activ din panoul Straturi"))
                    .clicked()
                    && let Some(id) = lid
                {
                    ui.close();
                    self.layer_from_selection(id);
                }
            });
            ui.menu_button(tr!("Selectare"), |ui| {
                self.selection_menu_items(ui);
            });
            ui.menu_button(tr!("Vizualizare"), |ui| {
                if ui.button(tr!("Extindere completă   Home")).clicked() {
                    ui.close();
                    self.zoom_full();
                }
                if ui
                    .add_enabled(
                        self.selected_layer.is_some(),
                        egui::Button::new(tr!("Zoom la stratul selectat")),
                    )
                    .clicked()
                {
                    ui.close();
                    if let Some(b) = self
                        .selected_layer
                        .and_then(|id| self.layer(id))
                        .map(|l| l.map_bounds())
                    {
                        self.zoom_to(b);
                    }
                }
                ui.separator();
                if ui.button(tr!("Sistemul de coordonate al hărții…")).clicked() {
                    ui.close();
                    self.dlg.crs_picker =
                        Some(CrsPicker::new(CrsPurpose::Map, self.map_crs.clone()));
                }
                ui.separator();
                if ui.button(tr!("Tabel de atribute   T")).clicked() {
                    ui.close();
                    self.dlg.table.open = true;
                    if self.dlg.table.layer_id.is_none() {
                        self.dlg.table.layer_id = self.selected_layer;
                    }
                }
                ui.separator();
                if ui.button(tr!("Vederea anterioară   Backspace")).clicked() {
                    ui.close();
                    self.history_back();
                }
                if ui.button(tr!("Vederea următoare   Alt+Dreapta")).clicked() {
                    ui.close();
                    self.history_forward();
                }
                ui.separator();
                if ui
                    .checkbox(&mut self.show_labels, tr!("Afișează etichetele"))
                    .changed()
                {
                    self.settings.show_labels = self.show_labels;
                    self.settings.save();
                }
                if ui
                    .checkbox(
                        &mut self.settings.show_nav,
                        tr!("Controale de navigație pe hartă"),
                    )
                    .changed()
                {
                    self.settings.save();
                }
                ui.checkbox(&mut self.show_identify, tr!("Fereastra de identificare"));
                let mut lp = self.settings.layers_panel;
                if ui
                    .checkbox(&mut lp, tr!("Panoul Straturi   Ctrl+L"))
                    .on_hover_text(tr!("Ascuns, harta ocupă toată lățimea; panoul revine cu un click pe banda din stânga"))
                    .changed()
                {
                    self.layers_toggle = Some(lp);
                }
                ui.separator();
                ui.label(tr!("Temă:"));
                let ctx = ui.ctx().clone();
                for t in [Theme::DarkGrey, Theme::Light] {
                    if ui.radio(self.settings.theme == t, t.label()).clicked() {
                        self.set_theme(&ctx, t);
                    }
                }
                ui.separator();
                ui.label("Limbă / Language:");
                for l in [cadastru::i18n::Lang::Ro, cadastru::i18n::Lang::En] {
                    if ui.radio(cadastru::i18n::lang() == l, l.native_name()).clicked() {
                        self.set_lang(l);
                    }
                }
            });
            ui.menu_button(tr!("Instrumente"), |ui| {
                if ui.button(tr!("Deplasare   P")).clicked() {
                    self.tool = Tool::Pan;
                    ui.close();
                }
                if ui.button(tr!("Zoom + (click sau dreptunghi)   Z")).clicked() {
                    self.tool = Tool::ZoomIn;
                    ui.close();
                }
                if ui.button(tr!("Zoom − (click sau dreptunghi)   X")).clicked() {
                    self.tool = Tool::ZoomOut;
                    ui.close();
                }
                if ui.button(tr!("Identificare   I")).clicked() {
                    self.tool = Tool::Identify;
                    ui.close();
                }
                if ui.button(tr!("Măsurare distanță / arie   M")).clicked() {
                    self.tool = Tool::Measure;
                    ui.close();
                }
                ui.separator();
                if ui.button(tr!("Căutare…   Ctrl+F")).clicked() {
                    self.dlg.search.open = true;
                    self.dlg.search.focus = true;
                    ui.close();
                }
                if ui
                    .button(tr!("Redraw (curăță și redesenează harta)   F5"))
                    .on_hover_text(tr!("Ca REDRAWALL / REGEN în CAD: șterge grafica temporară, regenerează straturile și reîncarcă harta de fundal"))
                    .clicked()
                {
                    ui.close();
                    self.redraw_all(ui.ctx());
                }
                ui.separator();
                if ui.button(tr!("Convertor de coordonate…")).clicked() {
                    ui.close();
                    self.dlg.converter.open = true;
                    if self.dlg.converter.src.is_none() {
                        self.dlg.converter.src =
                            self.map_crs.clone().or_else(|| Crs::from_epsg(3844).ok());
                        self.dlg.converter.dst = Some(Crs::wgs84());
                    }
                }
                if ui.button(tr!("TransDatRO (ETRS89 – Stereo 70 / Stereo 30, altitudini MN75)…")).clicked() {
                    ui.close();
                    self.open_transdatro();
                }
                if ui
                    .checkbox(
                        &mut self.settings.transdatro,
                        tr!("Reproiecție Stereo 70 cu TransDatRO"),
                    )
                    .on_hover_text(tr!("Debifat: Helmert 7 parametri (EPSG:15994)"))
                    .changed()
                {
                    cadastru::crs::set_transdatro(self.settings.transdatro);
                    self.settings.save();
                    self.refresh_projections();
                }
                if ui.button(tr!("Conversie fișiere în lot…")).clicked() {
                    ui.close();
                    if self.dlg.batch.is_none() {
                        self.dlg.batch = Some(BatchDlg::default());
                    }
                }
                ui.separator();
                if ui.button(tr!("Opțiuni import CGXML…")).clicked() {
                    ui.close();
                    self.show_cgxml_opts = true;
                }
                if ui
                    .button(tr!("Formate ECW / MrSID (GDAL)…"))
                    .on_hover_text(tr!("Citirea rasterelor ECW, MrSID și JPEG 2000 prin GDAL-ul unei instalări QGIS / OSGeo4W"))
                    .clicked()
                {
                    ui.close();
                    self.dlg.ecw.open = true;
                }
            });
            ui.menu_button(tr!("Ajutor"), |ui| {
                if ui.button(tr!("Manual de utilizare (PDF)   F1")).clicked() {
                    ui.close();
                    self.open_manual();
                }
                if ui.button(tr!("Scurtături și utilizare")).clicked() {
                    ui.close();
                    self.dlg.help = true;
                }
                if ui.button(tr!("Despre Cadastru")).clicked() {
                    ui.close();
                    self.dlg.about = true;
                }
            });
        });
    }

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            // fereastră foarte îngustă: listele derulante doar cu pictograme (numele în indicii)
            let compact = ui.available_width() < 900.0;
            let lbl = |s: &'static str| if compact { "" } else { s };

            // Fișier: un singur instrument → buton simplu
            if icon_button(
                ui,
                Icon::Open,
                lbl(tr!("Deschide")),
                false,
                tr!("Deschide fișiere: SHP, GPKG, DXF, CGXML, GeoTIFF … (Ctrl+O)"),
            )
            .clicked()
            {
                self.open_dialog();
            }
            ui.separator();

            // Vizualizare: tot ce schimbă doar vederea hărții
            let view_tool = matches!(self.tool, Tool::Pan | Tool::ZoomIn | Tool::ZoomOut);
            let view_icon = match self.tool {
                Tool::ZoomIn => Icon::ZoomIn,
                Tool::ZoomOut => Icon::ZoomOut,
                _ => Icon::Pan,
            };
            tool_dropdown(
                ui,
                view_icon,
                lbl(tr!("Vizualizare")),
                view_tool,
                tr!("Vizualizare: deplasare, zoom, extindere completă, vederea anterioară / următoare, Redraw"),
                |ui| {
                    for (t, icon, label, key) in [
                        (Tool::Pan, Icon::Pan, tr!("Deplasare"), "P"),
                        (Tool::ZoomIn, Icon::ZoomIn, tr!("Zoom + (click sau dreptunghi)"), "Z"),
                        (Tool::ZoomOut, Icon::ZoomOut, tr!("Zoom − (click sau dreptunghi)"), "X"),
                    ] {
                        if menu_item(ui, Some(icon), label, key, self.tool == t, true).clicked() {
                            self.tool = t;
                            ui.close();
                        }
                    }
                    ui.separator();
                    if menu_item(ui, Some(Icon::FullExtent), tr!("Extindere completă"), "Home", false, true).clicked() {
                        self.zoom_full();
                        ui.close();
                    }
                    if menu_item(ui, Some(Icon::Back), tr!("Vederea anterioară"), "Backspace", false, true).clicked() {
                        self.history_back();
                        ui.close();
                    }
                    if menu_item(ui, Some(Icon::Forward), tr!("Vederea următoare"), tr!("Alt+Dreapta"), false, true).clicked() {
                        self.history_forward();
                        ui.close();
                    }
                    ui.separator();
                    if menu_item(ui, Some(Icon::Redraw), tr!("Redraw (curăță și redesenează)"), "F5", false, true)
                        .on_hover_text(tr!("Curăță și redesenează harta, ca REDRAWALL / REGEN în CAD: șterge măsurătoarea, marcajele și evidențierile temporare, regenerează straturile și reîncarcă tile-urile de fundal eșuate (F5)"))
                        .clicked()
                    {
                        ui.close();
                        self.redraw_all(ui.ctx());
                    }
                },
            );

            // Interogare: informații despre entități și locuri
            tool_dropdown(
                ui,
                Icon::Identify,
                lbl(tr!("Interogare")),
                self.tool == Tool::Identify,
                tr!("Interogare: identificare, căutare, tabel de atribute"),
                |ui| {
                    if menu_item(ui, Some(Icon::Identify), tr!("Identificare"), "I", self.tool == Tool::Identify, true)
                        .on_hover_text(tr!("Click pe o entitate pentru atribute (I)"))
                        .clicked()
                    {
                        self.tool = Tool::Identify;
                        ui.close();
                    }
                    if menu_item(ui, Some(Icon::Search), tr!("Căutare…"), "Ctrl+F", self.dlg.search.open, true)
                        .on_hover_text(tr!("Caută un loc sau o adresă (OpenStreetMap, Google), ori entități după valorile câmpurilor (Ctrl+F)"))
                        .clicked()
                    {
                        self.dlg.search.open = !self.dlg.search.open;
                        self.dlg.search.focus = self.dlg.search.open;
                        ui.close();
                    }
                    if menu_item(ui, Some(Icon::Table), tr!("Tabel de atribute"), "T", self.dlg.table.open, true).clicked() {
                        self.open_table();
                        ui.close();
                    }
                },
            );

            // Selectare: pe hartă, după atribute, Clear
            let (nsel, nlay) = self.selection_totals();
            let sel_tip = if nsel > 0 {
                tr!("Selectare: pe hartă, după atribute, Clear – {} entități selectate în {} straturi", nsel, nlay)
            } else {
                tr!("Selectare: pe hartă, după atribute, Clear").to_string()
            };
            tool_dropdown(ui, Icon::Select, lbl(tr!("Selectare")), self.tool == Tool::Select, &sel_tip, |ui| {
                self.selection_menu_items(ui);
            });

            // Editare: un singur instrument → buton simplu
            let can_parcel = self.parcel_selection();
            let tip = match &can_parcel {
                Ok(_) => tr!("Parcelare / detașare: împarte poligonul selectat în loturi").to_string(),
                Err(e) => e.clone(),
            };
            if ui
                .add_enabled_ui(can_parcel.is_ok() || self.dlg.parcel.open, |ui| {
                    icon_button(ui, Icon::Parcel, lbl(tr!("Parcelare")), self.dlg.parcel.open, &tip)
                })
                .inner
                .clicked()
            {
                if self.dlg.parcel.open {
                    self.dlg.parcel.open = false;
                    self.dlg.parcel.pick = parcel::Pick::None;
                } else {
                    self.open_parcel();
                }
            }

            // Instrumente
            tool_dropdown(
                ui,
                if self.dlg.converter.open && self.tool != Tool::Measure { Icon::Convert } else { Icon::Measure },
                lbl(tr!("Instrumente")),
                self.tool == Tool::Measure,
                tr!("Instrumente: măsurare, convertor de coordonate"),
                |ui| {
                    if menu_item(ui, Some(Icon::Measure), tr!("Măsurare distanță / arie"), "M", self.tool == Tool::Measure, true)
                        .on_hover_text(tr!("Click pentru puncte, dublu-click pentru final, Esc pentru ștergere (M)"))
                        .clicked()
                    {
                        self.tool = Tool::Measure;
                        ui.close();
                    }
                    if menu_item(ui, Some(Icon::Convert), tr!("Convertor de coordonate…"), "", self.dlg.converter.open, true).clicked() {
                        self.open_converter();
                        ui.close();
                    }
                },
            );

            // Hartă: harta de fundal și sistemul hărții (codul sistemului rămâne vizibil)
            let crs_short = self.map_crs.as_ref().map(|c| c.short()).unwrap_or_else(|| tr!("nedefinit").into());
            let map_label = if compact { String::new() } else { format!("{} · {}", tr!("Hartă"), crs_short) };
            let mut pick_crs = false;
            tool_dropdown(
                ui,
                Icon::Globe,
                &map_label,
                false,
                &format!(
                    "{}\n{}",
                    self.map_crs.as_ref().map(|c| c.to_string()).unwrap_or_default(),
                    tr!("Hartă: harta de fundal (Basemap) și sistemul de coordonate al hărții")
                ),
                |ui| {
                    self.basemap_menu(ui);
                    if menu_item(
                        ui,
                        Some(Icon::Globe),
                        &format!("{} ({})…", tr!("Sistemul hărții"), crs_short),
                        "",
                        false,
                        true,
                    )
                    .on_hover_text(self.map_crs.as_ref().map(|c| c.to_string()).unwrap_or_default())
                    .clicked()
                    {
                        pick_crs = true;
                        ui.close();
                    }
                },
            );
            if pick_crs {
                self.dlg.crs_picker = Some(CrsPicker::new(CrsPurpose::Map, self.map_crs.clone()));
            }
            // comutatorul de limbă, aliniat la dreapta
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(4.0);
                self.lang_toggle(ui);
            });
        });
    }

    /// Instrumentele de selecție: în meniul „Selectare” și în lista derulantă din bară.
    fn selection_menu_items(&mut self, ui: &mut egui::Ui) {
        let (nsel, nlay) = self.selection_totals();
        if menu_item(ui, Some(Icon::Select), tr!("Selectare pe hartă"), "S", self.tool == Tool::Select, true)
            .on_hover_text(tr!("Click = selectează entitatea, Ctrl+click = adaugă / scoate, Shift = adaugă, Alt = scoate, dreptunghi = selectează în zonă (S)"))
            .clicked()
        {
            self.tool = Tool::Select;
            ui.close();
        }
        if menu_item(ui, Some(Icon::SelectAttr), tr!("Selectare după atribute…"), "Ctrl+Shift+F", self.dlg.attrsel.open, true)
            .on_hover_text(tr!("Selectare după atribute: expresie de tipul status = 1 AND is_active = 1 (Ctrl+Shift+F)"))
            .clicked()
        {
            ui.close();
            self.open_attr_select(None);
        }
        let clear_label = match (nsel, nlay) {
            (0, _) => "Clear".to_string(),
            (1, _) => tr!("Clear (o entitate selectată)").to_string(),
            (n, 1) => tr!("Clear ({} entități selectate)", n),
            (n, k) => tr!("Clear ({} entități selectate în {} straturi)", n, k),
        };
        if menu_item(ui, Some(Icon::ClearSel), &clear_label, "Ctrl+Shift+A", false, nsel > 0 || self.selection.is_some())
            .on_hover_text(tr!("Golește selecția din toate straturile, pe toată harta"))
            .clicked()
        {
            self.clear_selection();
            ui.close();
        }
        ui.separator();
        if menu_item(ui, None, tr!("Zoom la selecție"), "", false, nsel > 0).clicked() {
            self.zoom_to_selection(None);
            ui.close();
        }
        let lid = self.selected_layer;
        let n_active = lid
            .and_then(|id| self.layer(id))
            .map(|l| match &l.kind {
                LayerKind::Vector(v) => v.sel.len(),
                _ => 0,
            })
            .unwrap_or(0);
        if menu_item(ui, None, tr!("Make Layer with selected objects"), "", false, n_active > 0)
            .on_hover_text(tr!("Strat nou doar cu obiectele selectate din stratul activ (panoul Straturi)"))
            .clicked()
            && let Some(id) = lid
        {
            ui.close();
            self.layer_from_selection(id);
        }
        if ui
            .checkbox(&mut self.settings.select_active_only, tr!("Selectare doar în stratul activ"))
            .on_hover_text(tr!("Altfel, în toate straturile vectoriale vizibile"))
            .changed()
        {
            self.settings.save();
        }
    }

    /// Butonul RO | EN din bara de instrumente.
    fn lang_toggle(&mut self, ui: &mut egui::Ui) {
        use cadastru::i18n::{Lang, lang};
        let cur = lang();
        let frame = egui::Frame::new()
            .stroke(ui.visuals().widgets.inactive.bg_stroke)
            .corner_radius(4.0)
            .inner_margin(egui::Margin::same(2));
        let mut pick = None;
        frame
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                // right_to_left: EN apare în dreapta, RO în stânga
                for l in [Lang::En, Lang::Ro] {
                    let text = RichText::new(l.code()).strong().size(12.0);
                    if ui
                        .selectable_label(cur == l, text)
                        .on_hover_text(match l {
                            Lang::Ro => "Interfața în limba română",
                            Lang::En => "Switch the interface to English",
                        })
                        .clicked()
                    {
                        pick = Some(l);
                    }
                }
            })
            .response
            .on_hover_text("Limbă / Language");
        if let Some(l) = pick {
            self.set_lang(l);
        }
    }

    fn layers_panel(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            if small_icon_button(ui, Icon::Collapse, true, tr!("Ascunde panoul Straturi (Ctrl+L): harta ocupă toată lățimea")).clicked() {
                self.layers_toggle = Some(false);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                let sel = self.selected_layer;
                if small_icon_button(ui, Icon::Remove, sel.is_some(), tr!("Elimină stratul selectat")).clicked()
                    && let Some(id) = sel
                {
                    self.remove_layer(id);
                }
                if small_icon_button(ui, Icon::Down, sel.is_some(), tr!("Coboară stratul")).clicked()
                    && let Some(i) = sel.and_then(|id| self.layers.iter().position(|l| l.id == id))
                    && i > 0
                {
                    self.layers.swap(i, i - 1);
                }
                if small_icon_button(ui, Icon::Up, sel.is_some(), tr!("Urcă stratul")).clicked()
                    && let Some(i) = sel.and_then(|id| self.layers.iter().position(|l| l.id == id))
                    && i + 1 < self.layers.len()
                {
                    self.layers.swap(i, i + 1);
                }
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    ui.add(egui::Label::new(RichText::new(tr!("Straturi")).heading()).truncate());
                });
            });
        });
        ui.separator();
        if self.layers.is_empty() {
            ui.label(
                RichText::new(
                    tr!("Niciun strat. Folosiți Fișier > Deschide sau trageți fișiere peste hartă."),
                )
                .italics()
                .weak(),
            );
        }
        let mut action: Option<(u64, &'static str)> = None;
        egui::ScrollArea::vertical()
            .id_salt("layers_scroll")
            .max_height(ui.available_height() * 0.45)
            .show(ui, |ui| {
                for l in self.layers.iter_mut().rev() {
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut l.visible, "");
                        layer_swatch(ui, &l.kind);
                        let nsel = match &l.kind {
                            LayerKind::Vector(v) => v.sel.len(),
                            _ => 0,
                        };
                        let mut text = RichText::new(if nsel > 0 {
                            format!("{}  [{nsel}]", l.name)
                        } else {
                            l.name.clone()
                        });
                        if l.crs_assumed {
                            text = text.color(Color32::from_rgb(200, 120, 0));
                        }
                        // numele lung se taie la marginea panoului; întreg apare în indiciu
                        let r = ui
                            .add(egui::Button::selectable(self.selected_layer == Some(l.id), text).truncate())
                            .on_hover_text(tr!(
                                "{}\n{}\n{}\nSistem: {}{}",
                                l.name,
                                l.source()
                                    .map(|p| p.display().to_string())
                                    .unwrap_or_default(),
                                l.summary(),
                                l.crs()
                                    .map(|c| c.to_string())
                                    .unwrap_or_else(|| tr!("nedefinit").into()),
                                if l.crs_assumed { tr!(" (presupus)") } else { "" }
                            ));
                        if r.clicked() {
                            self.selected_layer = Some(l.id);
                        }
                        if r.double_clicked() {
                            action = Some((l.id, "zoom"));
                        }
                        r.context_menu(|ui| {
                            self.selected_layer = Some(l.id);
                            if ui.button(tr!("Zoom la strat")).clicked() {
                                action = Some((l.id, "zoom"));
                                ui.close();
                            }
                            if l.is_vector() && ui.button(tr!("Tabel de atribute")).clicked() {
                                action = Some((l.id, "table"));
                                ui.close();
                            }
                            if ui.button(tr!("Setează sistemul de coordonate…")).clicked() {
                                action = Some((l.id, "crs"));
                                ui.close();
                            }
                            if ui
                                .button(tr!("Folosește sistemul stratului pentru hartă"))
                                .clicked()
                            {
                                action = Some((l.id, "mapcrs"));
                                ui.close();
                            }
                            if ui.button(tr!("Export…")).clicked() {
                                action = Some((l.id, "export"));
                                ui.close();
                            }
                            if let LayerKind::Vector(v) = &l.kind
                                && (v.data.geom_kind() == GeomKind::Mixed
                                    || l.source().is_some_and(|p| formats::ext_of(p) == "dxf"))
                                && ui.button(tr!("Export pe tipuri de geometrie (SHP)…")).clicked()
                            {
                                action = Some((l.id, "split"));
                                ui.close();
                            }
                            if let LayerKind::Vector(v) = &mut l.kind
                                && matches!(v.data.geom_kind(), GeomKind::Polygon | GeomKind::Mixed)
                            {
                                ui.checkbox(&mut v.style.no_fill, tr!("Fără umplere (No fill)"));
                            }
                            if let LayerKind::Vector(v) = &l.kind {
                                let n = v.sel.len();
                                ui.menu_button(tr!("Selecție"), |ui| {
                                    if ui.button(tr!("După atribute…")).clicked() {
                                        action = Some((l.id, "sel_attr"));
                                        ui.close();
                                    }
                                    if ui.button(tr!("Selectează tot")).clicked() {
                                        action = Some((l.id, "sel_all"));
                                        ui.close();
                                    }
                                    if ui.button(tr!("Inversează selecția")).clicked() {
                                        action = Some((l.id, "sel_invert"));
                                        ui.close();
                                    }
                                    if ui.add_enabled(n > 0, egui::Button::new(tr!("Deselectează"))).clicked() {
                                        action = Some((l.id, "sel_clear"));
                                        ui.close();
                                    }
                                    if ui.add_enabled(n > 0, egui::Button::new(tr!("Zoom la selecție"))).clicked() {
                                        action = Some((l.id, "sel_zoom"));
                                        ui.close();
                                    }
                                    if ui
                                        .add_enabled(n > 0, egui::Button::new(tr!("Exportă selecția ({n})…")))
                                        .clicked()
                                    {
                                        action = Some((l.id, "export"));
                                        ui.close();
                                    }
                                });
                                if n > 0
                                    && ui
                                        .button(tr!("Make Layer with selected objects ({n})"))
                                        .on_hover_text(tr!("Adaugă în hartă un strat nou, doar cu obiectele selectate din acest strat"))
                                        .clicked()
                                {
                                    action = Some((l.id, "sel_layer"));
                                    ui.close();
                                }
                                if ui
                                    .button(tr!("Scoate entitățile din afara României…"))
                                    .on_hover_text(tr!("Fereastra filtrului de zonă, cu acest strat ales"))
                                    .clicked()
                                {
                                    action = Some((l.id, "area"));
                                    ui.close();
                                }
                            }
                            let is_cgxml = l.source().is_some_and(|p| {
                                formats::gdb::gdb_root(p).is_none()
                                    && (p.is_dir()
                                        || matches!(formats::ext_of(p).as_str(), "cgxml" | "zip"))
                            });
                            if is_cgxml {
                                ui.separator();
                                if ui
                                    .button(tr!("CGXML: salvează ca teren.shp / constructii.shp…"))
                                    .clicked()
                                {
                                    action = Some((l.id, "cgxml_shp"));
                                    ui.close();
                                }
                                if ui.button(tr!("CGXML: reîncarcă cu X și Y inversate")).clicked() {
                                    action = Some((l.id, "cgxml_swap"));
                                    ui.close();
                                }
                            }
                            ui.separator();
                            if ui.button(tr!("Elimină")).clicked() {
                                action = Some((l.id, "remove"));
                                ui.close();
                            }
                        });
                    });
                }
            });
        if let Some((id, a)) = action {
            match a {
                "zoom" => {
                    if let Some(b) = self.layer(id).map(|l| l.map_bounds()) {
                        self.zoom_to(b);
                    }
                }
                "table" => {
                    self.dlg.table.open = true;
                    self.dlg.table.layer_id = Some(id);
                }
                "crs" => {
                    self.dlg.crs_picker = Some(CrsPicker::new(
                        CrsPurpose::Layer(id),
                        self.layer(id).and_then(|l| l.crs().cloned()),
                    ))
                }
                "mapcrs" => {
                    if let Some(c) = self.layer(id).and_then(|l| l.crs().cloned()) {
                        self.set_map_crs(Some(c));
                    }
                }
                "export" => self.dlg.export = Some(ExportDlg::new(self, id)),
                "split" => self.open_split_dialog(Some(id)),
                "sel_all" => self.selection_action(id, "all"),
                "sel_invert" => self.selection_action(id, "invert"),
                "sel_clear" => self.selection_action(id, "clear"),
                "sel_zoom" => self.selection_action(id, "zoom"),
                "sel_layer" => self.layer_from_selection(id),
                "sel_attr" => self.open_attr_select(Some(id)),
                "area" => {
                    self.open_load_filter_dialog();
                    self.dlg.load_filter.set_layer(id);
                }
                "remove" => self.remove_layer(id),
                "cgxml_shp" => {
                    if let Some(src) = self
                        .layer(id)
                        .and_then(|l| l.source())
                        .map(|p| p.to_path_buf())
                    {
                        self.save_cgxml_shp(src);
                    }
                }
                "cgxml_swap" => {
                    if let Some(src) = self
                        .layer(id)
                        .and_then(|l| l.source())
                        .map(|p| p.to_path_buf())
                    {
                        // inversează față de starea curentă a stratului (detectată după ultimele opțiuni folosite)
                        let cur = self
                            .layer(id)
                            .map(|l| l.cgxml_swapped)
                            .unwrap_or(self.settings.cgxml.swap_xy);
                        self.reload_cgxml(src, !cur);
                    }
                }
                _ => {}
            }
        }
        ui.separator();
        if let Some(id) = self.selected_layer {
            egui::ScrollArea::both()
                .id_salt("props_scroll")
                .show(ui, |ui| {
                    self.layer_properties(ui, id);
                });
        }
    }

    /// Panoul Straturi ascuns: bandă îngustă; click oriunde pe ea îl readuce.
    fn layers_strip(&mut self, ui: &mut egui::Ui) {
        let rect = ui.max_rect();
        let resp = ui
            .allocate_rect(rect, egui::Sense::click())
            .on_hover_text(tr!("Arată panoul Straturi (Ctrl+L)"));
        let v = ui.visuals();
        let fg = if resp.hovered() { v.strong_text_color() } else { v.text_color() };
        let p = ui.painter();
        if resp.hovered() {
            p.rect_filled(rect, 0.0, v.widgets.hovered.weak_bg_fill);
        }
        let cx = rect.center().x;
        let ic = |y: f32| egui::Rect::from_center_size(egui::pos2(cx, y), egui::vec2(18.0, 18.0));
        theme::paint_icon(p, ic(rect.top() + 14.0), Icon::Expand, fg);
        theme::paint_icon(p, ic(rect.top() + 38.0), Icon::Layers, fg);
        let n = self.layers.len();
        let title = if n > 0 { tr!("Straturi ({n})") } else { tr!("Straturi").to_string() };
        let galley = p.layout_no_wrap(title, egui::FontId::proportional(13.0), fg);
        // text vertical, de sus în jos
        let pos = egui::pos2(cx + galley.size().y / 2.0, rect.top() + 56.0);
        p.add(egui::epaint::TextShape::new(pos, galley, fg).with_angle(std::f32::consts::FRAC_PI_2));
        if resp.clicked() {
            self.layers_toggle = Some(true);
        }
    }

    fn layer_properties(&mut self, ui: &mut egui::Ui, id: u64) {
        let map_crs = self.map_crs.clone();
        let mut open_crs = false;
        let Some(l) = self.layers.iter_mut().find(|l| l.id == id) else {
            return;
        };
        ui.add(egui::Label::new(RichText::new(&l.name).strong()).truncate())
            .on_hover_text(&l.name);
        egui::Grid::new("layer_info")
            .num_columns(2)
            .spacing([8.0, 2.0])
            .show(ui, |ui| {
                ui.label(tr!("Fișier:"));
                match l.source().and_then(|p| p.file_name()) {
                    Some(f) => ui.label(f.to_string_lossy().to_string()),
                    None => ui
                        .label(RichText::new(tr!("(temporar, în memorie)")).italics().weak())
                        .on_hover_text(tr!("Stratul nu are fișier: exportați-l (Export…) ca să-l păstrați; nu se salvează în spațiul de lucru.")),
                };
                ui.end_row();
                ui.label(tr!("Conținut:"));
                ui.label(l.summary());
                ui.end_row();
                ui.label(tr!("Sistem:"));
                ui.horizontal(|ui| {
                    let c = l
                        .crs()
                        .map(|c| c.short())
                        .unwrap_or_else(|| tr!("nedefinit").into());
                    ui.label(if l.crs_assumed {
                        tr!("{c} (presupus)")
                    } else {
                        c
                    });
                    if ui.small_button(tr!("schimbă")).clicked() {
                        open_crs = true;
                    }
                });
                ui.end_row();
            });
        ui.add(egui::Slider::new(&mut l.opacity, 0.0..=1.0).text(tr!("Opacitate")));
        match &mut l.kind {
            LayerKind::Vector(v) => {
                ui.collapsing(tr!("Stil"), |ui| {
                    let s = &mut v.style;
                    egui::Grid::new("vstyle").num_columns(2).show(ui, |ui| {
                        ui.label(tr!("Contur:"));
                        let mut c = s.stroke_c();
                        if ui.color_edit_button_srgba(&mut c).changed() {
                            s.stroke = c.to_srgba_unmultiplied();
                        }
                        ui.end_row();
                        ui.label(tr!("Grosime:"));
                        ui.add(egui::Slider::new(&mut s.width, 0.0..=10.0));
                        ui.end_row();
                        ui.label(tr!("Umplere:"));
                        ui.horizontal(|ui| {
                            ui.add_enabled_ui(!s.no_fill, |ui| {
                                let mut c = s.fill_c();
                                if ui.color_edit_button_srgba(&mut c).changed() {
                                    s.fill = c.to_srgba_unmultiplied();
                                }
                            });
                            ui.checkbox(&mut s.no_fill, tr!("Fără umplere (No fill)"))
                                .on_hover_text(tr!("Poligoanele se desenează doar cu conturul; punctele își păstrează culoarea"));
                        });
                        ui.end_row();
                        ui.label(tr!("Mărime punct:"));
                        ui.add(egui::Slider::new(&mut s.point_size, 1.0..=20.0));
                        ui.end_row();
                        ui.label(tr!("Etichete:"));
                        egui::ComboBox::from_id_salt("label_field")
                            .selected_text(s.label_field.clone().unwrap_or_else(|| tr!("(fără)").into()))
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut s.label_field, None, tr!("(fără)"));
                                for f in &v.data.fields {
                                    ui.selectable_value(
                                        &mut s.label_field,
                                        Some(f.name.clone()),
                                        &f.name,
                                    );
                                }
                            });
                        ui.end_row();
                        ui.label(tr!("Text etichetă:"));
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::DragValue::new(&mut s.label_size)
                                    .range(6.0..=40.0)
                                    .suffix(" pt"),
                            );
                            let mut c = s.label_c();
                            if ui.color_edit_button_srgba(&mut c).changed() {
                                s.label_color = c.to_srgba_unmultiplied();
                            }
                        });
                        ui.end_row();
                        ui.label(tr!("Halo etichete:"));
                        egui::ComboBox::from_id_salt("halo_mode")
                            .selected_text(s.halo.label())
                            .show_ui(ui, |ui| {
                                for m in layers::HaloMode::ALL {
                                    ui.selectable_value(&mut s.halo, m, m.label());
                                }
                            })
                            .response
                            .on_hover_text(tr!("Contur: urmează literele · Dreptunghi: fundal sub text"));
                        ui.end_row();
                        if s.halo != layers::HaloMode::Off {
                            ui.label(tr!("Culoare halo:"));
                            ui.horizontal(|ui| {
                                let mut auto = s.halo_color.is_none();
                                if ui
                                    .checkbox(&mut auto, tr!("Automată"))
                                    .on_hover_text(tr!("Alb pentru text închis, negru pentru text deschis"))
                                    .changed()
                                {
                                    s.halo_color = if auto { None } else { Some(s.halo_rgb()) };
                                }
                                let mut rgb = s.halo_rgb();
                                let r = ui.add_enabled_ui(!auto, |ui| ui.color_edit_button_srgb(&mut rgb)).inner;
                                if r.changed() {
                                    s.halo_color = Some(rgb);
                                }
                            });
                            ui.end_row();
                            ui.label(tr!("Transparență halo:"));
                            ui.add(
                                egui::Slider::new(&mut s.halo_transparency, 0.0..=100.0)
                                    .suffix(" %")
                                    .integer(),
                            )
                            .on_hover_text(tr!("0 % = opac, 100 % = invizibil"));
                            ui.end_row();
                            ui.label(tr!("Grosime halo:"));
                            ui.add(
                                egui::DragValue::new(&mut s.halo_width)
                                    .range(0.5..=6.0)
                                    .speed(0.1)
                                    .max_decimals(1)
                                    .suffix(" pt"),
                            )
                            .on_hover_text(tr!("Grosimea halo-ului (la dreptunghi: marginea din jurul textului)"));
                            ui.end_row();
                        }
                    });
                });
                ui.collapsing(tr!("Câmpuri"), |ui| {
                    for f in &v.data.fields {
                        ui.label(format!("{} — {}", f.name, f.ftype.label()));
                    }
                });
            }
            LayerKind::Raster(r) => {
                let mut changed = false;
                ui.collapsing(tr!("Afișare raster"), |ui| {
                    let nb = r.data.bands.len();
                    let st = &mut r.style;
                    if nb >= 3 {
                        changed |= ui.checkbox(&mut st.rgb, tr!("Compunere RGB")).changed();
                    }
                    if st.rgb {
                        for (k, name) in [tr!("Roșu"), tr!("Verde"), tr!("Albastru")].iter().enumerate() {
                            ui.horizontal(|ui| {
                                ui.label(*name);
                                egui::ComboBox::from_id_salt(format!("band{k}"))
                                    .selected_text(tr!("Banda {}", st.bands[k] + 1))
                                    .show_ui(ui, |ui| {
                                        for b in 0..nb {
                                            changed |= ui
                                                .selectable_value(
                                                    &mut st.bands[k],
                                                    b,
                                                    tr!("Banda {}", b + 1),
                                                )
                                                .changed();
                                        }
                                    });
                                changed |= ui
                                    .add(
                                        egui::DragValue::new(&mut st.min[k])
                                            .speed(1.0)
                                            .prefix("min "),
                                    )
                                    .changed();
                                changed |= ui
                                    .add(
                                        egui::DragValue::new(&mut st.max[k])
                                            .speed(1.0)
                                            .prefix("max "),
                                    )
                                    .changed();
                            });
                        }
                    } else {
                        if nb > 1 {
                            egui::ComboBox::from_id_salt("band0")
                                .selected_text(tr!("Banda {}", st.bands[0] + 1))
                                .show_ui(ui, |ui| {
                                    for b in 0..nb {
                                        changed |= ui
                                            .selectable_value(
                                                &mut st.bands[0],
                                                b,
                                                tr!("Banda {}", b + 1),
                                            )
                                            .changed();
                                    }
                                });
                        }
                        ui.horizontal(|ui| {
                            changed |= ui
                                .add(
                                    egui::DragValue::new(&mut st.min[0])
                                        .speed(1.0)
                                        .prefix("min "),
                                )
                                .changed();
                            changed |= ui
                                .add(
                                    egui::DragValue::new(&mut st.max[0])
                                        .speed(1.0)
                                        .prefix("max "),
                                )
                                .changed();
                        });
                        egui::ComboBox::from_label(tr!("Rampă de culori"))
                            .selected_text(st.ramp.label())
                            .show_ui(ui, |ui| {
                                for rm in cadastru::rasterops::Ramp::ALL {
                                    changed |=
                                        ui.selectable_value(&mut st.ramp, rm, rm.label()).changed();
                                }
                            });
                        changed |= ui
                            .checkbox(&mut st.hillshade, tr!("Umbrire relief (hillshade)"))
                            .changed();
                    }
                    let mut white = st.transparent_color == Some([255, 255, 255]);
                    if ui
                        .checkbox(&mut white, tr!("Alb transparent (hărți scanate)"))
                        .changed()
                    {
                        st.transparent_color = if white { Some([255, 255, 255]) } else { None };
                        changed = true;
                    }
                    if ui.button(tr!("Întindere automată")).clicked() {
                        let keep = (
                            st.rgb,
                            st.bands,
                            st.ramp,
                            st.hillshade,
                            st.transparent_color,
                        );
                        *st = cadastru::rasterops::RasterStyle::auto(&r.data);
                        (
                            st.rgb,
                            st.bands,
                            st.ramp,
                            st.hillshade,
                            st.transparent_color,
                        ) = keep;
                        if !st.rgb {
                            let s = r.data.stats(st.bands[0]);
                            st.min[0] = s.min;
                            st.max[0] = s.max;
                        }
                        changed = true;
                    }
                });
                ui.collapsing(tr!("Informații raster"), |ui| {
                    let (px, py) = match &r.data.full {
                        Some(f) => {
                            let g = f.0.geo_transform();
                            ((g[1].powi(2) + g[4].powi(2)).sqrt(), (g[2].powi(2) + g[5].powi(2)).sqrt())
                        }
                        None => r.data.pixel_size(),
                    };
                    if let Some(f) = &r.data.full {
                        let (w, h) = f.0.size();
                        ui.label(tr!("Format: {} (citit prin GDAL, doar vizualizare)", f.0.driver()));
                        ui.label(tr!("Dimensiune: {} x {} px; în memorie: imagine de ansamblu {} x {} px", w, h, r.data.width, r.data.height));
                    }
                    ui.label(tr!("Tip date: {}", r.data.bands[0].type_name()));
                    ui.label(format!("Pixel: {} x {}", fmt_real(px), fmt_real(py)));
                    ui.label(tr!(
                        "NoData: {}",
                        r.data.nodata.map(fmt_real).unwrap_or_else(|| "-".into())
                    ));
                    ui.label(tr!("Extindere: {}", r.data.bounds()));
                });
                if changed {
                    r.dirty = true;
                    r.detail_key = None;
                }
            }
        }
        let _ = map_crs;
        if open_crs {
            self.dlg.crs_picker = Some(CrsPicker::new(
                CrsPurpose::Layer(id),
                self.layer(id).and_then(|l| l.crs().cloned()),
            ));
        }
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if self.bulk.is_some() {
                self.bulk_stop_button(ui);
            }
            self.load_filter_indicator(ui);
            if !self.jobs.is_empty() {
                ui.spinner();
                ui.label(
                    self.jobs
                        .iter()
                        .map(|j| j.label.clone())
                        .collect::<Vec<_>>()
                        .join("; "),
                );
                ui.separator();
            }
            if let Some((x, y)) = self.cursor_map {
                let geo = self.map_crs.as_ref().is_some_and(|c| c.is_geographic());
                if geo {
                    ui.monospace(format!("Lon {:.7}  Lat {:.7}", x, y));
                } else {
                    ui.monospace(format!("X {:.2}  Y {:.2}", x, y));
                }
                if !geo && let Some((lon, lat)) = self.to_wgs.as_ref().and_then(|t| t.xy(x, y)) {
                    ui.separator();
                    ui.monospace(format!(
                        "WGS84 {:.6}°, {:.6}°  ({} {})",
                        lat,
                        lon,
                        cadastru::crs::to_dms(lat, true),
                        cadastru::crs::to_dms(lon, false)
                    ));
                }
                ui.separator();
            }
            ui.monospace(tr!(
                "Scara 1:{}",
                self.scale_denominator().round() as i64
            ));
            ui.separator();
            ui.label(&self.status);
        });
    }

    fn handle_dropped_files(&mut self, ctx: &egui::Context) {
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .filter(|p| !p.as_os_str().is_empty())
                .collect()
        });
        for p in dropped {
            if formats::ext_of(&p) == "cgw" {
                self.pending_workspace = Some(p);
            } else {
                self.open_path(p, ReadOptions::default());
            }
        }
    }

    fn errors_window(&mut self, ctx: &egui::Context) {
        if self.errors.is_empty() {
            return;
        }
        let mut open = true;
        egui::Window::new(tr!("Erori")).id(egui::Id::unique("win_mod_0"))
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_width(520.0)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .max_height(300.0)
                    .show(ui, |ui| {
                        for e in &self.errors {
                            ui.label(RichText::new(e).color(ui.visuals().error_fg_color));
                            ui.separator();
                        }
                    });
                if ui.button(tr!("Închide")).clicked() {
                    self.errors.clear();
                }
            });
        if !open {
            self.errors.clear();
        }
    }
}

/// Stil implicit pentru straturile CGXML (teren / construcții).
fn style_cgxml(v: &mut VLayer) {
    if v.data.name == "constructii" {
        v.style.stroke = [235, 70, 70, 255];
        v.style.fill = [235, 70, 70, 80];
        v.style.width = 1.4;
    } else {
        v.style.stroke = [255, 176, 32, 255];
        v.style.fill = [255, 176, 32, 45];
        v.style.width = 1.8;
        // eticheta: numărul cadastral (E2IDENTIF) sau PAPERCADNO
        for f in ["E2IDENTIF", "PAPERCADNO"] {
            if let Some(i) = v.data.field_index(f)
                && v.data
                    .features
                    .iter()
                    .any(|ft| !ft.attrs[i].to_text().is_empty())
            {
                v.style.label_field = Some(f.to_string());
                break;
            }
        }
    }
}

impl App {
    fn cgxml_options_window(&mut self, ctx: &egui::Context) {
        if !self.show_cgxml_opts {
            return;
        }
        let mut open = true;
        let mut changed = false;
        egui::Window::new(tr!("Opțiuni import CGXML")).id(egui::Id::unique("win_mod_1")).open(&mut open).collapsible(false).resizable(false).show(ctx, |ui| {
            ui.label(RichText::new(tr!("Fișierele .cgxml (și arhivele ZIP / dosarele cu CGXML) sunt convertite automat la deschidere în straturile „teren” și „constructii”, ca în CGXML2SHP.")).weak());
            ui.add_space(4.0);
            let o = &mut self.settings.cgxml;
            changed |= ui.checkbox(&mut o.close_ring, tr!("Închide inelul poligon")).on_hover_text(tr!("Repetă primul punct la final dacă lipsește")).changed();
            changed |= ui.checkbox(&mut o.rich_dbf, tr!("Atribute extinse")).on_hover_text("MEAS_AREA · AREA_CALC · AREA_DIFF · E2IDENTIF · CADSECTOR · TOPONO · OPERTYPE · EXPORTDATE").changed();
            changed |= ui.checkbox(&mut o.buildings, tr!("Include construcții (strat separat)")).changed();
            changed |= ui.checkbox(&mut o.swap_xy, tr!("Forțează inversare X / Y")).on_hover_text(tr!("Când poligoanele apar rotite/oglindite (X = Nord, Y = Est în fișier)")).changed();
            changed |= ui.checkbox(&mut o.id_code, tr!("Include CNP/CUI în PROPRIETAR")).on_hover_text(tr!("Date cu caracter personal (GDPR)")).changed();
            ui.add_space(4.0);
            ui.label(RichText::new(tr!("Opțiunile se aplică fișierelor deschise de acum înainte.")).weak().small());
        });
        if changed {
            self.settings.save();
        }
        if !open {
            self.show_cgxml_opts = false;
        }
    }
}

/// Mostră grafică pentru tipul și stilul stratului.
fn layer_swatch(ui: &mut egui::Ui, kind: &LayerKind) {
    let (r, _) = ui.allocate_exact_size(egui::vec2(16.0, 14.0), egui::Sense::hover());
    let p = ui.painter();
    match kind {
        LayerKind::Vector(v) => {
            let st = &v.style;
            let stroke = egui::Stroke::new(st.width.clamp(1.0, 2.5), st.stroke_c());
            match v.data.geom_kind() {
                GeomKind::Point => {
                    p.circle_filled(r.center(), 4.0, st.fill_c().to_opaque());
                    p.circle_stroke(r.center(), 4.0, egui::Stroke::new(1.0, st.stroke_c()));
                }
                GeomKind::Line => {
                    p.line_segment(
                        [
                            r.left_bottom() + egui::vec2(1.0, -2.0),
                            r.right_top() + egui::vec2(-1.0, 2.0),
                        ],
                        stroke,
                    );
                }
                _ => {
                    let rr = r.shrink(2.0);
                    if !st.no_fill {
                        p.rect_filled(rr, 1.0, st.fill_c());
                    }
                    p.rect_stroke(rr, 1.0, stroke, egui::StrokeKind::Inside);
                }
            }
        }
        LayerKind::Raster(_) => {
            let rr = r.shrink(1.0);
            let w = rr.width() / 3.0;
            for (i, c) in [
                Color32::from_rgb(56, 140, 80),
                Color32::from_rgb(214, 196, 120),
                Color32::from_rgb(150, 105, 80),
            ]
            .iter()
            .enumerate()
            {
                p.rect_filled(
                    egui::Rect::from_min_size(
                        rr.min + egui::vec2(w * i as f32, 0.0),
                        egui::vec2(w, rr.height()),
                    ),
                    0.0,
                    *c,
                );
            }
        }
    }
}

/// Pe Windows folosește fontul sistemului (Segoe UI) pentru aspect nativ și diacritice.
pub fn setup_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    let dir = std::env::var("WINDIR")
        .map(|w| PathBuf::from(w).join("Fonts"))
        .unwrap_or_else(|_| PathBuf::from("C:\\Windows\\Fonts"));
    let mut added = vec![];
    for (name, file) in [
        ("segoeui", "segoeui.ttf"),
        ("seguisym", "seguisym.ttf"),
        ("seguiemj", "seguiemj.ttf"),
    ] {
        if let Ok(bytes) = std::fs::read(dir.join(file)) {
            fonts.font_data.insert(
                name.to_owned(),
                std::sync::Arc::new(egui::FontData::from_owned(bytes)),
            );
            added.push(name.to_owned());
        }
    }
    if added.is_empty() {
        return;
    }
    if let Some(prop) = fonts.families.get_mut(&egui::FontFamily::Proportional) {
        for (i, n) in added.iter().enumerate() {
            if i == 0 && n == "segoeui" {
                prop.insert(0, n.clone());
            } else {
                prop.push(n.clone());
            }
        }
    }
    if let Some(mono) = fonts.families.get_mut(&egui::FontFamily::Monospace) {
        for n in &added {
            mono.push(n.clone());
        }
    }
    ctx.set_fonts(fonts);
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if let Some(p) = self.pending_workspace.take() {
            self.load_workspace(&p, &ctx);
        }
        self.poll_jobs(&ctx);
        self.handle_dropped_files(&ctx);
        self.handle_shortcuts(&ctx);

        egui::Panel::top("menu").show(ui, |ui| {
            self.menu_bar(ui);
        });
        egui::Panel::top("toolbar").show(ui, |ui| {
            self.toolbar(ui);
        });
        egui::Panel::bottom("status").show(ui, |ui| {
            self.status_bar(ui);
        });
        // Panoul Straturi: se ascunde cu « (sau Ctrl+L, sau trăgând marginea spre stânga);
        // ascuns rămâne o bandă îngustă pe care un click îl readuce.
        let mut open = self.settings.layers_panel;
        egui::Panel::show_switched(
            ui,
            &mut open,
            egui::Panel::left("layers_collapsed").resizable(true).exact_size(LAYERS_STRIP),
            egui::Panel::left("layers")
                .resizable(true)
                .default_size(290.0)
                .min_size(120.0)
                .max_size(900.0),
            |ui, expanded| {
                if expanded {
                    self.layers_panel(ui)
                } else {
                    self.layers_strip(ui)
                }
            },
        );
        let open = match self.layers_toggle.take() {
            Some(v) => v,
            None => open,
        };
        if open != self.settings.layers_panel {
            self.settings.layers_panel = open;
            self.settings.save();
        }
        egui::CentralPanel::no_frame().show(ui, |ui| {
            self.map_ui(ui);
        });

        self.dialogs(&ctx);
        self.cgxml_options_window(&ctx);
        self.errors_window(&ctx);
    }
}
