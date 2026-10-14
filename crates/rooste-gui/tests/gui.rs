//! GUI tests with egui_kittest, against the in-process SSH server.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use rooste_core::{Config, MemoryStore, Profile, Theme};
use rooste_gui::RoosteApp;
use rooste_gui::app::{Dialog, TabState};
use rooste_gui::tab::SessionTab;
use rooste_proto::ssh::known_hosts;
use rooste_testkit::{TEST_PASSWORD, TEST_USER, TestServer};
use tempfile::TempDir;

struct Fixture {
    dir: TempDir,
    net: tokio::runtime::Runtime,
    server: Option<TestServer>,
}

impl Fixture {
    fn new() -> Self {
        Self {
            dir: tempfile::tempdir().unwrap(),
            net: tokio::runtime::Runtime::new().unwrap(),
            server: None,
        }
    }

    /// Also start an SSH server whose host key is already trusted.
    fn with_server() -> Self {
        let mut f = Self::new();
        let server = f.net.block_on(TestServer::start());
        known_hosts::learn(
            &f.known_hosts(),
            "127.0.0.1",
            server.addr.port(),
            &server.host_key,
        )
        .unwrap();
        f.server = Some(server);
        f
    }

    fn start_untrusted_server(&self) -> TestServer {
        self.net.block_on(TestServer::start())
    }

    fn config_path(&self) -> PathBuf {
        self.dir.path().join("config.toml")
    }

    fn known_hosts(&self) -> PathBuf {
        self.dir.path().join("known_hosts")
    }

    fn app(&self, config: Config) -> RoosteApp {
        RoosteApp::with_config(
            config,
            Some(self.config_path()),
            Arc::new(MemoryStore::default()),
        )
        .with_known_hosts(self.known_hosts())
    }

    fn saved_config(&self) -> Config {
        Config::load(&self.config_path()).unwrap()
    }

    fn profile(&self, name: &str) -> Profile {
        Profile {
            name: name.into(),
            host: "127.0.0.1".into(),
            port: self.server.as_ref().map(|s| s.addr.port()),
            user: TEST_USER.into(),
            use_agent: false,
            ..Profile::default()
        }
    }
}

/// Render a few frames. (`Harness::run` waits for the UI to go idle, which
/// never happens while a session streams output.)
fn settle(harness: &mut Harness<'_, RoosteApp>) {
    for _ in 0..4 {
        harness.step();
    }
}

fn harness(app: RoosteApp) -> Harness<'static, RoosteApp> {
    Harness::builder()
        .with_size([1100.0, 700.0])
        .build_ui_state(|ui, app: &mut RoosteApp| app.show(ui), app)
}

fn first_tab(app: &RoosteApp) -> Option<&SessionTab> {
    app.tabs().next()
}

fn screen_text(app: &RoosteApp) -> String {
    first_tab(app)
        .map(|t| t.terminal.screen_text())
        .unwrap_or_default()
}

/// Step the UI until `done` holds, failing after a timeout.
fn wait_for(harness: &mut Harness<'_, RoosteApp>, what: &str, done: impl Fn(&RoosteApp) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !done(harness.state()) {
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {what}; screen:\n{}\napp: {:?}",
            screen_text(harness.state()),
            harness.state()
        );
        harness.step();
        std::thread::sleep(Duration::from_millis(10));
    }
    harness.step();
}

/// The last widget with `label`: dialogs are drawn after (and on top of)
/// the window, which may have widgets with the same label.
fn last<'a>(harness: &'a Harness<'_, RoosteApp>, label: &'a str) -> egui_kittest::Node<'a> {
    harness
        .query_all_by_label(label)
        .last()
        .unwrap_or_else(|| panic!("no widget labeled {label:?}"))
}

/// Move the pointer onto a widget, then click it, as a mouse user would.
/// (A click without a preceding hover can be ignored by egui.)
fn click(harness: &mut Harness<'_, RoosteApp>, label: &str) {
    last(harness, label).hover();
    settle(harness);
    last(harness, label).click();
    settle(harness);
}

fn type_into(harness: &mut Harness<'_, RoosteApp>, label: &str, text: &str) {
    click(harness, label);
    last(harness, label).type_text(text);
    settle(harness);
}

fn open_menu_item(harness: &mut Harness<'_, RoosteApp>, menu: &str, item: &str) {
    harness.get_by_label(menu).click();
    settle(harness);
    harness.get_by_label(item).click();
    settle(harness);
}

