//! The SFTP file panel: a worker task owning the SFTP session, and the UI
//! showing a directory with transfers.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc as std_mpsc;
use std::sync::{Arc, Mutex};

use egui::{RichText, Ui};
use rooste_sftp::{Entry, Sftp, format_size};
use tokio::sync::mpsc;

#[derive(Debug)]
pub enum FilesCommand {
    List(String),
    Download {
        id: u64,
        remote: String,
        local: PathBuf,
    },
    Upload {
        id: u64,
        local: PathBuf,
        remote: String,
    },
    Mkdir(String),
    Remove {
        path: String,
        dir: bool,
    },
    Rename {
        from: String,
        to: String,
    },
}

#[derive(Debug)]
pub enum FilesEvent {
    Listing {
        path: String,
        entries: Vec<Entry>,
    },
    Progress {
        id: u64,
        done: u64,
        total: u64,
    },
    Finished {
        id: u64,
        result: Result<(), String>,
    },
    /// A directory operation finished; `dir` should be listed again.
    Changed {
        dir: String,
    },
    Error(String),
}

/// The UI's side of an SFTP worker.
#[derive(Debug)]
pub struct FilesHandle {
    commands: mpsc::UnboundedSender<FilesCommand>,
    events: std_mpsc::Receiver<FilesEvent>,
    cancelled: Arc<Mutex<HashSet<u64>>>,
    pub home: String,
}

impl FilesHandle {
    fn send(&self, command: FilesCommand) {
        let _ = self.commands.send(command);
    }

    fn cancel(&self, id: u64) {
        self.cancelled
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id);
    }
}

/// Start a worker for `sftp`. `wake` repaints the UI when events arrive.
pub fn spawn(sftp: Sftp, home: String, wake: Arc<dyn Fn() + Send + Sync>) -> FilesHandle {
    let (commands, mut rx) = mpsc::unbounded_channel::<FilesCommand>();
    let (tx, events) = std_mpsc::channel();
    let cancelled: Arc<Mutex<HashSet<u64>>> = Arc::default();
    let is_cancelled = cancelled.clone();
    let send = move |event: FilesEvent| {
        if tx.send(event).is_ok() {
            wake();
        }
    };
    tokio::spawn(async move {
        while let Some(command) = rx.recv().await {
            match command {
                FilesCommand::List(path) => match sftp.list(&path).await {
                    Ok(entries) => send(FilesEvent::Listing { path, entries }),
                    Err(e) => send(FilesEvent::Error(format!("{path}: {e}"))),
                },
                FilesCommand::Download { id, remote, local } => {
                    let result = sftp
                        .download(&remote, &local, |done, total| {
                            send(FilesEvent::Progress { id, done, total });
                            !is_cancelled
                                .lock()
                                .unwrap_or_else(|e| e.into_inner())
                                .contains(&id)
                        })
                        .await;
                    send(FilesEvent::Finished {
                        id,
                        result: result.map(drop).map_err(|e| e.to_string()),
                    });
                }
                FilesCommand::Upload { id, local, remote } => {
                    let result = sftp
                        .upload(&local, &remote, |done, total| {
                            send(FilesEvent::Progress { id, done, total });
                            !is_cancelled
                                .lock()
                                .unwrap_or_else(|e| e.into_inner())
                                .contains(&id)
                        })
                        .await;
                    send(FilesEvent::Finished {
                        id,
                        result: result.map(drop).map_err(|e| e.to_string()),
                    });
                    send(FilesEvent::Changed {
                        dir: rooste_sftp::parent(&remote),
                    });
                }
                FilesCommand::Mkdir(path) => {
                    let result = sftp.mkdir(&path).await;
                    report(&send, &path, result);
                }
                FilesCommand::Remove { path, dir } => {
                    let result = if dir {
                        sftp.remove_dir(&path).await
                    } else {
                        sftp.remove_file(&path).await
                    };
                    report(&send, &path, result);
                }
                FilesCommand::Rename { from, to } => {
                    let result = sftp.rename(&from, &to).await;
                    report(&send, &from, result);
                }
            }
        }
    });
    FilesHandle {
        commands,
        events,
        cancelled,
        home,
    }
}

