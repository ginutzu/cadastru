//! Filtrul de încărcare: la deschiderea fișierelor vectoriale se păstrează doar entitățile din
//! interiorul României (încadrarea în Stereo 70, EPSG:3844) sau dintr-un alt dreptunghi.

use super::App;
use super::layers::LayerKind;
use cadastru::area::{self, AreaFilter, ROMANIA_3844};
use cadastru::i18n::tr;
use cadastru::model::Bounds;
use eframe::egui::{self, RichText};

/// Culoarea indicatorului din bara de stare.
const AMBER: egui::Color32 = egui::Color32::from_rgb(230, 160, 40);

#[derive(Default)]
pub struct LoadFilterDlg {
    pub open: bool,
    /// textul dreptunghiului (minX, minY, maxX, maxY)
    text: String,
    /// stratul pe care se aplică filtrul acum
    layer: Option<u64>,
}

impl LoadFilterDlg {
    /// Stratul ales pentru filtrarea imediată.
    pub fn set_layer(&mut self, id: u64) {
        self.layer = Some(id);
    }
}

impl App {
    /// Filtrul de încărcare, dacă este activ.
    pub fn load_filter(&self) -> Option<AreaFilter> {
        let s = &self.settings;
        if !s.load_area_filter {
            return None;
        }
        Some(self.area_filter())
    }

    /// Filtrul de zonă configurat (activ sau nu).
    pub fn area_filter(&self) -> AreaFilter {
        let [a, b, c, d] = self.settings.load_area_bounds;
        let bounds = Bounds::new(a, b, c, d);
        AreaFilter {
            bounds: if bounds.is_valid() && bounds.width() > 0.0 && bounds.height() > 0.0 {
                bounds
            } else {
                AreaFilter::romania().bounds
            },
            partial: self.settings.load_area_partial,
        }
    }

