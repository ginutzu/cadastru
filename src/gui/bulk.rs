//! Deschiderea în masă: toate fișierele de un anumit tip dintr-un dosar (și din subdosare).

use super::{App, Job, JobResult};
use cadastru::formats::{self, FormatInfo, ReadOptions};
use cadastru::model::Dataset;
use cadastru::i18n::tr;
use eframe::egui::{self, RichText};
use std::path::{Path, PathBuf};
use super::layers::{Prebuilt, prebuild};
use cadastru::crs::Crs;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, channel, sync_channel};

/// Un set de date citit, cu sistemul presupus (dacă lipsea) și geometria de afișare.
pub type Prepared = (Dataset, Option<bool>, Option<Prebuilt>);

/// Citește un fișier și pregătește afișarea, în firul de lucru.
fn prepare(path: &Path, opts: &ReadOptions, map_crs: &Option<Crs>) -> anyhow::Result<Vec<Prepared>> {
    let ds = formats::open_ex(path, opts)?.0;
    Ok(ds
        .into_iter()
        .map(|mut d| {
            let mut assumed = false;
            if d.crs().is_none() {
                let b = d.bounds();
                if let Some(g) = cadastru::crs::guess_crs(b.minx, b.miny, b.maxx, b.maxy).or_else(|| map_crs.clone()) {
                    assumed = true;
                    match &mut d {
                        Dataset::Vector(v) => v.crs = Some(g),
                        Dataset::Raster(r) => r.crs = Some(g),
                    }
                }
            }
            // fără sistem al hărții, primul strat îl stabilește: se folosește sistemul lui
            let mc = map_crs.clone().or_else(|| d.crs().cloned());
            let pre = match &d {
                Dataset::Vector(v) => Some(prebuild(v, mc.as_ref())),
                Dataset::Raster(_) => None,
            };
            (d, Some(assumed), pre)
        })
        .collect())
}

fn n_files(n: usize) -> String {
    cadastru::i18n::count(n, "fișier", "fișiere", "file", "files")
}

fn n_layers(n: usize) -> String {
    cadastru::i18n::count(n, "strat", "straturi", "layer", "layers")
}

/// Formatele care nu se deschid în masă: CSV (cere alegerea coloanelor), CGXML/ZIP (au comanda
/// lor, care unește dosarul într-un singur strat) și tabelele interne ale unui .gdb.
fn bulk_format(f: &FormatInfo) -> bool {
    f.read && !f.exts.iter().any(|e| matches!(*e, "csv" | "cgxml" | "zip"))
}

/// Formatul unui fișier găsit (pentru dosare .gdb: File Geodatabase).
fn format_of(p: &Path) -> Option<&'static FormatInfo> {
    let e = if p.is_dir() { "gdb".to_string() } else { formats::ext_of(p) };
    if e == "gdbtable" || e == "gdbtablx" {
        return None;
    }
    formats::FORMATS
        .iter()
        .filter(|f| bulk_format(f))
        .find(|f| f.exts.contains(&e.as_str()))
}

/// Ordine „naturală”: s2 înaintea lui s10, fără deosebire de majuscule.
pub fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let (mut x, mut y) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (x.peek().copied(), y.peek().copied()) {
            (None, None) => return a.cmp(b),
            (None, _) => return std::cmp::Ordering::Less,
            (_, None) => return std::cmp::Ordering::Greater,
            (Some(c), Some(d)) if c.is_ascii_digit() && d.is_ascii_digit() => {
                let mut n1 = String::new();
                while let Some(c) = x.peek().copied().filter(|c| c.is_ascii_digit()) {
                    n1.push(c);
                    x.next();
                }
                let mut n2 = String::new();
                while let Some(d) = y.peek().copied().filter(|d| d.is_ascii_digit()) {
                    n2.push(d);
                    y.next();
                }
                let (t1, t2) = (n1.trim_start_matches('0'), n2.trim_start_matches('0'));
                let o = t1.len().cmp(&t2.len()).then_with(|| t1.cmp(t2));
                if o != std::cmp::Ordering::Equal {
                    return o;
                }
            }
            (Some(c), Some(d)) => {
                let o = c.to_lowercase().cmp(d.to_lowercase());
                if o != std::cmp::Ordering::Equal {
                    return o;
                }
                x.next();
                y.next();
            }
        }
    }
}

