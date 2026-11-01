//! Ferestrele de dialog.

use super::layers::*;
use super::workspace::LayerSettings;
use super::{App, Job, JobResult};
use cadastru::convert::{self, ConvertOptions};
use cadastru::crs::{self, Crs, Transformer};
use cadastru::formats::{self, Kind, ReadOptions, WriteOptions};
use cadastru::model::*;
use cadastru::rasterops::{self, Resampling};
use eframe::egui::{self, RichText};
use egui_extras::{Column, TableBuilder};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::channel;
use cadastru::i18n::tr;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CrsPurpose {
    Map,
    Layer(u64),
    Export,
    ConvSrc,
    ConvDst,
    Batch,
    BatchSrc,
    Csv,
    Split,
}

pub struct CrsPicker {
    pub purpose: CrsPurpose,
    pub query: String,
    pub results: Vec<(u32, String)>,
    pub custom: String,
    pub chosen: Option<Crs>,
    pub error: Option<String>,
}

impl CrsPicker {
    pub fn new(purpose: CrsPurpose, current: Option<Crs>) -> CrsPicker {
        CrsPicker {
            purpose,
            query: String::new(),
            results: crs::search("", 50),
            custom: String::new(),
            chosen: current,
            error: None,
        }
    }
}

pub struct ExportDlg {
    pub layer_id: u64,
    pub is_vector: bool,
    pub fmt: usize,
    pub crs_mode: u8,
    pub custom_crs: Option<Crs>,
    pub res: String,
    pub resampling: Resampling,
    pub cp1250: bool,
    pub csv_wkt: bool,
    pub name_field: Option<String>,
    pub fields: Vec<String>,
    /// numărul de entități selectate în strat
    pub nsel: usize,
    /// exportă numai entitățile selectate
    pub only_sel: bool,
}

impl ExportDlg {
    pub fn new(app: &App, id: u64) -> ExportDlg {
        let l = app.layer(id);
        let is_vector = l.is_some_and(|l| l.is_vector());
        let fields = match l.map(|l| &l.kind) {
            Some(LayerKind::Vector(v)) => v.data.fields.iter().map(|f| f.name.clone()).collect(),
            _ => vec![],
        };
        let nsel = match l.map(|l| &l.kind) {
            Some(LayerKind::Vector(v)) => v.sel.len(),
            _ => 0,
        };
        ExportDlg {
            layer_id: id,
            is_vector,
            fmt: 0,
            crs_mode: 0,
            custom_crs: None,
            res: String::new(),
            resampling: Resampling::Nearest,
            cp1250: false,
            csv_wkt: false,
            name_field: None,
            fields,
            nsel,
            only_sel: nsel > 0,
        }
    }
    fn formats(&self) -> Vec<&'static formats::FormatInfo> {
        let k = if self.is_vector {
            Kind::Vector
        } else {
            Kind::Raster
        };
        formats::FORMATS
            .iter()
            .filter(|f| f.write && f.kind == k)
            .collect()
    }
}

#[derive(Default)]
pub struct ConverterDlg {
    pub open: bool,
    pub src: Option<Crs>,
    pub dst: Option<Crs>,
    pub x: String,
    pub y: String,
    pub result: String,
    pub h: String,
    pub batch_in: String,
    pub batch_out: String,
    pub pick_from_map: bool,
}

impl ConverterDlg {
    pub fn set_from_map(&mut self, x: f64, y: f64, map_crs: Option<Crs>) {
        let (mut px, mut py) = (x, y);
        if let (Some(m), Some(s)) = (&map_crs, &self.src)
            && let Ok(t) = Transformer::new(m, s)
            && let Some((a, b)) = t.xy(x, y)
        {
            px = a;
            py = b;
        }
        let geo = self.src.as_ref().is_some_and(|c| c.is_geographic());
        self.x = if geo {
            format!("{px:.9}")
        } else {
            format!("{px:.3}")
        };
        self.y = if geo {
            format!("{py:.9}")
        } else {
            format!("{py:.3}")
        };
        self.compute();
    }

    fn compute(&mut self) {
        let (Some(s), Some(d)) = (&self.src, &self.dst) else {
            self.result = tr!("Alegeți sistemele sursă și destinație").into();
            return;
        };
        let parse = |t: &str| cadastru::formats::csv::parse_num(t);
        let (Some(x), Some(y)) = (parse(&self.x), parse(&self.y)) else {
            self.result = String::new();
            return;
        };
        self.result = match Transformer::new(s, d).ok().and_then(|t| t.xy_method(x, y)) {
            Some(((a, b), m)) => {
                let mut r = if d.is_geographic() {
                    format!(
                        "Lon (X): {a:.9}\nLat (Y): {b:.9}\n{} {}",
                        crs::to_dms(b, true),
                        crs::to_dms(a, false)
                    )
                } else {
                    tr!("X (Est): {a:.4}\nY (Nord): {b:.4}")
                };
                if let Some(hs) = self.height_line(s, d, x, y) {
                    r.push('\n');
                    r.push_str(&hs);
                }
                if m != crs::Method::Identity {
                    r.push_str(&tr!("\nMetoda: {}", m.label()));
                }
                r
            }
            None => tr!("Transformarea a eșuat").into(),
        };
    }

    /// Altitudinea: între sistemele naționale (H Marea Neagră 1975) și celelalte (h elipsoidal).
    fn height_line(&self, s: &Crs, d: &Crs, x: f64, y: f64) -> Option<String> {
        let h = cadastru::formats::csv::parse_num(&self.h)?;
        let (ss, ds) = (s.stereo_kind().is_some(), d.stereo_kind().is_some());
        if ss == ds {
            return Some(tr!("Altitudine: {h:.3} m (nemodificată)"));
        }
        let etrs = Crs::from_epsg(4258).ok()?;
        let (lon, lat) = Transformer::new(s, &etrs).ok()?.xy(x, y)?;
        let model = crs::height_model();
        Some(if ss {
            match cadastru::transdatro::hmn_to_h(lat, lon, h, model) {
                Ok(v) => tr!("h elipsoidal ETRS89: {v:.3} m"),
                Err(e) => tr!("Altitudine: {e}"),
            }
        } else {
            match cadastru::transdatro::h_to_hmn(lat, lon, h, model) {
                Ok(v) => tr!("H Marea Neagră 1975: {v:.3} m"),
                Err(e) => tr!("Altitudine: {e}"),
            }
        })
    }

    fn compute_batch(&mut self) {
        let (Some(s), Some(d)) = (&self.src, &self.dst) else {
            return;
        };
        let Ok(t) = Transformer::new(s, d) else {
            return;
        };
        let prec = if d.is_geographic() { 9 } else { 4 };
        let mut out = String::new();
        for line in self.batch_in.lines() {
            let parts: Vec<&str> = line
                .split(|c: char| c.is_whitespace() || c == ';' || c == '\t')
                .filter(|p| !p.is_empty())
                .collect();
            // permite și separator virgulă dacă nu există alte separatoare
            let parts: Vec<String> = if parts.len() == 1 {
                line.split(',').map(|s| s.trim().to_string()).collect()
            } else {
                parts.iter().map(|s| s.to_string()).collect()
            };
            let nums: Vec<f64> = parts
                .iter()
                .filter_map(|p| cadastru::formats::csv::parse_num(p))
                .collect();
            if nums.len() >= 2 {
                // ultimele două numere = X Y (prima coloană poate fi un identificator)
                let (x, y) = if nums.len() >= 3 && parts.len() >= 3 {
                    (nums[nums.len() - 2], nums[nums.len() - 1])
                } else {
                    (nums[0], nums[1])
                };
                let id = if parts.len() >= 3 {
                    format!("{}\t", parts[0])
                } else {
                    String::new()
                };
                match t.xy(x, y) {
                    Some((a, b)) => out.push_str(&format!("{id}{a:.prec$}\t{b:.prec$}\n")),
                    None => out.push_str(&tr!("{id}EROARE\n")),
                }
            } else if !line.trim().is_empty() {
                out.push_str("?\n");
            }
        }
        self.batch_out = out;
    }
}

