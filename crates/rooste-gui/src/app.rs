//! The application window: menu, session manager, tabs and dialogs.

use std::path::PathBuf;
use std::sync::Arc;

use egui::{RichText, Ui};
use egui_tiles::{Container, LinearDir, SimplificationOptions, Tile, TileId, Tree, UiResponse};
use rooste_core::import::{putty, ssh_config};
use rooste_core::{Config, ConfigLocation, Profile, Protocol, SecretStore, Settings, Theme};
use rooste_proto::ssh::AgentSource;

use crate::dialogs::{self, Decision, ProfileEditor, SettingsEditor};
use crate::manager::{self, ManagerAction, ManagerState};
use crate::palette::{self as command_palette, Command};
use crate::picker::{PickId, PickKind, Picker, path_row};
use crate::session::ConnectRequest;
use crate::tab::{SessionTab, TabAction};
use crate::theme;
use crate::tools::{AgentAction, AgentWindow, KeyConverter, KeyGenerator};

pub use crate::tab::TabState;

/// Fields of the quick-connect form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectForm {
    pub host: String,
    pub port: String,
    pub user: String,
    pub password: String,
    pub key_file: String,
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
            key_file: non_empty(self.key_file.trim()).map(|p| rooste_core::expand_tilde(&p)),
            key_passphrase: None,
            use_agent: self.use_agent,
            allow_legacy: self.allow_legacy,
            known_hosts: None,
            cols: 80,
            rows: 24,
            prompt_for_password: true,
            proxy_jump: None,
            forward_agent: false,
            agent: AgentSource::Default,
            tunnels: Vec::new(),
            protocol: Protocol::Ssh,
            serial: Default::default(),
        })
    }
}

/// An open dialog.
#[derive(Debug)]
pub enum Dialog {
    EditProfile(Box<ProfileEditor>),
    Settings(SettingsEditor),
    DeleteProfile(String),
    CloseTab(TileId),
    QuickConnect,
    Palette { query: String },
    KeyGenerator(Box<KeyGenerator>),
    KeyConverter(Box<KeyConverter>),
    Agent(Box<AgentWindow>),
}

/// A message in the status bar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub text: String,
    pub error: bool,
    /// A web link shown after the text.
    pub link: Option<(String, String)>,
}

/// Where imports read from; overridable for tests.
#[derive(Debug, Clone, Default)]
pub struct ImportSources {
    pub putty_dir: Option<PathBuf>,
    pub ssh_config: Option<PathBuf>,
}

struct TabBehavior {
    close_requests: Vec<TileId>,
    /// Picked from a tab's right-click menu.
    menu_actions: Vec<(TileId, TabMenuAction)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TabMenuAction {
    Duplicate,
    SplitRight,
    SplitDown,
    Files,
    Reconnect,
    Close,
}

impl egui_tiles::Behavior<SessionTab> for TabBehavior {
    fn pane_ui(&mut self, ui: &mut Ui, _tile_id: TileId, tab: &mut SessionTab) -> UiResponse {
        tab.ui(ui);
        UiResponse::None
    }

    fn tab_title_for_pane(&mut self, tab: &SessionTab) -> egui::WidgetText {
        let mut job = egui::text::LayoutJob::default();
        let font = egui::FontId::proportional(14.0);
        if tab.has_ended() {
            let red = egui::TextFormat::simple(font.clone(), egui::Color32::from_rgb(220, 80, 80));
            job.append("● ", 0.0, red);
        }
        job.append(
            &tab.title(),
            0.0,
            egui::TextFormat::simple(font, egui::Color32::PLACEHOLDER),
        );
        job.into()
    }

    fn is_tab_closable(&self, _tiles: &egui_tiles::Tiles<SessionTab>, _tile_id: TileId) -> bool {
        true
    }

    fn on_tab_close(
        &mut self,
        _tiles: &mut egui_tiles::Tiles<SessionTab>,
        tile_id: TileId,
    ) -> bool {
        // Closing is decided by the app (it may ask first).
        self.close_requests.push(tile_id);
        false
    }

    fn tab_bar_height(&self, _style: &egui::Style) -> f32 {
        30.0
    }

    fn tab_title_spacing(&self, _visuals: &egui::Visuals) -> f32 {
        12.0
    }

    fn on_tab_button(
        &mut self,
        tiles: &mut egui_tiles::Tiles<SessionTab>,
        tile_id: TileId,
        response: egui::Response,
    ) -> egui::Response {
        let Some(Tile::Pane(tab)) = tiles.get(tile_id) else {
            return response;
        };
        let ended = tab.has_ended();
        response.context_menu(|ui| {
            let mut item = |ui: &mut Ui, label: &str, action: TabMenuAction| {
                if ui.button(label).clicked() {
                    self.menu_actions.push((tile_id, action));
                    ui.close();
                }
            };
            item(ui, "Duplicate tab", TabMenuAction::Duplicate);
            item(ui, "Split right", TabMenuAction::SplitRight);
            item(ui, "Split down", TabMenuAction::SplitDown);
            item(ui, "Files (SFTP)", TabMenuAction::Files);
            if ended {
                item(ui, "Reconnect", TabMenuAction::Reconnect);
            }
            ui.separator();
            item(ui, "Close tab", TabMenuAction::Close);
        });
        response
    }