fn context_menu(harness: &mut Harness<'_, RoosteApp>, profile: &str, item: &str) {
    harness.get_by_label(profile).hover();
    settle(harness);
    harness.get_by_label(profile).click_secondary();
    settle(harness);
    // The welcome screen also has a "Connect" button; the menu item is last.
    last(harness, item).click();
    settle(harness);
}

fn write(path: &Path, text: &str) {
    std::fs::write(path, text).unwrap();
}

// ---- Quick connect ----------------------------------------------------

#[test]
fn quick_connect_requires_a_host() {
    let f = Fixture::new();
    let mut harness = harness(f.app(Config::default()));
    harness.get_by_label("Connect").click();
    settle(&mut harness);
    harness.get_by_label("Enter a host name.");
    assert_eq!(harness.state().tabs().count(), 0);
}

#[test]
fn quick_connect_trust_host_key_type_and_exit() {
    let f = Fixture::new();
    let server = f.start_untrusted_server();

    let mut harness = harness(f.app(Config::default()));
    type_into(&mut harness, "Host", "127.0.0.1");
    {
        let quick = &mut harness.state_mut().quick;
        quick.port = server.addr.port().to_string();
        quick.user = TEST_USER.into();
        quick.password = TEST_PASSWORD.into();
        quick.use_agent = false;
    }
    assert_eq!(
        harness.state().quick.host,
        "127.0.0.1",
        "typed into the Host field"
    );
    harness.get_by_label("Connect").click();

    wait_for(&mut harness, "host key prompt", |app| {
        first_tab(app).is_some_and(|t| t.pending_host_key().is_some())
    });
    harness.get_by_label("Trust and connect").click();

    wait_for(&mut harness, "shell prompt", |app| {
        first_tab(app).is_some_and(|t| t.state == TabState::Connected)
            && screen_text(app).contains('$')
    });
    assert!(f.known_hosts().exists(), "trusted key was recorded");

    // The fake shell reports the PTY size it was told about: it must match
    // the grid the widget laid out, not the initial 80x24.
    let (cols, rows) = first_tab(harness.state()).unwrap().terminal.size();
    assert_ne!((cols, rows), (80, 24));
    harness.event(egui::Event::Text("size".into()));
    harness.key_press(egui::Key::Enter);
    let expected = format!("{cols}x{rows}");
    wait_for(&mut harness, "size reply", |app| {
        screen_text(app).contains(&expected)
    });

    harness.event(egui::Event::Text("exit 4".into()));
    harness.key_press(egui::Key::Enter);
    wait_for(&mut harness, "session end", |app| {
        first_tab(app).is_some_and(|t| t.state == TabState::Closed(Some(4)))
    });
    harness.get_by_label("Session closed (exit status 4)");
    harness.get_by_label("Reconnect");
}

#[test]
fn rejected_host_key_fails_the_connection() {
    let f = Fixture::new();
    let server = f.start_untrusted_server();

    let mut harness = harness(f.app(Config::default()));
    {
        let quick = &mut harness.state_mut().quick;
        quick.host = "127.0.0.1".into();
        quick.port = server.addr.port().to_string();
        quick.user = TEST_USER.into();
        quick.password = TEST_PASSWORD.into();
        quick.use_agent = false;
    }
    harness.get_by_label("Connect").click();
    wait_for(&mut harness, "host key prompt", |app| {
        first_tab(app).is_some_and(|t| t.pending_host_key().is_some())
    });
    harness.get_by_label("Cancel").click();
    wait_for(&mut harness, "failure", |app| {
        matches!(first_tab(app).map(|t| &t.state), Some(TabState::Failed(_)))
    });
    let Some(TabState::Failed(message)) = first_tab(harness.state()).map(|t| t.state.clone())
    else {
        unreachable!()
    };
    assert!(message.contains("not trusted"), "{message}");
}

// ---- Session manager ---------------------------------------------------

