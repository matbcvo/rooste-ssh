//! One terminal tab: a session, its terminal and pending prompts.

use egui::{Align, Layout, RichText, Ui};
use rooste_core::Settings;
use rooste_proto::ssh::UnknownHostKey;
use rooste_term::{Palette, TermEvent, Terminal};
use tokio::sync::oneshot;

use crate::session::{ConnectRequest, PromptKind, Session, SessionCommand, SessionEvent};
use crate::terminal_view::TerminalView;
use crate::theme;

/// Connection state of a tab.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TabState {
    Connecting(String),
    Connected,
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
    Reconnect,
    /// Save this password for the tab's profile.
    SavePassword {
        profile_id: String,
        password: String,
    },
}

#[derive(Debug)]
pub struct SessionTab {
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
    session: Session,
    host_key: Option<(UnknownHostKey, oneshot::Sender<bool>)>,
    pub prompt: Option<PendingPrompt>,
    /// Grid size the server was last told about.
    remote_size: (u16, u16),
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
        let wake_ctx = ctx.clone();
        let session = Session::spawn(runtime, request.clone(), move || wake_ctx.request_repaint());
        let mut view = TerminalView::default();
        theme::apply_to_view(&mut view, settings);
        Self {
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
            request,
            profile_id: None,
            save_password: false,
            session,
            host_key: None,
            prompt: None,
            actions: Vec::new(),
        }
    }

    /// Title for the tab bar.
    pub fn title(&self) -> String {
        let marker = match self.state {
            TabState::Connecting(_) => "… ",
            TabState::Connected => "",
            TabState::Closed(_) | TabState::Failed(_) => "✕ ",
        };
        format!("{marker}{}", self.name)
    }

    pub fn is_active(&self) -> bool {
        matches!(self.state, TabState::Connecting(_) | TabState::Connected)
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
    }

    pub fn palette(&self) -> Palette {
        self.terminal.palette()
    }

    /// Handle session and terminal events.
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
                SessionEvent::Connected => self.state = TabState::Connected,
                SessionEvent::Output(bytes) => self.terminal.advance(&bytes),
                SessionEvent::Closed { exit_status } => {
                    self.prompt = None;
                    self.state = TabState::Closed(exit_status)
                }
                SessionEvent::Failed(message) => {
                    self.prompt = None;
                    self.state = TabState::Failed(message)
                }
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

    /// Draw the tab: a status line when not connected, then the terminal.
    pub fn ui(&mut self, ui: &mut Ui) {
        if !matches!(self.state, TabState::Connected) {
            ui.horizontal(|ui| {
                let status = self.status_text();
                match self.state {
                    TabState::Failed(_) => ui.colored_label(ui.visuals().error_fg_color, status),
                    _ => ui.label(RichText::new(status).italics()),
                };
                if !self.is_active() {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.button("Reconnect").clicked() {
                            self.actions.push(TabAction::Reconnect);
                        }
                    });
                }
            });
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
                self.session.send(SessionCommand::Input(out.send));
            }
        }
    }
}
