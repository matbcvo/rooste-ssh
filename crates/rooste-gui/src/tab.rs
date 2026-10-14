//! One terminal tab: a session, its terminal and pending prompts.

use std::time::{Duration, Instant};

use egui::{RichText, Ui};
use rooste_core::Settings;
use rooste_protocols::ssh::UnknownHostKey;
use rooste_terminal::{Palette, TermEvent, Terminal};
use tokio::sync::oneshot;

use crate::files::FilesPanel;
use crate::session::{
    ConnectRequest, Failure, PromptKind, Session, SessionCommand, SessionEvent, TunnelStatus,
};
use crate::terminal_view::TerminalView;
use crate::theme;

/// Reconnect attempts before giving up.
const MAX_RECONNECTS: u32 = 10;

/// Connection state of a tab.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TabState {
    Connecting(String),
    Connected,
    /// The connection dropped; trying again at `at`.
    Reconnecting {
        attempt: u32,
        at: Instant,
    },
    Closed(Option<u32>),
    Failed(String),
}

/// A secret the session is waiting for.
#[derive(Debug)]
pub struct PendingPrompt {
    pub kind: PromptKind,
    pub retry: bool,
    pub input: String,
    reply: oneshot::Sender<Option<String>>,
}

/// What a tab asks the application to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TabAction {
    /// Save this password for the tab's profile.
    SavePassword {
        profile_id: String,
        password: String,
    },
    /// The user allowed legacy algorithms for this tab (and its profile).
    EnableLegacy { profile_id: Option<String> },
    /// The user chose this folder with "Download to…".
    RememberDownloadFolder(std::path::PathBuf),
}

#[derive(Debug)]
pub struct SessionTab {
    /// Unique and stable for the tab's lifetime (egui_tiles moves panes in
    /// memory, so addresses cannot serve as widget ids).
    pub id: u64,
    /// Name of the saved profile, or `user@host`.
    pub name: String,
    /// Title set by the remote application, if any.
    pub remote_title: Option<String>,
    pub state: TabState,
    pub terminal: Terminal,
    pub view: TerminalView,
    pub request: ConnectRequest,
    pub profile_id: Option<String>,
    /// Save a password entered at the prompt for this profile.
    pub save_password: bool,
    pub tunnels: Vec<TunnelStatus>,
    /// Input typed here goes to every broadcasting tab.
    pub broadcast: bool,
    /// Input waiting to be broadcast by the app.
    pub outbox: Vec<u8>,
    pub auto_reconnect: bool,
    /// Why the last attempt failed, when it did.
    pub failure: Option<Failure>,
    pub prompt: Option<PendingPrompt>,
    /// The SFTP side panel, when shown.
    pub files: Option<FilesPanel>,
    download_dir: std::path::PathBuf,
    /// The folder last chosen with "Download to…".
    recent_download_dir: Option<std::path::PathBuf>,
    session: Session,
    host_key: Option<(UnknownHostKey, oneshot::Sender<bool>)>,
    /// Grid size the server was last told about.
    remote_size: (u16, u16),
    reconnects: u32,
    runtime: tokio::runtime::Handle,
    ctx: egui::Context,
    actions: Vec<TabAction>,
}

impl SessionTab {
    pub fn open(
        runtime: &tokio::runtime::Handle,
        ctx: &egui::Context,
        name: String,
        request: ConnectRequest,
        settings: &Settings,
    ) -> Self {
        let mut view = TerminalView::default();
        theme::apply_to_view(&mut view, settings);
        static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        Self {
            id: NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            name,
            remote_title: None,
            state: TabState::Connecting("Connecting…".into()),
            terminal: Terminal::with_options(
                request.cols.max(1),
                request.rows.max(1),
                theme::palette(settings.theme),
                settings.scrollback_lines,
            ),
            view,
            remote_size: (request.cols.max(1), request.rows.max(1)),
            session: Self::spawn(runtime, ctx, request.clone()),
            request,
            profile_id: None,
            save_password: false,
            tunnels: Vec::new(),
            broadcast: false,
            outbox: Vec::new(),
            auto_reconnect: settings.auto_reconnect,
            failure: None,
            prompt: None,
            files: None,
            download_dir: crate::theme::download_dir(settings),
            recent_download_dir: crate::theme::recent_download_dir(settings),
            host_key: None,
            reconnects: 0,
            runtime: runtime.clone(),
            ctx: ctx.clone(),
            actions: Vec::new(),
        }
    }