#[test]
fn create_a_profile_in_the_editor() {
    let f = Fixture::new();
    let mut harness = harness(f.app(Config::default()));
    harness.get_by_label("➕").click();
    settle(&mut harness);
    harness.get_by_label("New session");

    harness.get_by_label("Save").click();
    settle(&mut harness);
    harness.get_by_label("Enter a host name.");

    type_into(&mut harness, "Name", "My server");
    type_into(&mut harness, "Host", "server.example.com");
    type_into(&mut harness, "Port", "2200");
    type_into(&mut harness, "Folder", "Work/Prod");
    harness.get_by_label("Save").click();
    settle(&mut harness);

    assert!(harness.state().dialog.is_none());
    harness.get_by_label("📁 Work");
    harness.get_by_label("My server");
    let saved = f.saved_config();
    assert_eq!(saved.profiles.len(), 1);
    let p = &saved.profiles[0];
    assert_eq!(
        (p.name.as_str(), p.host.as_str()),
        ("My server", "server.example.com")
    );
    assert_eq!(p.port, Some(2200));
    assert_eq!(p.folder, "Work/Prod");
}

#[test]
fn search_filters_the_list() {
    let f = Fixture::new();
    let config = Config {
        profiles: vec![
            Profile {
                name: "alpha".into(),
                host: "a.example.com".into(),
                tags: vec!["prod".into()],
                ..Profile::default()
            },
            Profile {
                name: "beta".into(),
                host: "b.example.com".into(),
                ..Profile::default()
            },
        ],
        ..Config::default()
    };
    let mut harness = harness(f.app(config));
    harness.get_by_label("alpha");
    harness.get_by_label("beta");
    type_into(&mut harness, "Search", "prod");
    harness.get_by_label("alpha");
    assert!(harness.query_by_label("beta").is_none());
}

#[test]
fn edit_duplicate_and_delete_from_the_context_menu() {
    let f = Fixture::new();
    let config = Config {
        profiles: vec![Profile {
            name: "router".into(),
            host: "10.0.0.1".into(),
            ..Profile::default()
        }],
        ..Config::default()
    };
    let mut harness = harness(f.app(config));

    context_menu(&mut harness, "router", "Duplicate");
    harness.get_by_label("router (copy)");
    assert_eq!(f.saved_config().profiles.len(), 2);

    context_menu(&mut harness, "router (copy)", "Edit…");
    harness.get_by_label("Edit session");
    type_into(&mut harness, "Name", " 2");
    harness.get_by_label("Save").click();
    settle(&mut harness);
    harness.get_by_label("router (copy) 2");

    context_menu(&mut harness, "router (copy) 2", "Delete…");
    harness.get_by_label("Delete the saved session \"router (copy) 2\"?");
    harness.get_by_label("Delete").click();
    settle(&mut harness);
    assert!(harness.query_by_label("router (copy) 2").is_none());
    let saved = f.saved_config();
    assert_eq!(saved.profiles.len(), 1);
    assert_eq!(saved.profiles[0].name, "router");
}

#[test]
fn import_from_ssh_config_and_putty() {
    let f = Fixture::new();
    let ssh_config = f.dir.path().join("ssh_config");
    write(
        &ssh_config,
        "Host web\n  HostName web.example.com\nHost db\n  User admin\n",
    );
    let putty = f.dir.path().join("putty");
    std::fs::create_dir(&putty).unwrap();
    write(
        &putty.join("Switch%201"),
        "HostName=192.168.0.2\nProtocol=telnet\n",
    );

    let app = f
        .app(Config::default())
        .with_import_sources(rooste_gui::app::ImportSources {
            putty_dir: Some(putty),
            ssh_config: Some(ssh_config),
        });
    let mut harness = harness(app);

    open_menu_item(&mut harness, "Session", "Import from ~/.ssh/config");
    harness.get_by_label("Imported 2 sessions from ~/.ssh/config.");
    harness.get_by_label("web");
    harness.get_by_label("db");

    open_menu_item(&mut harness, "Session", "Import from ~/.ssh/config");
    harness.get_by_label("Imported 0 sessions from ~/.ssh/config (2 already existed).");
    assert_eq!(harness.state().config.profiles.len(), 2, "no duplicates");

    open_menu_item(&mut harness, "Session", "Import from PuTTY");
    harness.get_by_label("Imported 1 sessions from PuTTY.");
    harness.get_by_label("Switch 1");
    assert_eq!(f.saved_config().profiles.len(), 3);

    // Telnet arrives in M5b; connecting explains instead of failing oddly.
    context_menu(&mut harness, "Switch 1", "Connect");
    harness.get_by_label("Telnet connections are not supported yet; only SSH is available.");
    assert_eq!(harness.state().tabs().count(), 0);
}

