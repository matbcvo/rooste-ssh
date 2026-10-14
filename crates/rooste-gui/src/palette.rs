//! The command palette (Ctrl+Shift+P): every action and saved session,
//! filtered by typing.

use egui::{Id, Modal};
use rooste_core::Profile;

use crate::dialogs::Decision;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Connect(String),
    NewSession,
    QuickConnect,
    ImportPutty,
    ImportSshConfig,
    Settings,
    DuplicateTab,
    ToggleFiles,
    SplitRight,
    SplitDown,
    ToggleBroadcast,
    ToggleSessionManager,
    ZoomIn,
    ZoomOut,
    ResetZoom,
    CloseTab,
}

/// Available commands with their labels.
pub fn commands(profiles: &[Profile], has_tab: bool) -> Vec<(String, Command)> {
    let mut list: Vec<(String, Command)> = profiles
        .iter()
        .map(|p| {
            (
                format!("Connect: {}", p.display_name()),
                Command::Connect(p.id.clone()),
            )
        })
        .collect();
    list.sort_by_key(|(label, _)| label.to_lowercase());
    list.extend([
        ("New session…".to_owned(), Command::NewSession),
        ("Quick connect…".to_owned(), Command::QuickConnect),
        ("Import from PuTTY".to_owned(), Command::ImportPutty),
        (
            "Import from ~/.ssh/config".to_owned(),
            Command::ImportSshConfig,
        ),
        ("Settings…".to_owned(), Command::Settings),
        (
            "Toggle session manager".to_owned(),
            Command::ToggleSessionManager,
        ),
    ]);
    if has_tab {
        list.extend([
            ("Files (SFTP)".to_owned(), Command::ToggleFiles),
            ("Duplicate tab".to_owned(), Command::DuplicateTab),
            ("Split right".to_owned(), Command::SplitRight),
            ("Split down".to_owned(), Command::SplitDown),
            ("Zoom in".to_owned(), Command::ZoomIn),
            ("Zoom out".to_owned(), Command::ZoomOut),
            ("Reset zoom".to_owned(), Command::ResetZoom),
            (
                "Toggle broadcast input".to_owned(),
                Command::ToggleBroadcast,
            ),
            ("Close tab".to_owned(), Command::CloseTab),
        ]);
    }
    list
}

/// Commands whose label contains every word of `query`, in order.
pub fn filter<'a>(commands: &'a [(String, Command)], query: &str) -> Vec<&'a (String, Command)> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    commands
        .iter()
        .filter(|(label, _)| {
            let label = label.to_lowercase();
            words.iter().all(|w| label.contains(w))
        })
        .collect()
}

pub fn show(
    ctx: &egui::Context,
    query: &mut String,
    commands: &[(String, Command)],
) -> Decision<Command> {
    let mut decision = Decision::Pending;
    let response = Modal::new(Id::new("command-palette"))
        .frame(crate::theme::dialog_frame(ctx))
        .show(ctx, |ui| {
            ui.set_width(420.0);
            let edit = ui.add(
                egui::TextEdit::singleline(query)
                    .margin(crate::theme::FIELD_MARGIN)
                    .hint_text("Type a command or session name")
                    .desired_width(f32::INFINITY),
            );
            edit.widget_info(|| {
                egui::WidgetInfo::labeled(egui::WidgetType::TextEdit, true, "Command")
            });
            edit.request_focus();
            let matches = filter(commands, query);
            if ui.input(|i| i.key_pressed(egui::Key::Enter))
                && let Some((_, command)) = matches.first()
            {
                decision = Decision::Accept(command.clone());
            }
            ui.separator();
            egui::ScrollArea::vertical()
                .max_height(300.0)
                .show(ui, |ui| {
                    if matches.is_empty() {
                        ui.weak("No matching commands");
                    }
                    for (label, command) in matches {
                        if ui.selectable_label(false, label).clicked() {
                            decision = Decision::Accept(command.clone());
                        }
                    }
                });
        });
    if decision == Decision::Pending
        && (response.should_close() || ctx.input(|i| i.key_pressed(egui::Key::Escape)))
    {
        decision = Decision::Cancel;
    }
    decision
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_and_filtering() {
        let profiles = vec![Profile {
            name: "Web server".into(),
            host: "h".into(),
            ..Profile::default()
        }];
        let without_tab = commands(&profiles, false);
        assert!(without_tab.iter().all(|(_, c)| *c != Command::SplitRight));
        let all = commands(&profiles, true);
        let hits = filter(&all, "conn web");
        assert_eq!(hits.len(), 1);
        assert!(matches!(hits[0].1, Command::Connect(_)));
        assert_eq!(filter(&all, "split").len(), 2);
        assert_eq!(filter(&all, "").len(), all.len());
        assert!(filter(&all, "zzz").is_empty());
    }
}
