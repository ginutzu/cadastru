//! Spațiul de lucru (.cgw): lista straturilor, stilurile, sistemul hărții și vederea curentă.

use super::layers::*;
use super::{App, Job, JobResult, View};
use cadastru::crs::{Crs, Transformer};
use cadastru::formats::{self, ReadOptions};
use cadastru::rasterops::RasterStyle;
use eframe::egui;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::mpsc::channel;
use cadastru::i18n::tr;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LayerSettings {
    pub path: PathBuf,
    pub source_layer: Option<String>,
    pub name: String,
    pub visible: bool,
    pub opacity: f32,
    pub crs: Option<String>,
    pub vector_style: Option<VectorStyle>,
    pub raster_style: Option<RasterStyle>,
    pub order: usize,
    #[serde(default)]
    pub csv_fields: [Option<String>; 3],
}

#[derive(Serialize, Deserialize)]
pub struct Workspace {
    pub app: String,
    pub version: u32,
    pub map_crs: Option<String>,
    pub view: Option<[f64; 3]>,
    pub layers: Vec<LayerSettings>,
    /// harta de fundal (id furnizor; lipsă = fără)
    #[serde(default)]
    pub basemap: Option<String>,
    #[serde(default)]
    pub basemap_opacity: Option<f32>,
}

impl App {
    pub fn save_workspace_dialog(&mut self, save_as: bool) {
        let path = match (&self.workspace_path, save_as) {
            (Some(p), false) => Some(p.clone()),
            _ => rfd::FileDialog::new()
                .set_title(tr!("Salvează spațiul de lucru"))
                .add_filter(tr!("Spațiu de lucru Cadastru"), &["cgw"])
                .set_file_name("proiect.cgw")
                .save_file(),
        };
        let Some(mut path) = path else { return };
        if formats::ext_of(&path) != "cgw" {
            path.set_extension("cgw");
        }
        match self.save_workspace(&path) {
            Ok(()) => {
                self.status = tr!("Spațiu de lucru salvat: {}", path.display());
                self.workspace_path = Some(path);
            }
            Err(e) => self
                .errors
                .push(tr!("Nu pot salva spațiul de lucru: {e:#}")),
        }
    }