#[test]
fn settings_are_saved_and_applied() {
    let f = Fixture::new();
    let mut harness = harness(f.app(Config::default()));
    open_menu_item(&mut harness, "Session", "Settings…");
    assert!(matches!(harness.state().dialog, Some(Dialog::Settings(_))));
    harness.get_by_label("Light").click();
    settle(&mut harness);
    harness.get_by_label("Copy text when selected").click();
    settle(&mut harness);
    harness.get_by_label("Save").click();
    settle(&mut harness);

    let saved = f.saved_config();
    assert_eq!(saved.settings.theme, Theme::Light);
    assert!(!saved.settings.copy_on_select);
    assert!(
        !harness.ctx.global_style().visuals.dark_mode,
        "light visuals applied"
    );
}

// ---- Connecting saved profiles ------------------------------------------

#[test]
fn password_prompt_and_saving_the_password() {
    let f = Fixture::with_server();
    let profile = Profile {
        save_password: true,
        ..f.profile("test box")
    };
    let id = profile.id.clone();
    let config = Config {
        profiles: vec![profile],
        ..Config::default()
    };
    let mut harness = harness(f.app(config));

    context_menu(&mut harness, "test box", "Connect");
    let label = format!("Password for {TEST_USER}@127.0.0.1");
    wait_for(&mut harness, "password prompt", |app| {
        first_tab(app).is_some_and(|t| t.prompt.is_some())
    });

    // A wrong password is rejected and asked again.
    type_into(&mut harness, &label, "wrong");
    harness.get_by_label("Log in").click();
    wait_for(&mut harness, "retry prompt", |app| {
        first_tab(app).is_some_and(|t| t.prompt.as_ref().is_some_and(|p| p.retry))
    });
    harness.get_by_label("Not accepted, please try again.");

    type_into(&mut harness, &label, TEST_PASSWORD);
    harness.get_by_label("Log in").click();
    wait_for(&mut harness, "connected", |app| {
        first_tab(app).is_some_and(|t| t.state == TabState::Connected)
    });
    assert_eq!(first_tab(harness.state()).unwrap().name, "test box");
    assert_eq!(
        harness.state().secrets().get(&id).as_deref(),
        Some(TEST_PASSWORD),
        "accepted password saved for the profile"
    );
}

#[test]
fn saved_password_is_used_without_prompting() {
    let f = Fixture::with_server();
    let profile = Profile {
        save_password: true,
        ..f.profile("saved")
    };
    let id = profile.id.clone();
    let app = f.app(Config {
        profiles: vec![profile],
        ..Config::default()
    });
    app.secrets().set(&id, TEST_PASSWORD).unwrap();
    let mut harness = harness(app);
    context_menu(&mut harness, "saved", "Connect");
    wait_for(&mut harness, "connected", |app| {
        first_tab(app).is_some_and(|t| t.state == TabState::Connected)
    });
}

#[test]
fn cancelling_the_prompt_fails_the_login() {
    let f = Fixture::with_server();
    let mut harness = harness(f.app(Config {
        profiles: vec![f.profile("p")],
        ..Config::default()
    }));
    context_menu(&mut harness, "p", "Connect");
    wait_for(&mut harness, "password prompt", |app| {
        first_tab(app).is_some_and(|t| t.prompt.is_some())
    });
    harness.get_by_label("Cancel").click();
    wait_for(&mut harness, "failure", |app| {
        first_tab(app).is_some_and(|t| t.state == TabState::Failed("Login cancelled".into()))
    });
}

#[test]
fn tabs_open_side_by_side_and_close_with_confirmation() {
    let f = Fixture::with_server();
    let profiles = vec![
        Profile {
            save_password: true,
            ..f.profile("one")
        },
        Profile {
            save_password: true,
            ..f.profile("two")
        },
    ];
    let app = f.app(Config {
        profiles: profiles.clone(),
        ..Config::default()
    });
    for p in &profiles {
        app.secrets().set(&p.id, TEST_PASSWORD).unwrap();
    }
    let mut harness = harness(app);
    context_menu(&mut harness, "one", "Connect");
    context_menu(&mut harness, "two", "Connect");
    wait_for(&mut harness, "both connected", |app| {
        app.tabs().count() == 2 && app.tabs().all(|t| t.state == TabState::Connected)
    });

    // Ctrl+Shift+W asks before closing an active connection.
    harness.key_press_modifiers(egui::Modifiers::CTRL | egui::Modifiers::SHIFT, egui::Key::W);
    settle(&mut harness);
    harness.get_by_label("This tab has an active connection. Close it?");
    last(&harness, "Close").click();
    settle(&mut harness);
    assert_eq!(harness.state().tabs().count(), 1);
}