    fn spawn(
        runtime: &tokio::runtime::Handle,
        ctx: &egui::Context,
        request: ConnectRequest,
    ) -> Session {
        let wake = ctx.clone();
        Session::spawn(runtime, request, move || wake.request_repaint())
    }

    /// A new tab connecting to the same place.
    pub fn duplicate(&self, settings: &Settings) -> Self {
        let mut tab = Self::open(
            &self.runtime,
            &self.ctx,
            self.name.clone(),
            self.request.clone(),
            settings,
        );
        tab.profile_id = self.profile_id.clone();
        tab.save_password = self.save_password;
        tab
    }

    /// Connect again in this tab, keeping the terminal contents.
    pub fn restart(&mut self) {
        self.close();
        let (cols, rows) = self.terminal.size();
        (self.request.cols, self.request.rows) = (cols, rows);
        self.remote_size = (cols, rows);
        self.session = Self::spawn(&self.runtime, &self.ctx, self.request.clone());
        self.state = TabState::Connecting("Connecting…".into());
        self.failure = None;
        self.tunnels.clear();
        // The SFTP session belonged to the old connection.
        if self.files.is_some() {
            self.files = Some(self.new_files_panel());
        }
    }

    /// Connect again after a failure, with a fresh reconnect budget.
    pub fn reconnect(&mut self) {
        self.reconnects = 0;
        self.restart();
    }

    fn new_files_panel(&self) -> FilesPanel {
        FilesPanel::new(
            self.download_dir.clone(),
            self.recent_download_dir.clone(),
            self.runtime.clone(),
        )
    }

    /// Show or hide the SFTP file panel.
    pub fn toggle_files(&mut self) {
        if self.files.take().is_none() {
            self.files = Some(self.new_files_panel());
            if self.state == TabState::Connected {
                self.session.send(SessionCommand::OpenFiles);
            }
        }
    }

    /// Title for the tab bar.
    pub fn title(&self) -> String {
        let marker = match self.state {
            TabState::Connecting(_) | TabState::Reconnecting { .. } => "… ",
            TabState::Connected => "",
            // Shown as a red dot by the tab bar (see `has_ended`).
            TabState::Closed(_) | TabState::Failed(_) => "",
        };
        let legacy = if self.request.allow_legacy {
            "⚠ "
        } else {
            ""
        };
        let broadcast = if self.broadcast { "📢 " } else { "" };
        format!("{marker}{legacy}{broadcast}{}", self.name)
    }

    /// The session failed or closed.
    pub fn has_ended(&self) -> bool {
        matches!(self.state, TabState::Closed(_) | TabState::Failed(_))
    }

    pub fn is_active(&self) -> bool {
        matches!(
            self.state,
            TabState::Connecting(_) | TabState::Connected | TabState::Reconnecting { .. }
        )
    }

    /// The host key waiting for the user's decision, if any.
    pub fn pending_host_key(&self) -> Option<&UnknownHostKey> {
        self.host_key.as_ref().map(|(key, _)| key)
    }

    pub fn answer_host_key(&mut self, trust: bool) {
        if let Some((_, reply)) = self.host_key.take() {
            let _ = reply.send(trust);
        }
    }

    /// Answer the pending prompt; `None` cancels the login.
    pub fn answer_prompt(&mut self, answer: Option<String>) {
        if let Some(prompt) = self.prompt.take() {
            let _ = prompt.reply.send(answer);
        }
    }

    pub fn apply_settings(&mut self, settings: &Settings) {
        theme::apply_to_view(&mut self.view, settings);
        self.terminal.set_palette(theme::palette(settings.theme));
        self.terminal.set_scrollback(settings.scrollback_lines);
        self.auto_reconnect = settings.auto_reconnect;
        self.download_dir = crate::theme::download_dir(settings);
        self.recent_download_dir = crate::theme::recent_download_dir(settings);
        if let Some(files) = &mut self.files {
            files.download_dir = self.download_dir.clone();
            files.recent_dir = self.recent_download_dir.clone();
        }
    }