    /// Numele scurt al filtrului: „România” sau „zonă proprie”.
    pub fn load_filter_name(&self) -> &'static str {
        if self.area_filter().is_romania() { tr!("România") } else { tr!("zonă proprie") }
    }

    pub fn set_load_filter(&mut self, on: bool) {
        self.settings.load_area_filter = on;
        self.settings.save();
        self.status = if on {
            tr!(
                "Filtrul de încărcare este activ: fișierele deschise de acum se încarcă doar cu entitățile din {} ({}).",
                self.load_filter_name(),
                self.area_filter().describe()
            )
        } else {
            tr!("Filtrul de încărcare este oprit: fișierele se încarcă întregi.").into()
        };
    }

    /// Bifa din meniul Fișier.
    pub fn load_filter_checkbox(&mut self, ui: &mut egui::Ui) {
        let mut on = self.settings.load_area_filter;
        if ui
            .checkbox(&mut on, tr!("La deschidere: doar entitățile din interiorul României (EPSG:3844)"))
            .on_hover_text(tr!(
                "Bifat, la deschiderea fișierelor vectoriale (inclusiv deschiderea în masă și fișierele trase în fereastră) se încarcă doar entitățile aflate în interiorul încadrării României în Stereo 70: {}. Rasterele nu sunt afectate.",
                self.area_filter().describe()
            ))
            .changed()
        {
            self.set_load_filter(on);
        }
    }

    /// Indicatorul din bara de stare (click = opțiunile filtrului).
    pub fn load_filter_indicator(&mut self, ui: &mut egui::Ui) {
        if !self.settings.load_area_filter {
            return;
        }
        let t = tr!("Filtru la încărcare: {}", self.load_filter_name());
        if ui
            .add(egui::Button::new(RichText::new(t).color(AMBER)).small())
            .on_hover_text(tr!(
                "Fișierele vectoriale deschise se încarcă doar cu entitățile din zona {} (EPSG:3844). Click pentru opțiuni.",
                self.area_filter().describe()
            ))
            .clicked()
        {
            self.open_load_filter_dialog();
        }
        ui.separator();
    }

    pub fn open_load_filter_dialog(&mut self) {
        let txt = self.area_filter().describe().replace(" – ", ", ");
        let d = &mut self.dlg.load_filter;
        d.open = true;
        d.text = txt;
        d.layer = self.selected_layer;
    }

    pub fn load_filter_window(&mut self, ctx: &egui::Context) {
        if !self.dlg.load_filter.open {
            return;
        }
        let mut d = std::mem::take(&mut self.dlg.load_filter);
        let mut open = true;
        let mut changed = false;
        let mut apply_now: Option<u64> = None;
        let vec_layers: Vec<(u64, String)> = self
            .layers
            .iter()
            .rev()
            .filter(|l| l.is_vector())
            .map(|l| (l.id, l.name.clone()))
            .collect();
        if d.layer.is_none_or(|id| !vec_layers.iter().any(|(i, _)| *i == id)) {
            d.layer = vec_layers.first().map(|(i, _)| *i);
        }
        let s = &mut self.settings;
        egui::Window::new(tr!("Filtrul de încărcare"))
            .id(egui::Id::unique("win_loadfilter"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(460.0)
            .show(ctx, |ui| {
                changed |= ui
                    .checkbox(
                        &mut s.load_area_filter,
                        RichText::new(tr!("La deschidere: doar entitățile din interiorul zonei")).strong(),
                    )
                    .changed();
                ui.label(
                    RichText::new(tr!(
                        "Se aplică fișierelor vectoriale deschise de acum (Deschide date, deschiderea în masă, fișiere trase în fereastră). Coordonatele se transformă în Stereo 70 doar pentru test; straturile își păstrează sistemul. Rasterele nu sunt afectate."
                    ))
                    .weak(),
                );
                ui.add_space(4.0);
                egui::Grid::new("lf_grid").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
                    ui.label(tr!("Entitățile păstrate:"));
                    ui.vertical(|ui| {
                        changed |= ui
                            .radio_value(&mut s.load_area_partial, false, tr!("aflate în întregime în interior"))
                            .changed();
                        changed |= ui
                            .radio_value(&mut s.load_area_partial, true, tr!("și cele care doar ating zona"))
                            .changed();
                    });
                    ui.end_row();
                    ui.label(tr!("Zona (EPSG:3844):"));
                    ui.vertical(|ui| {
                        let r = ui.add(
                            egui::TextEdit::singleline(&mut d.text)
                                .desired_width(300.0)
                                .hint_text("minX, minY, maxX, maxY"),
                        );
                        let parsed = area::parse_bounds(&d.text);
                        if r.changed()
                            && let Some(b) = parsed
                        {
                            s.load_area_bounds = [b.minx, b.miny, b.maxx, b.maxy];
                            changed = true;
                        }
                        match parsed {
                            None => {
                                ui.colored_label(
                                    ui.visuals().error_fg_color,
                                    tr!("Scrieți patru numere: minX, minY, maxX, maxY (metri, Stereo 70)."),
                                );
                            }
                            Some(b) => {
                                let [a, bb, c, dd] = ROMANIA_3844;
                                let ro = b == Bounds::new(a, bb, c, dd);
                                ui.label(
                                    RichText::new(if ro {
                                        tr!("Încadrarea României (implicit)").to_string()
                                    } else {
                                        tr!("Zonă proprie: {:.1} × {:.1} km", b.width() / 1000.0, b.height() / 1000.0)
                                    })
                                    .weak(),
                                );
                            }
                        }
                        if ui.small_button(tr!("România (implicit)")).clicked() {
                            s.load_area_bounds = ROMANIA_3844;
                            d.text = AreaFilter::romania().describe().replace(" – ", ", ");
                            changed = true;
                        }
                    });
                    ui.end_row();
                });
                ui.separator();
                ui.label(RichText::new(tr!("Pentru un strat deja încărcat")).strong());
                ui.horizontal(|ui| {
                    let cur = vec_layers
                        .iter()
                        .find(|(i, _)| Some(*i) == d.layer)
                        .map(|(_, n)| n.clone())
                        .unwrap_or_else(|| tr!("(niciun strat vectorial)").into());
                    egui::ComboBox::from_id_salt("lf_layer").selected_text(cur).width(240.0).show_ui(ui, |ui| {
                        for (id, n) in &vec_layers {
                            ui.selectable_value(&mut d.layer, Some(*id), n);
                        }
                    });
                    if ui
                        .add_enabled(d.layer.is_some(), egui::Button::new(tr!("Scoate entitățile din afara zonei")))
                        .on_hover_text(tr!("Șterge din strat (doar din memorie, nu din fișier) entitățile aflate în afara zonei"))
                        .clicked()
                    {
                        apply_now = d.layer;
                    }
                });
            });
        if changed {
            self.settings.save();
        }
        if let Some(id) = apply_now {
            self.filter_loaded_layer(id);
        }
        d.open = open;
        self.dlg.load_filter = d;
    }

    /// Scoate dintr-un strat încărcat entitățile din afara zonei configurate.
    pub fn filter_loaded_layer(&mut self, id: u64) {
        let f = self.area_filter();
        let mc = self.map_crs.clone();
        let Some(l) = self.layer_mut(id) else { return };
        let name = l.name.clone();
        let LayerKind::Vector(v) = &mut l.kind else { return };
        let st = area::filter_layer(&mut v.data, &f);
        if st.removed() == 0 {
            self.status = tr!("„{}”: toate entitățile sunt în interiorul zonei.", name);
            return;
        }
        v.sel.clear();
        v.sel_gen = v.sel_gen.wrapping_add(1);
        l.rebuild(mc.as_ref());
        if self.selection.is_some_and(|(lid, _)| lid == id) {
            self.selection = None;
        }
        self.identify.retain(|r| r.layer_id != id);
        self.status = tr!("Filtru zonă: {}", st.message(&name));
    }
}
