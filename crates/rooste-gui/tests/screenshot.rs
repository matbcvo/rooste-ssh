//! Renders the terminal widget to `target/screenshots/` for visual review.
//! Needs a wgpu adapter (a software Vulkan driver is fine), so it is not
//! part of the default test run:
//!
//! ```text
//! cargo test -p rooste-gui --test screenshot -- --ignored
//! ```

use std::path::PathBuf;

use egui_kittest::Harness;
use rooste_gui::terminal_view::TerminalView;
use rooste_terminal::Terminal;

fn demo() -> String {
    let mut s = String::new();
    s.push_str("\x1b[1;32mmartin@server\x1b[0m:\x1b[1;34m~/project\x1b[0m$ ls --color\r\n");
    s.push_str("\x1b[1;34msrc\x1b[0m  \x1b[1;34mtests\x1b[0m  Cargo.toml  \x1b[1;32mbuild.sh\x1b[0m  \x1b[31marchive.tar.gz\x1b[0m\r\n");
    s.push_str("\x1b[1mbold\x1b[0m \x1b[3mitalic\x1b[0m \x1b[4munderline\x1b[0m \x1b[9mstrike\x1b[0m \x1b[7minverse\x1b[0m \x1b[2mdim\x1b[0m\r\n");
    for i in 0..16 {
        s.push_str(&format!("\x1b[48;5;{i}m  "));
    }
    s.push_str("\x1b[0m\r\n");
    for i in 0..48 {
        let v = i * 5;
        s.push_str(&format!("\x1b[48;2;{v};{};{}m ", 120 + i, 255 - v));
    }
    s.push_str("\x1b[0m\r\n");
    s.push_str("╔═══╗ ▁▂▃▄▅▆▇█ ░▒▓ ┏━━┓\r\n║ ║ ║ ▀▄▌▐       ┗━━┛\r\n╚═══╝\r\n");
    s.push_str("┌──────────┬────────┐\r\n│ Unicode  │ õäöü € │\r\n├──────────┼────────┤\r\n│ Wide     │ 日本語 │\r\n└──────────┴────────┘\r\n");
    s.push_str("\x1b[33mwarning:\x1b[0m selection, scrollback and \x1b[38;5;208m256 colors\x1b[0m work\r\n");
    s.push_str("\x1b[1;32mmartin@server\x1b[0m:\x1b[1;34m~/project\x1b[0m$ vim");
    s
}

#[test]
#[ignore = "renders with wgpu; run manually to inspect the output"]
fn render_terminal() {
    let mut terminal = Terminal::new(80, 24);
    terminal.advance(demo().as_bytes());
    let state = (terminal, TerminalView::default());
    let mut harness = Harness::builder().with_size([760.0, 330.0]).build_ui_state(
        |ui, (terminal, view): &mut (Terminal, TerminalView)| {
            view.show(ui, terminal);
        },
        state,
    );
    rooste_gui::fonts::install(&harness.ctx);
    harness.run();
    // Re-feed after the widget sized the grid to the window.
    let (terminal, _) = harness.state_mut();
    terminal.advance(b"\x1b[2J\x1b[H");
    terminal.advance(demo().as_bytes());
    terminal.select_start(0, 7, false, rooste_terminal::SelectionKind::Simple);
    terminal.select_update(7, 7, true);
    harness.run();

    let image = harness.render().expect("render with wgpu");
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/screenshots");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("terminal.png");
    image.save(&path).unwrap();
    println!("saved {}", path.display());
}

#[test]
#[ignore = "renders with wgpu; run manually to inspect the output"]
fn render_box_drawing() {
    let text = "╔═══╦═╗ ┏━┳━┓ ┌─┬─┐\r\n║ A ║B║ ┃x┃y┃ │1│2│\r\n╠═══╬═╣ ┣━╋━┫ ├─┼─┤\r\n╚═══╩═╝ ┗━┻━┛ └─┴─┘\r\n▁▂▃▄▅▆▇█ ▏▎▍▌▋▊▉ ░▒▓ ▀▄";
    let mut view = TerminalView::default();
    view.font_size = 28.0;
    let state = (Terminal::new(40, 6), view);
    let mut harness = Harness::builder().with_size([520.0, 220.0]).build_ui_state(
        |ui, (terminal, view): &mut (Terminal, TerminalView)| {
            view.show(ui, terminal);
        },
        state,
    );
    harness.run();
    harness.state_mut().0.advance(text.as_bytes());
    harness.state_mut().0.advance(b"\x1b[?25l");
    harness.run();
    let image = harness.render().expect("render with wgpu");
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/screenshots");
    std::fs::create_dir_all(&dir).unwrap();
    image.save(dir.join("boxes.png")).unwrap();
}

