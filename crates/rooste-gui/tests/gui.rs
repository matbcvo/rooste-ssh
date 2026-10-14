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

fn type_into(harness: &mut Harness<'_, RoosteApp>, label: &str, text: &str) {
    last(harness, label).focus();
    harness.run();
    last(harness, label).type_text(text);
    harness.run();
}

fn open_menu_item(harness: &mut Harness<'_, RoosteApp>, menu: &str, item: &str) {
    harness.get_by_label(menu).click();
    harness.run();
    harness.get_by_label(item).click();
    harness.run();
}

fn context_menu(harness: &mut Harness<'_, RoosteApp>, profile: &str, item: &str) {
    harness.get_by_label(profile).click_secondary();
    harness.run();
    // The welcome screen also has a "Connect" button; the menu item is last.
    last(harness, item).click();
    harness.run();
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
    harness.run();
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
    harness.run();
    harness.get_by_label("New session");

    harness.get_by_label("Save").click();
    harness.run();
    harness.get_by_label("Enter a host name.");

    type_into(&mut harness, "Name", "My server");
    type_into(&mut harness, "Host", "server.example.com");
    type_into(&mut harness, "Port", "2200");
    type_into(&mut harness, "Folder", "Work/Prod");
    harness.get_by_label("Save").click();
    harness.run();

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
    harness.run();
    harness.get_by_label("router (copy) 2");

    context_menu(&mut harness, "router (copy) 2", "Delete…");
    harness.get_by_label("Delete the saved session \"router (copy) 2\"?");
    harness.get_by_label("Delete").click();
    harness.run();
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
    harness.run();
    harness.get_by_label("Copy text when selected").click();
    harness.run();
    harness.get_by_label("Save").click();
    harness.run();

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
    harness.run();
    harness.get_by_label("This tab has an active connection. Close it?");
    last(&harness, "Close").click();
    harness.run();
    assert_eq!(harness.state().tabs().count(), 1);
}