#[derive(Default)]
pub struct TableDlg {
    pub open: bool,
    pub layer_id: Option<u64>,
    pub filter: String,
    pub sort: Option<(usize, bool)>,
    /// (strat, filtru, sortare, nr. entități, doar selecția, generația selecției)
    pub cache_key: Option<(u64, String, Option<(usize, bool)>, usize, bool, u64)>,
    pub rows: Vec<usize>,
    pub scroll_to: Option<usize>,
    /// afișează doar entitățile selectate
    pub only_sel: bool,
    /// rândul de la care pornește selecția cu Shift+click
    pub anchor: Option<usize>,
}

pub struct CsvImport {
    pub path: PathBuf,
    pub headers: Vec<String>,
    pub preview: Vec<Vec<String>>,
    pub x: Option<usize>,
    pub y: Option<usize>,
    pub z: Option<usize>,
    pub wkt: Option<usize>,
    pub use_wkt: bool,
    pub crs: Option<Crs>,
}

impl CsvImport {
    pub fn new(path: &std::path::Path) -> anyhow::Result<CsvImport> {
        let (headers, rows) = cadastru::formats::csv::read_table(path, &ReadOptions::default())?;
        let (x, y, z, wkt) = cadastru::formats::csv::detect_columns(&headers);
        let geo = x.is_some_and(|i| {
            ["lon", "long", "lng", "longitude", "longitudine"]
                .contains(&headers[i].to_lowercase().as_str())
        });
        let crs = cadastru::formats::read_prj_sidecar(path)
            .or_else(|| if geo { Some(Crs::wgs84()) } else { None });
        Ok(CsvImport {
            path: path.to_path_buf(),
            headers,
            preview: rows.into_iter().take(6).collect(),
            x,
            y,
            z,
            use_wkt: wkt.is_some() && x.is_none(),
            wkt,
            crs,
        })
    }
}

pub struct BatchDlg {
    pub in_dir: Option<PathBuf>,
    pub out_dir: Option<PathBuf>,
    pub to_idx: usize,
    pub only: String,
    pub crs: Option<Crs>,
    pub src_crs: Option<Crs>,
    pub recursive: bool,
    pub cp1250: bool,
    pub log: Vec<String>,
    pub running: bool,
}

impl Default for BatchDlg {
    fn default() -> Self {
        BatchDlg {
            in_dir: None,
            out_dir: None,
            to_idx: 1,
            only: String::new(),
            crs: None,
            src_crs: None,
            recursive: false,
            cp1250: false,
            log: vec![],
            running: false,
        }
    }
}

#[derive(Default)]
pub struct Dialogs {
    pub crs_picker: Option<CrsPicker>,
    pub export: Option<ExportDlg>,
    pub converter: ConverterDlg,
    pub tdr: super::tdr::TdrDlg,
    pub print: super::print::PrintDlg,
    pub table: TableDlg,
    pub search: super::search::SearchDlg,
    pub split: Option<super::split::SplitDlg>,
    pub bulk: Option<super::bulk::BulkDlg>,
    pub attrsel: super::attrsel::AttrSelDlg,
    pub load_filter: super::loadfilter::LoadFilterDlg,
    pub csv: Option<CsvImport>,
    pub batch: Option<BatchDlg>,
    pub about: bool,
    pub help: bool,
    pub pending_settings: HashMap<PathBuf, Vec<LayerSettings>>,
    pub ws_order: HashMap<u64, usize>,
    pub ws_view: Option<super::View>,
}

fn crs_button(ui: &mut egui::Ui, c: &Option<Crs>, empty: &str) -> bool {
    let t = c
        .as_ref()
        .map(|c| c.to_string())
        .unwrap_or_else(|| empty.to_string());
    ui.button(t).clicked()
}

impl App {
    pub fn zoom_to_feature(&mut self, lid: u64, fi: usize) {
        let b = self.layer(lid).and_then(|l| match &l.kind {
            LayerKind::Vector(v) => v.disp.iter().find(|d| d.feat == fi).map(|d| d.bbox),
            _ => None,
        });
        if let Some(b) = b {
            if b.width() == 0.0 && b.height() == 0.0 {
                self.view.cx = b.minx;
                self.view.cy = b.miny;
                let geo = self.map_crs.as_ref().is_some_and(|c| c.is_geographic());
                let max = if geo { 0.00002 } else { 2.0 };
                self.view.scale = self.view.scale.min(max);
            } else {
                let pad = b.width().max(b.height()) * 0.15;
                self.zoom_to(b.expanded(pad));
            }
            self.selection = Some((lid, fi));
        }
    }