fn report(send: &impl Fn(FilesEvent), path: &str, result: Result<(), rooste_sftp::SftpError>) {
    match result {
        Ok(()) => send(FilesEvent::Changed {
            dir: rooste_sftp::parent(path),
        }),
        Err(e) => send(FilesEvent::Error(format!("{path}: {e}"))),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Download,
    Upload,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransferState {
    Running,
    Done,
    Failed(String),
}

#[derive(Debug, Clone)]
pub struct Transfer {
    pub id: u64,
    pub name: String,
    pub direction: Direction,
    pub done: u64,
    pub total: u64,
    pub state: TransferState,
    /// Where a download was saved.
    pub local: PathBuf,
}

/// A pending in-panel edit.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Edit {
    NewFolder(String),
    Rename { from: String, name: String },
    Delete(Entry),
}

/// State of the file panel in a tab.
#[derive(Debug)]
pub struct FilesPanel {
    pub handle: Option<FilesHandle>,
    pub cwd: String,
    pub entries: Vec<Entry>,
    pub selected: Option<String>,
    pub transfers: Vec<Transfer>,
    pub error: Option<String>,
    pub download_dir: PathBuf,
    path_input: String,
    edit: Option<Edit>,
    next_id: u64,
}

impl FilesPanel {
    pub fn new(download_dir: PathBuf) -> Self {
        Self {
            handle: None,
            cwd: String::new(),
            entries: Vec::new(),
            selected: None,
            transfers: Vec::new(),
            error: None,
            download_dir,
            path_input: String::new(),
            edit: None,
            next_id: 0,
        }
    }

    pub fn attach(&mut self, handle: FilesHandle) {
        let home = handle.home.clone();
        self.handle = Some(handle);
        self.open(&home);
    }

    fn open(&mut self, path: &str) {
        if let Some(handle) = &self.handle {
            handle.send(FilesCommand::List(path.to_owned()));
        }
    }

    pub fn download(&mut self, entry: &Entry) {
        let Some(handle) = &self.handle else { return };
        self.next_id += 1;
        let local = unique_path(&self.download_dir, &entry.name);
        handle.send(FilesCommand::Download {
            id: self.next_id,
            remote: entry.path.clone(),
            local: local.clone(),
        });
        self.transfers.push(Transfer {
            id: self.next_id,
            name: entry.name.clone(),
            direction: Direction::Download,
            done: 0,
            total: entry.size,
            state: TransferState::Running,
            local,
        });
    }

    pub fn upload(&mut self, local: &Path) {
        let Some(handle) = &self.handle else { return };
        let Some(name) = local.file_name().map(|n| n.to_string_lossy().into_owned()) else {
            return;
        };
        if local.is_dir() {
            self.error = Some(format!("{name}: uploading folders is not supported yet"));
            return;
        }
        self.next_id += 1;
        handle.send(FilesCommand::Upload {
            id: self.next_id,
            local: local.to_owned(),
            remote: rooste_sftp::join(&self.cwd, &name),
        });
        self.transfers.push(Transfer {
            id: self.next_id,
            name,
            direction: Direction::Upload,
            done: 0,
            total: std::fs::metadata(local).map(|m| m.len()).unwrap_or(0),
            state: TransferState::Running,
            local: local.to_owned(),
        });
    }

    pub fn poll(&mut self) {
        let Some(handle) = &self.handle else { return };
        let events: Vec<FilesEvent> = handle.events.try_iter().collect();
        for event in events {
            match event {
                FilesEvent::Listing { path, entries } => {
                    self.path_input = path.clone();
                    self.cwd = path;
                    self.entries = entries;
                    self.selected = None;
                    self.error = None;
                }
                FilesEvent::Progress { id, done, total } => {
                    if let Some(t) = self.transfers.iter_mut().find(|t| t.id == id) {
                        (t.done, t.total) = (done, total);
                    }
                }
                FilesEvent::Finished { id, result } => {
                    if let Some(t) = self.transfers.iter_mut().find(|t| t.id == id) {
                        t.state = match result {
                            Ok(()) => TransferState::Done,
                            Err(e) => TransferState::Failed(e),
                        };
                    }
                }
                FilesEvent::Changed { dir } => {
                    if dir == self.cwd {
                        let cwd = self.cwd.clone();
                        self.open(&cwd);
                    }
                }
                FilesEvent::Error(message) => self.error = Some(message),
            }
        }
    }

    pub fn ui(&mut self, ui: &mut Ui) {
        if self.handle.is_none() {
            ui.label(RichText::new("Starting SFTP…").italics());
            return;
        }
        // Files dropped onto the window are uploaded here.
        let dropped: Vec<PathBuf> = ui.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .filter(|p| !p.as_os_str().is_empty())
                .collect()
        });
        for path in dropped {
            self.upload(&path);
        }

        ui.horizontal(|ui| {
            if ui.button("⬆").on_hover_text("Parent folder").clicked() {
                let parent = rooste_sftp::parent(&self.cwd);
                self.open(&parent);
            }
            if ui.button("⟳").on_hover_text("Refresh").clicked() {
                let cwd = self.cwd.clone();
                self.open(&cwd);
            }
            let path = ui.add(
                egui::TextEdit::singleline(&mut self.path_input)
                    .margin(crate::theme::FIELD_MARGIN)
                    .desired_width(f32::INFINITY),
            );
            path.widget_info(|| {
                egui::WidgetInfo::labeled(egui::WidgetType::TextEdit, true, "Remote path")
            });
            if path.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                let target = self.path_input.clone();
                self.open(&target);
            }
        });
        ui.horizontal(|ui| {
            if ui.button("New folder").clicked() {
                self.edit = Some(Edit::NewFolder(String::new()));
            }
            let selected = self.selected_entry().cloned();
            match &selected {
                Some(entry) if entry.is_dir() => {
                    if ui.button("Open").clicked() {
                        let path = entry.path.clone();
                        self.open(&path);
                    }
                }
                _ => {
                    if ui
                        .add_enabled(selected.is_some(), egui::Button::new("Download"))
                        .clicked()
                        && let Some(entry) = &selected
                    {
                        self.download(entry);
                    }
                }
            }
            if ui
                .add_enabled(selected.is_some(), egui::Button::new("Rename"))
                .clicked()
                && let Some(entry) = &selected
            {
                self.edit = Some(Edit::Rename {
                    from: entry.path.clone(),
                    name: entry.name.clone(),
                });
            }
            if ui
                .add_enabled(selected.is_some(), egui::Button::new("Delete"))
                .clicked()
                && let Some(entry) = selected
            {
                self.edit = Some(Edit::Delete(entry));
            }
        });
        self.edit_ui(ui);
        if let Some(error) = &self.error {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
        ui.separator();

        let mut open = None;
        let mut download = None;
        egui::ScrollArea::vertical()
            .max_height((ui.available_height() - 90.0).max(80.0))
            .auto_shrink([false, true])
            .show(ui, |ui| {
                if self.entries.is_empty() {
                    ui.weak("Empty folder");
                }
                for entry in &self.entries {
                    let icon = if entry.is_dir() { "📁" } else { "📄" };
                    let selected = self.selected.as_deref() == Some(entry.path.as_str());
                    let label = if entry.is_dir() {
                        format!("{icon} {}", entry.name)
                    } else {
                        format!("{icon} {}  ({})", entry.name, format_size(entry.size))
                    };
                    let response = ui.selectable_label(selected, label);
                    if response.clicked() {
                        self.selected = Some(entry.path.clone());
                    }
                    if response.double_clicked() {
                        if entry.is_dir() {
                            open = Some(entry.path.clone());
                        } else {
                            download = Some(entry.clone());
                        }
                    }
                    response.context_menu(|ui| {
                        if entry.is_dir() {
                            if ui.button("Open").clicked() {
                                open = Some(entry.path.clone());
                                ui.close();
                            }
                        } else if ui.button("Download").clicked() {
                            download = Some(entry.clone());
                            ui.close();
                        }
                    });
                }
            });
        if let Some(path) = open {
            self.open(&path);
        }
        if let Some(entry) = download {
            self.download(&entry);
        }

        ui.separator();
        ui.weak(format!(
            "Drop files here to upload. Downloads go to {}.",
            self.download_dir.display()
        ));
        let mut cancel = None;
        for transfer in self.transfers.iter().rev().take(5) {
            ui.horizontal(|ui| {
                let arrow = match transfer.direction {
                    Direction::Download => "⬇",
                    Direction::Upload => "⬆",
                };
                ui.label(format!("{arrow} {}", transfer.name));
                match &transfer.state {
                    TransferState::Running => {
                        let fraction = if transfer.total == 0 {
                            0.0
                        } else {
                            transfer.done as f32 / transfer.total as f32
                        };
                        ui.add(
                            egui::ProgressBar::new(fraction)
                                .desired_width(100.0)
                                .show_percentage(),
                        );
                        if ui.small_button("✕").on_hover_text("Cancel").clicked() {
                            cancel = Some(transfer.id);
                        }
                    }
                    TransferState::Done => {
                        ui.weak("done");
                    }
                    TransferState::Failed(e) => {
                        ui.colored_label(ui.visuals().error_fg_color, e);
                    }
                }
            });
        }
        if let (Some(id), Some(handle)) = (cancel, &self.handle) {
            handle.cancel(id);
        }
    }

    fn selected_entry(&self) -> Option<&Entry> {
        let selected = self.selected.as_deref()?;
        self.entries.iter().find(|e| e.path == selected)
    }

    fn edit_ui(&mut self, ui: &mut Ui) {
        let Some(edit) = self.edit.as_mut() else {
            return;
        };
        let mut done = false;
        let mut command = None;
        ui.horizontal(|ui| match edit {
            Edit::NewFolder(name) => {
                ui.add(
                    egui::TextEdit::singleline(name)
                        .hint_text("folder name")
                        .desired_width(140.0),
                )
                .widget_info(|| {
                    egui::WidgetInfo::labeled(egui::WidgetType::TextEdit, true, "Folder name")
                });
                if ui.button("Create").clicked() && !name.trim().is_empty() {
                    command = Some(FilesCommand::Mkdir(rooste_sftp::join(
                        &self.cwd,
                        name.trim(),
                    )));
                    done = true;
                }
                if ui.button("Cancel").clicked() {
                    done = true;
                }
            }
            Edit::Rename { from, name } => {
                ui.add(egui::TextEdit::singleline(name).desired_width(140.0))
                    .widget_info(|| {
                        egui::WidgetInfo::labeled(egui::WidgetType::TextEdit, true, "New name")
                    });
                if ui.button("Rename").clicked() && !name.trim().is_empty() {
                    command = Some(FilesCommand::Rename {
                        from: from.clone(),
                        to: rooste_sftp::join(&rooste_sftp::parent(from), name.trim()),
                    });
                    done = true;
                }
                if ui.button("Cancel").clicked() {
                    done = true;
                }
            }
            Edit::Delete(entry) => {
                ui.label(format!("Delete {}?", entry.name));
                if ui.button("Delete").clicked() {
                    command = Some(FilesCommand::Remove {
                        path: entry.path.clone(),
                        dir: entry.is_dir(),
                    });
                    done = true;
                }
                if ui.button("Cancel").clicked() {
                    done = true;
                }
            }
        });
        if let (Some(command), Some(handle)) = (command, &self.handle) {
            handle.send(command);
        }
        if done {
            self.edit = None;
        }
    }
}

