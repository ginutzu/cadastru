//! Exportul straturilor (de regulă DXF) în câte un shapefile pe tip de geometrie:
//! toate punctele într-un shapefile de puncte, liniile și poliliniile într-unul de linii,
//! poligoanele într-unul de poligoane.

use super::App;
use super::dialogs::{CrsPicker, CrsPurpose};
use super::layers::LayerKind;
use super::{Job, JobResult};
use cadastru::convert::{self, SplitOptions};
use cadastru::crs::Crs;
use cadastru::formats::{self, WriteOptions};
use cadastru::i18n::tr;
use eframe::egui::{self, RichText};
use std::sync::mpsc::channel;

pub struct SplitDlg {
    /// straturile vectoriale și dacă sunt incluse
    pub layers: Vec<(u64, bool)>,
    pub points: bool,
    pub lines: bool,
    pub polygons: bool,
    pub closed_as_lines: bool,
    pub source_field: bool,
    pub only_sel: bool,
    /// 0 = sistemul hărții, 1 = fără reproiectare, 2 = altul
    pub crs_mode: u8,
    pub custom_crs: Option<Crs>,
    pub cp1250: bool,
}

/// Stratul provine dintr-un fișier DXF.
fn is_dxf(l: &super::layers::LayerEntry) -> bool {
    l.source().is_some_and(|p| formats::ext_of(p) == "dxf")
}

impl App {
    /// Deschide fereastra; `focus` = stratul din care s-a pornit (din meniul contextual).
    pub fn open_split_dialog(&mut self, focus: Option<u64>) {
        let any_dxf = self.layers.iter().any(|l| l.is_vector() && is_dxf(l));
        let layers = self
            .layers
            .iter()
            .filter(|l| l.is_vector())
            .map(|l| {
                let on = match focus {
                    Some(f) if !is_dxf(l) || !any_dxf => l.id == f,
                    _ if any_dxf => is_dxf(l),
                    _ => self.selected_layer == Some(l.id),
                };
                (l.id, on)
            })
            .collect();
        self.dlg.split = Some(SplitDlg {
            layers,
            points: true,
            lines: true,
            polygons: true,
            closed_as_lines: false,
            source_field: true,
            only_sel: false,
            crs_mode: if self.map_crs.is_some() { 0 } else { 1 },
            custom_crs: None,
            cp1250: false,
        });
    }