// ---- M4: power features --------------------------------------------------

fn connected_count(app: &RoosteApp) -> usize {
    app.tabs()
        .filter(|t| t.state == TabState::Connected)
        .count()
}

/// An app with saved-password profiles for each name, all pointing at the
/// fixture's server.
fn app_with(f: &Fixture, profiles: Vec<Profile>) -> RoosteApp {
    let profiles: Vec<Profile> = profiles
        .into_iter()
        .map(|p| Profile {
            save_password: true,
            ..p
        })
        .collect();
    let app = f.app(Config {
        profiles: profiles.clone(),
        ..Config::default()
    });
    for p in &profiles {
        app.secrets().set(&p.id, TEST_PASSWORD).unwrap();
    }
    app
}

fn tab_text(app: &RoosteApp, index: usize) -> String {
    app.tabs()
        .nth(index)
        .map(|t| t.terminal.screen_text())
        .unwrap_or_default()
}

#[test]
fn split_right_opens_a_second_session_beside_the_first() {
    let f = Fixture::with_server();
    let mut harness = harness(app_with(&f, vec![f.profile("one")]));
    context_menu(&mut harness, "one", "Connect");
    wait_for(&mut harness, "connected", |app| connected_count(app) == 1);

    open_menu_item(&mut harness, "Tab", "Split right");
    wait_for(&mut harness, "both connected", |app| {
        connected_count(app) == 2
    });
    let root = harness.state().tree.root.unwrap();
    assert!(
        matches!(
            harness.state().tree.tiles.get(root),
            Some(egui_tiles::Tile::Container(egui_tiles::Container::Linear(
                _
            )))
        ),
        "the root is now a horizontal split"
    );
}

#[test]
fn broadcast_input_reaches_every_tab() {
    let f = Fixture::with_server();
    let mut harness = harness(app_with(&f, vec![f.profile("one"), f.profile("two")]));
    context_menu(&mut harness, "one", "Connect");
    context_menu(&mut harness, "two", "Connect");
    wait_for(&mut harness, "both connected", |app| {
        connected_count(app) == 2
    });

    open_menu_item(&mut harness, "Tab", "Broadcast input to all tabs");
    harness.get_by_label("Broadcasting input to 2 connected tabs.");
    harness.event(egui::Event::Text("hello all".into()));
    harness.key_press(egui::Key::Enter);
    wait_for(&mut harness, "echo in both tabs", |app| {
        tab_text(app, 0).contains("echo: hello all") && tab_text(app, 1).contains("echo: hello all")
    });
}

#[test]
fn command_palette_connects_a_session() {
    let f = Fixture::with_server();
    let mut harness = harness(app_with(&f, vec![f.profile("palette target")]));
    harness.key_press_modifiers(egui::Modifiers::CTRL | egui::Modifiers::SHIFT, egui::Key::P);
    settle(&mut harness);
    harness.get_by_label("Command").type_text("conn palette");
    settle(&mut harness);
    harness.get_by_label("Connect: palette target");
    harness.key_press(egui::Key::Enter);
    settle(&mut harness);
    wait_for(&mut harness, "connected", |app| connected_count(app) == 1);
    assert!(harness.state().dialog.is_none());
}

#[test]
fn dropped_connections_reconnect_automatically() {
    let f = Fixture::with_server();
    let mut harness = harness(app_with(&f, vec![f.profile("flaky")]));
    context_menu(&mut harness, "flaky", "Connect");
    wait_for(&mut harness, "connected", |app| connected_count(app) == 1);

    f.net
        .block_on(f.server.as_ref().unwrap().drop_connections());
    wait_for(&mut harness, "reconnecting", |app| {
        first_tab(app).is_some_and(|t| matches!(t.state, TabState::Reconnecting { .. }))
    });
    wait_for(&mut harness, "reconnected", |app| {
        connected_count(app) == 1 && screen_text(app).contains("[reconnected]")
    });
}

