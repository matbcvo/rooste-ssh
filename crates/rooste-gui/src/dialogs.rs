//! Modal dialogs. Each returns what the user decided; the app applies it.

use std::path::PathBuf;

use egui::{Id, Modal, Ui};
use rooste_core::{Profile, Protocol, Settings, Theme};
use rooste_proto::ssh::UnknownHostKey;

use crate::session::PromptKind;

/// Outcome of a dialog for one frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision<T> {
    Pending,
    Cancel,
    Accept(T),
}

fn labeled(
    ui: &mut Ui,
    label: &str,
    add: impl FnOnce(&mut Ui) -> egui::Response,
) -> egui::Response {
    let label = ui.label(label);
    let response = add(ui).labelled_by(label.id);
    ui.end_row();
    response
}

fn text(ui: &mut Ui, label: &str, value: &mut String) -> egui::Response {
    labeled(ui, label, |ui| {
        ui.add(egui::TextEdit::singleline(value).desired_width(300.0))
    })
}

fn buttons(ui: &mut Ui, accept: &str) -> (bool, bool) {
    let mut result = (false, false);
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        result.0 = ui.button(accept).clicked();
        result.1 = ui.button("Cancel").clicked();
    });
    result
}

/// Editing a profile, with string fields for parsing on save.
#[derive(Debug, Clone, PartialEq)]
pub struct ProfileEditor {
    pub profile: Profile,
    pub port: String,
    pub key_file: String,
    pub tags: String,
    pub proxy_jump: String,
    /// New password to store; empty keeps the stored one.
    pub password: String,
    pub is_new: bool,
    pub error: Option<String>,
}

impl ProfileEditor {
    pub fn new(profile: Profile, is_new: bool) -> Self {
        Self {
            port: profile.port.map(|p| p.to_string()).unwrap_or_default(),
            key_file: profile
                .key_file
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
            tags: profile.tags.join(", "),
            proxy_jump: profile.proxy_jump.clone().unwrap_or_default(),
            password: String::new(),
            profile,
            is_new,
            error: None,
        }
    }

    /// Validate the fields into a profile.
    pub fn finish(&self) -> Result<Profile, String> {
        let mut profile = self.profile.clone();
        profile.host = profile.host.trim().to_owned();
        profile.name = profile.name.trim().to_owned();
        profile.folder = profile
            .folder
            .split('/')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("/");
        if profile.host.is_empty() {
            return Err("Enter a host name.".into());
        }
        profile.port = match self.port.trim() {
            "" => None,
            port => Some(
                port.parse::<u16>()
                    .ok()
                    .filter(|&p| p != 0)
                    .ok_or("Port must be a number between 1 and 65535.")?,
            ),
        };
        let trimmed = |s: &str| (!s.trim().is_empty()).then(|| s.trim().to_owned());
        profile.key_file = trimmed(&self.key_file).map(PathBuf::from);
        profile.proxy_jump = trimmed(&self.proxy_jump);
        profile.tags = self
            .tags
            .split(',')
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(str::to_owned)
            .collect();
        Ok(profile)
    }