#[test]
#[ignore = "renders with wgpu; run manually to inspect the output"]
fn render_app() {
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use rooste_core::{Config, MemoryStore, Profile};
    use rooste_gui::RoosteApp;
    use rooste_testkit::{TEST_PASSWORD, TEST_USER, TestServer};

    let net = tokio::runtime::Runtime::new().unwrap();
    let server = net.block_on(TestServer::start());
    let dir = tempfile::tempdir().unwrap();
    let known_hosts = dir.path().join("known_hosts");
    rooste_protocols::ssh::known_hosts::learn(
        &known_hosts,
        "127.0.0.1",
        server.addr.port(),
        &server.host_key,
    )
    .unwrap();
    let p = |name: &str, folder: &str| Profile {
        name: name.into(),
        folder: folder.into(),
        host: format!("{}.example.com", name.to_lowercase().replace(' ', "-")),
        user: "deploy".into(),
        ..Profile::default()
    };
    let live = Profile {
        name: "Test server".into(),
        host: "127.0.0.1".into(),
        port: Some(server.addr.port()),
        user: TEST_USER.into(),
        use_agent: false,
        save_password: true,
        ..Profile::default()
    };
    let config = Config {
        profiles: vec![
            p("Web 1", "Work/Production"),
            p("Web 2", "Work/Production"),
            p("Database", "Work/Production"),
            p("Staging", "Work"),
            p("Raspberry Pi", "Home"),
            live.clone(),
        ],
        ..Config::default()
    };
    let app = RoosteApp::with_config(config, None, Arc::new(MemoryStore::default()))
        .with_known_hosts(known_hosts);
    app.secrets().set(&live.id, TEST_PASSWORD).unwrap();
    let mut harness = Harness::builder()
        .with_size([1000.0, 560.0])
        .build_ui_state(|ui, app: &mut RoosteApp| app.show(ui), app);
    rooste_gui::fonts::install(&harness.ctx);
    let ctx = harness.ctx.clone();
    harness.state_mut().connect_profile(&ctx, &live.id);
    let deadline = Instant::now() + Duration::from_secs(10);
    while !harness
        .state()
        .tabs()
        .any(|t| t.terminal.screen_text().contains('$'))
    {
        assert!(Instant::now() < deadline);
        harness.step();
        std::thread::sleep(Duration::from_millis(10));
    }
    harness.event(egui::Event::Text("hello from rooste".into()));
    harness.key_press(egui::Key::Enter);
    for _ in 0..20 {
        harness.step();
        std::thread::sleep(Duration::from_millis(10));
    }
    harness.run();
    let image = harness.render().expect("render with wgpu");
    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/screenshots");
    std::fs::create_dir_all(&out).unwrap();
    image.save(out.join("app.png")).unwrap();
}