    fn simplification_options(&self) -> SimplificationOptions {
        SimplificationOptions {
            all_panes_must_have_tabs: true,
            ..SimplificationOptions::default()
        }
    }
}

/// The rooste-ssh application.
pub struct RoosteApp {
    runtime: tokio::runtime::Runtime,
    pub config: Config,
    config_path: Option<PathBuf>,
    secrets: Arc<dyn SecretStore>,
    pub tree: Tree<SessionTab>,
    pub manager: ManagerState,
    pub dialog: Option<Dialog>,
    pub quick: ConnectForm,
    pub quick_error: Option<String>,
    pub notice: Option<Notice>,
    pub show_manager: bool,
    /// Input in one tab goes to all connected tabs.
    pub broadcast: bool,
    agent: AgentSource,
    /// The built-in SSH agent, once started.
    pub builtin_agent: Option<rooste_keys::Agent>,
    /// Result of the startup update check, when it finishes.
    update_check: Option<std::sync::mpsc::Receiver<Option<crate::update::Release>>>,
    /// Serial ports found when the profile editor opened.
    serial_ports: Vec<String>,
    /// Native file dialogs for the "Browse…" buttons.
    pub picker: Picker,
    known_hosts: Option<PathBuf>,
    imports: ImportSources,
    window_title: String,
    applied_theme: Option<Theme>,
}

impl std::fmt::Debug for RoosteApp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RoosteApp")
            .field(
                "tabs",
                &self.tabs().map(|t| (&t.name, &t.state)).collect::<Vec<_>>(),
            )
            .field("dialog", &self.dialog)
            .field("notice", &self.notice)
            .finish_non_exhaustive()
    }
}

impl RoosteApp {
    /// The app with the user's configuration and the OS credential store.
    pub fn new() -> Self {
        let location = ConfigLocation::detect();
        let (config, path, notice) = match Config::load(&location.path) {
            Ok(config) => (config, Some(location.path), None),
            Err(e) => (
                Config::default(),
                // Do not overwrite a file we could not read.
                None,
                Some(Notice {
                    text: format!("{e}; changes will not be saved"),
                    error: true,
                    link: None,
                }),
            ),
        };
        let mut app = Self::with_config(config, path, Arc::new(crate::secrets::KeyringStore));
        app.notice = notice;
        app
    }

    /// An app with the given configuration. `config_path` of `None` keeps
    /// changes in memory only.
    pub fn with_config(
        config: Config,
        config_path: Option<PathBuf>,
        secrets: Arc<dyn SecretStore>,
    ) -> Self {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("rooste-net")
            .enable_all()
            .build()
            .expect("start tokio runtime");
        let picker = Picker::new(runtime.handle().clone());
        Self {
            runtime,
            config,
            config_path,
            secrets,
            tree: Tree::empty("tabs"),
            manager: ManagerState::default(),
            dialog: None,
            quick: ConnectForm::default(),
            quick_error: None,
            notice: None,
            show_manager: true,
            broadcast: false,
            agent: AgentSource::Default,
            builtin_agent: None,
            update_check: None,
            serial_ports: Vec::new(),
            picker,
            known_hosts: None,
            imports: ImportSources::default(),
            window_title: String::new(),
            applied_theme: None,
        }
    }

    /// Use a specific known_hosts file instead of `~/.ssh/known_hosts`.
    pub fn with_known_hosts(mut self, path: PathBuf) -> Self {
        self.known_hosts = Some(path);
        self
    }

