//! "Browse…" buttons that open the system file dialog without blocking the
//! UI: the dialog runs on the tokio runtime and its answer arrives on a
//! later frame.

use std::path::{Path, PathBuf};

use tokio::sync::oneshot;

/// Which field a dialog is for, so its answer lands in the right place.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PickId {
    QuickKeyFile,
    ProfileKeyFile,
    KeyGenSave,
    ConvertInput,
    ConvertOutput,
    AgentKeyFile,
    DownloadDir,
    /// "Download to…" in the file panel.
    DownloadTo,
    /// "Upload…" and "Upload folder…" in the file panel.
    UploadFiles,
    UploadFolder,
    /// Settings: the folder for session logs.
    LogDir,
    /// "Start logging…" for a tab.
    SessionLog,
    /// "Record session…" for a tab.
    SessionRecording,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickKind {
    OpenFile,
    /// One or more files.
    OpenFiles,
    SaveFile,
    Folder,
}

#[derive(Debug)]
pub struct Picker {
    runtime: tokio::runtime::Handle,
    pending: Option<(PickId, oneshot::Receiver<Vec<PathBuf>>)>,
    ready: Option<(PickId, Vec<PathBuf>)>,
    /// Tests: answer every dialog with this path instead of opening one.
    pub fake_answer: Option<PathBuf>,
}

impl Picker {
    pub fn new(runtime: tokio::runtime::Handle) -> Self {
        Self {
            runtime,
            pending: None,
            ready: None,
            fake_answer: None,
        }
    }

    /// Collect a finished dialog's answer. Call once per frame.
    pub fn poll(&mut self) {
        if let Some((id, rx)) = &mut self.pending {
            match rx.try_recv() {
                Ok(answer) => {
                    self.ready = (!answer.is_empty()).then_some((*id, answer));
                    self.pending = None;
                }
                Err(oneshot::error::TryRecvError::Empty) => {}
                Err(oneshot::error::TryRecvError::Closed) => self.pending = None,
            }
        }
    }

    /// Draw a "Browse…" button. Returns the chosen path (once) after the
    /// dialog closes. `current` is where the dialog starts.
    pub fn button(
        &mut self,
        ui: &mut egui::Ui,
        id: PickId,
        kind: PickKind,
        current: &str,
    ) -> Option<PathBuf> {
        let busy = self.pending.is_some();
        let clicked = ui
            .add_enabled(!busy, egui::Button::new("Browse…"))
            .on_hover_text(match kind {
                PickKind::Folder => "Choose a folder",
                _ => "Choose a file",
            })
            .clicked();
        if clicked {
            self.open(ui.ctx(), id, kind, current);
        }
        self.take(id)
    }

    /// The chosen path for `id`, once, after its dialog closes.
    pub fn take(&mut self, id: PickId) -> Option<PathBuf> {
        self.take_all(id).into_iter().next()
    }

    /// All chosen paths for `id` (several for [`PickKind::OpenFiles`]),
    /// once, after its dialog closes; empty until then.
    pub fn take_all(&mut self, id: PickId) -> Vec<PathBuf> {
        match self.ready.take() {
            Some((ready, paths)) if ready == id => paths,
            other => {
                self.ready = other;
                Vec::new()
            }
        }
    }

    /// Whether a dialog is open.
    pub fn is_busy(&self) -> bool {
        self.pending.is_some()
    }

    /// Open a dialog without a button (e.g. from a menu); collect the answer
    /// with [`Self::take`]. Ignored while another dialog is open.
    pub fn open(&mut self, ctx: &egui::Context, id: PickId, kind: PickKind, current: &str) {
        if self.is_busy() {
            return;
        }
        if let Some(path) = self.fake_answer.clone() {
            self.ready = Some((id, vec![path]));
            return;
        }
        let start = start_dir(current, id);
        let file_name = Path::new(current.trim())
            .file_name()
            .map(|n| n.to_string_lossy().into_owned());
        let (tx, rx) = oneshot::channel();
        let ctx = ctx.clone();
        self.runtime.spawn(async move {
            let mut dialog = rfd::AsyncFileDialog::new();
            if let Some(dir) = start {
                dialog = dialog.set_directory(dir);
            }
            let answer = match kind {
                PickKind::OpenFile => dialog.pick_file().await.into_iter().collect(),
                PickKind::OpenFiles => dialog.pick_files().await.unwrap_or_default(),
                PickKind::Folder => dialog.pick_folder().await.into_iter().collect(),
                PickKind::SaveFile => {
                    if let Some(name) = file_name {
                        dialog = dialog.set_file_name(name);
                    }
                    dialog.save_file().await.into_iter().collect()
                }
            };
            let paths: Vec<PathBuf> = answer.iter().map(|h| h.path().to_owned()).collect();
            let _ = tx.send(paths);
            ctx.request_repaint();
        });
        self.pending = Some((id, rx));
    }
}

/// The folder of the current value if it exists, else `~/.ssh` for key
/// files, else the home directory.
fn start_dir(current: &str, id: PickId) -> Option<PathBuf> {
    let current = rooste_core::expand_tilde(current.trim());
    let dir = if current.is_dir() {
        Some(current)
    } else {
        current.parent().filter(|p| p.is_dir()).map(Path::to_owned)
    };
    dir.or_else(|| {
        let home = std::env::home_dir()?;
        let ssh = home.join(".ssh");
        let key_file = matches!(
            id,
            PickId::QuickKeyFile
                | PickId::ProfileKeyFile
                | PickId::KeyGenSave
                | PickId::ConvertInput
                | PickId::ConvertOutput
                | PickId::AgentKeyFile
        );
        Some(if key_file && ssh.is_dir() { ssh } else { home })
    })
}

/// A grid row: label, text field and a "Browse…" button that fills it.
/// Returns the text field's response.
pub fn path_row(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut String,
    picker: &mut Picker,
    id: PickId,
    kind: PickKind,
) -> egui::Response {
    let label = ui.label(label);
    let size = egui::vec2(crate::theme::FIELD_WIDTH, ui.spacing().interact_size.y);
    let layout = egui::Layout::right_to_left(egui::Align::Center);
    let response = ui
        .allocate_ui_with_layout(size, layout, |ui| {
            let picked = picker.button(ui, id, kind, value);
            let response = ui
                .add(
                    egui::TextEdit::singleline(value)
                        .margin(crate::theme::FIELD_MARGIN)
                        .desired_width(f32::INFINITY),
                )
                .labelled_by(label.id);
            if let Some(path) = picked {
                *value = path.display().to_string();
            }
            response
        })
        .inner;
    ui.end_row();
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_directory() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("id_ed25519");
        std::fs::write(&file, "").unwrap();
        let text = file.display().to_string();
        assert_eq!(
            start_dir(&text, PickId::ProfileKeyFile).unwrap(),
            dir.path()
        );
        let folder = dir.path().display().to_string();
        assert_eq!(start_dir(&folder, PickId::DownloadDir).unwrap(), dir.path());
        assert!(
            start_dir("", PickId::ProfileKeyFile).is_some(),
            "falls back to a home folder"
        );
    }
}