/// `dir/name`, or `dir/name (1)` etc. if that already exists.
fn unique_path(dir: &Path, name: &str) -> PathBuf {
    let candidate = dir.join(name);
    if !candidate.exists() {
        return candidate;
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => (stem, format!(".{ext}")),
        _ => (name, String::new()),
    };
    (1..)
        .map(|n| dir.join(format!("{stem} ({n}){ext}")))
        .find(|p| !p.exists())
        .expect("some free name")
}

/// `~/Downloads`, or the home directory if that does not exist.
pub fn default_download_dir() -> PathBuf {
    let home = std::env::home_dir().unwrap_or_else(std::env::temp_dir);
    let downloads = home.join("Downloads");
    if downloads.is_dir() { downloads } else { home }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unique_download_names() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(unique_path(dir.path(), "a.txt"), dir.path().join("a.txt"));
        std::fs::write(dir.path().join("a.txt"), "").unwrap();
        assert_eq!(
            unique_path(dir.path(), "a.txt"),
            dir.path().join("a (1).txt")
        );
        std::fs::write(dir.path().join("a (1).txt"), "").unwrap();
        assert_eq!(
            unique_path(dir.path(), "a.txt"),
            dir.path().join("a (2).txt")
        );
        std::fs::write(dir.path().join("Makefile"), "").unwrap();
        assert_eq!(
            unique_path(dir.path(), "Makefile"),
            dir.path().join("Makefile (1)")
        );
    }
}
