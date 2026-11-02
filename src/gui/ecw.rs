//! Fereastra „Formate ECW / MrSID”: starea GDAL-ului folosit pentru citire
//! și alegerea dosarului unei instalări QGIS / OSGeo4W / GDAL.

use super::App;
use cadastru::formats::gdalrt;
use cadastru::i18n::tr;
use eframe::egui::{self, Color32, RichText};

#[derive(Default)]
pub struct EcwDlg {
    pub open: bool,
    /// rezultatul ultimei verificări: Ok(text stare) sau Err(mesaj)
    checked: Option<Result<(), String>>,
}

impl App {
    /// Aplică dosarul GDAL din setări (la pornire și la schimbare).
    pub fn apply_gdal_hint(&self) {
        let p = self.settings.gdal_path.trim();
        gdalrt::set_hint((!p.is_empty()).then(|| std::path::PathBuf::from(p)));
    }

    pub fn ecw_window(&mut self, ctx: &egui::Context) {
        if !self.dlg.ecw.open {
            return;
        }
        let mut open = true;
        let mut recheck = false;
        egui::Window::new(tr!("Formate ECW / MrSID"))
            .id(egui::Id::unique("win_ecw"))
            .open(&mut open)
            .default_width(560.0)
            .resizable(true)
            .collapsible(false)
            .show(ctx, |ui| {
                ui.label(tr!(
                    "ECW și MrSID sunt formate proprietare. Cadastru le deschide doar pentru vizualizare, prin GDAL-ul unei instalări QGIS sau OSGeo4W de pe acest calculator, care are driverele gratuite de citire. Cadastru nu scrie și nu convertește în aceste formate."
                ));
                ui.add_space(6.0);
                let loaded = gdalrt::loaded();
                match (&loaded, &self.dlg.ecw.checked) {
                    (Some(g), _) => {
                        egui::Grid::new("gdal_state").num_columns(2).spacing([12.0, 4.0]).show(ui, |ui| {
                            ui.label(tr!("GDAL:"));
                            ui.label(RichText::new(format!("{}  ({})", g.version, g.path.display())).strong());
                            ui.end_row();
                            for (name, ok) in g.driver_status() {
                                ui.label(name);
                                if ok {
                                    ui.label(RichText::new(tr!("disponibil")).color(Color32::from_rgb(90, 190, 110)));
                                } else {
                                    ui.label(RichText::new(tr!("lipsește")).color(Color32::from_rgb(230, 110, 90)));
                                }
                                ui.end_row();
                            }
                        });
                        if !g.has_driver("MrSID") {
                            ui.label(
                                RichText::new(tr!(
                                    "Pentru MrSID: în OSGeo4W Setup (din meniul Start al QGIS) alegeți Advanced Install și bifați pachetul gdal-mrsid."
                                ))
                                .weak()
                                .small(),
                            );
                        }
                        if !g.has_driver("ECW") {
                            ui.label(
                                RichText::new(tr!(
                                    "Pentru ECW: instalatorul complet QGIS include citirea ECW; în OSGeo4W bifați pachetul gdal-ecw."
                                ))
                                .weak()
                                .small(),
                            );
                        }
                    }
                    (None, Some(Err(e))) => {
                        ui.label(RichText::new(e).color(Color32::from_rgb(230, 140, 90)));
                    }
                    (None, _) => {
                        ui.label(RichText::new(tr!("GDAL nu a fost încă încărcat (se caută la deschiderea primului fișier ECW / MrSID).")).weak());
                    }
                }
                ui.add_space(6.0);
                ui.separator();
                ui.label(tr!("Dosarul GDAL (opțional, altfel se caută automat QGIS / OSGeo4W):"));
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button(tr!("Alege…")).clicked()
                            && let Some(d) = rfd::FileDialog::new()
                                .set_title(tr!("Dosarul „bin” al QGIS / OSGeo4W (conține gdal*.dll)"))
                                .pick_folder()
                        {
                            self.settings.gdal_path = d.display().to_string();
                            self.settings.save();
                            self.apply_gdal_hint();
                            recheck = true;
                        }
                        let r = ui.add(
                            egui::TextEdit::singleline(&mut self.settings.gdal_path)
                                .hint_text("C:\\Program Files\\QGIS 3.40\\bin")
                                .desired_width(ui.available_width()),
                        );
                        if r.lost_focus() {
                            self.settings.save();
                            self.apply_gdal_hint();
                        }
                    });
                });
                ui.horizontal(|ui| {
                    if loaded.is_none() && ui.button(tr!("Caută GDAL acum")).clicked() {
                        recheck = true;
                    }
                    if loaded.is_some() && !self.settings.gdal_path.trim().is_empty() {
                        ui.label(
                            RichText::new(tr!("Un alt dosar se folosește după repornirea aplicației."))
                                .weak()
                                .small(),
                        );
                    }
                });
                ui.add_space(4.0);
                ui.collapsing(tr!("Licențe"), |ui| {
                    ui.label(RichText::new(tr!(
                        "Decodarea ECW folosește ERDAS ECW/JP2 SDK (Hexagon), iar MrSID – MrSID Decode SDK (Extensis / LizardTech); ambele sunt gratuite pentru citire și vin cu instalarea GDAL (QGIS / OSGeo4W), sub licențele lor. Cadastru nu include aceste biblioteci și nu le modifică."
                    )).small());
                    ui.label(RichText::new(tr!(
                        "Imaginile se afișează la rezoluția ecranului: în memorie se ține o imagine de ansamblu (max. 4096 px), iar detaliul se citește din fișier pentru zona vizibilă și la tipărire."
                    )).small().weak());
                });
            });
        if recheck {
            self.dlg.ecw.checked = Some(gdalrt::get().map(|_| ()).map_err(|e| e.to_string()));
        }
        self.dlg.ecw.open = open;
    }
}
