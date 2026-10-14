//! rooste-ssh desktop application.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use rooste_gui::RoosteApp;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("rooste-ssh")
            .with_inner_size([960.0, 620.0])
            .with_min_inner_size([400.0, 240.0]),
        ..Default::default()
    };
    eframe::run_native(
        "rooste-ssh",
        options,
        Box::new(|cc| {
            rooste_gui::fonts::install(&cc.egui_ctx);
            Ok(Box::new(RoosteApp::new()))
        }),
    )
}