    pub fn export_all_gpkg(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .set_title(tr!("Export în GeoPackage"))
            .add_filter("GeoPackage", &["gpkg"])
            .set_file_name("export.gpkg")
            .save_file()
        else {
            return;
        };
        let layers: Vec<VectorLayer> = self
            .layers
            .iter()
            .filter_map(|l| match &l.kind {
                LayerKind::Vector(v) => {
                    let mut d = v.data.clone();
                    d.name = l.name.clone();
                    Some(d)
                }
                _ => None,
            })
            .collect();
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            // nume unice
            let mut layers = layers;
            let mut seen: Vec<String> = vec![];
            for l in &mut layers {
                let mut n = formats::sanitize(&l.name);
                while seen.contains(&n.to_lowercase()) {
                    n.push('_');
                }
                seen.push(n.to_lowercase());
                l.name = n;
            }
            let refs: Vec<&VectorLayer> = layers.iter().collect();
            let r = formats::write_vector(
                &refs,
                &path,
                &WriteOptions {
                    overwrite: true,
                    ..Default::default()
                },
            );
            let _ = tx.send(JobResult::Exported(r));
        });
        self.jobs.push(Job {
            label: tr!("Export GeoPackage").into(),
            rx,
        });
    }

    fn apply_crs_choice(&mut self, purpose: CrsPurpose, c: Crs) {
        match purpose {
            CrsPurpose::Map => self.set_map_crs(Some(c)),
            CrsPurpose::Layer(id) => {
                let had_map = self.map_crs.is_some();
                if let Some(l) = self.layer_mut(id) {
                    l.set_crs(Some(c.clone()));
                    l.crs_assumed = false;
                }
                if !had_map {
                    self.set_map_crs(Some(c));
                } else {
                    let mc = self.map_crs.clone();
                    if let Some(l) = self.layer_mut(id) {
                        l.rebuild(mc.as_ref());
                    }
                }
                self.status = tr!("Sistemul stratului a fost actualizat.").into();
            }
            CrsPurpose::Export => {
                if let Some(e) = self.dlg.export.as_mut() {
                    e.custom_crs = Some(c);
                    e.crs_mode = 2;
                }
            }
            CrsPurpose::Split => {
                if let Some(s) = self.dlg.split.as_mut() {
                    s.custom_crs = Some(c);
                    s.crs_mode = 2;
                }
            }
            CrsPurpose::ConvSrc => {
                self.dlg.converter.src = Some(c);
                self.dlg.converter.compute();
            }
            CrsPurpose::ConvDst => {
                self.dlg.converter.dst = Some(c);
                self.dlg.converter.compute();
            }
            CrsPurpose::Batch => {
                if let Some(b) = self.dlg.batch.as_mut() {
                    b.crs = Some(c);
                }
            }
            CrsPurpose::BatchSrc => {
                if let Some(b) = self.dlg.batch.as_mut() {
                    b.src_crs = Some(c);
                }
            }
            CrsPurpose::Csv => {
                if let Some(d) = self.dlg.csv.as_mut() {
                    d.crs = Some(c);
                }
            }
        }
    }

    pub fn dialogs(&mut self, ctx: &egui::Context) {
        self.crs_picker_window(ctx);
        self.export_window(ctx);
        self.split_window(ctx);
        self.bulk_window(ctx);
        self.attrsel_window(ctx);
        self.load_filter_window(ctx);
        self.converter_window(ctx);
        self.transdatro_window(ctx);
        self.basemap_custom_window(ctx);
        self.print_window(ctx);
        self.table_window(ctx);
        self.search_window(ctx);
        self.identify_window(ctx);
        self.csv_window(ctx);
        self.batch_window(ctx);
        self.help_windows(ctx);
    }

    fn crs_picker_window(&mut self, ctx: &egui::Context) {
        let Some(mut p) = self.dlg.crs_picker.take() else {
            return;
        };
        let mut open = true;
        let mut done: Option<Crs> = None;
        let mut cancel = false;
        let title = match p.purpose {
            CrsPurpose::Map => tr!("Sistemul de coordonate al hărții"),
            CrsPurpose::Layer(_) => tr!("Sistemul de coordonate al stratului"),
            _ => tr!("Alegeți sistemul de coordonate"),
        };
        egui::Window::new(title).id(egui::Id::unique("win_dialogs_0")).open(&mut open).default_width(560.0).default_height(520.0).collapsible(false).show(ctx, |ui| {
            if let CrsPurpose::Layer(_) = p.purpose {
                ui.label(RichText::new(tr!("Atenție: se schimbă doar interpretarea coordonatelor stratului (nu se transformă datele). Pentru transformare folosiți Export.")).weak());
            }
            ui.horizontal(|ui| {
                ui.label(tr!("Caută:"));
                let r = ui.add(egui::TextEdit::singleline(&mut p.query).hint_text(tr!("cod EPSG sau nume (ex. 3844, stereo, utm 35)")).desired_width(320.0));
                if r.changed() {
                    p.results = crs::search(&p.query, 400);
                }
            });
            if p.query.is_empty() {
                ui.label(RichText::new(tr!("Sisteme uzuale:")).strong());
            }
            egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                for sc in crs::special_crs(&p.query) {
                    let sel = p.chosen.as_ref() == Some(&sc);
                    if ui.selectable_label(sel, format!("TransDatRO  —  {}", sc.name)).clicked() {
                        p.chosen = Some(sc);
                        p.error = None;
                    }
                }
                for (code, name) in &p.results {
                    let sel = p.chosen.as_ref().and_then(|c| c.epsg) == Some(*code);
                    if ui.selectable_label(sel, format!("EPSG:{code}  —  {name}")).clicked() {
                        match Crs::from_epsg(*code) {
                            Ok(c) => {
                                p.chosen = Some(c);
                                p.error = None;
                            }
                            Err(e) => p.error = Some(e.to_string()),
                        }
                    }
                }
                if p.results.is_empty() && crs::special_crs(&p.query).is_empty() {
                    ui.label(tr!("Niciun rezultat."));
                }
            });
            ui.separator();
            ui.label(tr!("Sau definiție personalizată (șir PROJ sau WKT):"));
            ui.add(egui::TextEdit::multiline(&mut p.custom).desired_rows(2).desired_width(f32::INFINITY).hint_text("+proj=sterea +lat_0=46 +lon_0=25 +k=0.99975 +x_0=500000 +y_0=500000 +ellps=krass +towgs84=..."));
            if ui.button(tr!("Folosește definiția")).clicked() {
                match Crs::parse(&p.custom) {
                    Ok(c) => {
                        p.chosen = Some(c);
                        p.error = None;
                    }
                    Err(e) => p.error = Some(e.to_string()),
                }
            }
            ui.separator();
            if let Some(c) = &p.chosen {
                ui.label(RichText::new(c.to_string()).strong());
                ui.label(RichText::new(&c.proj).monospace().small());
            }
            if let Some(e) = &p.error {
                ui.colored_label(ui.visuals().error_fg_color, e);
            }
            ui.horizontal(|ui| {
                if ui.add_enabled(p.chosen.is_some(), egui::Button::new("OK")).clicked() {
                    done = p.chosen.clone();
                }
                if ui.button(tr!("Anulează")).clicked() {
                    cancel = true;
                }
            });
        });
        if let Some(c) = done {
            self.apply_crs_choice(p.purpose, c);
        } else if open && !cancel {
            self.dlg.crs_picker = Some(p);
        }
    }

    fn export_window(&mut self, ctx: &egui::Context) {
        let Some(mut e) = self.dlg.export.take() else {
            return;
        };
        let mut open = true;
        let mut go = false;
        let mut pick_crs = false;
        let layer_name = self
            .layer(e.layer_id)
            .map(|l| l.name.clone())
            .unwrap_or_default();
        let layer_crs = self.layer(e.layer_id).and_then(|l| l.crs().cloned());
        // selecția se poate schimba cât timp fereastra este deschisă
        let nsel_now = match self.layer(e.layer_id).map(|l| &l.kind) {
            Some(LayerKind::Vector(v)) => v.sel.len(),
            _ => 0,
        };
        if nsel_now > 0 && e.nsel == 0 {
            e.only_sel = true;
        }
        e.nsel = nsel_now;
        egui::Window::new(format!("Export: {layer_name}")).id(egui::Id::unique("win_dialogs_1"))
            .open(&mut open)
            .collapsible(false)
            .default_width(440.0)
            .show(ctx, |ui| {
                let fmts = e.formats();
                e.fmt = e.fmt.min(fmts.len().saturating_sub(1));
                egui::Grid::new("exp")
                    .num_columns(2)
                    .spacing([10.0, 6.0])
                    .show(ui, |ui| {
                        ui.label(tr!("Format:"));
                        egui::ComboBox::from_id_salt("expfmt")
                            .selected_text(fmts.get(e.fmt).map(|f| f.label()).unwrap_or(""))
                            .show_ui(ui, |ui| {
                                for (i, f) in fmts.iter().enumerate() {
                                    ui.selectable_value(&mut e.fmt, i, f.label());
                                }
                            });
                        ui.end_row();
                        ui.label(tr!("Sistem de ieșire:"));
                        ui.vertical(|ui| {
                            ui.radio_value(
                                &mut e.crs_mode,
                                0,
                                tr!(
                                    "Al stratului ({})",
                                    layer_crs
                                        .as_ref()
                                        .map(|c| c.short())
                                        .unwrap_or_else(|| tr!("nedefinit").into())
                                ),
                            );
                            ui.radio_value(
                                &mut e.crs_mode,
                                1,
                                tr!(
                                    "Al hărții ({})",
                                    self.map_crs
                                        .as_ref()
                                        .map(|c| c.short())
                                        .unwrap_or_else(|| tr!("nedefinit").into())
                                ),
                            );
                            ui.horizontal(|ui| {
                                ui.radio_value(&mut e.crs_mode, 2, tr!("Altul:"));
                                if crs_button(ui, &e.custom_crs, tr!("alege…")) {
                                    pick_crs = true;
                                }
                            });
                        });
                        ui.end_row();
                        let ext = fmts.get(e.fmt).map(|f| f.exts[0]).unwrap_or("");
                        if e.is_vector && e.nsel > 0 {
                            ui.label(tr!("Entități:"));
                            ui.vertical(|ui| {
                                ui.radio_value(&mut e.only_sel, true, tr!("numai cele selectate ({})", e.nsel));
                                ui.radio_value(&mut e.only_sel, false, tr!("toate"));
                            });
                            ui.end_row();
                        }
                        if e.is_vector {
                            if ext == "shp" {
                                ui.label(tr!("Codificare DBF:"));
                                ui.horizontal(|ui| {
                                    ui.radio_value(&mut e.cp1250, false, "UTF-8");
                                    ui.radio_value(&mut e.cp1250, true, "Windows-1250");
                                });
                                ui.end_row();
                            }
                            if ext == "csv" {
                                ui.label(tr!("Geometrie:"));
                                ui.checkbox(&mut e.csv_wkt, tr!("scrie ca WKT"));
                                ui.end_row();
                            }
                            if matches!(ext, "kml" | "gpx" | "dxf") {
                                ui.label(if ext == "dxf" {
                                    tr!("Câmp text:")
                                } else {
                                    tr!("Câmp nume:")
                                });
                                egui::ComboBox::from_id_salt("namef")
                                    .selected_text(
                                        e.name_field.clone().unwrap_or_else(|| tr!("(automat)").into()),
                                    )
                                    .show_ui(ui, |ui| {
                                        ui.selectable_value(&mut e.name_field, None, tr!("(automat)"));
                                        for f in &e.fields {
                                            ui.selectable_value(
                                                &mut e.name_field,
                                                Some(f.clone()),
                                                f,
                                            );
                                        }
                                    });
                                ui.end_row();
                            }
                        } else {
                            ui.label(tr!("Rezoluție:"));
                            ui.add(
                                egui::TextEdit::singleline(&mut e.res)
                                    .hint_text(tr!("automată"))
                                    .desired_width(120.0),
                            );
                            ui.end_row();
                            ui.label(tr!("Reeșantionare:"));
                            ui.horizontal(|ui| {
                                ui.radio_value(
                                    &mut e.resampling,
                                    Resampling::Nearest,
                                    tr!("Cel mai apropiat"),
                                );
                                ui.radio_value(
                                    &mut e.resampling,
                                    Resampling::Bilinear,
                                    tr!("Biliniară"),
                                );
                            });
                            ui.end_row();
                        }
                    });
                if matches!(fmts.get(e.fmt).map(|f| f.exts[0]), Some("kml" | "gpx")) {
                    ui.label(
                        RichText::new(
                            tr!("KML și GPX sunt întotdeauna în WGS84 (transformare automată)."),
                        )
                        .weak(),
                    );
                }
                ui.separator();
                if ui.button(tr!("Export…")).clicked() {
                    go = true;
                }
            });
        if pick_crs {
            self.dlg.crs_picker = Some(CrsPicker::new(CrsPurpose::Export, e.custom_crs.clone()));
        }
        if go {
            let fmts = e.formats();
            let Some(f) = fmts.get(e.fmt) else { return };
            let ext = f.exts[0];
            let path = rfd::FileDialog::new()
                .set_title(tr!("Salvează ca"))
                .add_filter(f.label(), f.exts)
                .set_file_name(format!(
                    "{}{}.{}",
                    formats::sanitize(&layer_name),
                    if e.is_vector && e.only_sel && e.nsel > 0 { tr!("_selectie") } else { "" },
                    ext
                ))
                .save_file();
            if let Some(mut path) = path {
                if formats::ext_of(&path).is_empty() {
                    path.set_extension(ext);
                }
                let target = match e.crs_mode {
                    1 => self.map_crs.clone(),
                    2 => e.custom_crs.clone(),
                    _ => None,
                };
                let wopts = WriteOptions {
                    encoding: if e.cp1250 {
                        Some("cp1250".into())
                    } else {
                        None
                    },
                    overwrite: true,
                    csv_wkt: e.csv_wkt,
                    name_field: e.name_field.clone(),
                    ..Default::default()
                };
                let res = cadastru::formats::csv::parse_num(&e.res);
                let rs = e.resampling;
                let data = self.layer(e.layer_id).map(|l| match &l.kind {
                    LayerKind::Vector(v) => {
                        let mut d = if e.only_sel && !v.sel.is_empty() {
                            v.selected_data()
                        } else {
                            v.data.clone()
                        };
                        d.name = l.name.clone();
                        Dataset::Vector(d)
                    }
                    LayerKind::Raster(r) => Dataset::Raster(r.data.clone()),
                });
                if let Some(data) = data {
                    let (tx, rx) = channel();
                    std::thread::spawn(move || {
                        let r = (|| -> anyhow::Result<Vec<PathBuf>> {
                            match data {
                                Dataset::Vector(mut v) => {
                                    if let Some(t) = &target
                                        && v.crs.as_ref() != Some(t)
                                    {
                                        crs::reproject_layer(&mut v, t)?;
                                    }
                                    formats::write_vector(&[&v], &path, &wopts)
                                }
                                Dataset::Raster(r) => {
                                    let r2 = match &target {
                                        Some(t) if r.crs.as_ref() != Some(t) => {
                                            rasterops::warp(&r, t, res, rs)?
                                        }
                                        _ => match res {
                                            Some(x) => rasterops::resize(&r, x, rs)?,
                                            None => r,
                                        },
                                    };
                                    formats::write_raster(&r2, &path, &wopts)
                                }
                            }
                        })();
                        let _ = tx.send(JobResult::Exported(r));
                    });
                    self.jobs.push(Job {
                        label: tr!("Export {layer_name}"),
                        rx,
                    });
                }
                return;
            }
        }
        if open {
            self.dlg.export = Some(e);
        }
    }

    fn converter_window(&mut self, ctx: &egui::Context) {
        if !self.dlg.converter.open {
            return;
        }
        let mut open = true;
        let mut pick: Option<CrsPurpose> = None;
        let c = &mut self.dlg.converter;
        egui::Window::new(tr!("Convertor de coordonate")).id(egui::Id::unique("win_dialogs_2"))
            .open(&mut open)
            .default_width(520.0)
            .show(ctx, |ui| {
                egui::Grid::new("conv")
                    .num_columns(2)
                    .spacing([10.0, 6.0])
                    .show(ui, |ui| {
                        ui.label(tr!("Din:"));
                        if crs_button(ui, &c.src, tr!("alege…")) {
                            pick = Some(CrsPurpose::ConvSrc);
                        }
                        ui.end_row();
                        ui.label(tr!("În:"));
                        ui.horizontal(|ui| {
                            if crs_button(ui, &c.dst, tr!("alege…")) {
                                pick = Some(CrsPurpose::ConvDst);
                            }
                            if ui.button(tr!("Inversează")).clicked() {
                                std::mem::swap(&mut c.src, &mut c.dst);
                                c.compute();
                            }
                        });
                        ui.end_row();
                        let geo = c.src.as_ref().is_some_and(|s| s.is_geographic());
                        ui.label(if geo { tr!("Longitudine (X):") } else { tr!("X (Est):") });
                        if ui
                            .add(egui::TextEdit::singleline(&mut c.x).desired_width(200.0))
                            .changed()
                        {
                            c.compute();
                        }
                        ui.end_row();
                        ui.label(if geo { tr!("Latitudine (Y):") } else { tr!("Y (Nord):") });
                        if ui
                            .add(egui::TextEdit::singleline(&mut c.y).desired_width(200.0))
                            .changed()
                        {
                            c.compute();
                        }
                        ui.end_row();
                        let src_st = c.src.as_ref().is_some_and(|s| s.stereo_kind().is_some());
                        ui.label(if src_st { tr!("Altitudine H MN75:") } else { tr!("Altitudine h:") })
                            .on_hover_text(tr!("Opțional. Între Stereo 70/30 (H Marea Neagră 1975) și celelalte sisteme (h elipsoidal ETRS89) se aplică cvasigeoidul TransDatRO."));
                        if ui
                            .add(
                                egui::TextEdit::singleline(&mut c.h)
                                    .desired_width(120.0)
                                    .hint_text(tr!("opțional")),
                            )
                            .changed()
                        {
                            c.compute();
                        }
                        ui.end_row();
                    });
                ui.checkbox(
                    &mut c.pick_from_map,
                    tr!("Preia punctul printr-un click pe hartă"),
                );
                ui.separator();
                ui.horizontal(|ui| {
                    ui.add(
                        egui::Label::new(RichText::new(&c.result).monospace().size(14.0))
                            .selectable(true),
                    );
                    if !c.result.is_empty()
                        && ui
                            .small_button(tr!("Copiază"))
                            .on_hover_text(tr!("Copiază"))
                            .clicked()
                    {
                        ui.ctx().copy_text(c.result.clone());
                    }
                });
                ui.separator();
                ui.collapsing(tr!("Conversie listă de puncte"), |ui| {
                    ui.label(
                        RichText::new(
                            tr!("O linie pe punct: „X Y” sau „ID X Y” (separator spațiu, tab, ; sau ,)"),
                        )
                        .weak(),
                    );
                    ui.columns(2, |cols| {
                        egui::ScrollArea::vertical()
                            .id_salt("bin")
                            .max_height(220.0)
                            .show(&mut cols[0], |ui| {
                                ui.add(
                                    egui::TextEdit::multiline(&mut c.batch_in)
                                        .desired_rows(10)
                                        .font(egui::TextStyle::Monospace)
                                        .hint_text("587932.46 325798.36"),
                                );
                            });
                        egui::ScrollArea::vertical()
                            .id_salt("bout")
                            .max_height(220.0)
                            .show(&mut cols[1], |ui| {
                                ui.add(
                                    egui::TextEdit::multiline(&mut c.batch_out.as_str())
                                        .desired_rows(10)
                                        .font(egui::TextStyle::Monospace),
                                );
                            });
                    });
                    ui.horizontal(|ui| {
                        if ui.button(tr!("Transformă lista")).clicked() {
                            c.compute_batch();
                        }
                        if ui.button(tr!("Copiază rezultatul")).clicked() {
                            ui.ctx().copy_text(c.batch_out.clone());
                        }
                    });
                });
            });
        if !open {
            self.dlg.converter.open = false;
            self.dlg.converter.pick_from_map = false;
        }
        if let Some(p) = pick {
            let cur = if p == CrsPurpose::ConvSrc {
                self.dlg.converter.src.clone()
            } else {
                self.dlg.converter.dst.clone()
            };
            self.dlg.crs_picker = Some(CrsPicker::new(p, cur));
        }
    }

    fn table_window(&mut self, ctx: &egui::Context) {
        if !self.dlg.table.open {
            return;
        }
        let mut open = true;
        let vec_layers: Vec<(u64, String)> = self
            .layers
            .iter()
            .filter(|l| l.is_vector())
            .map(|l| (l.id, l.name.clone()))
            .collect();
        if self
            .dlg
            .table
            .layer_id
            .is_none_or(|id| !vec_layers.iter().any(|(i, _)| *i == id))
        {
            self.dlg.table.layer_id = vec_layers.first().map(|(i, _)| *i);
        }
        // (strat, entitate, rândul afișat, taste, dublu-click)
        let mut select: Option<(u64, usize, usize, egui::Modifiers, bool)> = None;
        // operații pe selecție: (strat, ce, entități)
        let mut sel_op: Option<(u64, &'static str, Vec<usize>)> = None;
        let selection = self.selection;
        let t = &mut self.dlg.table;
        let layer = t
            .layer_id
            .and_then(|id| self.layers.iter().find(|l| l.id == id));
        egui::Window::new(tr!("Tabel de atribute")).id(egui::Id::unique("win_dialogs_3")).open(&mut open).default_size([760.0, 420.0]).resizable(true).show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(tr!("Strat:"));
                egui::ComboBox::from_id_salt("tbl_layer").selected_text(layer.map(|l| l.name.clone()).unwrap_or_default()).show_ui(ui, |ui| {
                    for (id, n) in &vec_layers {
                        ui.selectable_value(&mut t.layer_id, Some(*id), n);
                    }
                });
                ui.label(tr!("Filtru:"));
                ui.add(egui::TextEdit::singleline(&mut t.filter).hint_text(tr!("text căutat în orice câmp")).desired_width(220.0));
            });
            let Some(l) = layer else {
                ui.label(tr!("Niciun strat vectorial."));
                return;
            };
            let LayerKind::Vector(v) = &l.kind else { return };
            ui.horizontal(|ui| {
                let n = v.sel.len();
                ui.label(RichText::new(tr!("Selectate: {n}")).color(if n > 0 { crate::gui::select::SELECTED } else { ui.visuals().weak_text_color() }));
                ui.checkbox(&mut t.only_sel, tr!("Doar selecția"));
                ui.separator();
                if ui.small_button(tr!("Selectează rândurile afișate")).clicked() {
                    sel_op = Some((l.id, "rows", t.rows.clone()));
                }
                if ui.add_enabled(n > 0, egui::Button::new(tr!("Deselectează")).small()).clicked() {
                    sel_op = Some((l.id, "clear", vec![]));
                }
                if ui.small_button(tr!("Inversează")).clicked() {
                    sel_op = Some((l.id, "invert", vec![]));
                }
                if ui.add_enabled(n > 0, egui::Button::new(tr!("Zoom la selecție")).small()).clicked() {
                    sel_op = Some((l.id, "zoom", vec![]));
                }
                if ui.add_enabled(n > 0, egui::Button::new(tr!("Exportă selecția…")).small()).clicked() {
                    sel_op = Some((l.id, "export", vec![]));
                }
                if ui.small_button(tr!("După atribute…")).on_hover_text(tr!("Selectare după atribute (Ctrl+Shift+F)")).clicked() {
                    sel_op = Some((l.id, "attr", vec![]));
                }
            });
            let key = (l.id, t.filter.clone(), t.sort, v.data.features.len(), t.only_sel, if t.only_sel { v.sel_gen } else { 0 });
            if t.cache_key.as_ref() != Some(&key) {
                let q = t.filter.to_lowercase();
                let mut rows: Vec<usize> = (0..v.data.features.len()).filter(|&i| !t.only_sel || v.sel.contains(&i)).filter(|&i| q.is_empty() || v.data.features[i].attrs.iter().any(|a| a.to_text().to_lowercase().contains(&q))).collect();
                if let Some((col, asc)) = t.sort {
                    rows.sort_by(|&a, &b| {
                        let (va, vb) = (&v.data.features[a].attrs[col], &v.data.features[b].attrs[col]);
                        let o = match (va.as_f64(), vb.as_f64(), matches!(va, Value::Text(_))) {
                            (Some(x), Some(y), false) => x.total_cmp(&y),
                            _ => va.to_text().to_lowercase().cmp(&vb.to_text().to_lowercase()),
                        };
                        if asc { o } else { o.reverse() }
                    });
                }
                t.rows = rows;
                t.cache_key = Some(key);
            }
            ui.label(tr!("{} din {} entități", t.rows.len(), v.data.features.len()));
            let nf = v.data.fields.len();
            let scroll_row = t.scroll_to.take().and_then(|fi| t.rows.iter().position(|&r| r == fi));
            let mut tb = TableBuilder::new(ui)
                .striped(true)
                .resizable(true)
                .sense(egui::Sense::click())
                .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                .column(Column::exact(56.0))
                .columns(Column::initial(120.0).at_least(40.0).clip(true), nf)
                .min_scrolled_height(0.0)
                .max_scroll_height(f32::INFINITY);
            if let Some(r) = scroll_row {
                tb = tb.scroll_to_row(r, Some(egui::Align::Center));
            }
            let mut new_sort = t.sort;
            tb.header(22.0, |mut h| {
                h.col(|ui| {
                    ui.strong("#");
                });
                for (i, f) in v.data.fields.iter().enumerate() {
                    h.col(|ui| {
                        let arrow = match t.sort {
                            Some((c, true)) if c == i => tr!(" (cresc.)"),
                            Some((c, false)) if c == i => tr!(" (descr.)"),
                            _ => "",
                        };
                        if ui.add(egui::Button::new(RichText::new(format!("{}{}", f.name, arrow)).strong()).frame(false)).clicked() {
                            new_sort = match t.sort {
                                Some((c, true)) if c == i => Some((i, false)),
                                _ => Some((i, true)),
                            };
                        }
                    });
                }
            })
            .body(|body| {
                body.rows(20.0, t.rows.len(), |mut row| {
                    let ri = row.index();
                    let fi = t.rows[ri];
                    row.set_selected(v.sel.contains(&fi) || selection == Some((l.id, fi)));
                    row.col(|ui| {
                        ui.label(RichText::new((fi + 1).to_string()).weak());
                    });
                    for a in &v.data.features[fi].attrs {
                        row.col(|ui| {
                            ui.label(a.to_text());
                        });
                    }
                    let r = row.response();
                    let mods = r.ctx.input(|i| i.modifiers);
                    if r.clicked() {
                        select = Some((l.id, fi, ri, mods, false));
                    }
                    if r.double_clicked() {
                        select = Some((l.id, fi, ri, mods, true));
                    }
                });
            });
            t.sort = new_sort;
            ui.label(RichText::new(tr!("Click = selectare, Ctrl+click = adaugă / scoate, Shift+click = interval, dublu-click = zoom la entitate. Click pe antet = sortare.")).weak().small());
        });
        if let Some((lid, fi, ri, mods, zoom)) = select {
            use crate::gui::select::SelMode;
            let rows = self.dlg.table.rows.clone();
            let anchor = self.dlg.table.anchor;
            let (feats, mode): (Vec<usize>, SelMode) = if mods.shift
                && let Some(a) = anchor.filter(|&a| a < rows.len())
            {
                let (x, y) = (a.min(ri), a.max(ri));
                (rows[x..=y].to_vec(), if mods.command { SelMode::Add } else { SelMode::Replace })
            } else if mods.command {
                (vec![fi], SelMode::Toggle)
            } else {
                (vec![fi], SelMode::Replace)
            };
            if !mods.shift {
                self.dlg.table.anchor = Some(ri);
            }
            if mode == SelMode::Replace {
                for l in &mut self.layers {
                    if let LayerKind::Vector(v) = &mut l.kind {
                        v.clear_sel();
                    }
                }
            }
            if let Some(LayerKind::Vector(v)) = self.layer_mut(lid).map(|l| &mut l.kind) {
                v.apply_sel(feats, mode);
            }
            self.selection = Some((lid, fi));
            self.status = self.selection_status();
            if zoom {
                self.zoom_to_feature(lid, fi);
            }
        }
        if let Some((lid, what, feats)) = sel_op {
            match what {
                "rows" => {
                    if let Some(LayerKind::Vector(v)) = self.layer_mut(lid).map(|l| &mut l.kind) {
                        v.apply_sel(feats, crate::gui::select::SelMode::Replace);
                    }
                    self.status = self.selection_status();
                }
                "export" => self.dlg.export = Some(ExportDlg::new(self, lid)),
                "attr" => self.open_attr_select(Some(lid)),
                w => self.selection_action(lid, w),
            }
        }
        if !open {
            self.dlg.table.open = false;
        }
    }

    fn identify_window(&mut self, ctx: &egui::Context) {
        if !self.show_identify {
            return;
        }
        let mut open = true;
        let mut zoom: Option<(u64, usize)> = None;
        egui::Window::new(tr!("Identificare")).id(egui::Id::unique("win_dialogs_4"))
            .open(&mut open)
            .default_width(380.0)
            .default_pos(ctx.content_rect().right_top() + egui::vec2(-400.0, 80.0))
            .show(ctx, |ui| {
                if self.identify.is_empty() {
                    ui.label(tr!("Folosiți instrumentul Identificare (I) și faceți click pe hartă."));
                }
                egui::ScrollArea::vertical()
                    .max_height(480.0)
                    .show(ui, |ui| {
                        for (k, r) in self.identify.iter().enumerate() {
                            egui::CollapsingHeader::new(RichText::new(&r.layer).strong())
                                .id_salt(("idr", k))
                                .default_open(true)
                                .show(ui, |ui| {
                                    egui::Grid::new(("idg", k))
                                        .striped(true)
                                        .num_columns(2)
                                        .show(ui, |ui| {
                                            for (a, b) in &r.rows {
                                                ui.label(RichText::new(a).weak());
                                                ui.add(
                                                    egui::Label::new(b.as_str())
                                                        .selectable(true)
                                                        .wrap(),
                                                );
                                                ui.end_row();
                                            }
                                        });
                                    if let Some(fi) = r.feat
                                        && ui.button(tr!("Zoom la entitate")).clicked()
                                    {
                                        zoom = Some((r.layer_id, fi));
                                    }
                                });
                        }
                    });
            });
        if let Some((l, f)) = zoom {
            self.zoom_to_feature(l, f);
        }
        if !open {
            self.show_identify = false;
        }
    }

    fn csv_window(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.dlg.csv.take() else {
            return;
        };
        let mut open = true;
        let mut ok = false;
        let mut pick = false;
        let mut load_filter = self.settings.load_area_filter;
        let mut filter_changed = false;
        let title = tr!(
            "Import text: {}",
            d.path
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default()
        );
        egui::Window::new(title).id(egui::Id::unique("win_dialogs_5"))
            .open(&mut open)
            .default_width(620.0)
            .collapsible(false)
            .show(ctx, |ui| {
                let opt_combo = |ui: &mut egui::Ui,
                                 id: &str,
                                 v: &mut Option<usize>,
                                 headers: &[String],
                                 allow_none: bool| {
                    egui::ComboBox::from_id_salt(id)
                        .selected_text(
                            v.map(|i| headers[i].clone())
                                .unwrap_or_else(|| tr!("(niciuna)").into()),
                        )
                        .show_ui(ui, |ui| {
                            if allow_none {
                                ui.selectable_value(v, None, tr!("(niciuna)"));
                            }
                            for (i, h) in headers.iter().enumerate() {
                                ui.selectable_value(v, Some(i), h);
                            }
                        });
                };
                if d.wkt.is_some() {
                    ui.checkbox(&mut d.use_wkt, tr!("Geometria este în coloana WKT"));
                }
                egui::Grid::new("csvg")
                    .num_columns(2)
                    .spacing([10.0, 6.0])
                    .show(ui, |ui| {
                        if d.use_wkt {
                            ui.label(tr!("Coloana WKT:"));
                            opt_combo(ui, "cw", &mut d.wkt, &d.headers, false);
                            ui.end_row();
                        } else {
                            ui.label(tr!("X (Est / longitudine):"));
                            opt_combo(ui, "cx", &mut d.x, &d.headers, false);
                            ui.end_row();
                            ui.label(tr!("Y (Nord / latitudine):"));
                            opt_combo(ui, "cy", &mut d.y, &d.headers, false);
                            ui.end_row();
                            ui.label(tr!("Z (cotă):"));
                            opt_combo(ui, "cz", &mut d.z, &d.headers, true);
                            ui.end_row();
                        }
                        ui.label(tr!("Sistem de coordonate:"));
                        if crs_button(ui, &d.crs, tr!("alege… (implicit: detecție automată)")) {
                            pick = true;
                        }
                        ui.end_row();
                    });
                if !d.use_wkt {
                    if ui
                        .button(tr!("Inversează X și Y"))
                        .on_hover_text(tr!("Topografic, în Stereo 70 X = Nord și Y = Est"))
                        .clicked()
                    {
                        std::mem::swap(&mut d.x, &mut d.y);
                    }
                }
                ui.separator();
                ui.label(RichText::new(tr!("Previzualizare")).strong());
                egui::ScrollArea::horizontal().show(ui, |ui| {
                    egui::Grid::new("csvprev").striped(true).show(ui, |ui| {
                        for (i, h) in d.headers.iter().enumerate() {
                            let mark = if d.use_wkt && d.wkt == Some(i) {
                                " [WKT]"
                            } else if !d.use_wkt && d.x == Some(i) {
                                " [X]"
                            } else if !d.use_wkt && d.y == Some(i) {
                                " [Y]"
                            } else if !d.use_wkt && d.z == Some(i) {
                                " [Z]"
                            } else {
                                ""
                            };
                            ui.strong(format!("{h}{mark}"));
                        }
                        ui.end_row();
                        for r in &d.preview {
                            for c in r {
                                let s: String = c.chars().take(40).collect();
                                ui.label(s);
                            }
                            ui.end_row();
                        }
                    });
                });
                ui.separator();
                if ui
                    .checkbox(&mut load_filter, tr!("Doar entitățile din interiorul României (EPSG:3844)"))
                    .on_hover_text(tr!("Filtrul de încărcare (aceeași bifă ca în meniul Fișier)"))
                    .changed()
                {
                    filter_changed = true;
                }
                let valid = if d.use_wkt {
                    d.wkt.is_some()
                } else {
                    d.x.is_some() && d.y.is_some() && d.x != d.y
                };
                if ui
                    .add_enabled(valid, egui::Button::new(tr!("Încarcă")))
                    .clicked()
                {
                    ok = true;
                }
            });
        if pick {
            self.dlg.crs_picker = Some(CrsPicker::new(CrsPurpose::Csv, d.crs.clone()));
        }
        if filter_changed {
            self.set_load_filter(load_filter);
        }
        if ok {
            let mut o = ReadOptions {
                crs: d.crs.clone(),
                area: self.load_filter(),
                ..Default::default()
            };
            if !d.use_wkt {
                o.x_field = d.x.map(|i| d.headers[i].clone());
                o.y_field = d.y.map(|i| d.headers[i].clone());
                o.z_field = d.z.map(|i| d.headers[i].clone());
            }
            let p = d.path.clone();
            let (tx, rx) = channel();
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
                label: tr!("Import CSV").into(),
                rx,
            });
            return;
        }
        if open {
            self.dlg.csv = Some(d);
        }
    }

    fn batch_window(&mut self, ctx: &egui::Context) {
        let Some(mut b) = self.dlg.batch.take() else {
            return;
        };
        let mut open = true;
        let mut pick: Option<CrsPurpose> = None;
        let mut start = false;
        let outs: Vec<&'static str> = formats::FORMATS
            .iter()
            .filter(|f| f.write)
            .map(|f| f.exts[0])
            .collect();
        egui::Window::new(tr!("Conversie fișiere în lot")).id(egui::Id::unique("win_dialogs_6"))
            .open(&mut open)
            .default_width(640.0)
            .show(ctx, |ui| {
                egui::Grid::new("batchg")
                    .num_columns(2)
                    .spacing([10.0, 6.0])
                    .show(ui, |ui| {
                        ui.label(tr!("Dosar sursă:"));
                        ui.horizontal(|ui| {
                            if ui.button(tr!("Alege…")).clicked()
                                && let Some(p) = rfd::FileDialog::new().pick_folder()
                            {
                                b.in_dir = Some(p);
                            }
                            ui.label(
                                b.in_dir
                                    .as_ref()
                                    .map(|p| p.display().to_string())
                                    .unwrap_or_else(|| "-".into()),
                            );
                        });
                        ui.end_row();
                        ui.label(tr!("Dosar destinație:"));
                        ui.horizontal(|ui| {
                            if ui.button(tr!("Alege…")).clicked()
                                && let Some(p) = rfd::FileDialog::new().pick_folder()
                            {
                                b.out_dir = Some(p);
                            }
                            ui.label(
                                b.out_dir
                                    .as_ref()
                                    .map(|p| p.display().to_string())
                                    .unwrap_or_else(|| "-".into()),
                            );
                        });
                        ui.end_row();
                        ui.label(tr!("Format de ieșire:"));
                        b.to_idx = b.to_idx.min(outs.len() - 1);
                        egui::ComboBox::from_id_salt("bto")
                            .selected_text(outs[b.to_idx])
                            .show_ui(ui, |ui| {
                                for (i, e) in outs.iter().enumerate() {
                                    let name = formats::FORMATS
                                        .iter()
                                        .find(|f| f.exts[0] == *e)
                                        .map(|f| f.label())
                                        .unwrap_or("");
                                    ui.selectable_value(&mut b.to_idx, i, format!("{e} — {name}"));
                                }
                            });
                        ui.end_row();
                        ui.label(tr!("Doar extensiile:"));
                        ui.add(
                            egui::TextEdit::singleline(&mut b.only)
                                .hint_text(tr!("ex. shp,dxf (gol = toate)"))
                                .desired_width(200.0),
                        );
                        ui.end_row();
                        ui.label(tr!("Sistem sursă:"));
                        ui.horizontal(|ui| {
                            if crs_button(ui, &b.src_crs, tr!("din fișiere")) {
                                pick = Some(CrsPurpose::BatchSrc);
                            }
                            if b.src_crs.is_some() && ui.small_button(tr!("Șterge")).clicked() {
                                b.src_crs = None;
                            }
                        });
                        ui.end_row();
                        ui.label(tr!("Reproiectare în:"));
                        ui.horizontal(|ui| {
                            if crs_button(ui, &b.crs, tr!("fără reproiectare")) {
                                pick = Some(CrsPurpose::Batch);
                            }
                            if b.crs.is_some() && ui.small_button(tr!("Șterge")).clicked() {
                                b.crs = None;
                            }
                        });
                        ui.end_row();
                        ui.label(tr!("Opțiuni:"));
                        ui.horizontal(|ui| {
                            ui.checkbox(&mut b.recursive, tr!("include subdosarele"));
                            ui.checkbox(&mut b.cp1250, tr!("DBF în Windows-1250"));
                        });
                        ui.end_row();
                    });
                ui.separator();
                ui.horizontal(|ui| {
                    let can = !b.running && b.in_dir.is_some() && b.out_dir.is_some();
                    if ui
                        .add_enabled(can, egui::Button::new(tr!("Pornește conversia")))
                        .clicked()
                    {
                        start = true;
                    }
                    if b.running {
                        ui.spinner();
                        ui.label(tr!("Se lucrează…"));
                    }
                });
                egui::ScrollArea::vertical()
                    .max_height(240.0)
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        for l in &b.log {
                            let col = if l.starts_with("EROARE") {
                                ui.visuals().error_fg_color
                            } else {
                                ui.visuals().text_color()
                            };
                            ui.label(RichText::new(l).monospace().small().color(col));
                        }
                    });
            });
        if start && let (Some(i), Some(o)) = (b.in_dir.clone(), b.out_dir.clone()) {
            b.running = true;
            b.log.clear();
            let to = outs[b.to_idx].to_string();
            let exts: Vec<String> = if b.only.trim().is_empty() {
                convert::default_input_exts(&to)
            } else {
                b.only
                    .split(',')
                    .map(|s| s.trim().trim_start_matches('.').to_lowercase())
                    .collect()
            };
            let co = ConvertOptions {
                read: ReadOptions {
                    crs: b.src_crs.clone(),
                    ..Default::default()
                },
                write: WriteOptions {
                    overwrite: true,
                    encoding: if b.cp1250 {
                        Some("cp1250".into())
                    } else {
                        None
                    },
                    ..Default::default()
                },
                target: b.crs.clone(),
                ..Default::default()
            };
            let rec = b.recursive;
            let (tx, rx) = channel();
            std::thread::spawn(move || {
                let tx2 = tx.clone();
                let r = convert::batch(&i, &o, &to, &exts, rec, &co, &mut |l| {
                    let _ = tx2.send(JobResult::Log(l));
                });
                let msg = match r {
                    Ok((ok, err)) => {
                        tr!("Conversie în lot terminată: {ok} reușite, {err} erori")
                    }
                    Err(e) => tr!("EROARE: {e:#}"),
                };
                let _ = tx.send(JobResult::Done(msg));
            });
            self.jobs.push(Job {
                label: tr!("Conversie în lot").into(),
                rx,
            });
        }
        if let Some(p) = pick {
            let cur = if p == CrsPurpose::Batch {
                b.crs.clone()
            } else {
                b.src_crs.clone()
            };
            self.dlg.crs_picker = Some(CrsPicker::new(p, cur));
        }
        if open || b.running {
            self.dlg.batch = Some(b);
        }
    }

    fn help_windows(&mut self, ctx: &egui::Context) {
        egui::Window::new(tr!("Despre Cadastru")).id(egui::Id::unique("win_dialogs_7")).open(&mut self.dlg.about).collapsible(false).resizable(false).show(ctx, |ui| {
            ui.heading(format!("Cadastru {}", env!("CARGO_PKG_VERSION")));
            ui.label(tr!("Aplicație GIS desktop scrisă în Rust."));
            ui.horizontal(|ui| {
                ui.label(tr!("Autor:"));
                ui.label(RichText::new("Ginu Popescu").strong());
            });
            ui.horizontal(|ui| {
                ui.label("E-mail:");
                ui.hyperlink_to("scriemiaici@proton.me", "mailto:scriemiaici@proton.me");
            });
            ui.label(tr!("Vizualizare hărți vectoriale și raster, conversie între formate, reproiecție."));
            ui.separator();
            ui.label(tr!("Biblioteci: egui/eframe, proj4rs (transformări), crs-definitions (EPSG), tiff, image, rusqlite, earcutr."));
            ui.label(tr!("TransDatRO: algoritm și griduri © ANCPI (Agenția Națională de Cadastru și Publicitate Imobiliară)."));
            ui.label(RichText::new(tr!("Transformarea Stereo 70 <-> ETRS89/WGS84 folosește implicit TransDatRO (precizie centimetrică); opțional Helmert 7 parametri (EPSG:15994).")).weak());
            ui.label(RichText::new(tr!("Hărțile de fundal aparțin furnizorilor lor (OpenStreetMap, Esri, Google, Microsoft, CARTO, BKG, EOX) și se folosesc conform condițiilor fiecăruia; atribuirea apare în colțul hărții.")).weak());
        });
        egui::Window::new(tr!("Utilizare și scurtături")).id(egui::Id::unique("win_dialogs_8")).open(&mut self.dlg.help).default_width(480.0).show(ctx, |ui| {
            egui::Grid::new("help").striped(true).num_columns(2).show(ui, |ui| {
                for (k, d) in [
                    ("F1", tr!("Manualul de utilizare (PDF)")),
                    ("Ctrl+O", tr!("Deschide fișiere (sau trageți-le peste fereastră)")),
                    ("Ctrl+S", tr!("Salvează spațiul de lucru (.cgw)")),
                    ("Ctrl+P", tr!("Tipărire (și Fișier > Salvează ca PDF)")),
                    (tr!("Rotița mouse"), tr!("Zoom la cursor")),
                    (tr!("Trage / buton mijloc"), tr!("Deplasare hartă")),
                    (tr!("Dublu-click"), tr!("Zoom ×2 (instrumentul Deplasare)")),
                    ("Home", tr!("Extindere completă")),
                    ("+ / -", tr!("Mărire / micșorare")),
                    ("P / Z / X / I / M", tr!("Deplasare / Zoom + / Zoom − / Identificare / Măsurare")),
                    (tr!("Săgeți"), tr!("Deplasare cu un sfert de ecran")),
                    (tr!("Backspace / Alt+Dreapta"), tr!("Vederea anterioară / următoare")),
                    (tr!("Control pe hartă"), tr!("Săgeți = deplasare (ține apăsat pentru continuu), centru = extindere completă, +/− = zoom")),
                    ("T", tr!("Tabel de atribute")),
                    ("S", tr!("Selectare: click, Ctrl+click = adaugă / scoate, dreptunghi = zonă")),
                    ("Ctrl+Shift+A", tr!("Deselectează tot")),
                    ("Ctrl+F", tr!("Căutare: loc, adresă, coordonate sau după câmpuri")),
                    ("Ctrl+Shift+F", tr!("Selectare după atribute (expresie: status = 1 AND is_active = 1)")),
                    ("F5", tr!("Redraw: curăță grafica temporară și redesenează harta")),
                    ("Basemap", tr!("Harta de fundal (OSM, Esri, Google, Bing, CARTO…), opacitate, șablon XYZ propriu")),
                    ("Esc", tr!("Anulează măsurarea și selecția")),
                    (tr!("Click dreapta"), tr!("Meniu: copiere coordonate, centrare, identificare")),
                ] {
                    ui.monospace(k);
                    ui.label(d);
                    ui.end_row();
                }
            });
            ui.separator();
            ui.label(tr!("Straturile fără fișier .prj primesc un sistem presupus (WGS84 sau Stereo 70 după extindere), afișat cu portocaliu. Corectați-l din meniul contextual al stratului > „Setează sistemul de coordonate”."));
        });
    }
}