/// Mărimea datelor: fișierul, plus .dbf pentru shapefile, plus conținutul unui dosar .gdb.
fn data_size(p: &Path) -> u64 {
    let len = |p: &Path| std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
    if p.is_dir() {
        std::fs::read_dir(p)
            .map(|rd| rd.filter_map(|e| e.ok()).map(|e| len(&e.path())).sum())
            .unwrap_or(0)
    } else if formats::ext_of(p) == "shp" {
        len(p) + len(&p.with_extension("dbf"))
    } else {
        len(p)
    }
}

fn fmt_size(b: u64) -> String {
    let mb = b as f64 / 1_048_576.0;
    if mb >= 1024.0 { format!("{:.1} GB", mb / 1024.0) } else { format!("{:.1} MB", mb) }
}

/// Parcurge dosarul recursiv; dosarele .gdb sunt luate ca un singur element.
fn scan(dir: &Path, cancel: &AtomicBool, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<PathBuf> = rd.filter_map(|e| e.ok().map(|e| e.path())).collect();
    entries.sort_by(|a, b| natural_cmp(&a.to_string_lossy(), &b.to_string_lossy()));
    for p in entries {
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        if p.is_dir() {
            if formats::gdb::gdb_root(&p).as_deref() == Some(p.as_path()) {
                out.push(p);
            } else {
                scan(&p, cancel, out);
            }
        } else if format_of(&p).is_some() {
            out.push(p);
        }
    }
}