/// The welcome screen and the main dialogs, one image each.
#[test]
#[ignore = "renders with wgpu; run manually to inspect the output"]
fn render_dialogs() {
    use std::sync::Arc;

    use egui_kittest::kittest::Queryable;
    use rooste_core::{Config, MemoryStore, Profile};
    use rooste_gui::RoosteApp;

    let p = |name: &str, folder: &str| Profile {
        name: name.into(),
        folder: folder.into(),
        host: format!("{}.example.com", name.to_lowercase().replace(' ', "-")),
        user: "deploy".into(),
        ..Profile::default()
    };
    let config = Config {
        profiles: vec![
            p("Web 1", "Work/Production"),
            p("Database", "Work/Production"),
            p("Staging", "Work"),
            p("Raspberry Pi", "Home"),
        ],
        ..Config::default()
    };
    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/screenshots");
    std::fs::create_dir_all(&out).unwrap();
    let shots: [(&str, &[&str]); 6] = [
        ("welcome", &[]),
        ("update-banner", &[]),
        ("welcome-light", &[]),
        ("profile-editor", &["Session", "New session…"]),
        ("settings", &["Session", "Settings…"]),
        ("key-generator", &["Tools", "Key generator…"]),
    ];
    for (name, clicks) in shots {
        let mut config = config.clone();
        if name.ends_with("-light") {
            config.settings.theme = rooste_core::Theme::Light;
        }
        let mut app = RoosteApp::with_config(config, None, Arc::new(MemoryStore::default()));
        if name == "update-banner" {
            app.update_banner = Some(rooste_gui::update::Release {
                version: "0.2.0".into(),
                url: "https://example.com".into(),
                download: "https://example.com".into(),
            });
        }
        let mut harness = Harness::builder()
            .with_size([1000.0, 640.0])
            .build_ui_state(|ui, app: &mut RoosteApp| app.show(ui), app);
        rooste_gui::fonts::install(&harness.ctx);
        harness.run();
        for label in clicks {
            harness.get_by_label(label).click();
            harness.run();
        }
        let image = harness.render().expect("render with wgpu");
        image.save(out.join(format!("{name}.png"))).unwrap();
    }
}

/// A tab whose login failed with a long error message.
#[test]
#[ignore = "renders with wgpu; run manually to inspect the output"]
fn render_failed_tab() {
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use egui_kittest::kittest::Queryable;
    use rooste_core::{Config, MemoryStore};
    use rooste_gui::RoosteApp;
    use rooste_testkit::{TEST_USER, TestServer};

    let net = tokio::runtime::Runtime::new().unwrap();
    let server = net.block_on(TestServer::start());
    let dir = tempfile::tempdir().unwrap();
    let known_hosts = dir.path().join("known_hosts");
    rooste_protocols::ssh::known_hosts::learn(
        &known_hosts,
        "127.0.0.1",
        server.addr.port(),
        &server.host_key,
    )
    .unwrap();
    let not_a_key = dir
        .path()
        .join("a-folder-with-a-rather-long-name")
        .join("another-long-folder-name-to-make-this-wrap")
        .join("private_key3.openssh");
    std::fs::create_dir_all(not_a_key.parent().unwrap()).unwrap();
    std::fs::write(&not_a_key, "not a key\n").unwrap();

    let mut app = RoosteApp::with_config(Config::default(), None, Arc::new(MemoryStore::default()))
        .with_known_hosts(known_hosts);
    app.quick.host = "127.0.0.1".into();
    app.quick.port = server.addr.port().to_string();
    app.quick.user = TEST_USER.into();
    app.quick.use_agent = false;
    app.quick.key_file = not_a_key.display().to_string();
    let mut harness = Harness::builder()
        .with_size([1000.0, 640.0])
        .build_ui_state(|ui, app: &mut RoosteApp| app.show(ui), app);
    rooste_gui::fonts::install(&harness.ctx);
    harness.run();
    harness.get_by_label("Connect").hover();
    harness.run();
    harness.get_by_label("Connect").click();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !harness.state().tabs().any(|t| t.has_ended()) {
        assert!(
            Instant::now() < deadline,
            "the login should fail: {:?}",
            (
                harness
                    .state()
                    .tabs()
                    .map(|t| t.status_text())
                    .collect::<Vec<_>>(),
                &harness.state().quick_error
            )
        );
        harness.step();
        std::thread::sleep(Duration::from_millis(10));
    }
    for _ in 0..5 {
        harness.step();
    }
    let image = harness.render().expect("render with wgpu");
    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/screenshots");
    std::fs::create_dir_all(&out).unwrap();
    image.save(out.join("failed-tab.png")).unwrap();
}