    pub fn palette(&self) -> Palette {
        self.terminal.palette()
    }

    /// Send input to the remote side (used for broadcasts).
    pub fn send_input(&mut self, bytes: Vec<u8>) {
        if self.state == TabState::Connected {
            self.terminal.scroll_to_bottom();
            self.session.send(SessionCommand::Input(bytes));
        }
    }

    fn note(&mut self, text: &str) {
        self.terminal
            .advance(format!("\r\n\x1b[33m{text}\x1b[0m\r\n").as_bytes());
    }

    fn failed(&mut self, failure: Failure) {
        self.prompt = None;
        if failure.lost && self.auto_reconnect && self.reconnects < MAX_RECONNECTS {
            self.reconnects += 1;
            let delay = Duration::from_secs(2u64.pow(self.reconnects.min(5)).min(30));
            self.note(&format!(
                "[{} — reconnecting in {}s]",
                failure.message,
                delay.as_secs()
            ));
            self.state = TabState::Reconnecting {
                attempt: self.reconnects,
                at: Instant::now() + delay,
            };
            self.failure = Some(failure);
            return;
        }
        if failure.lost {
            self.note(&format!("[{}]", failure.message));
        }
        self.state = TabState::Failed(failure.message.clone());
        self.failure = Some(failure);
    }

    /// Handle session and terminal events, and due reconnects.
    pub fn poll(&mut self, ctx: &egui::Context) {
        for event in self.session.poll() {
            match event {
                SessionEvent::Status(text) => self.state = TabState::Connecting(text),
                SessionEvent::HostKey { key, reply } => self.host_key = Some((key, reply)),
                SessionEvent::Prompt { kind, retry, reply } => {
                    self.prompt = Some(PendingPrompt {
                        kind,
                        retry,
                        input: String::new(),
                        reply,
                    })
                }
                SessionEvent::PasswordAccepted(password) => {
                    if self.save_password
                        && let Some(profile_id) = &self.profile_id
                    {
                        self.actions.push(TabAction::SavePassword {
                            profile_id: profile_id.clone(),
                            password,
                        });
                    }
                }
                SessionEvent::Connected => {
                    if self.reconnects > 0 {
                        self.note("[reconnected]");
                    }
                    self.reconnects = 0;
                    self.state = TabState::Connected;
                    if self.files.is_some() {
                        self.session.send(SessionCommand::OpenFiles);
                    }
                }
                SessionEvent::Files(handle) => {
                    if let Some(files) = &mut self.files {
                        files.attach(handle);
                    }
                }
                SessionEvent::FilesFailed(message) => {
                    if let Some(files) = &mut self.files {
                        files.error = Some(message);
                    }
                }
                SessionEvent::Tunnels(statuses) => self.tunnels = statuses,
                SessionEvent::Output(bytes) => self.terminal.advance(&bytes),
                SessionEvent::Closed { exit_status } => {
                    self.prompt = None;
                    self.state = TabState::Closed(exit_status)
                }
                SessionEvent::Failed(failure) => self.failed(failure),
            }
        }
        if let TabState::Reconnecting { at, .. } = self.state {
            let now = Instant::now();
            if now >= at {
                self.restart();
            } else {
                ctx.request_repaint_after(at - now);
            }
        }
        for event in self.terminal.take_events() {
            match event {
                TermEvent::Reply(bytes) => self.session.send(SessionCommand::Input(bytes)),
                TermEvent::Title(title) => self.remote_title = Some(title),
                TermEvent::ResetTitle => self.remote_title = None,
                TermEvent::Clipboard(text) => ctx.copy_text(text),
                TermEvent::Bell => {}
            }
        }
    }

    /// Actions queued since the last call.
    pub fn take_actions(&mut self) -> Vec<TabAction> {
        std::mem::take(&mut self.actions)
    }