/// Fereastra de alegere a tipului.
pub struct BulkDlg {
    pub dir: PathBuf,
    scan_rx: Option<Receiver<Vec<(PathBuf, &'static FormatInfo, u64)>>>,
    /// toate fișierele găsite (recursiv), relativ la `dir`, cu formatul și mărimea lor
    found: Vec<(PathBuf, &'static FormatInfo, u64)>,
    scanned: bool,
    /// numele formatului ales
    pub fmt: Option<&'static str>,
    pub recursive: bool,
    pub rel_names: bool,
    cancel: Arc<AtomicBool>,
}

impl BulkDlg {
    fn files(&self) -> Vec<&PathBuf> {
        self.found
            .iter()
            .filter(|(p, _, _)| self.recursive || p.components().count() == 1)
            .filter(|(_, f, _)| self.fmt == Some(f.name))
            .map(|(p, _, _)| p)
            .collect()
    }
    /// Mărimea totală a fișierelor alese (octeți).
    fn total_size(&self) -> u64 {
        self.found
            .iter()
            .filter(|(p, _, _)| self.recursive || p.components().count() == 1)
            .filter(|(_, f, _)| self.fmt == Some(f.name))
            .map(|(_, _, s)| *s)
            .sum()
    }
    /// (format, număr în dosar, număr în subdosare)
    fn groups(&self) -> Vec<(&'static FormatInfo, usize, usize)> {
        let mut g: Vec<(&'static FormatInfo, usize, usize)> = vec![];
        for (p, f, _) in &self.found {
            let f = *f;
            let top = p.components().count() == 1;
            match g.iter_mut().find(|x| x.0.name == f.name) {
                Some(x) => {
                    if top {
                        x.1 += 1
                    } else {
                        x.2 += 1
                    }
                }
                None => g.push((f, top as usize, (!top) as usize)),
            }
        }
        g
    }
}

/// Starea unei deschideri în masă în curs.
pub struct BulkRun {
    pub total: usize,
    pub done: usize,
    pub layers: usize,
    pub errors: Vec<String>,
    pub cancel: Arc<AtomicBool>,
    pub first_id: u64,
}

/// Numele stratului: calea relativă fără extensie („Comuna/teren”), iar pentru fișierele
/// cu mai multe straturi (GeoPackage, .gdb) „Comuna/fișier · strat”.
pub fn layer_name(rel: &Path, ds_name: &str, rel_names: bool) -> String {
    let stem = rel.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let prefix = match rel.parent().filter(|p| rel_names && !p.as_os_str().is_empty()) {
        Some(par) => {
            let par = par.to_string_lossy().replace('\\', "/");
            format!("{par}/{stem}")
        }
        None => stem.clone(),
    };
    if ds_name.is_empty() || ds_name.eq_ignore_ascii_case(&stem) {
        prefix
    } else if prefix == stem && !rel_names {
        ds_name.to_string()
    } else {
        format!("{prefix} · {ds_name}")
    }
}

impl App {
    pub fn open_bulk_dialog(&mut self) {
        let Some(dir) = rfd::FileDialog::new()
            .set_title(tr!("Dosarul din care se deschid fișierele"))
            .pick_folder()
        else {
            return;
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let (tx, rx) = channel();
        let d = dir.clone();
        let c = cancel.clone();
        std::thread::spawn(move || {
            let mut out = vec![];
            scan(&d, &c, &mut out);
            let rel = out
                .into_iter()
                .filter_map(|p| {
                    let f = format_of(&p)?;
                    let size = data_size(&p);
                    p.strip_prefix(&d).ok().map(|r| (r.to_path_buf(), f, size))
                })
                .collect();
            let _ = tx.send(rel);
        });
        self.dlg.bulk = Some(BulkDlg {
            dir,
            scan_rx: Some(rx),
            found: vec![],
            scanned: false,
            fmt: None,
            recursive: true,
            rel_names: true,
            cancel,
        });
    }

    pub fn bulk_window(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.dlg.bulk.take() else { return };
        let mut load_filter = self.settings.load_area_filter;
        let filter_was = load_filter;
        let filter_desc = self.area_filter().describe();
        if let Some(rx) = &d.scan_rx {
            match rx.try_recv() {
                Ok(v) => {
                    d.found = v;
                    d.scanned = true;
                    d.scan_rx = None;
                    // implicit: tipul cel mai numeros
                    d.fmt = d.groups().into_iter().max_by_key(|g| g.1 + g.2).map(|g| g.0.name);
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    ctx.request_repaint_after(std::time::Duration::from_millis(100))
                }
                Err(_) => {
                    d.scan_rx = None;
                    d.scanned = true;
                }
            }
        }
        let mut open = true;
        let mut go = false;
        egui::Window::new(tr!("Deschide toate fișierele de un tip"))
            .id(egui::Id::unique("win_bulk"))
            .open(&mut open)
            .collapsible(false)
            .default_width(520.0)
            .show(ctx, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(tr!("Dosar:"));
                    ui.add(egui::Label::new(RichText::new(d.dir.display().to_string()).monospace()).wrap());
                });
                if !d.scanned {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(tr!("Se caută fișierele…"));
                    });
                    return;
                }
                ui.checkbox(&mut d.recursive, tr!("Inclusiv subdosarele (recursiv)"));
                ui.separator();
                let groups = d.groups();
                if groups.is_empty() {
                    ui.label(RichText::new(tr!("Nu am găsit fișiere GIS în acest dosar.")).weak());
                    return;
                }
                ui.strong(tr!("Tipul de fișiere:"));
                egui::Grid::new("bulk_types").num_columns(3).spacing([12.0, 4.0]).striped(true).show(ui, |ui| {
                    for (f, top, sub) in &groups {
                        let n = if d.recursive { top + sub } else { *top };
                        let txt = format!("{}  (.{})", f.label(), f.exts.join(", ."));
                        ui.add_enabled_ui(n > 0, |ui| {
                            ui.radio_value(&mut d.fmt, Some(f.name), txt);
                        });
                        ui.label(RichText::new(match f.kind {
                            formats::Kind::Vector => tr!("vector"),
                            formats::Kind::Raster => tr!("raster"),
                        }).weak());
                        ui.label(if d.recursive && *sub > 0 {
                            format!("{} ({top} {}, {sub} {})", n_files(n), tr!("în dosar"), tr!("în subdosare"))
                        } else {
                            n_files(n)
                        });
                        ui.end_row();
                    }
                });
                ui.separator();
                ui.checkbox(&mut d.rel_names, tr!("Numele straturilor cu calea din subdosare (ex. Comuna/teren)"));
                ui.checkbox(&mut load_filter, tr!("Doar entitățile din interiorul României (EPSG:3844)"))
                    .on_hover_text(tr!("Filtrul de încărcare (aceeași bifă ca în meniul Fișier); zona: {}", filter_desc));
                let files = d.files();
                if !files.is_empty() {
                    ui.label(RichText::new(tr!("Fișierele care se deschid:")).weak());
                    egui::ScrollArea::vertical().id_salt("bulk_files").max_height(150.0).auto_shrink([false, true]).show(ui, |ui| {
                        for p in files.iter().take(500) {
                            ui.label(RichText::new(p.display().to_string()).monospace().small());
                        }
                        if files.len() > 500 {
                            ui.label(RichText::new(tr!("… încă {}", files.len() - 500)).weak());
                        }
                    });
                }
                let size = d.total_size();
                if !files.is_empty() {
                    ui.label(tr!("Total: {}, {}", n_files(files.len()), fmt_size(size)));
                }
                // în memorie, datele ocupă de câteva ori mărimea fișierelor (geometria de afișare)
                if files.len() > 300 || size > 500 * 1_048_576 {
                    ui.label(RichText::new(tr!("Atenție: {} – încărcarea poate dura și poate ocupa multă memorie.", fmt_size(size))).color(egui::Color32::from_rgb(220, 140, 0)));
                }
                ui.add_space(4.0);
                if ui
                    .add_enabled(!files.is_empty(), egui::Button::new(RichText::new(tr!("Deschide {}", n_files(files.len()))).strong()))
                    .clicked()
                {
                    go = true;
                }
            });
        if load_filter != filter_was {
            self.set_load_filter(load_filter);
        }
        if go {
            let files: Vec<PathBuf> = d.files().into_iter().cloned().collect();
            self.run_bulk(d.dir.clone(), files, d.rel_names);
            return;
        }
        if open {
            self.dlg.bulk = Some(d);
        } else {
            d.cancel.store(true, Ordering::Relaxed);
        }
    }

    /// Încarcă fișierele unul după altul, într-un fir separat; straturile apar pe rând.
    fn run_bulk(&mut self, dir: PathBuf, files: Vec<PathBuf>, rel_names: bool) {
        let cancel = Arc::new(AtomicBool::new(false));
        let total = files.len();
        let mut opts = ReadOptions::default();
        opts.cgxml = self.settings.cgxml.clone();
        opts.area = self.load_filter();
        // coadă mică: firele nu citesc mult înaintea interfeței (memorie, oprire rapidă)
        let (tx, rx) = sync_channel(8);
        let c = cancel.clone();
        let seqs: Vec<u64> = (0..total).map(|_| self.next_seq()).collect();
        let map_crs = self.map_crs.clone();
        let work: Arc<Vec<(PathBuf, u64)>> = Arc::new(files.into_iter().zip(seqs).collect());
        let next = Arc::new(AtomicUsize::new(0));
        let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(2).clamp(1, 4);
        std::thread::spawn(move || {
            let mut handles = vec![];
            for _ in 0..threads {
                let (tx, c, work, next, dir, opts, map_crs) =
                    (tx.clone(), c.clone(), work.clone(), next.clone(), dir.clone(), opts.clone(), map_crs.clone());
                handles.push(std::thread::spawn(move || {
                    loop {
                        if c.load(Ordering::Relaxed) {
                            return;
                        }
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        let Some((rel, seq)) = work.get(i).cloned() else { return };
                        let path = dir.join(&rel);
                        let result = prepare(&path, &opts, &map_crs);
                        if tx
                            .send(JobResult::BulkItem { path, rel, result, seq, rel_names })
                            .is_err()
                        {
                            return;
                        }
                    }
                }));
            }
            for h in handles {
                let _ = h.join();
            }
            let _ = tx.send(JobResult::BulkDone);
        });
        self.bulk = Some(BulkRun {
            total,
            done: 0,
            layers: 0,
            errors: vec![],
            cancel,
            first_id: self.next_id,
        });
        self.jobs.push(Job {
            label: tr!("Se deschid: {}", n_files(total)),
            rx,
        });
    }

    /// Un fișier din deschiderea în masă a fost citit.
    pub fn bulk_item(
        &mut self,
        path: PathBuf,
        rel: PathBuf,
        result: anyhow::Result<Vec<Prepared>>,
        seq: u64,
        rel_names: bool,
    ) {
        // după „Oprește”, fișierele deja citite nu mai sunt adăugate
        if self.bulk.as_ref().is_some_and(|b| b.cancel.load(Ordering::Relaxed)) {
            return;
        }
        let ok = match result {
            Ok(ds) => {
                let names: Vec<String> = ds.iter().map(|d| layer_name(&rel, d.0.name(), rel_names)).collect();
                let ids = self.add_prepared(&path, ds, seq);
                for (id, n) in ids.iter().zip(names) {
                    if let Some(l) = self.layer_mut(*id) {
                        l.name = n;
                    }
                }
                Some(ids.len())
            }
            Err(e) => {
                if let Some(b) = self.bulk.as_mut() {
                    b.errors.push(format!("{}: {e:#}", rel.display()));
                }
                None
            }
        };
        if let Some(b) = self.bulk.as_mut() {
            b.done += 1;
            if let Some(n) = ok {
                b.layers += n;
            }
            self.status = tr!("Citite {} din {}…", b.done, n_files(b.total));
        }
    }

    /// Sfârșitul deschiderii în masă: rezumat, erori, zoom pe straturile noi.
    pub fn bulk_done(&mut self) {
        let Some(b) = self.bulk.take() else { return };
        let stopped = b.cancel.load(Ordering::Relaxed);
        self.status = if stopped {
            tr!("Oprit după {} din {}: {} deschise", b.done, n_files(b.total), n_layers(b.layers))
        } else {
            tr!("Deschise: {} din {}", n_layers(b.layers), n_files(b.done))
        };
        if !b.errors.is_empty() {
            let mut msg = if b.errors.len() == 1 {
                tr!("Un fișier nu a putut fi deschis:").to_string()
            } else {
                tr!("{} nu au putut fi deschise:", n_files(b.errors.len()))
            };
            for e in b.errors.iter().take(30) {
                msg.push('\n');
                msg.push_str(e);
            }
            if b.errors.len() > 30 {
                msg.push_str(&format!("\n… (+{})", b.errors.len() - 30));
            }
            self.errors.push(msg);
        }
        // zoom pe straturile încărcate acum
        let mut bb = cadastru::model::Bounds::empty();
        for l in self.layers.iter().filter(|l| l.id >= b.first_id) {
            let lb = l.map_bounds();
            if lb.is_valid() {
                bb.merge(&lb);
            }
        }
        if bb.is_valid() {
            self.zoom_to(bb);
        }
    }

    /// Butonul de oprire din bara de stare.
    pub fn bulk_stop_button(&mut self, ui: &mut egui::Ui) {
        if let Some(b) = &self.bulk
            && ui
                .small_button(tr!("Oprește ({}/{})", b.done, b.total))
                .on_hover_text(tr!("Oprește deschiderea fișierelor; straturile deja încărcate rămân"))
                .clicked()
        {
            b.cancel.store(true, Ordering::Relaxed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn natural_order() {
        let mut v = vec!["s10", "s2", "S1", "a", "s02b"];
        v.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(v, ["a", "S1", "s2", "s02b", "s10"]);
    }

    #[test]
    fn names() {
        let p = Path::new("Comuna A/teren.shp");
        assert_eq!(layer_name(p, "teren", true), "Comuna A/teren");
        assert_eq!(layer_name(p, "teren", false), "teren");
        assert_eq!(layer_name(Path::new("teren.shp"), "teren", true), "teren");
        assert_eq!(layer_name(Path::new("X/date.gpkg"), "parcele", true), "X/date · parcele");
        assert_eq!(layer_name(Path::new("date.gpkg"), "parcele", false), "parcele");
    }

    #[test]
    fn scan_finds_types() {
        let d = std::env::temp_dir().join(format!("cadastru_bulk_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("sub/adanc")).unwrap();
        for f in ["a.shp", "a.dbf", "b.dxf", "sub/c.shp", "sub/adanc/d.shp", "sub/x.csv", "sub/r.tif", "z.cgxml"] {
            std::fs::write(d.join(f), b"").unwrap();
        }
        std::fs::create_dir_all(d.join("sub/date.gdb")).unwrap();
        std::fs::write(d.join("sub/date.gdb/a00000001.gdbtable"), b"").unwrap();
        let mut out = vec![];
        scan(&d, &AtomicBool::new(false), &mut out);
        let rel: Vec<String> = out
            .iter()
            .map(|p| p.strip_prefix(&d).unwrap().to_string_lossy().replace('\\', "/"))
            .collect();
        assert_eq!(rel, ["a.shp", "b.dxf", "sub/adanc/d.shp", "sub/c.shp", "sub/date.gdb", "sub/r.tif"]);
        let _ = std::fs::remove_dir_all(&d);
    }
}