    pub fn show(&mut self, ctx: &egui::Context, folders: &[String]) -> Decision<(Profile, String)> {
        let mut decision = Decision::Pending;
        let title = if self.is_new {
            "New session"
        } else {
            "Edit session"
        };
        Modal::new(Id::new("profile-editor")).show(ctx, |ui| {
            ui.heading(title);
            ui.add_space(6.0);
            egui::Grid::new("profile-fields")
                .num_columns(2)
                .spacing([12.0, 6.0])
                .show(ui, |ui| {
                    let p = &mut self.profile;
                    text(ui, "Name", &mut p.name);
                    labeled(ui, "Folder", |ui| {
                        ui.horizontal(|ui| {
                            let response = ui.add(
                                egui::TextEdit::singleline(&mut p.folder)
                                    .hint_text("e.g. Work/Production")
                                    .desired_width(270.0),
                            );
                            if !folders.is_empty() {
                                ui.menu_button("▾", |ui| {
                                    for folder in folders {
                                        if ui.button(folder).clicked() {
                                            p.folder = folder.clone();
                                            ui.close();
                                        }
                                    }
                                });
                            }
                            response
                        })
                        .inner
                    });
                    labeled(ui, "Protocol", |ui| {
                        egui::ComboBox::from_id_salt("protocol")
                            .selected_text(protocol_name(p.protocol))
                            .show_ui(ui, |ui| {
                                for protocol in Protocol::ALL {
                                    ui.selectable_value(
                                        &mut p.protocol,
                                        protocol,
                                        protocol_name(protocol),
                                    );
                                }
                            })
                            .response
                    });
                    text(ui, "Host", &mut p.host);
                    text(ui, "Port", &mut self.port);
                    text(ui, "User", &mut p.user);
                    text(ui, "Key file", &mut self.key_file);
                    labeled(ui, "", |ui| ui.checkbox(&mut p.use_agent, "Use SSH agent"));
                    labeled(ui, "", |ui| {
                        ui.checkbox(&mut p.save_password, "Save password in the system keyring")
                    });
                    if p.save_password {
                        labeled(ui, "Password", |ui| {
                            ui.add(
                                egui::TextEdit::singleline(&mut self.password)
                                    .password(true)
                                    .hint_text("unchanged")
                                    .desired_width(300.0),
                            )
                        });
                    }
                    labeled(ui, "", |ui| {
                        ui.checkbox(&mut p.allow_legacy, "Allow legacy algorithms")
                            .on_hover_text(
                                "Offer outdated SHA-1 and CBC algorithms for old devices",
                            )
                    });
                    text(ui, "Jump host", &mut self.proxy_jump);
                    text(ui, "Tags", &mut self.tags);
                    labeled(ui, "Notes", |ui| {
                        ui.add(
                            egui::TextEdit::multiline(&mut p.notes)
                                .desired_rows(2)
                                .desired_width(300.0),
                        )
                    });
                });
            if let Some(error) = &self.error {
                ui.colored_label(ui.visuals().error_fg_color, error);
            }
            let (save, cancel) = buttons(ui, "Save");
            if save {
                match self.finish() {
                    Ok(profile) => decision = Decision::Accept((profile, self.password.clone())),
                    Err(e) => self.error = Some(e),
                }
            }
            if cancel {
                decision = Decision::Cancel;
            }
        });
        decision
    }
}

pub fn protocol_name(protocol: Protocol) -> &'static str {
    match protocol {
        Protocol::Ssh => "SSH",
        Protocol::Serial => "Serial",
        Protocol::Telnet => "Telnet",
        Protocol::Raw => "Raw TCP",
    }
}

/// Editing application settings.
#[derive(Debug, Clone, PartialEq)]
pub struct SettingsEditor {
    pub settings: Settings,
}

impl SettingsEditor {
    pub fn show(&mut self, ctx: &egui::Context) -> Decision<Settings> {
        let mut decision = Decision::Pending;
        Modal::new(Id::new("settings")).show(ctx, |ui| {
            ui.heading("Settings");
            ui.add_space(6.0);
            let s = &mut self.settings;
            egui::Grid::new("settings-fields")
                .num_columns(2)
                .spacing([12.0, 6.0])
                .show(ui, |ui| {
                    labeled(ui, "Font size", |ui| {
                        ui.add(egui::Slider::new(&mut s.font_size, 8.0..=32.0).step_by(1.0))
                    });
                    labeled(ui, "Scrollback lines", |ui| {
                        ui.add(
                            egui::DragValue::new(&mut s.scrollback_lines)
                                .range(100..=200_000)
                                .speed(100),
                        )
                    });
                    labeled(ui, "Theme", |ui| {
                        ui.horizontal(|ui| {
                            ui.selectable_value(&mut s.theme, Theme::Dark, "Dark");
                            ui.selectable_value(&mut s.theme, Theme::Light, "Light");
                        })
                        .response
                    });
                    text(ui, "Default user", &mut s.default_user);
                    labeled(ui, "", |ui| {
                        ui.checkbox(&mut s.copy_on_select, "Copy text when selected")
                    });
                    labeled(ui, "", |ui| {
                        ui.checkbox(&mut s.confirm_close, "Ask before closing a connected tab")
                    });
                });
            let (save, cancel) = buttons(ui, "Save");
            if save {
                decision = Decision::Accept(self.settings.clone());
            }
            if cancel {
                decision = Decision::Cancel;
            }
        });
        decision
    }
}