/// The SFTP file panel with a few entries, two of them selected.
#[test]
#[ignore = "renders with wgpu; run manually to inspect the output"]
fn render_files_panel() {
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use rooste_core::{Config, MemoryStore, Profile};
    use rooste_gui::RoosteApp;
    use rooste_testkit::{TEST_PASSWORD, TEST_USER, TestServer};

    let net = tokio::runtime::Runtime::new().unwrap();
    let server = net.block_on(TestServer::start());
    let root = server.sftp_root().to_owned();
    std::fs::create_dir(root.join("logs")).unwrap();
    for name in ["backup.tar.gz", "notes.txt", "report.pdf"] {
        std::fs::write(root.join(name), name.repeat(100)).unwrap();
    }
    let dir = tempfile::tempdir().unwrap();
    let known_hosts = dir.path().join("known_hosts");
    rooste_protocols::ssh::known_hosts::learn(
        &known_hosts,
        "127.0.0.1",
        server.addr.port(),
        &server.host_key,
    )
    .unwrap();
    let live = Profile {
        name: "Test server".into(),
        host: "127.0.0.1".into(),
        port: Some(server.addr.port()),
        user: TEST_USER.into(),
        use_agent: false,
        save_password: true,
        ..Profile::default()
    };
    let config = Config {
        profiles: vec![live.clone()],
        ..Config::default()
    };
    let app = RoosteApp::with_config(config, None, Arc::new(MemoryStore::default()))
        .with_known_hosts(known_hosts);
    app.secrets().set(&live.id, TEST_PASSWORD).unwrap();
    let mut harness = Harness::builder()
        .with_size([1000.0, 560.0])
        .build_ui_state(|ui, app: &mut RoosteApp| app.show(ui), app);
    rooste_gui::fonts::install(&harness.ctx);
    let ctx = harness.ctx.clone();
    harness.state_mut().connect_profile(&ctx, &live.id);
    let deadline = Instant::now() + Duration::from_secs(10);
    let listed = |app: &RoosteApp| {
        app.tabs()
            .next()
            .and_then(|t| t.files.as_ref())
            .is_some_and(|f| f.entries.len() == 4)
    };
    while !listed(harness.state()) {
        assert!(Instant::now() < deadline);
        if harness
            .state()
            .tabs()
            .any(|t| t.files.is_none() && t.state == rooste_gui::app::TabState::Connected)
        {
            harness
                .state_mut()
                .tabs_mut()
                .next()
                .unwrap()
                .toggle_files();
        }
        harness.step();
        std::thread::sleep(Duration::from_millis(10));
    }
    let files = harness
        .state_mut()
        .tabs_mut()
        .next()
        .unwrap()
        .files
        .as_mut()
        .unwrap();
    files.selected = files.entries[1..3].iter().map(|e| e.path.clone()).collect();
    for _ in 0..5 {
        harness.step();
    }
    let image = harness.render().expect("render with wgpu");
    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/screenshots");
    std::fs::create_dir_all(&out).unwrap();
    image.save(out.join("files-panel.png")).unwrap();
}

