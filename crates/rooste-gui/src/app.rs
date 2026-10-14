//! The application window.

use std::path::PathBuf;

use egui::{Align, Layout, RichText, Ui};
use rooste_proto::ssh::UnknownHostKey;
use rooste_term::{TermEvent, Terminal};
use tokio::sync::oneshot;

use crate::session::{ConnectRequest, Session, SessionCommand, SessionEvent};
use crate::terminal_view::TerminalView;

/// Connection state of a tab.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TabState {
    Connecting(String),
    Connected,
    Closed(Option<u32>),
    Failed(String),
}

/// One terminal session.
#[derive(Debug)]
pub struct SessionTab {
    pub title: String,
    pub state: TabState,
    pub terminal: Terminal,
    view: TerminalView,
    session: Session,
    request: ConnectRequest,
    host_key: Option<(UnknownHostKey, oneshot::Sender<bool>)>,
    /// Grid size the server was last told about.
    remote_size: (u16, u16),
}

impl SessionTab {
    fn open(
        runtime: &tokio::runtime::Handle,
        ctx: &egui::Context,
        request: ConnectRequest,
    ) -> Self {
        let wake_ctx = ctx.clone();
        let session = Session::spawn(runtime, request.clone(), move || wake_ctx.request_repaint());
        Self {
            title: format!("{}@{}", request.user, request.host),
            state: TabState::Connecting("Connecting…".into()),
            terminal: Terminal::new(request.cols.max(1), request.rows.max(1)),
            view: TerminalView::default(),
            session,
            remote_size: (request.cols.max(1), request.rows.max(1)),
            request,
            host_key: None,
        }
    }

    /// The host key waiting for the user's decision, if any.
    pub fn pending_host_key(&self) -> Option<&UnknownHostKey> {
        self.host_key.as_ref().map(|(key, _)| key)
    }

    fn answer_host_key(&mut self, trust: bool) {
        if let Some((_, reply)) = self.host_key.take() {
            let _ = reply.send(trust);
        }
    }

    fn poll(&mut self, ctx: &egui::Context) {
        for event in self.session.poll() {
            match event {
                SessionEvent::Status(text) => self.state = TabState::Connecting(text),
                SessionEvent::HostKey { key, reply } => self.host_key = Some((key, reply)),
                SessionEvent::Connected => self.state = TabState::Connected,
                SessionEvent::Output(bytes) => self.terminal.advance(&bytes),
                SessionEvent::Closed { exit_status } => self.state = TabState::Closed(exit_status),
                SessionEvent::Failed(message) => self.state = TabState::Failed(message),
            }
        }
        for event in self.terminal.take_events() {
            match event {
                TermEvent::Reply(bytes) => self.session.send(SessionCommand::Input(bytes)),
                TermEvent::Title(title) => self.title = title,
                TermEvent::ResetTitle => {
                    self.title = format!("{}@{}", self.request.user, self.request.host)
                }
                TermEvent::Clipboard(text) => ctx.copy_text(text),
                TermEvent::Bell => {}
            }
        }
    }

    fn status_text(&self) -> String {
        match &self.state {
            TabState::Connecting(text) => text.clone(),
            TabState::Connected => "Connected".into(),
            TabState::Closed(Some(code)) => format!("Session closed (exit status {code})"),
            TabState::Closed(None) => "Session closed".into(),
            TabState::Failed(message) => message.clone(),
        }
    }

    fn close(&mut self) {
        self.answer_host_key(false);
        self.session.send(SessionCommand::Close);
    }
}

/// Fields of the "new connection" form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectForm {
    pub host: String,
    pub port: String,
    pub user: String,
    pub password: String,
    pub key_file: String,
    pub key_passphrase: String,
    pub use_agent: bool,
    pub allow_legacy: bool,
}

impl Default for ConnectForm {
    fn default() -> Self {
        Self {
            host: String::new(),
            port: "22".into(),
            user: std::env::var("USER")
                .or_else(|_| std::env::var("USERNAME"))
                .unwrap_or_default(),
            password: String::new(),
            key_file: String::new(),
            key_passphrase: String::new(),
            use_agent: true,
            allow_legacy: false,
        }
    }
}