    pub fn save_workspace(&self, path: &Path) -> anyhow::Result<()> {
        let base = path.parent().map(|p| p.to_path_buf()).unwrap_or_default();
        let mut layers = vec![];
        for (order, l) in self.layers.iter().enumerate() {
            let Some(src) = l.source() else { continue };
            let rel = src
                .strip_prefix(&base)
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|_| src.to_path_buf());
            layers.push(LayerSettings {
                path: rel,
                source_layer: l.source_layer.clone(),
                name: l.name.clone(),
                visible: l.visible,
                opacity: l.opacity,
                crs: l.crs().map(|c| c.to_user_string()),
                vector_style: match &l.kind {
                    LayerKind::Vector(v) => Some(v.style.clone()),
                    _ => None,
                },
                raster_style: match &l.kind {
                    LayerKind::Raster(r) => Some(r.style.clone()),
                    _ => None,
                },
                order,
                csv_fields: l.csv_fields.clone(),
            });
        }
        let ws = Workspace {
            app: "Cadastru".into(),
            version: 1,
            map_crs: self.map_crs.as_ref().map(|c| c.to_user_string()),
            view: Some([self.view.cx, self.view.cy, self.view.scale]),
            layers,
            basemap: Some(self.basemap_ws_id().unwrap_or_else(|| "none".into())),
            basemap_opacity: Some(self.basemap.opacity),
        };
        std::fs::write(path, serde_json::to_string_pretty(&ws)?)?;
        Ok(())
    }

    pub fn load_workspace(&mut self, path: &Path, _ctx: &egui::Context) {
        let r = (|| -> anyhow::Result<Workspace> {
            Ok(serde_json::from_str(&std::fs::read_to_string(path)?)?)
        })();
        let ws = match r {
            Ok(w) => w,
            Err(e) => {
                self.errors
                    .push(tr!("Spațiu de lucru invalid {}: {e:#}", path.display()));
                return;
            }
        };
        let base = path.parent().map(|p| p.to_path_buf()).unwrap_or_default();
        self.layers.clear();
        self.selection = None;
        self.identify.clear();
        self.dlg.pending_settings.clear();
        self.dlg.ws_order.clear();
        self.map_crs = ws.map_crs.as_deref().and_then(|s| Crs::parse(s).ok());
        self.to_wgs = self
            .map_crs
            .as_ref()
            .and_then(|m| Transformer::new(m, &Crs::wgs84()).ok());
        // spațiile de lucru vechi (fără câmpul „basemap”) păstrează harta de fundal curentă
        if let Some(b) = ws.basemap.as_deref() {
            self.basemap_from_ws_id(if b == "none" { None } else { Some(b) });
        }
        if let Some(o) = ws.basemap_opacity {
            self.basemap.opacity = o.clamp(0.0, 1.0);
        }
        if let Some([cx, cy, s]) = ws.view {
            self.view = View { cx, cy, scale: s };
            self.dlg.ws_view = Some(self.view);
            self.view_initialized = true;
        }
        for mut s in ws.layers {
            if s.path.is_relative() {
                s.path = base.join(&s.path);
            }
            let key = s.path.clone();
            let first = !self.dlg.pending_settings.contains_key(&key);
            let csv = s.csv_fields.clone();
            self.dlg
                .pending_settings
                .entry(key.clone())
                .or_default()
                .push(s);
            if first {
                let opts = ReadOptions {
                    x_field: csv[0].clone(),
                    y_field: csv[1].clone(),
                    z_field: csv[2].clone(),
                    cgxml: self.settings.cgxml.clone(),
                    ..Default::default()
                };
                let (tx, rx) = channel();
                let seq = self.next_seq();
                let p = key.clone();
                let o = opts.clone();
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
                        key.file_name()
                            .map(|s| s.to_string_lossy().to_string())
                            .unwrap_or_default()
                    ),
                    rx,
                });
            }
        }
        self.workspace_path = Some(path.to_path_buf());
        self.status = tr!("Spațiu de lucru: {}", path.display());
    }

    pub fn apply_layer_settings(&mut self, ids: &[u64], settings: Vec<LayerSettings>) {
        let mc = self.map_crs.clone();
        let mut keep: Vec<u64> = vec![];
        for id in ids {
            let Some(l) = self.layer_mut(*id) else {
                continue;
            };
            let s = settings
                .iter()
                .find(|s| s.source_layer.is_some() && s.source_layer == l.source_layer)
                .or_else(|| {
                    if ids.len() == 1 {
                        settings.first()
                    } else {
                        None
                    }
                });
            let Some(s) = s else { continue };
            keep.push(*id);
            l.name = s.name.clone();
            l.visible = s.visible;
            l.opacity = s.opacity;
            if let Some(c) = s.crs.as_deref().and_then(|c| Crs::parse(c).ok())
                && l.crs() != Some(&c)
            {
                l.set_crs(Some(c));
            }
            l.crs_assumed = false;
            match &mut l.kind {
                LayerKind::Vector(v) => {
                    if let Some(st) = &s.vector_style {
                        v.style = st.clone();
                    }
                }
                LayerKind::Raster(r) => {
                    if let Some(st) = &s.raster_style
                        && st.bands.iter().all(|&b| b < r.data.bands.len())
                    {
                        r.style = st.clone();
                    }
                }
            }
            l.csv_fields = s.csv_fields.clone();
            l.rebuild(mc.as_ref());
            self.dlg.ws_order.insert(*id, s.order);
        }
        // straturi din fișier care nu erau în spațiul de lucru
        for id in ids {
            if !keep.contains(id) {
                self.remove_layer(*id);
            }
        }
        let order = &self.dlg.ws_order;
        self.layers
            .sort_by_key(|l| order.get(&l.id).copied().unwrap_or(usize::MAX));
        if let Some(v) = self.dlg.ws_view {
            self.view = v;
            self.view_initialized = true;
        }
        if self.dlg.pending_settings.is_empty() {
            self.dlg.ws_view = None;
            let n = self.layers.len();
            self.status = match n {
                1 => tr!("Spațiu de lucru încărcat: 1 strat").to_string(),
                n if n % 100 >= 20 || n % 100 == 0 && n > 0 => tr!("Spațiu de lucru încărcat: {n} de straturi"),
                n => tr!("Spațiu de lucru încărcat: {n} straturi"),
            };
        }
    }
}