    /// Check GitHub for a newer release in the background, if enabled.
    pub fn with_update_check(mut self) -> Self {
        if self.config.settings.check_updates {
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let latest = crate::update::fetch_latest(
                    crate::update::RELEASES_API,
                    env!("CARGO_PKG_VERSION"),
                )
                .ok()
                .flatten();
                let _ = tx.send(latest);
            });
            self.update_check = Some(rx);
        }
        self
    }

    /// Use a specific SSH agent for login and forwarding.
    pub fn with_agent(mut self, agent: AgentSource) -> Self {
        self.agent = agent;
        self
    }

    pub fn with_import_sources(mut self, imports: ImportSources) -> Self {
        self.imports = imports;
        self
    }

    pub fn secrets(&self) -> &dyn SecretStore {
        &*self.secrets
    }

    /// All open tabs.
    pub fn tabs(&self) -> impl Iterator<Item = &SessionTab> {
        self.tree.tiles.tiles().filter_map(|tile| match tile {
            Tile::Pane(tab) => Some(tab),
            Tile::Container(_) => None,
        })
    }

    fn tab_ids(&self) -> Vec<TileId> {
        self.tree
            .tiles
            .iter()
            .filter(|(_, tile)| matches!(tile, Tile::Pane(_)))
            .map(|(id, _)| *id)
            .collect()
    }

    fn tab_mut(&mut self, id: TileId) -> Option<&mut SessionTab> {
        match self.tree.tiles.get_mut(id) {
            Some(Tile::Pane(tab)) => Some(tab),
            _ => None,
        }
    }

    /// The first visible tab.
    fn active_tab_id(&self) -> Option<TileId> {
        self.tree
            .active_tiles()
            .into_iter()
            .find(|id| matches!(self.tree.tiles.get(*id), Some(Tile::Pane(_))))
    }

    fn save_config(&mut self) {
        if let Some(path) = &self.config_path
            && let Err(e) = self.config.save(path)
        {
            self.notice = Some(Notice {
                text: e.to_string(),
                error: true,
                link: None,
            });
        }
    }

    fn notify(&mut self, text: impl Into<String>, error: bool) {
        self.notice = Some(Notice {
            text: text.into(),
            error,
            link: None,
        });
    }

    fn request_for(&self, profile: &Profile) -> ConnectRequest {
        let user = if profile.user.is_empty() {
            self.config.settings.default_user.clone()
        } else {
            profile.user.clone()
        };
        ConnectRequest {
            host: profile.host.clone(),
            port: profile.effective_port(),
            user,
            password: profile
                .save_password
                .then(|| self.secrets.get(&profile.id))
                .flatten(),
            key_file: profile
                .key_file
                .as_ref()
                .map(|p| rooste_core::expand_tilde(&p.to_string_lossy())),
            key_passphrase: None,
            use_agent: profile.use_agent,
            allow_legacy: profile.allow_legacy,
            known_hosts: self.known_hosts.clone(),
            cols: 80,
            rows: 24,
            prompt_for_password: true,
            proxy_jump: profile.proxy_jump.clone(),
            forward_agent: profile.forward_agent,
            agent: self.session_agent(),
            tunnels: profile.tunnels.clone(),
            protocol: profile.protocol,
            serial: profile.serial,
        }
    }

    /// Open a tab for a saved profile.
    pub fn connect_profile(&mut self, ctx: &egui::Context, profile_id: &str) {
        let Some(profile) = self.config.profile(profile_id).cloned() else {
            return;
        };
        let request = self.request_for(&profile);
        let mut tab = SessionTab::open(
            self.runtime.handle(),
            ctx,
            profile.display_name(),
            request,
            &self.config.settings,
        );
        tab.profile_id = Some(profile.id.clone());
        tab.save_password = profile.save_password;
        self.add_tab(tab);
    }

    fn quick_connect(&mut self, ctx: &egui::Context) -> bool {
        match self.quick.request() {
            Ok(mut request) => {
                request.known_hosts = self.known_hosts.clone();
                let name = format!("{}@{}", request.user, request.host);
                let tab = SessionTab::open(
                    self.runtime.handle(),
                    ctx,
                    name,
                    request,
                    &self.config.settings,
                );
                self.add_tab(tab);
                self.quick_error = None;
                self.quick.password.clear();
                true
            }
            Err(message) => {
                self.quick_error = Some(message);
                false
            }
        }
    }

    fn add_tab(&mut self, tab: SessionTab) {
        self.add_tab_beside(tab, self.active_tab_id());
    }

    /// Add a tab to the tab group of `beside` (or the root).
    fn add_tab_beside(&mut self, tab: SessionTab, beside: Option<TileId>) {
        let id = self.tree.tiles.insert_pane(tab);
        let target = beside
            .and_then(|tile| self.tree.tiles.parent_of(tile))
            .or(self.tree.root);
        match target.and_then(|t| self.tree.tiles.get_mut(t)) {
            Some(Tile::Container(Container::Tabs(tabs))) => {
                tabs.add_child(id);
                tabs.set_active(id);
            }
            _ => {
                let children = self.tree.root.into_iter().chain([id]).collect();
                let tabs = self.tree.tiles.insert_tab_tile(children);
                if let Some(Tile::Container(Container::Tabs(t))) = self.tree.tiles.get_mut(tabs) {
                    t.set_active(id);
                }
                self.tree.root = Some(tabs);
            }
        }
    }

    fn request_close(&mut self, id: TileId) {
        let confirm = self.config.settings.confirm_close;
        match self.tab_mut(id) {
            Some(tab) if confirm && tab.is_active() => self.dialog = Some(Dialog::CloseTab(id)),
            Some(_) => self.close_tab(id),
            None => {}
        }
    }

    fn close_tab(&mut self, id: TileId) {
        if let Some(tab) = self.tab_mut(id) {
            tab.close();
        }
        self.tree.remove_recursively(id);
    }

    /// The agent offered to sessions.
    fn session_agent(&self) -> AgentSource {
        match &self.builtin_agent {
            Some(agent) if self.config.settings.use_builtin_agent => {
                AgentSource::Path(agent.path().to_owned())
            }
            _ => self.agent.clone(),
        }
    }

    fn agent_action(&mut self, window: &mut AgentWindow, action: AgentAction) {
        window.error = None;
        let result: Result<(), String> = match action {
            AgentAction::Start => self
                .runtime
                .block_on(rooste_keys::Agent::start(None))
                .map(|agent| self.builtin_agent = Some(agent))
                .map_err(|e| format!("Could not start the agent: {e}")),
            AgentAction::Stop => {
                self.builtin_agent = None;
                window.keys.clear();
                Ok(())
            }
            AgentAction::Load => (|| {
                let agent = self
                    .builtin_agent
                    .as_ref()
                    .ok_or("The agent is not running.")?;
                let path = rooste_core::expand_tilde(window.key_file.trim());
                let text = std::fs::read_to_string(&path)
                    .map_err(|e| format!("{}: {e}", path.display()))?;
                let passphrase =
                    (!window.passphrase.is_empty()).then_some(window.passphrase.as_str());
                let key = rooste_keys::load(&text, passphrase).map_err(|e| e.to_string())?;
                self.runtime
                    .block_on(agent.add(&key))
                    .map_err(|e| e.to_string())?;
                window.key_file.clear();
                window.passphrase.clear();
                Ok(())
            })(),
            AgentAction::Remove(fingerprint) => (|| {
                let agent = self
                    .builtin_agent
                    .as_ref()
                    .ok_or("The agent is not running.")?;
                let key = window
                    .keys
                    .iter()
                    .find(|k| rooste_keys::fingerprint(k) == fingerprint)
                    .ok_or("No such key.")?;
                self.runtime
                    .block_on(agent.remove(key))
                    .map_err(|e| e.to_string())
            })(),
            AgentAction::Refresh => Ok(()),
        };
        if let Err(e) = result {
            window.error = Some(e);
        }
        if let Some(agent) = &self.builtin_agent {
            match self.runtime.block_on(agent.keys()) {
                Ok(keys) => window.keys = keys,
                Err(e) => window.error = Some(e.to_string()),
            }
        }
    }

    /// A new tab connecting like the active one.
    pub fn duplicate_active(&mut self) {
        if let Some(id) = self.active_tab_id() {
            self.duplicate_tab(id);
        }
    }

    /// A new tab connecting like `id`, in the same tab group.
    fn duplicate_tab(&mut self, id: TileId) {
        let settings = self.config.settings.clone();
        if let Some(tab) = self
            .tree
            .tiles
            .get_pane(&id)
            .map(|tab| tab.duplicate(&settings))
        {
            self.add_tab_beside(tab, Some(id));
        }
    }

    /// Open a copy of the active tab beside it.
    pub fn split_active(&mut self, dir: LinearDir) {
        if let Some(id) = self.active_tab_id() {
            self.split_tab(id, dir);
        }
    }

    /// Open a copy of tab `active` beside its tab group.
    fn split_tab(&mut self, active: TileId, dir: LinearDir) {
        let settings = self.config.settings.clone();
        let Some(tab) = self
            .tree
            .tiles
            .get_pane(&active)
            .map(|t| t.duplicate(&settings))
        else {
            return;
        };
        // Split the tab group holding the active tab.
        let group = self.tree.tiles.parent_of(active).unwrap_or(active);
        let pane = self.tree.tiles.insert_pane(tab);
        let new_group = self.tree.tiles.insert_tab_tile(vec![pane]);
        let split = match dir {
            LinearDir::Horizontal => self
                .tree
                .tiles
                .insert_horizontal_tile(vec![group, new_group]),
            LinearDir::Vertical => self.tree.tiles.insert_vertical_tile(vec![group, new_group]),
        };
        match self.tree.tiles.parent_of(group).filter(|&p| p != split) {
            Some(parent) => {
                if let Some(Tile::Container(container)) = self.tree.tiles.get_mut(parent) {
                    replace_child(container, group, split);
                }
            }
            None => self.tree.root = Some(split),
        }
    }

    fn tab_menu_action(&mut self, id: TileId, action: TabMenuAction) {
        match action {
            TabMenuAction::Duplicate => self.duplicate_tab(id),
            TabMenuAction::SplitRight => self.split_tab(id, LinearDir::Horizontal),
            TabMenuAction::SplitDown => self.split_tab(id, LinearDir::Vertical),
            TabMenuAction::Files => {
                if let Some(tab) = self.tab_mut(id) {
                    tab.toggle_files();
                }
            }
            TabMenuAction::Reconnect => {
                if let Some(tab) = self.tab_mut(id) {
                    tab.reconnect();
                }
            }
            TabMenuAction::Close => self.request_close(id),
        }
    }

    fn toggle_broadcast(&mut self) {
        self.broadcast = !self.broadcast;
        let count = self
            .tabs()
            .filter(|t| t.state == TabState::Connected)
            .count();
        if self.broadcast {
            self.notify(
                format!("Broadcasting input to {count} connected tabs."),
                false,
            );
        } else {
            self.notify("Stopped broadcasting input.", false);
        }
    }

    fn run_command(&mut self, ctx: &egui::Context, command: Command) {
        match command {
            Command::Connect(id) => self.connect_profile(ctx, &id),
            Command::NewSession => self.handle_manager(ctx, ManagerAction::New),
            Command::QuickConnect => self.dialog = Some(Dialog::QuickConnect),
            Command::ImportPutty => self.import(true),
            Command::ImportSshConfig => self.import(false),
            Command::Settings => {
                self.dialog = Some(Dialog::Settings(SettingsEditor {
                    settings: self.config.settings.clone(),
                }))
            }
            Command::DuplicateTab => self.duplicate_active(),
            Command::ToggleFiles => {
                if let Some(id) = self.active_tab_id()
                    && let Some(tab) = self.tab_mut(id)
                {
                    tab.toggle_files();
                }
            }
            Command::SplitRight => self.split_active(LinearDir::Horizontal),
            Command::SplitDown => self.split_active(LinearDir::Vertical),
            Command::ToggleBroadcast => self.toggle_broadcast(),
            Command::ToggleSessionManager => self.show_manager = !self.show_manager,
            Command::CloseTab => {
                if let Some(id) = self.active_tab_id() {
                    self.request_close(id);
                }
            }
        }
    }

    fn import(&mut self, from_putty: bool) {
        let (source, result) = if from_putty {
            let result = match &self.imports.putty_dir {
                Some(dir) => putty::import_dir(dir),
                None => putty::import_default(),
            };
            ("PuTTY", result)
        } else {
            let result = match &self.imports.ssh_config {
                Some(path) => ssh_config::import_file(path),
                None => ssh_config::import_default(),
            };
            ("~/.ssh/config", result)
        };
        match result {
            Ok(profiles) => {
                let found = profiles.len();
                let added = self.config.merge_imported(profiles);
                self.save_config();
                let skipped = found - added;
                let text = match (added, skipped) {
                    (0, 0) => format!("No sessions found in {source}."),
                    (added, 0) => format!("Imported {added} sessions from {source}."),
                    (added, skipped) => format!(
                        "Imported {added} sessions from {source} ({skipped} already existed)."
                    ),
                };
                self.notify(text, false);
            }
            Err(e) => self.notify(format!("Could not import from {source}: {e}"), true),
        }
    }

    fn apply_settings(&mut self, settings: Settings) {
        self.config.settings = settings;
        self.save_config();
        let settings = self.config.settings.clone();
        for (_, tile) in self.tree.tiles.iter_mut() {
            if let Tile::Pane(tab) = tile {
                tab.apply_settings(&settings);
            }
        }
    }

    fn handle_manager(&mut self, ctx: &egui::Context, action: ManagerAction) {
        match action {
            ManagerAction::New => {
                let profile = Profile {
                    user: self.config.settings.default_user.clone(),
                    ..Profile::default()
                };
                self.serial_ports = rooste_proto::serial::available_ports();
                self.dialog = Some(Dialog::EditProfile(Box::new(ProfileEditor::new(
                    profile, true,
                ))));
            }
            ManagerAction::Connect(id) => self.connect_profile(ctx, &id),
            ManagerAction::Edit(id) => {
                if let Some(profile) = self.config.profile(&id) {
                    self.serial_ports = rooste_proto::serial::available_ports();
                    self.dialog = Some(Dialog::EditProfile(Box::new(ProfileEditor::new(
                        profile.clone(),
                        false,
                    ))));
                }
            }
            ManagerAction::Duplicate(id) => {
                if let Some(copy) = self.config.profile(&id).map(Profile::duplicate) {
                    self.manager.selected = Some(copy.id.clone());
                    self.config.upsert_profile(copy);
                    self.save_config();
                }
            }
            ManagerAction::Delete(id) => self.dialog = Some(Dialog::DeleteProfile(id)),
        }
    }

    /// Draw one frame.
    pub fn show(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        self.picker.poll();
        if self.applied_theme != Some(self.config.settings.theme) {
            ctx.set_visuals(theme::visuals(self.config.settings.theme));
            ctx.all_styles_mut(theme::spacing);
            self.applied_theme = Some(self.config.settings.theme);
        }

        if let Some(rx) = &self.update_check
            && let Ok(result) = rx.try_recv()
        {
            self.update_check = None;
            if let Some(release) = result
                && self.notice.is_none()
            {
                self.notice = Some(Notice {
                    text: format!("Rooste SSH {} is available.", release.version),
                    error: false,
                    link: Some(("Download".into(), release.url)),
                });
            }
        }

        // Session events and tab actions.
        for id in self.tab_ids() {
            let actions = match self.tab_mut(id) {
                Some(tab) => {
                    tab.poll(&ctx);
                    tab.take_actions()
                }
                None => continue,
            };
            for action in actions {
                match action {
                    TabAction::EnableLegacy { profile_id } => {
                        if let Some(profile) = profile_id
                            .and_then(|id| self.config.profiles.iter_mut().find(|p| p.id == id))
                        {
                            profile.allow_legacy = true;
                            self.save_config();
                        }
                    }
                    TabAction::SavePassword {
                        profile_id,
                        password,
                    } => {
                        if let Err(e) = self.secrets.set(&profile_id, &password) {
                            self.notify(e, true);
                        }
                    }
                }
            }
        }

        self.menu_bar(ui);
        self.status_bar(ui);
        if self.show_manager {
            let mut action = None;
            egui::Panel::left("session-manager")
                .frame(theme::side_panel_frame(&ctx))
                .resizable(true)
                .default_size(240.0)
                .show(ui, |ui| {
                    action = manager::show(ui, &mut self.manager, &self.config.profiles);
                });
            if let Some(action) = action {
                self.handle_manager(&ctx, action);
            }
        }

        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(ui, |ui| {
                if self.tree.is_empty() {
                    self.welcome(ui);
                } else {
                    let mut behavior = TabBehavior {
                        close_requests: Vec::new(),
                        menu_actions: Vec::new(),
                    };
                    let broadcast = self.broadcast;
                    for (_, tile) in self.tree.tiles.iter_mut() {
                        if let Tile::Pane(tab) = tile {
                            tab.broadcast = broadcast && tab.state == TabState::Connected;
                        }
                    }
                    self.tree.ui(&mut behavior, ui);
                    self.deliver_broadcast();
                    for id in behavior.close_requests {
                        self.request_close(id);
                    }
                    for (id, action) in behavior.menu_actions {
                        self.tab_menu_action(id, action);
                    }
                }
            });

        self.dialogs(&ctx);
        self.shortcuts(&ctx);
        self.update_window_title(&ctx);
    }

    /// Send input typed in broadcasting tabs to every connected tab.
    fn deliver_broadcast(&mut self) {
        let mut input = Vec::new();
        for (_, tile) in self.tree.tiles.iter_mut() {
            if let Tile::Pane(tab) = tile {
                input.append(&mut tab.outbox);
            }
        }
        if input.is_empty() {
            return;
        }
        for (_, tile) in self.tree.tiles.iter_mut() {
            if let Tile::Pane(tab) = tile {
                tab.send_input(input.clone());
            }
        }
    }

    fn menu_bar(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        egui::Panel::top("menu").show(ui, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                ui.menu_button("Session", |ui| {
                    if ui.button("New session…").clicked() {
                        self.handle_manager(&ctx, ManagerAction::New);
                        ui.close();
                    }
                    if ui.button("Quick connect…").clicked() {
                        self.dialog = Some(Dialog::QuickConnect);
                        ui.close();
                    }
                    ui.separator();
                    if ui.button("Import from PuTTY").clicked() {
                        self.import(true);
                        ui.close();
                    }
                    if ui.button("Import from ~/.ssh/config").clicked() {
                        self.import(false);
                        ui.close();
                    }
                    ui.separator();
                    if ui.button("Settings…").clicked() {
                        self.dialog = Some(Dialog::Settings(SettingsEditor {
                            settings: self.config.settings.clone(),
                        }));
                        ui.close();
                    }
                });
                ui.menu_button("Tab", |ui| {
                    let has_tab = self.active_tab_id().is_some();
                    for (label, command) in [
                        ("Files (SFTP)", Command::ToggleFiles),
                        ("Duplicate tab", Command::DuplicateTab),
                        ("Split right", Command::SplitRight),
                        ("Split down", Command::SplitDown),
                    ] {
                        if ui.add_enabled(has_tab, egui::Button::new(label)).clicked() {
                            self.run_command(&ctx, command);
                            ui.close();
                        }
                    }
                    if ui
                        .checkbox(&mut self.broadcast, "Broadcast input to all tabs")
                        .clicked()
                    {
                        self.broadcast = !self.broadcast;
                        self.toggle_broadcast();
                        ui.close();
                    }
                    ui.separator();
                    if ui
                        .add_enabled(has_tab, egui::Button::new("Close tab"))
                        .clicked()
                    {
                        self.run_command(&ctx, Command::CloseTab);
                        ui.close();
                    }
                });
                ui.menu_button("Tools", |ui| {
                    if ui.button("Key generator…").clicked() {
                        self.dialog = Some(Dialog::KeyGenerator(Box::default()));
                        ui.close();
                    }
                    if ui.button("Convert key (PuTTY / OpenSSH)…").clicked() {
                        self.dialog = Some(Dialog::KeyConverter(Box::default()));
                        ui.close();
                    }
                    if ui.button("SSH agent…").clicked() {
                        let mut window = AgentWindow::default();
                        self.agent_action(&mut window, AgentAction::Refresh);
                        self.dialog = Some(Dialog::Agent(Box::new(window)));
                        ui.close();
                    }
                });
                ui.menu_button("View", |ui| {
                    if ui
                        .checkbox(&mut self.show_manager, "Session manager")
                        .clicked()
                    {
                        ui.close();
                    }
                    if ui.button("Command palette…").clicked() {
                        self.dialog = Some(Dialog::Palette {
                            query: String::new(),
                        });
                        ui.close();
                    }
                });
            });
        });
    }

    fn status_bar(&mut self, ui: &mut Ui) {
        let Some(notice) = self.notice.clone() else {
            return;
        };
        egui::Panel::bottom("status").show(ui, |ui| {
            ui.horizontal(|ui| {
                if notice.error {
                    ui.colored_label(ui.visuals().error_fg_color, &notice.text);
                } else {
                    ui.label(&notice.text);
                }
                if let Some((label, url)) = &notice.link {
                    ui.hyperlink_to(label, url);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button("✕").on_hover_text("Dismiss").clicked() {
                        self.notice = None;
                    }
                });
            });
        });
    }

    fn welcome(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.add_space(48.0);
            ui.vertical_centered(|ui| {
                ui.heading("Rooste SSH");
                ui.add_space(4.0);
                ui.label(RichText::new("Double-click a saved session, or connect quickly:").weak());
            });
            ui.add_space(24.0);
            // Center the form, with at least a small gap from the panel edge.
            let indent = ((ui.available_width() - 420.0) / 2.0).max(16.0);
            ui.horizontal(|ui| {
                ui.add_space(indent);
                ui.vertical(|ui| {
                    if self.quick_form(ui) {
                        self.quick_connect(&ctx);
                    }
                });
            });
        });
    }

    /// The quick-connect fields. Returns true when the user submits.
    fn quick_form(&mut self, ui: &mut Ui) -> bool {
        let mut submit = false;
        egui::Grid::new("quick-connect")
            .num_columns(2)
            .spacing(crate::theme::FORM_SPACING)
            .show(ui, |ui| {
                let mut field = |ui: &mut Ui, label: &str, value: &mut String, password: bool| {
                    let label = ui.label(label);
                    let edit = egui::TextEdit::singleline(value)
                        .margin(crate::theme::FIELD_MARGIN)
                        .password(password)
                        .desired_width(crate::theme::FIELD_WIDTH);
                    let response = ui.add(edit).labelled_by(label.id);
                    submit |=
                        response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    ui.end_row();
                };
                field(ui, "Host", &mut self.quick.host, false);
                field(ui, "Port", &mut self.quick.port, false);
                field(ui, "User", &mut self.quick.user, false);
                field(ui, "Password", &mut self.quick.password, true);
                let key_file = path_row(
                    ui,
                    "Key file",
                    &mut self.quick.key_file,
                    &mut self.picker,
                    PickId::QuickKeyFile,
                    PickKind::OpenFile,
                );
                submit |= key_file.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                ui.label("");
                ui.checkbox(&mut self.quick.use_agent, "Use SSH agent");
                ui.end_row();
                ui.label("");
                ui.checkbox(&mut self.quick.allow_legacy, "Allow legacy algorithms")
                    .on_hover_text("Offer outdated SHA-1 and CBC algorithms for old devices");
                ui.end_row();
            });
        if let Some(error) = &self.quick_error {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
        submit | ui.button("Connect").clicked()
    }

    fn dialogs(&mut self, ctx: &egui::Context) {
        // Host keys and prompts from sessions take priority.
        for id in self.tab_ids() {
            let Some(tab) = self.tab_mut(id) else {
                continue;
            };
            if let Some(key) = tab.pending_host_key().cloned() {
                match dialogs::host_key(ctx, &key) {
                    Decision::Accept(()) => tab.answer_host_key(true),
                    Decision::Cancel => tab.answer_host_key(false),
                    Decision::Pending => {}
                }
                return;
            }
            if let Some(prompt) = tab.prompt.as_mut() {
                let decision = dialogs::prompt(ctx, &prompt.kind, prompt.retry, &mut prompt.input);
                match decision {
                    Decision::Accept(secret) => tab.answer_prompt(Some(secret)),
                    Decision::Cancel => tab.answer_prompt(None),
                    Decision::Pending => {}
                }
                return;
            }
        }

        let Some(mut dialog) = self.dialog.take() else {
            return;
        };
        let keep = match &mut dialog {
            Dialog::EditProfile(editor) => {
                match editor.show(
                    ctx,
                    &self.config.folders(),
                    &self.serial_ports,
                    &mut self.picker,
                ) {
                    Decision::Pending => true,
                    Decision::Cancel => false,
                    Decision::Accept((profile, password)) => {
                        if profile.save_password && !password.is_empty() {
                            if let Err(e) = self.secrets.set(&profile.id, &password) {
                                self.notify(e, true);
                            }
                        } else if !profile.save_password {
                            self.secrets.delete(&profile.id);
                        }
                        self.manager.selected = Some(profile.id.clone());
                        self.config.upsert_profile(profile);
                        self.save_config();
                        false
                    }
                }
            }
            Dialog::Settings(editor) => match editor.show(ctx, &mut self.picker) {
                Decision::Pending => true,
                Decision::Cancel => false,
                Decision::Accept(settings) => {
                    self.apply_settings(settings);
                    false
                }
            },
            Dialog::DeleteProfile(id) => {
                let name = self
                    .config
                    .profile(id)
                    .map(Profile::display_name)
                    .unwrap_or_default();
                match dialogs::confirm(
                    ctx,
                    "delete-profile",
                    "Delete session",
                    &format!("Delete the saved session \"{name}\"?"),
                    "Delete",
                ) {
                    Decision::Pending => true,
                    Decision::Cancel => false,
                    Decision::Accept(()) => {
                        self.secrets.delete(id);
                        self.config.remove_profile(id);
                        self.save_config();
                        false
                    }
                }
            }
            Dialog::CloseTab(id) => {
                let id = *id;
                match dialogs::confirm(
                    ctx,
                    "close-tab",
                    "Close tab",
                    "This tab has an active connection. Close it?",
                    "Close",
                ) {
                    Decision::Pending => true,
                    Decision::Cancel => false,
                    Decision::Accept(()) => {
                        self.close_tab(id);
                        false
                    }
                }
            }
            Dialog::Palette { query } => {
                let commands = command_palette::commands(
                    &self.config.profiles,
                    self.active_tab_id().is_some(),
                );
                match command_palette::show(ctx, query, &commands) {
                    Decision::Pending => true,
                    Decision::Cancel => false,
                    Decision::Accept(command) => {
                        self.run_command(ctx, command);
                        false
                    }
                }
            }
            Dialog::KeyGenerator(generator) => {
                generator.show(ctx, &mut self.picker) == Decision::Pending
            }
            Dialog::KeyConverter(converter) => {
                converter.show(ctx, &mut self.picker) == Decision::Pending
            }
            Dialog::Agent(window) => {
                let mut use_agent = self.config.settings.use_builtin_agent;
                let (decision, action) = window.show(
                    ctx,
                    self.builtin_agent.as_ref(),
                    &mut use_agent,
                    &mut self.picker,
                );
                if use_agent != self.config.settings.use_builtin_agent {
                    self.config.settings.use_builtin_agent = use_agent;
                    self.save_config();
                }
                if let Some(action) = action {
                    self.agent_action(window, action);
                }
                decision == Decision::Pending
            }
            Dialog::QuickConnect => {
                let mut keep = true;
                egui::Modal::new(egui::Id::new("quick-connect-dialog"))
                    .frame(crate::theme::dialog_frame(ctx))
                    .show(ctx, |ui| {
                        ui.heading("Quick connect");
                        ui.add_space(10.0);
                        if self.quick_form(ui) && self.quick_connect(ctx) {
                            keep = false;
                        }
                        if ui.button("Cancel").clicked() {
                            keep = false;
                        }
                    });
                keep
            }
        };
        if keep && self.dialog.is_none() {
            self.dialog = Some(dialog);
        }
    }

    fn shortcuts(&mut self, ctx: &egui::Context) {
        use egui::{Key, KeyboardShortcut, Modifiers};
        let ctrl_shift = Modifiers::CTRL | Modifiers::SHIFT;
        if ctx.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(ctrl_shift, Key::T))) {
            self.dialog = Some(Dialog::QuickConnect);
        }
        if ctx.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(ctrl_shift, Key::P))) {
            self.dialog = Some(Dialog::Palette {
                query: String::new(),
            });
        }
        if ctx.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(ctrl_shift, Key::W)))
            && let Some(id) = self.active_tab_id()
        {
            self.request_close(id);
        }
    }

    fn update_window_title(&mut self, ctx: &egui::Context) {
        let title = match self
            .active_tab_id()
            .and_then(|id| self.tree.tiles.get_pane(&id))
        {
            Some(tab) => format!(
                "{} — Rooste SSH",
                tab.remote_title.clone().unwrap_or_else(|| tab.name.clone())
            ),
            None => "Rooste SSH".into(),
        };
        if title != self.window_title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.window_title = title;
        }
    }
}

/// Replace `old` with `new` among a container's children.
fn replace_child(container: &mut Container, old: TileId, new: TileId) {
    let children = match container {
        Container::Tabs(tabs) => {
            if tabs.active == Some(old) {
                tabs.active = Some(new);
            }
            &mut tabs.children
        }
        Container::Linear(linear) => &mut linear.children,
        Container::Grid(_) => return,
    };
    for child in children.iter_mut() {
        if *child == old {
            *child = new;
        }
    }
}

impl Default for RoosteApp {
    fn default() -> Self {
        Self::new()
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
    fn quick_form_validation() {
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
        assert!(request.prompt_for_password);
    }
}