impl ConnectForm {
    /// Validate the form into a request, or explain what is missing.
    pub fn request(&self) -> Result<ConnectRequest, String> {
        let host = self.host.trim();
        if host.is_empty() {
            return Err("Enter a host name.".into());
        }
        let port = self
            .port
            .trim()
            .parse::<u16>()
            .ok()
            .filter(|&p| p != 0)
            .ok_or("Port must be a number between 1 and 65535.")?;
        let user = self.user.trim();
        if user.is_empty() {
            return Err("Enter a user name.".into());
        }
        let non_empty = |s: &str| (!s.is_empty()).then(|| s.to_owned());
        Ok(ConnectRequest {
            host: host.to_owned(),
            port,
            user: user.to_owned(),
            password: non_empty(&self.password),
            key_file: non_empty(self.key_file.trim()).map(PathBuf::from),
            key_passphrase: non_empty(&self.key_passphrase),
            use_agent: self.use_agent,
            allow_legacy: self.allow_legacy,
            known_hosts: None,
            cols: 80,
            rows: 24,
        })
    }
}

/// The rooste-ssh application.
pub struct RoosteApp {
    runtime: tokio::runtime::Runtime,
    pub form: ConnectForm,
    pub form_error: Option<String>,
    pub tab: Option<SessionTab>,
    known_hosts: Option<PathBuf>,
    window_title: String,
}

impl std::fmt::Debug for RoosteApp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RoosteApp")
            .field("tab", &self.tab)
            .finish_non_exhaustive()
    }
}

impl Default for RoosteApp {
    fn default() -> Self {
        Self::new()
    }
}

impl RoosteApp {
    pub fn new() -> Self {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("rooste-net")
            .enable_all()
            .build()
            .expect("start tokio runtime");
        Self {
            runtime,
            form: ConnectForm::default(),
            form_error: None,
            tab: None,
            known_hosts: None,
            window_title: String::new(),
        }
    }

    /// Use a specific known_hosts file instead of `~/.ssh/known_hosts`.
    pub fn with_known_hosts(mut self, path: PathBuf) -> Self {
        self.known_hosts = Some(path);
        self
    }

    fn connect(&mut self, ctx: &egui::Context) {
        match self.form.request() {
            Ok(mut request) => {
                request.known_hosts = self.known_hosts.clone();
                self.form_error = None;
                self.tab = Some(SessionTab::open(self.runtime.handle(), ctx, request));
            }
            Err(message) => self.form_error = Some(message),
        }
    }

    /// Draw one frame.
    pub fn show(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        if let Some(tab) = self.tab.as_mut() {
            tab.poll(&ctx);
        }
        self.update_window_title(&ctx);

        if self.tab.is_some() {
            self.session_ui(ui);
        } else {
            self.form_ui(ui);
        }
        self.host_key_dialog(&ctx);
    }

