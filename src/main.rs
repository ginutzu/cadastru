//! Cadastru – aplicație GIS desktop (interfața grafică).
#![cfg_attr(windows, windows_subsystem = "windows")]

mod gui;

use eframe::egui;
use std::path::PathBuf;

/// Iconiță generată: glob cu meridiane și paralele.
fn make_icon() -> egui::IconData {
    let n = 64usize;
    let mut rgba = vec![0u8; n * n * 4];
    let c = (n as f32 - 1.0) / 2.0;
    let r = c - 1.0;
    for y in 0..n {
        for x in 0..n {
            let (dx, dy) = (x as f32 - c, y as f32 - c);
            let d = (dx * dx + dy * dy).sqrt();
            let o = (y * n + x) * 4;
            if d <= r {
                // ocean + uscat stilizat
                let land =
                    ((dx * 0.18).sin() + (dy * 0.21).cos() * 0.8 + (dx * dy * 0.004).sin()) > 0.9;
                let (mut cr, mut cg, mut cb) = if land {
                    (76u8, 160u8, 80u8)
                } else {
                    (35u8, 110u8, 190u8)
                };
                // grilă
                let lon = (dx / r).asin() * 4.0 / std::f32::consts::PI;
                let lat = dy / r * 3.0;
                if (lon - lon.round()).abs() < 0.06 || (lat - lat.round()).abs() < 0.05 {
                    cr = 235;
                    cg = 240;
                    cb = 245;
                }
                if d > r - 2.0 {
                    cr = 20;
                    cg = 50;
                    cb = 90;
                }
                rgba[o..o + 4].copy_from_slice(&[cr, cg, cb, 255]);
            }
        }
    }
    egui::IconData {
        rgba,
        width: n as u32,
        height: n as u32,
    }
}

fn main() -> eframe::Result {
    let files: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Cadastru")
            .with_inner_size([1400.0, 880.0])
            .with_min_inner_size([800.0, 500.0])
            // la pornire fereastra ocupă tot ecranul (maximizată, cu bara de titlu)
            .with_maximized(true)
            .with_drag_and_drop(true)
            .with_icon(std::sync::Arc::new(make_icon())),
        ..Default::default()
    };
    eframe::run_native(
        "Cadastru",
        options,
        Box::new(move |cc| Ok(Box::new(gui::App::new(cc, files)))),
    )
}
const _LANG_PACK: &[u8] = include_bytes!("../lang/en.tsv");