#[test]
fn legacy_only_server_offers_a_retry_with_legacy_algorithms() {
    use std::borrow::Cow;
    let f = Fixture::new();
    let legacy = russh::Preferred {
        kex: Cow::Borrowed(&[russh::kex::DH_G14_SHA1]),
        ..russh::Preferred::default()
    };
    let server = f.net.block_on(TestServer::start_with(legacy));
    known_hosts::learn(
        &f.known_hosts(),
        "127.0.0.1",
        server.addr.port(),
        &server.host_key,
    )
    .unwrap();
    let profile = Profile {
        port: Some(server.addr.port()),
        ..f.profile("old switch")
    };
    let id = profile.id.clone();
    let mut harness = harness(app_with(&f, vec![profile]));
    context_menu(&mut harness, "old switch", "Connect");
    wait_for(&mut harness, "failure", |app| {
        matches!(first_tab(app).map(|t| &t.state), Some(TabState::Failed(_)))
    });
    harness
        .get_by_label("Allow legacy algorithms and retry")
        .click();
    settle(&mut harness);
    wait_for(&mut harness, "connected with legacy", |app| {
        connected_count(app) == 1
    });
    assert!(first_tab(harness.state()).unwrap().title().contains('⚠'));
    assert!(
        f.saved_config().profile(&id).unwrap().allow_legacy,
        "saved on the profile"
    );
}

#[test]
fn saved_tunnels_start_with_the_session() {
    use std::io::{Read, Write};
    let f = Fixture::with_server();
    let echo = f.net.block_on(rooste_testkit::EchoServer::start());
    let free = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let listen = free.local_addr().unwrap().port();
    drop(free);
    // A port another socket is listening on cannot be bound again.
    let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let taken_port = taken.local_addr().unwrap().port();
    let profile = Profile {
        tunnels: vec![
            rooste_core::Tunnel {
                kind: rooste_core::TunnelKind::Local,
                listen: listen.to_string(),
                target: echo.addr.to_string(),
            },
            rooste_core::Tunnel {
                kind: rooste_core::TunnelKind::Local,
                listen: taken_port.to_string(),
                target: "127.0.0.1:1".into(),
            },
        ],
        ..f.profile("tunnels")
    };
    let mut harness = harness(app_with(&f, vec![profile]));
    context_menu(&mut harness, "tunnels", "Connect");
    wait_for(&mut harness, "tunnel status", |app| {
        first_tab(app).is_some_and(|t| t.tunnels.len() == 2)
    });
    let tunnels = first_tab(harness.state()).unwrap().tunnels.clone();
    assert!(tunnels[0].result.is_ok(), "{tunnels:?}");
    assert!(
        tunnels[1].result.is_err(),
        "port 1 cannot be bound: {tunnels:?}"
    );

    let mut stream = std::net::TcpStream::connect(("127.0.0.1", listen)).unwrap();
    stream.write_all(b"via tunnel").unwrap();
    let mut back = [0u8; 10];
    stream.read_exact(&mut back).unwrap();
    assert_eq!(&back, b"via tunnel");
}

#[test]
fn jump_host_and_agent_forwarding() {
    let f = Fixture::with_server();
    let target = f.net.block_on(TestServer::start());
    known_hosts::learn(
        &f.known_hosts(),
        "127.0.0.1",
        target.addr.port(),
        &target.host_key,
    )
    .unwrap();
    let agent = f.net.block_on(rooste_testkit::TestAgent::start());
    let profile = Profile {
        port: Some(target.addr.port()),
        proxy_jump: Some(format!(
            "{TEST_USER}@127.0.0.1:{}",
            f.server.as_ref().unwrap().addr.port()
        )),
        use_agent: true,
        forward_agent: true,
        ..f.profile("behind bastion")
    };
    let app = f
        .app(Config {
            profiles: vec![profile],
            ..Config::default()
        })
        .with_agent(rooste_proto::ssh::AgentSource::Path(
            agent.path().to_owned(),
        ));
    let mut harness = harness(app);
    context_menu(&mut harness, "behind bastion", "Connect");
    wait_for(&mut harness, "connected through the jump host", |app| {
        connected_count(app) == 1 && screen_text(app).contains('$')
    });
    harness.event(egui::Event::Text("agent-keys".into()));
    harness.key_press(egui::Key::Enter);
    wait_for(&mut harness, "forwarded agent", |app| {
        screen_text(app).contains("agent: 1 keys")
    });
}