    pub fn status_text(&self) -> String {
        match &self.state {
            TabState::Connecting(text) => text.clone(),
            TabState::Connected => "Connected".into(),
            TabState::Reconnecting { attempt, at } => {
                let secs = at.saturating_duration_since(Instant::now()).as_secs() + 1;
                format!(
                    "Connection lost. Reconnecting in {secs}s (attempt {attempt} of {MAX_RECONNECTS})…"
                )
            }
            TabState::Closed(Some(code)) => format!("Session closed (exit status {code})"),
            TabState::Closed(None) => "Session closed".into(),
            TabState::Failed(message) => message.clone(),
        }
    }

    pub fn close(&mut self) {
        self.answer_host_key(false);
        self.answer_prompt(None);
        self.session.send(SessionCommand::Close);
    }

    /// Draw the tab: status and tunnel lines when needed, then the terminal.
    pub fn ui(&mut self, ui: &mut Ui) {
        if !matches!(self.state, TabState::Connected) {
            // The status on its own line, wrapped (errors can be long), with
            // any buttons below it.
            egui::Frame::NONE
                .inner_margin(egui::Margin::symmetric(8, 6))
                .show(ui, |ui| {
                    let status = self.status_text();
                    let text = match self.state {
                        TabState::Failed(_) => {
                            RichText::new(status).color(ui.visuals().error_fg_color)
                        }
                        _ => RichText::new(status).italics(),
                    };
                    ui.add(egui::Label::new(text).wrap());
                    ui.horizontal(|ui| match self.state {
                        TabState::Reconnecting { .. } => {
                            if ui.button("Cancel").clicked() {
                                let message = self.failure.as_ref().map_or_else(
                                    || "Connection lost".into(),
                                    |f| f.message.clone(),
                                );
                                self.state = TabState::Failed(message);
                            }
                            if ui.button("Reconnect now").clicked() {
                                self.restart();
                            }
                        }
                        TabState::Closed(_) | TabState::Failed(_) => {
                            if ui.button("Reconnect").clicked() {
                                self.reconnect();
                            }
                            if self.failure.as_ref().is_some_and(|f| f.legacy_hint)
                                && ui
                                    .button("Allow legacy algorithms and retry")
                                    .on_hover_text(
                                        "This server only supports outdated SHA-1/CBC algorithms. \
                                     Only continue if you trust the network.",
                                    )
                                    .clicked()
                            {
                                self.request.allow_legacy = true;
                                self.actions.push(TabAction::EnableLegacy {
                                    profile_id: self.profile_id.clone(),
                                });
                                self.restart();
                            }
                        }
                        _ => {}
                    });
                });
        }
        if !self.tunnels.is_empty() {
            ui.horizontal_wrapped(|ui| {
                for tunnel in &self.tunnels {
                    match &tunnel.result {
                        Ok(detail) => {
                            ui.label(RichText::new(format!("✔ {}", tunnel.description)).small())
                                .on_hover_text(detail);
                        }
                        Err(error) => {
                            ui.label(
                                RichText::new(format!("✖ {}", tunnel.description))
                                    .small()
                                    .color(ui.visuals().error_fg_color),
                            )
                            .on_hover_text(error);
                        }
                    }
                }
            });
        }
        let panel_id = egui::Id::new(("files", self.id));
        if let Some(files) = &mut self.files {
            files.poll();
            egui::Panel::right(panel_id)
                .frame(crate::theme::side_panel_frame(ui.ctx()))
                .resizable(true)
                .default_size(340.0)
                .min_size(280.0)
                .show(ui, |ui| files.ui(ui));
            if let Some(dir) = files.chosen_dir.take() {
                self.actions.push(TabAction::RememberDownloadFolder(dir));
            }
        }
        let out = self.view.show(ui, &mut self.terminal);
        if self.state == TabState::Connected {
            // Also catches resizes that happened while connecting.
            let size = self.terminal.size();
            if size != self.remote_size {
                self.session.send(SessionCommand::Resize(size.0, size.1));
                self.remote_size = size;
            }
            if !out.send.is_empty() {
                if self.broadcast {
                    self.outbox.extend(out.send);
                } else {
                    self.session.send(SessionCommand::Input(out.send));
                }
            }
        }
    }
}