    pub fn split_window(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.dlg.split.take() else { return };
        // straturile închise între timp dispar din listă
        d.layers.retain(|(id, _)| self.layer(*id).is_some());
        let mut open = true;
        let mut go = false;
        let mut pick_crs = false;
        let nsel: usize = d
            .layers
            .iter()
            .filter(|(_, on)| *on)
            .filter_map(|(id, _)| match self.layer(*id).map(|l| &l.kind) {
                Some(LayerKind::Vector(v)) => Some(v.sel.len()),
                _ => None,
            })
            .sum();
        if nsel == 0 {
            d.only_sel = false;
        }
        egui::Window::new(tr!("Export pe tipuri de geometrie (Shapefile)"))
            .id(egui::Id::unique("win_split"))
            .open(&mut open)
            .collapsible(false)
            .default_width(480.0)
            .show(ctx, |ui| {
                ui.label(tr!(
                    "Entitățile din straturile bifate sunt unite și scrise în câte un shapefile pentru fiecare tip de geometrie."
                ));
                ui.add_space(4.0);
                ui.strong(tr!("Straturi:"));
                egui::ScrollArea::vertical()
                    .id_salt("split_layers")
                    .max_height(160.0)
                    .show(ui, |ui| {
                        for (id, on) in &mut d.layers {
                            if let Some(l) = self.layers.iter().find(|l| l.id == *id) {
                                let mut t = l.name.clone();
                                if let Some(p) = l.source().and_then(|p| p.file_name()) {
                                    t = format!("{t}   ({})", p.to_string_lossy());
                                }
                                ui.checkbox(on, t);
                            }
                        }
                    });
                ui.horizontal(|ui| {
                    if ui.small_button(tr!("Toate")).clicked() {
                        d.layers.iter_mut().for_each(|(_, on)| *on = true);
                    }
                    if ui.small_button(tr!("Doar DXF")).clicked() {
                        for (id, on) in &mut d.layers {
                            *on = self.layer(*id).is_some_and(is_dxf);
                        }
                    }
                    if ui.small_button(tr!("Niciunul")).clicked() {
                        d.layers.iter_mut().for_each(|(_, on)| *on = false);
                    }
                });
                ui.separator();
                egui::Grid::new("split_grid").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
                    ui.label(tr!("Fișiere:"));
                    ui.vertical(|ui| {
                        ui.checkbox(&mut d.points, tr!("puncte » …_puncte.shp (puncte, texte, blocuri)"));
                        ui.checkbox(&mut d.lines, tr!("linii » …_linii.shp (linii, polilinii, arce)"));
                        ui.checkbox(&mut d.polygons, tr!("poligoane » …_poligoane.shp"));
                    });
                    ui.end_row();
                    ui.label(tr!("Poliliniile închise:"));
                    ui.vertical(|ui| {
                        ui.radio_value(&mut d.closed_as_lines, false, tr!("ca poligoane"));
                        ui.radio_value(&mut d.closed_as_lines, true, tr!("ca linii (hașurile rămân poligoane)"));
                    });
                    ui.end_row();
                    if nsel > 0 {
                        ui.label(tr!("Entități:"));
                        ui.vertical(|ui| {
                            ui.radio_value(&mut d.only_sel, false, tr!("toate"));
                            ui.radio_value(&mut d.only_sel, true, tr!("numai cele selectate ({})", nsel));
                        });
                        ui.end_row();
                    }
                    ui.label(tr!("Sistem de ieșire:"));
                    ui.vertical(|ui| {
                        if let Some(m) = &self.map_crs {
                            ui.radio_value(&mut d.crs_mode, 0, tr!("Al hărții ({})", m.short()));
                        }
                        ui.radio_value(&mut d.crs_mode, 1, tr!("Fără reproiectare (al straturilor)"));
                        ui.horizontal(|ui| {
                            ui.radio_value(&mut d.crs_mode, 2, tr!("Altul:"));
                            let t = d.custom_crs.as_ref().map(|c| c.to_string()).unwrap_or_else(|| tr!("alege…").into());
                            if ui.button(t).clicked() {
                                pick_crs = true;
                            }
                        });
                    });
                    ui.end_row();
                    ui.label(tr!("Codificare DBF:"));
                    ui.horizontal(|ui| {
                        ui.radio_value(&mut d.cp1250, false, "UTF-8");
                        ui.radio_value(&mut d.cp1250, true, "Windows-1250");
                    });
                    ui.end_row();
                    ui.label(tr!("Câmp suplimentar:"));
                    ui.checkbox(&mut d.source_field, tr!("Fisier – stratul (fișierul) de origine"));
                    ui.end_row();
                });
                ui.separator();
                let n_on = d.layers.iter().filter(|(_, on)| *on).count();
                let can = n_on > 0 && (d.points || d.lines || d.polygons);
                ui.horizontal(|ui| {
                    if ui.add_enabled(can, egui::Button::new(RichText::new(tr!("Export…")).strong())).clicked() {
                        go = true;
                    }
                    ui.label(RichText::new(tr!("Straturi alese: {}", n_on)).weak());
                });
            });
        if pick_crs {
            self.dlg.crs_picker = Some(CrsPicker::new(CrsPurpose::Split, d.custom_crs.clone()));
        }
        if go {
            self.run_split(&d);
        }
        if open && !go {
            self.dlg.split = Some(d);
        }
    }

    fn run_split(&mut self, d: &SplitDlg) {
        let chosen: Vec<u64> = d.layers.iter().filter(|(_, on)| *on).map(|(id, _)| *id).collect();
        let default_name = if chosen.len() == 1 {
            self.layer(chosen[0]).map(|l| formats::sanitize(&l.name)).unwrap_or_default()
        } else {
            "dxf".to_string()
        };
        let Some(mut path) = rfd::FileDialog::new()
            .set_title(tr!("Numele de bază al shapefile-urilor (se adaugă _puncte, _linii, _poligoane)"))
            .add_filter("ESRI Shapefile", &["shp"])
            .set_file_name(format!("{default_name}.shp"))
            .save_file()
        else {
            return;
        };
        // dacă s-a ales un fișier deja sufixat, se folosește baza lui
        let stem = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        for suf in ["_puncte", "_linii", "_poligoane"] {
            if let Some(b) = stem.strip_suffix(suf) {
                path.set_file_name(format!("{b}.shp"));
                break;
            }
        }
        path.set_extension("shp");
        let layers: Vec<cadastru::model::VectorLayer> = chosen
            .iter()
            .filter_map(|id| self.layer(*id))
            .filter_map(|l| match &l.kind {
                LayerKind::Vector(v) => {
                    let mut data = if d.only_sel {
                        if v.sel.is_empty() {
                            return None;
                        }
                        v.selected_data()
                    } else {
                        v.data.clone()
                    };
                    data.name = l.name.clone();
                    Some(data)
                }
                _ => None,
            })
            .collect();
        let target = match d.crs_mode {
            0 => self.map_crs.clone(),
            2 => d.custom_crs.clone(),
            _ => None,
        };
        if d.crs_mode == 2 && target.is_none() {
            self.errors.push(tr!("Alegeți sistemul de ieșire.").into());
            return;
        }
        let so = SplitOptions {
            points: d.points,
            lines: d.lines,
            polygons: d.polygons,
            closed_as_lines: d.closed_as_lines,
            source_field: d.source_field.then(|| "Fisier".to_string()),
        };
        let wo = WriteOptions {
            encoding: d.cp1250.then(|| "cp1250".to_string()),
            overwrite: true,
            ..Default::default()
        };
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let r = convert::export_by_kind(&layers, target.as_ref(), &path, &so, &wo);
            let _ = tx.send(match r {
                Ok(r) => {
                    let files = r
                        .written
                        .iter()
                        .map(|(p, _, n)| {
                            format!("{} ({n})", p.file_name().unwrap_or_default().to_string_lossy())
                        })
                        .collect::<Vec<_>>()
                        .join(", ");
                    let mut msg = tr!("Export pe tipuri reușit: {files}");
                    if !r.warnings.is_empty() {
                        msg.push_str(" — ");
                        msg.push_str(&r.warnings.join("; "));
                    }
                    JobResult::Done(msg)
                }
                Err(e) => JobResult::Exported(Err(e)),
            });
        });
        self.jobs.push(Job {
            label: tr!("Export pe tipuri de geometrie").into(),
            rx,
        });
    }
}