#[test]
fn tunnel_editor_validates() {
    let f = Fixture::new();
    let mut harness = harness(f.app(Config::default()));
    harness.get_by_label("➕").click();
    settle(&mut harness);
    type_into(&mut harness, "Host", "h.example.com");
    harness.get_by_label("Add tunnel").click();
    settle(&mut harness);
    type_into(&mut harness, "Listen", "8080");
    type_into(&mut harness, "Target", "nohost");
    harness.get_by_label("Save").click();
    settle(&mut harness);
    assert!(
        harness.state().dialog.is_some(),
        "invalid tunnel keeps the editor open"
    );
    assert!(
        harness
            .query_all_by_label("Tunnel L 8080 → nohost: `nohost` has an invalid port")
            .next()
            .is_some()
    );
    type_into(&mut harness, "Target", ":5432");
    harness.get_by_label("Save").click();
    settle(&mut harness);
    assert!(harness.state().dialog.is_none());
    let saved = f.saved_config();
    assert_eq!(saved.profiles[0].tunnels[0].target, "nohost:5432");
}

// ---- M5: built-in tools ----------------------------------------------------

/// A file dropped onto the window (egui only defines the trait).
#[derive(Debug)]
struct Dropped(PathBuf);

impl egui::DroppedFile for Dropped {
    fn path(&self) -> &Path {
        &self.0
    }

    fn bytes(&self) -> Result<Vec<u8>, String> {
        std::fs::read(&self.0).map_err(|e| e.to_string())
    }
}

fn files_entries(app: &RoosteApp) -> Vec<String> {
    first_tab(app)
        .and_then(|t| t.files.as_ref())
        .map(|f| f.entries.iter().map(|e| e.name.clone()).collect())
        .unwrap_or_default()
}

#[test]
fn file_panel_browse_transfer_and_manage() {
    let f = Fixture::with_server();
    let root = f.server.as_ref().unwrap().sftp_root().to_owned();
    std::fs::create_dir(root.join("logs")).unwrap();
    std::fs::write(root.join("logs").join("app.log"), "log line\n").unwrap();
    std::fs::write(root.join("notes.txt"), "remote notes").unwrap();
    let downloads = f.dir.path().join("downloads");
    std::fs::create_dir(&downloads).unwrap();
    let mut config = Config::default();
    config.settings.download_dir = downloads.display().to_string();
    config.profiles = vec![Profile {
        save_password: true,
        ..f.profile("files")
    }];
    let id = config.profiles[0].id.clone();
    let app = f.app(config);
    app.secrets().set(&id, TEST_PASSWORD).unwrap();
    let mut harness = harness(app);
    context_menu(&mut harness, "files", "Connect");
    wait_for(&mut harness, "connected", |app| connected_count(app) == 1);

    open_menu_item(&mut harness, "Tab", "Files (SFTP)");
    wait_for(&mut harness, "listing", |app| {
        files_entries(app) == ["logs", "notes.txt"]
    });

    // Select a file and download it with the toolbar button.
    click(&mut harness, "📄 notes.txt  (12 B)");
    click(&mut harness, "Download");
    wait_for(&mut harness, "download", |_| {
        downloads.join("notes.txt").exists()
    });
    assert_eq!(
        std::fs::read_to_string(downloads.join("notes.txt")).unwrap(),
        "remote notes"
    );

    // Open a folder.
    click(&mut harness, "📁 logs");
    click(&mut harness, "Open");
    settle(&mut harness);
    wait_for(&mut harness, "logs folder", |app| {
        files_entries(app) == ["app.log"]
    });
    click(&mut harness, "⬆");
    wait_for(&mut harness, "back up", |app| files_entries(app).len() == 2);

    // Upload by dropping a file onto the window.
    let local = f.dir.path().join("upload.txt");
    std::fs::write(&local, "from here").unwrap();
    harness
        .input_mut()
        .dropped_files
        .push(Arc::new(Dropped(local.clone())));
    settle(&mut harness);
    wait_for(&mut harness, "upload", |app| {
        files_entries(app).contains(&"upload.txt".to_owned())
    });
    assert_eq!(
        std::fs::read_to_string(root.join("upload.txt")).unwrap(),
        "from here"
    );

    // New folder, rename and delete.
    click(&mut harness, "New folder");
    type_into(&mut harness, "Folder name", "made-here");
    click(&mut harness, "Create");
    wait_for(&mut harness, "mkdir", |_| root.join("made-here").is_dir());

    click(&mut harness, "📄 upload.txt  (9 B)");
    click(&mut harness, "Rename");
    type_into(&mut harness, "New name", "-2");
    click(&mut harness, "Rename");
    wait_for(&mut harness, "rename", |_| {
        root.join("upload.txt-2").exists()
    });

    click(&mut harness, "📄 upload.txt-2  (9 B)");
    click(&mut harness, "Delete");
    harness.get_by_label("Delete upload.txt-2?");
    click(&mut harness, "Delete");
    wait_for(&mut harness, "delete", |_| {
        !root.join("upload.txt-2").exists()
    });
}