/// Screenshots for the README, written to `target/screenshots/readme-*.png`:
/// two sessions side by side, the file panel, the session editor, search,
/// the command palette, the key generator, and the light theme.
#[test]
#[ignore = "renders with wgpu; run manually to inspect the output"]
fn render_readme() {
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use egui_kittest::kittest::Queryable;
    use rooste_core::{Config, MemoryStore, Profile, Theme, Tunnel, TunnelKind};
    use rooste_gui::RoosteApp;
    use rooste_gui::app::TabState;
    use rooste_testkit::{TEST_PASSWORD, TEST_USER, TestServer};

    const PROMPT_WEB: &str = "\x1b[1;32mdeploy@web-1\x1b[0m:\x1b[1;34m~\x1b[0m$ ";
    const PROMPT_DB: &str = "\x1b[1;32mdeploy@db-primary\x1b[0m:\x1b[1;34m~\x1b[0m$ ";
    // Short lines, so both halves of the split view fit without wrapping.
    let web_output = format!(
        "{PROMPT_WEB}systemctl is-active nginx php8.3-fpm\r\n\
         \x1b[1;32mactive\x1b[0m\r\n\
         \x1b[1;32mactive\x1b[0m\r\n\
         {PROMPT_WEB}ls -l --color\r\n\
         drwxr-xr-x 2 deploy deploy 4.0K Oct 14 \x1b[1;34mlogs\x1b[0m\r\n\
         drwxr-xr-x 5 deploy deploy 4.0K Oct 13 \x1b[1;34mreleases\x1b[0m\r\n\
         -rw-r--r-- 1 deploy deploy  48K Oct 13 \x1b[1;31mbackup.tar.gz\x1b[0m\r\n\
         -rwxr-xr-x 1 deploy deploy  900 Oct 12 \x1b[1;32mdeploy.sh\x1b[0m\r\n\
         -rw-r--r-- 1 deploy deploy 2.4K Oct 10 nginx.conf\r\n\
         {PROMPT_WEB}tail -n 5 logs/access.log\r\n\
         16:02:11  GET   /api/health      \x1b[32m200\x1b[0m    3 ms\r\n\
         16:02:13  POST  /api/orders      \x1b[32m201\x1b[0m   48 ms\r\n\
         16:02:14  GET   /api/orders/88   \x1b[33m404\x1b[0m    5 ms\r\n\
         16:02:17  POST  /api/payments    \x1b[31m502\x1b[0m  930 ms\r\n\
         16:02:19  GET   /api/health      \x1b[32m200\x1b[0m    2 ms\r\n\
         {PROMPT_WEB}"
    );
    let db_output = format!(
        "{PROMPT_DB}psql -c 'select count(*) from orders'\r\n\
         \x20count\r\n\
         -------\r\n\
         \x2048213\r\n\
         (1 row)\r\n\
         {PROMPT_DB}df -h /srv\r\n\
         Filesystem      Size  Used Avail Use% Mounted on\r\n\
         /dev/nvme1n1    500G  212G  288G  \x1b[32m43%\x1b[0m /srv\r\n\
         {PROMPT_DB}uptime\r\n\
         \x2016:02:31 up 41 days, load average: 0.42, 0.38, 0.35\r\n\
         {PROMPT_DB}"
    );

    let net = tokio::runtime::Runtime::new().unwrap();
    let server = net.block_on(TestServer::start());
    let root = server.sftp_root().to_owned();
    for dir in ["logs", "releases"] {
        std::fs::create_dir(root.join(dir)).unwrap();
    }
    for (name, size) in [
        ("backup-2026-10-13.tar.gz", 48_000),
        ("deploy.sh", 900),
        ("nginx.conf", 2_400),
    ] {
        std::fs::write(root.join(name), vec![b'x'; size]).unwrap();
    }
    let p = |name: &str, folder: &str| Profile {
        name: name.into(),
        folder: folder.into(),
        host: format!("{}.example.com", name.to_lowercase().replace(' ', "-")),
        user: "deploy".into(),
        ..Profile::default()
    };
    let live = Profile {
        name: "web-1".into(),
        folder: "Production".into(),
        host: "127.0.0.1".into(),
        port: Some(server.addr.port()),
        user: TEST_USER.into(),
        use_agent: false,
        save_password: true,
        ..Profile::default()
    };
    // Shown in the session editor (never connected).
    let web2 = Profile {
        proxy_jump: Some("bastion.example.com".into()),
        tags: vec!["nginx".into(), "eu-west".into()],
        tunnels: vec![
            Tunnel {
                kind: TunnelKind::Local,
                listen: "5432".into(),
                target: "db.internal:5432".into(),
            },
            Tunnel {
                kind: TunnelKind::Dynamic,
                listen: "1080".into(),
                target: String::new(),
            },
        ],
        ..p("web-2", "Production")
    };
    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/screenshots");
    std::fs::create_dir_all(&out).unwrap();

    // A window connected to web-1, with `setup` run before rendering.
    let shot =
        |name: &str, theme: Theme, height: f32, setup: &dyn Fn(&mut Harness<'_, RoosteApp>)| {
            let dir = tempfile::tempdir().unwrap();
            let known_hosts = dir.path().join("known_hosts");
            rooste_protocols::ssh::known_hosts::learn(
                &known_hosts,
                "127.0.0.1",
                server.addr.port(),
                &server.host_key,
            )
            .unwrap();
            let mut config = Config {
                profiles: vec![
                    live.clone(),
                    web2.clone(),
                    p("db-primary", "Production"),
                    p("staging", "Staging"),
                    p("Raspberry Pi", "Home"),
                    p("NAS", "Home"),
                ],
                ..Config::default()
            };
            config.settings.theme = theme;
            let app = RoosteApp::with_config(config, None, Arc::new(MemoryStore::default()))
                .with_known_hosts(known_hosts);
            app.secrets().set(&live.id, TEST_PASSWORD).unwrap();
            let mut harness = Harness::builder()
                .with_size([1280.0, height])
                .build_ui_state(|ui, app: &mut RoosteApp| app.show(ui), app);
            rooste_gui::fonts::install(&harness.ctx);
            let ctx = harness.ctx.clone();
            harness.state_mut().connect_profile(&ctx, &live.id);
            wait(&mut harness, |app| {
                app.tabs().any(|t| t.state == TabState::Connected)
            });
            setup(&mut harness);
            // Keep the mouse pointer out of the picture.
            harness.event(egui::Event::PointerGone);
            for _ in 0..8 {
                harness.step();
            }
            harness
                .render()
                .expect("render with wgpu")
                .save(out.join(format!("readme-{name}.png")))
                .unwrap();
        };
    fn wait(harness: &mut Harness<'_, RoosteApp>, done: impl Fn(&RoosteApp) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !done(harness.state()) {
            assert!(Instant::now() < deadline, "timed out");
            harness.step();
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    let fill = |tab: &mut rooste_gui::tab::SessionTab, text: &str| {
        tab.terminal.advance(b"\x1b[2J\x1b[H");
        tab.terminal.advance(text.as_bytes());
    };
    // web-1 on the left, db-primary split off to the right.
    let split = |harness: &mut Harness<'_, RoosteApp>| {
        harness
            .state_mut()
            .split_active(egui_tiles::LinearDir::Horizontal);
        wait(harness, |app| {
            app.tabs()
                .filter(|t| t.state == TabState::Connected)
                .count()
                == 2
        });
        for _ in 0..5 {
            harness.step();
        }
        let mut tabs: Vec<_> = harness.state_mut().tabs_mut().collect();
        tabs.sort_by_key(|t| t.id);
        fill(tabs[0], &web_output);
        tabs[1].name = "db-primary".into();
        fill(tabs[1], &db_output);
    };

    shot("split", Theme::Dark, 720.0, &split);
    shot("light", Theme::Light, 720.0, &split);
    shot("files", Theme::Dark, 720.0, &|harness| {
        let tab = harness.state_mut().tabs_mut().next().unwrap();
        tab.toggle_files();
        wait(harness, |app| {
            app.tabs()
                .next()
                .and_then(|t| t.files.as_ref())
                .is_some_and(|f| f.entries.len() == 5)
        });
        let tab = harness.state_mut().tabs_mut().next().unwrap();
        fill(tab, &web_output);
        let files = tab.files.as_mut().unwrap();
        files.selected = vec![files.entries[2].path.clone()];
    });
    shot("search", Theme::Dark, 720.0, &|harness| {
        let tab = harness.state_mut().tabs_mut().next().unwrap();
        fill(tab, &web_output);
        tab.open_find();
        let find = tab.find.as_mut().unwrap();
        find.query = "api/orders".into();
        find.found = Some(
            tab.terminal
                .find("api/orders", rooste_terminal::SearchDirection::Up),
        );
    });
    shot("editor", Theme::Dark, 960.0, &|harness| {
        let tab = harness.state_mut().tabs_mut().next().unwrap();
        fill(tab, &web_output);
        harness.get_by_label("web-2").hover();
        harness.step();
        harness.get_by_label("web-2").click_secondary();
        harness.step();
        harness.step();
        harness.get_by_label("Edit…").click();
        harness.step();
    });
    shot("palette", Theme::Dark, 720.0, &|harness| {
        let tab = harness.state_mut().tabs_mut().next().unwrap();
        fill(tab, &web_output);
        harness.key_press_modifiers(egui::Modifiers::CTRL | egui::Modifiers::SHIFT, egui::Key::P);
        harness.step();
        harness.event(egui::Event::Text("split".into()));
        harness.step();
    });
    shot("keygen", Theme::Dark, 720.0, &|harness| {
        let tab = harness.state_mut().tabs_mut().next().unwrap();
        fill(tab, &web_output);
        harness.get_by_label("Tools").click();
        harness.step();
        harness.get_by_label("Key generator…").click();
        harness.step();
        if let Some(rooste_gui::app::Dialog::KeyGenerator(generator)) =
            &mut harness.state_mut().dialog
        {
            generator.comment = "deploy@laptop".into();
            generator.path = "~/.ssh/id_ed25519".into();
        }
    });
}
