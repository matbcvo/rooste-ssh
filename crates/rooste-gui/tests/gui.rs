//! GUI tests with egui_kittest, against the in-process SSH server.

use std::time::{Duration, Instant};

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use rooste_gui::RoosteApp;
use rooste_gui::app::TabState;
use rooste_testkit::{TEST_PASSWORD, TEST_USER, TestServer};

fn harness(app: RoosteApp) -> Harness<'static, RoosteApp> {
    Harness::builder()
        .with_size([900.0, 600.0])
        .build_ui_state(|ui, app: &mut RoosteApp| app.show(ui), app)
}

/// Step the UI until `done` holds, failing after a timeout.
fn wait_for(harness: &mut Harness<'_, RoosteApp>, what: &str, done: impl Fn(&RoosteApp) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !done(harness.state()) {
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {what}; screen:\n{}\nstate: {:?}",
            screen_text(harness.state()),
            harness.state().tab.as_ref().map(|t| &t.state)
        );
        harness.step();
        std::thread::sleep(Duration::from_millis(10));
    }
    harness.step();
}

fn screen_text(app: &RoosteApp) -> String {
    app.tab
        .as_ref()
        .map(|t| t.terminal.screen_text())
        .unwrap_or_default()
}

#[test]
fn connect_requires_a_host() {
    let mut harness = harness(RoosteApp::new());
    harness.get_by_label("Connect").click();
    harness.run();
    harness.get_by_label("Enter a host name.");
    assert!(harness.state().tab.is_none());
}

#[test]
fn connect_trust_host_key_type_and_exit() {
    let net = tokio::runtime::Runtime::new().unwrap();
    let server = net.block_on(TestServer::start());
    let dir = tempfile::tempdir().unwrap();
    let known_hosts = dir.path().join("known_hosts");

    let mut harness = harness(RoosteApp::new().with_known_hosts(known_hosts.clone()));
    let host = harness.get_by_label("Host");
    host.focus();
    harness.run();
    harness.get_by_label("Host").type_text("127.0.0.1");
    harness.run();
    {
        let form = &mut harness.state_mut().form;
        form.port = server.addr.port().to_string();
        form.user = TEST_USER.into();
        form.password = TEST_PASSWORD.into();
        form.use_agent = false;
    }
    assert_eq!(
        harness.state().form.host,
        "127.0.0.1",
        "typed into the Host field"
    );
    harness.get_by_label("Connect").click();

    wait_for(&mut harness, "host key prompt", |app| {
        app.tab
            .as_ref()
            .is_some_and(|t| t.pending_host_key().is_some())
    });
    harness.get_by_label("Trust and connect").click();

    wait_for(&mut harness, "shell prompt", |app| {
        app.tab
            .as_ref()
            .is_some_and(|t| t.state == TabState::Connected)
            && screen_text(app).contains("$")
    });
    assert!(known_hosts.exists(), "trusted key was recorded");

    // The fake shell reports the PTY size it was told about: it must match
    // the grid the widget laid out for the window, not the initial 80x24.
    let (cols, rows) = harness.state().tab.as_ref().unwrap().terminal.size();
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
        app.tab
            .as_ref()
            .is_some_and(|t| t.state == TabState::Closed(Some(4)))
    });
    harness.get_by_label("Session closed (exit status 4)");
    harness.get_by_label("Reconnect");
}

#[test]
fn rejected_host_key_fails_the_connection() {
    let net = tokio::runtime::Runtime::new().unwrap();
    let server = net.block_on(TestServer::start());
    let dir = tempfile::tempdir().unwrap();

    let mut harness = harness(RoosteApp::new().with_known_hosts(dir.path().join("known_hosts")));
    {
        let form = &mut harness.state_mut().form;
        form.host = "127.0.0.1".into();
        form.port = server.addr.port().to_string();
        form.user = TEST_USER.into();
        form.password = TEST_PASSWORD.into();
        form.use_agent = false;
    }
    harness.get_by_label("Connect").click();
    wait_for(&mut harness, "host key prompt", |app| {
        app.tab
            .as_ref()
            .is_some_and(|t| t.pending_host_key().is_some())
    });
    harness.get_by_label("Cancel").click();
    wait_for(&mut harness, "failure", |app| {
        matches!(
            app.tab.as_ref().map(|t| &t.state),
            Some(TabState::Failed(_))
        )
    });
    let TabState::Failed(message) = &harness.state().tab.as_ref().unwrap().state else {
        unreachable!()
    };
    assert!(message.contains("not trusted"), "{message}");
}
