//! Meniul „Nomenclatoare” > „SIRUTA”: modulul SIRUTA (proiectul SIRUTA de Ginu Popescu,
//! licență MIT) integrat în Cadastru și deschis într-o fereastră separată a sistemului
//! (viewport egui „deferred”, cu propria buclă de redesenare).
//!
//! - `data`: citirea CSV-ului INS, clasificarea (legenda SIRUTA) și căutarea fără diacritice;
//! - `update`: actualizarea automată de pe data.gov.ro (CKAN) și cache-ul local
//!   `%LOCALAPPDATA%\SIRUTA` (comun cu aplicația SIRUTA de sine stătătoare);
//! - `ui`: tema dark grey / neon și pictogramele vectoriale, aplicate numai în fereastra SIRUTA;
//! - `app`: interfața (căutare, filtre, tabel, detalii, bara de stare, Despre).

mod app;
mod data;
mod ui;
mod update;

use cadastru::i18n::tr;
use eframe::egui;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

pub use app::SirutaApp;

/// Pictograma SIRUTA, 128 × 128 RGBA.
pub(crate) const ICON_RGBA: &[u8] = include_bytes!("../../../assets/siruta_icon_128.rgba");

/// Identificatorul ferestrei SIRUTA.
pub fn viewport_id() -> egui::ViewportId {
    egui::ViewportId::from_hash_of("cadastru_siruta")
}

fn icon() -> Arc<egui::IconData> {
    static ICON: OnceLock<Arc<egui::IconData>> = OnceLock::new();
    ICON.get_or_init(|| {
        Arc::new(egui::IconData {
            rgba: ICON_RGBA.to_vec(),
            width: 128,
            height: 128,
        })
    })
    .clone()
}

/// Starea ferestrei SIRUTA, păstrată în aplicație. Datele rămân în memorie după închidere,
/// astfel încât redeschiderea este instantanee.
#[derive(Default)]
pub struct SirutaWin {
    state: Option<Arc<Mutex<SirutaApp>>>,
    open: Arc<AtomicBool>,
}

impl SirutaWin {
    pub fn is_open(&self) -> bool {
        self.open.load(Ordering::Relaxed)
    }

    /// Deschide fereastra (sau o aduce în față dacă e deja deschisă).
    pub fn open(&mut self, ctx: &egui::Context) {
        if self.is_open() {
            ctx.send_viewport_cmd_to(viewport_id(), egui::ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd_to(viewport_id(), egui::ViewportCommand::Focus);
            return;
        }
        match &self.state {
            None => self.state = Some(Arc::new(Mutex::new(SirutaApp::new(ctx)))),
            Some(st) => {
                if let Ok(mut a) = st.lock() {
                    a.reopen(ctx);
                }
            }
        }
        self.open.store(true, Ordering::Relaxed);
    }

    /// Se apelează la fiecare cadru al ferestrei principale.
    pub fn show(&self, ctx: &egui::Context) {
        if !self.is_open() {
            return;
        }
        let Some(state) = self.state.clone() else { return };
        let open = self.open.clone();
        let builder = egui::ViewportBuilder::default()
            .with_title(tr!("SIRUTA – nomenclatorul unităților administrativ-teritoriale"))
            .with_inner_size([1180.0, 760.0])
            .with_min_inner_size([980.0, 560.0])
            .with_maximized(true)
            .with_icon(icon());
        ctx.show_viewport_deferred(viewport_id(), builder, move |ui, _class| {
            if ui.input(|i| i.viewport().close_requested()) {
                open.store(false, Ordering::Relaxed);
                ui.ctx().request_repaint_of(egui::ViewportId::ROOT);
            }
            if let Ok(mut app) = state.lock() {
                app.ui(ui);
            }
        });
    }
}