/// Ask for a password or key passphrase.
pub fn prompt(
    ctx: &egui::Context,
    kind: &PromptKind,
    retry: bool,
    input: &mut String,
) -> Decision<String> {
    let mut decision = Decision::Pending;
    Modal::new(Id::new("secret-prompt")).show(ctx, |ui| {
        let (heading, label) = match kind {
            PromptKind::Password { user, host } => {
                ("Password", format!("Password for {user}@{host}"))
            }
            PromptKind::Passphrase { key_file } => (
                "Key passphrase",
                format!("Passphrase for key {}", key_file.display()),
            ),
        };
        ui.heading(heading);
        if retry {
            ui.colored_label(
                ui.visuals().error_fg_color,
                "Not accepted, please try again.",
            );
        }
        let label = ui.label(label);
        let response = ui
            .add(
                egui::TextEdit::singleline(input)
                    .password(true)
                    .desired_width(280.0),
            )
            .labelled_by(label.id);
        if !response.has_focus() && !response.lost_focus() {
            response.request_focus();
        }
        let enter = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        let (ok, cancel) = buttons(ui, "Log in");
        if ok || enter {
            decision = Decision::Accept(std::mem::take(input));
        } else if cancel {
            decision = Decision::Cancel;
        }
    });
    decision
}

/// Ask whether to trust an unknown host key.
pub fn host_key(ctx: &egui::Context, key: &UnknownHostKey) -> Decision<()> {
    let mut decision = Decision::Pending;
    Modal::new(Id::new("host-key")).show(ctx, |ui| {
        ui.set_max_width(460.0);
        ui.heading("Unknown host");
        let host = if key.port == 22 {
            key.host.clone()
        } else {
            format!("{}:{}", key.host, key.port)
        };
        ui.label(format!(
            "The authenticity of host {host} can't be established. Check that this fingerprint \
             matches the server before trusting it."
        ));
        ui.add_space(6.0);
        ui.label(format!("{} key fingerprint:", key.algorithm));
        ui.monospace(&key.fingerprint);
        let (trust, cancel) = buttons(ui, "Trust and connect");
        if trust {
            decision = Decision::Accept(());
        } else if cancel {
            decision = Decision::Cancel;
        }
    });
    decision
}

/// A yes/cancel question.
pub fn confirm(
    ctx: &egui::Context,
    id: &str,
    title: &str,
    text: &str,
    accept: &str,
) -> Decision<()> {
    let mut decision = Decision::Pending;
    Modal::new(Id::new(id)).show(ctx, |ui| {
        ui.set_max_width(420.0);
        ui.heading(title);
        ui.label(text);
        let (yes, cancel) = buttons(ui, accept);
        if yes {
            decision = Decision::Accept(());
        } else if cancel {
            decision = Decision::Cancel;
        }
    });
    decision
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editor_validation_and_parsing() {
        let mut editor = ProfileEditor::new(Profile::default(), true);
        assert_eq!(editor.finish().unwrap_err(), "Enter a host name.");
        editor.profile.host = " db.example.com ".into();
        editor.profile.folder = " Work / / Prod ".into();
        editor.port = "70000".into();
        assert!(editor.finish().unwrap_err().starts_with("Port"));
        editor.port = "2200".into();
        editor.tags = "db, prod,, ".into();
        editor.key_file = " ~/.ssh/id ".into();
        editor.proxy_jump = "".into();
        let p = editor.finish().unwrap();
        assert_eq!(p.host, "db.example.com");
        assert_eq!(p.folder, "Work/Prod");
        assert_eq!(p.port, Some(2200));
        assert_eq!(p.tags, ["db", "prod"]);
        assert_eq!(p.key_file, Some(PathBuf::from("~/.ssh/id")));
        assert_eq!(p.proxy_jump, None);
    }

    #[test]
    fn editor_round_trips_existing_profile() {
        let profile = Profile {
            host: "h".into(),
            port: Some(2222),
            tags: vec!["a".into(), "b".into()],
            proxy_jump: Some("jump".into()),
            key_file: Some("/k".into()),
            ..Profile::default()
        };
        let editor = ProfileEditor::new(profile.clone(), false);
        assert_eq!(editor.finish().unwrap(), profile);
    }
}
