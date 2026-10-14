//! Applying settings to the UI and terminals.

use rooste_core::{Settings, Theme};
use rooste_term::Palette;

use crate::terminal_view::TerminalView;

pub fn palette(theme: Theme) -> Palette {
    match theme {
        Theme::Dark => Palette::default(),
        Theme::Light => Palette::light(),
    }
}

pub fn visuals(theme: Theme) -> egui::Visuals {
    match theme {
        Theme::Dark => egui::Visuals::dark(),
        Theme::Light => egui::Visuals::light(),
    }
}

pub fn apply_to_view(view: &mut TerminalView, settings: &Settings) {
    view.font_size = settings.font_size.clamp(6.0, 48.0);
    view.copy_on_select = settings.copy_on_select;
}

/// Where SFTP downloads are saved.
pub fn download_dir(settings: &Settings) -> std::path::PathBuf {
    if settings.download_dir.trim().is_empty() {
        crate::files::default_download_dir()
    } else {
        rooste_core::expand_tilde(settings.download_dir.trim())
    }
}