    fn update_window_title(&mut self, ctx: &egui::Context) {
        let title = match &self.tab {
            Some(tab) => format!("{} — rooste-ssh", tab.title),
            None => "rooste-ssh".into(),
        };
        if title != self.window_title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.window_title = title;
        }
    }

    fn form_ui(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        egui::CentralPanel::default().show(ui, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(24.0);
                ui.heading("New connection");
                ui.add_space(12.0);
            });
            let mut submit = false;
            egui::Grid::new("connect-form")
                .num_columns(2)
                .spacing([12.0, 8.0])
                .show(ui, |ui| {
                    let mut field =
                        |ui: &mut Ui, label: &str, value: &mut String, password: bool| {
                            let label = ui.label(label);
                            let edit = egui::TextEdit::singleline(value)
                                .password(password)
                                .desired_width(280.0);
                            let response = ui.add(edit).labelled_by(label.id);
                            submit |= response.lost_focus()
                                && ui.input(|i| i.key_pressed(egui::Key::Enter));
                            ui.end_row();
                        };
                    field(ui, "Host", &mut self.form.host, false);
                    field(ui, "Port", &mut self.form.port, false);
                    field(ui, "User", &mut self.form.user, false);
                    field(ui, "Password", &mut self.form.password, true);
                    field(ui, "Key file", &mut self.form.key_file, false);
                    field(ui, "Key passphrase", &mut self.form.key_passphrase, true);
                    ui.label("");
                    ui.checkbox(&mut self.form.use_agent, "Use SSH agent");
                    ui.end_row();
                    ui.label("");
                    ui.checkbox(&mut self.form.allow_legacy, "Allow legacy algorithms")
                        .on_hover_text("Offer outdated SHA-1 and CBC algorithms for old devices");
                    ui.end_row();
                });
            ui.add_space(8.0);
            if let Some(error) = &self.form_error {
                ui.colored_label(ui.visuals().error_fg_color, error);
            }
            if ui.button("Connect").clicked() || submit {
                self.connect(&ctx);
            }
        });
    }

    fn session_ui(&mut self, ui: &mut Ui) {
        let mut back_to_form = false;
        let mut reconnect = false;
        let ctx = ui.ctx().clone();
        let Some(tab) = self.tab.as_mut() else {
            return;
        };

        egui::Panel::top("status").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(&tab.title).strong());
                ui.separator();
                let status = tab.status_text();
                match tab.state {
                    TabState::Failed(_) => ui.colored_label(ui.visuals().error_fg_color, status),
                    _ => ui.label(status),
                };
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| match tab.state {
                    TabState::Connecting(_) | TabState::Connected => {
                        if ui.button("Disconnect").clicked() {
                            tab.close();
                        }
                    }
                    TabState::Closed(_) | TabState::Failed(_) => {
                        if ui.button("New connection").clicked() {
                            back_to_form = true;
                        }
                        if ui.button("Reconnect").clicked() {
                            reconnect = true;
                        }
                    }
                });
            });
        });

        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(ui, |ui| {
                let out = tab.view.show(ui, &mut tab.terminal);
                if tab.state == TabState::Connected {
                    // Also catches resizes that happened while connecting.
                    let size = tab.terminal.size();
                    if size != tab.remote_size {
                        tab.session.send(SessionCommand::Resize(size.0, size.1));
                        tab.remote_size = size;
                    }
                    if !out.send.is_empty() {
                        tab.session.send(SessionCommand::Input(out.send));
                    }
                }
            });

        if reconnect {
            let mut request = tab.request.clone();
            let (cols, rows) = tab.terminal.size();
            (request.cols, request.rows) = (cols, rows);
            self.tab = Some(SessionTab::open(self.runtime.handle(), &ctx, request));
        } else if back_to_form {
            self.tab = None;
        }
    }

    fn host_key_dialog(&mut self, ctx: &egui::Context) {
        let Some(tab) = self.tab.as_mut() else {
            return;
        };
        let Some(key) = tab.pending_host_key().cloned() else {
            return;
        };
        let mut answer = None;
        egui::Modal::new(egui::Id::new("host-key")).show(ctx, |ui| {
            ui.set_max_width(460.0);
            ui.heading("Unknown host");
            let host = if key.port == 22 {
                key.host.clone()
            } else {
                format!("{}:{}", key.host, key.port)
            };
            ui.label(format!(
                "The authenticity of host {host} can't be established. Check that this \
                 fingerprint matches the server before trusting it."
            ));
            ui.add_space(6.0);
            ui.label(format!("{} key fingerprint:", key.algorithm));
            ui.monospace(&key.fingerprint);
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if ui.button("Trust and connect").clicked() {
                    answer = Some(true);
                }
                if ui.button("Cancel").clicked() {
                    answer = Some(false);
                }
            });
        });
        if let Some(trust) = answer {
            tab.answer_host_key(trust);
        }
    }
}

impl eframe::App for RoosteApp {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        self.show(ui);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn form_validation() {
        let mut form = ConnectForm {
            user: "me".into(),
            ..ConnectForm::default()
        };
        assert_eq!(form.request().unwrap_err(), "Enter a host name.");
        form.host = " example.com ".into();
        form.port = "0".into();
        assert!(form.request().unwrap_err().starts_with("Port"));
        form.port = "2222".into();
        form.password = "pw".into();
        let request = form.request().unwrap();
        assert_eq!(request.host, "example.com");
        assert_eq!(request.port, 2222);
        assert_eq!(request.password.as_deref(), Some("pw"));
        assert_eq!(request.key_file, None);
    }
}