#[test]
fn key_generator_saves_a_key_pair() {
    let f = Fixture::new();
    let mut harness = harness(f.app(Config::default()));
    open_menu_item(&mut harness, "Tools", "Key generator…");
    let path = f.dir.path().join("keys").join("id_test");
    let Some(Dialog::KeyGenerator(generator)) = &mut harness.state_mut().dialog else {
        panic!("key generator not open");
    };
    generator.path.clear();
    generator.comment.clear();
    type_into(&mut harness, "Save as", &path.display().to_string());
    type_into(&mut harness, "Comment", "made by the test");
    type_into(&mut harness, "Passphrase", "secret");
    type_into(&mut harness, "Repeat passphrase", "different");
    harness.get_by_label("Generate").click();
    settle(&mut harness);
    harness.get_by_label("The passphrases do not match.");

    let Some(Dialog::KeyGenerator(generator)) = &mut harness.state_mut().dialog else {
        unreachable!()
    };
    generator.confirm = "secret".into();
    harness.get_by_label("Generate").click();
    settle(&mut harness);
    harness.get_by_label("Copy public key");
    let private = std::fs::read_to_string(&path).unwrap();
    let key = rooste_keys::load(&private, Some("secret")).unwrap();
    let public = std::fs::read_to_string(path.with_file_name("id_test.pub")).unwrap();
    assert!(public.starts_with("ssh-ed25519 ") && public.trim_end().ends_with("made by the test"));
    assert_eq!(
        rooste_keys::PublicKey::from_openssh(public.trim()).unwrap(),
        *key.public_key()
    );
}

#[test]
fn key_converter_turns_ppk_into_openssh() {
    let f = Fixture::new();
    let mut harness = harness(f.app(Config::default()));
    open_menu_item(&mut harness, "Tools", "Convert key (PuTTY / OpenSSH)…");
    let input = rooste_testkit::key_path("ppk").join("v3-ed25519-encrypted.ppk");
    let output = f.dir.path().join("converted");
    type_into(&mut harness, "Key file", &input.display().to_string());
    type_into(
        &mut harness,
        "Passphrase",
        rooste_testkit::TEST_KEY_PASSPHRASE,
    );
    let Some(Dialog::KeyConverter(converter)) = &mut harness.state_mut().dialog else {
        panic!("converter not open");
    };
    converter.output = output.display().to_string();
    harness.get_by_label("Convert").click();
    settle(&mut harness);
    harness.get_by_label(&format!("Saved an OpenSSH key to {}.", output.display()));
    let text = std::fs::read_to_string(&output).unwrap();
    assert!(text.starts_with("-----BEGIN OPENSSH PRIVATE KEY-----"));
    rooste_keys::load(&text, None).unwrap();
}

#[test]
fn builtin_agent_logs_into_sessions() {
    let f = Fixture::with_server();
    let mut harness = harness(f.app(Config {
        profiles: vec![Profile {
            use_agent: true,
            ..f.profile("agent login")
        }],
        ..Config::default()
    }));
    open_menu_item(&mut harness, "Tools", "SSH agent…");
    harness.get_by_label("Start agent").click();
    settle(&mut harness);
    assert!(harness.state().builtin_agent.is_some());
    type_into(
        &mut harness,
        "Key file",
        &rooste_testkit::key_path("id_ed25519").display().to_string(),
    );
    harness.get_by_label("Load key").click();
    settle(&mut harness);
    harness
        .get_by_label("Use this agent for rooste-ssh sessions")
        .click();
    settle(&mut harness);
    let Some(Dialog::Agent(window)) = &harness.state().dialog else {
        panic!("agent window closed");
    };
    assert_eq!(window.keys.len(), 1, "{:?}", window.error);
    harness.get_by_label("Close").click();
    settle(&mut harness);
    assert!(harness.state().config.settings.use_builtin_agent);

    // No password is saved, so only the agent can log in.
    context_menu(&mut harness, "agent login", "Connect");
    wait_for(&mut harness, "connected via the built-in agent", |app| {
        connected_count(app) == 1
    });
    assert!(first_tab(harness.state()).unwrap().prompt.is_none());
}
