//! rooste-ssh desktop application.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use rooste_gui::RoosteApp;

/// 64x64 RGBA window icon (generated from `packaging/icons`).
const ICON: &[u8] = include_bytes!("../../../packaging/icons/icon-64.rgba");

fn main() -> eframe::Result {
    if std::env::args().any(|a| a == "--version" || a == "-V") {
        println!("rooste {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Rooste SSH")
            .with_app_id("io.github.matbcvo.rooste-ssh")
            .with_icon(egui::IconData {
                rgba: ICON.to_vec(),
                width: 64,
                height: 64,
            })
            .with_inner_size([960.0, 620.0])
            .with_min_inner_size([400.0, 240.0]),
        ..Default::default()
    };
    eframe::run_native(
        "rooste-ssh",
        options,
        Box::new(|cc| {
            rooste_gui::fonts::install(&cc.egui_ctx);
            Ok(Box::new(
                RoosteApp::new()
                    .with_context(cc.egui_ctx.clone())
                    .with_update_check(),
            ))
        }),
    )
}
