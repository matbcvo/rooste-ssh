//! Scripts that drive a session, written in [Rhai](https://rhai.rs): send
//! text, wait for output, ask the user. Each script runs on its own thread
//! and talks to its tab through channels; see `docs/scripting.md` for the
//! language reference used by script authors.
//!
//! Scripts are sandboxed by Rhai: they can't open files, start programs or
//! reach the network. They only get the functions registered here.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use rhai::{Dynamic, Engine, EvalAltResult, Position, Scope};
use tokio::sync::oneshot;

/// The folder scripts are kept in, next to the configuration file.
pub fn scripts_dir(config_path: Option<&Path>) -> Option<PathBuf> {
    config_path
        .and_then(Path::parent)
        .map(|dir| dir.join("scripts"))
}

/// The `.rhai` files in `dir`, by name.
pub fn list_scripts(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            name.ends_with(".rhai").then_some(name)
        })
        .collect();
    names.sort_by_key(|n| n.to_lowercase());
    names
}

/// Create the scripts folder with an example the first time it is opened.
pub fn ensure_scripts_dir(dir: &Path) -> std::io::Result<()> {
    if dir.is_dir() {
        return Ok(());
    }
    std::fs::create_dir_all(dir)?;
    std::fs::write(dir.join("example.rhai"), EXAMPLE)
}

const EXAMPLE: &str = r#"// An example Rooste SSH script. Run it from a tab's right-click menu
// (Run script) or the command palette. See docs/scripting.md for more.

notify(`Running on ${SESSION_NAME} (${USER}@${HOST})`);

send_line("uname -a");
if wait_for("Linux", 5) {
    notify("This is a Linux server.");
} else {
    notify("No answer within 5 seconds.");
}
"#;

/// What the session a script runs in is.
#[derive(Debug, Clone, Default)]
pub struct SessionInfo {
    pub name: String,
    pub host: String,
    pub user: String,
}

/// From a script to its tab.
#[derive(Debug)]
pub enum ScriptEvent {
    /// Send these bytes to the session, as if typed.
    Send(Vec<u8>),
    /// Show a message to the user.
    Notify(String),
    /// Ask the user; the answer (or `None` if cancelled) goes to `reply`.
    Ask {
        question: String,
        hidden: bool,
        reply: oneshot::Sender<Option<String>>,
    },
    /// The script ended: `Ok` when it ran to the end.
    Finished(Result<(), String>),
}

/// A running script, owned by its tab. Dropping it stops the script.
#[derive(Debug)]
pub struct ScriptHandle {
    pub name: String,
    output: mpsc::Sender<String>,
    events: mpsc::Receiver<ScriptEvent>,
    cancel: Arc<AtomicBool>,
}

impl ScriptHandle {
    /// Pass session output (as plain text) to the script.
    pub fn output(&self, text: &str) {
        let _ = self.output.send(text.to_owned());
    }

    /// Events the script has sent since the last call.
    pub fn events(&self) -> Vec<ScriptEvent> {
        self.events.try_iter().collect()
    }

    /// Ask the script to stop; it ends with "cancelled".
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

impl Drop for ScriptHandle {
    fn drop(&mut self) {
        self.cancel();
    }
}

/// Start running the script at `path`. `wake` is called when the script
/// has an event for the tab.
pub fn start(path: &Path, info: SessionInfo, wake: Arc<dyn Fn() + Send + Sync>) -> ScriptHandle {
    let name = path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    );
    let (output_tx, output_rx) = mpsc::channel();
    let (events_tx, events_rx) = mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let path = path.to_owned();
    let state = State {
        output: output_rx,
        buffer: String::new(),
        events: events_tx.clone(),
        cancel: cancel.clone(),
        wake: wake.clone(),
    };
    std::thread::Builder::new()
        .name(format!("script {name}"))
        .spawn(move || {
            let result = std::fs::read_to_string(&path)
                .map_err(|e| format!("{}: {e}", path.display()))
                .and_then(|source| run(&source, info, state));
            let _ = events_tx.send(ScriptEvent::Finished(result));
            wake();
        })
        .expect("start script thread");
    ScriptHandle {
        name,
        output: output_tx,
        events: events_rx,
        cancel,
    }
}

/// What the registered functions share.
struct State {
    output: mpsc::Receiver<String>,
    /// Output not yet consumed by `wait_for`.
    buffer: String,
    events: mpsc::Sender<ScriptEvent>,
    cancel: Arc<AtomicBool>,
    wake: Arc<dyn Fn() + Send + Sync>,
}

impl State {
    fn emit(&self, event: ScriptEvent) -> Result<(), Box<EvalAltResult>> {
        self.events.send(event).map_err(|_| stopped())?;
        (self.wake)();
        Ok(())
    }

    fn check_cancel(&self) -> Result<(), Box<EvalAltResult>> {
        if self.cancel.load(Ordering::Relaxed) {
            return Err(stopped());
        }
        Ok(())
    }

    /// Wait until `text` has appeared in the output, consuming the output
    /// up to and including it. False after `timeout`.
    fn wait_for(&mut self, text: &str, timeout: Duration) -> Result<bool, Box<EvalAltResult>> {
        let deadline = Instant::now() + timeout;
        loop {
            while let Ok(chunk) = self.output.try_recv() {
                self.buffer.push_str(&chunk);
            }
            if let Some(at) = self.buffer.find(text) {
                self.buffer.drain(..at + text.len());
                return Ok(true);
            }
            // Keep memory bounded on long-running sessions.
            if self.buffer.len() > MAX_BUFFER {
                let cut = self.buffer.len() - MAX_BUFFER / 2;
                let cut = (cut..self.buffer.len())
                    .find(|&i| self.buffer.is_char_boundary(i))
                    .unwrap_or(self.buffer.len());
                self.buffer.drain(..cut);
            }
            self.check_cancel()?;
            let now = Instant::now();
            if now >= deadline {
                return Ok(false);
            }
            match self.output.recv_timeout((deadline - now).min(POLL)) {
                Ok(chunk) => self.buffer.push_str(&chunk),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => return Err(stopped()),
            }
        }
    }

    fn sleep(&self, duration: Duration) -> Result<(), Box<EvalAltResult>> {
        let deadline = Instant::now() + duration;
        loop {
            self.check_cancel()?;
            let now = Instant::now();
            if now >= deadline {
                return Ok(());
            }
            std::thread::sleep((deadline - now).min(POLL));
        }
    }

    fn ask(&self, question: &str, hidden: bool) -> Result<String, Box<EvalAltResult>> {
        let (reply, answer) = oneshot::channel();
        self.emit(ScriptEvent::Ask {
            question: question.to_owned(),
            hidden,
            reply,
        })?;
        match answer.blocking_recv() {
            Ok(Some(text)) => Ok(text),
            // The user cancelled, or the tab went away.
            _ => Err(stopped()),
        }
    }
}

const POLL: Duration = Duration::from_millis(100);
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_BUFFER: usize = 256 * 1024;

fn stopped() -> Box<EvalAltResult> {
    EvalAltResult::ErrorTerminated(Dynamic::from("cancelled"), Position::NONE).into()
}

fn seconds(value: f64) -> Duration {
    Duration::from_secs_f64(value.clamp(0.0, 24.0 * 3600.0))
}

fn run(source: &str, info: SessionInfo, state: State) -> Result<(), String> {
    let cancel = state.cancel.clone();
    let state = Rc::new(RefCell::new(state));
    let mut engine = Engine::new();
    // Stop endless loops when asked, and keep scripts from using
    // unbounded memory.
    engine.on_progress(move |_| {
        cancel
            .load(Ordering::Relaxed)
            .then(|| Dynamic::from("cancelled"))
    });
    engine.set_max_string_size(1024 * 1024);
    engine.set_max_array_size(100_000);
    engine.set_max_map_size(100_000);
    engine.set_max_call_levels(64);

    let s = state.clone();
    engine.register_fn("send", move |text: &str| {
        s.borrow().emit(ScriptEvent::Send(text.as_bytes().to_vec()))
    });
    let s = state.clone();
    engine.register_fn("send_line", move |text: &str| {
        s.borrow()
            .emit(ScriptEvent::Send(format!("{text}\r").into_bytes()))
    });
    let s = state.clone();
    engine.register_fn("wait_for", move |text: &str| {
        s.borrow_mut().wait_for(text, DEFAULT_TIMEOUT)
    });
    let s = state.clone();
    engine.register_fn("wait_for", move |text: &str, timeout: f64| {
        s.borrow_mut().wait_for(text, seconds(timeout))
    });
    let s = state.clone();
    engine.register_fn("wait_for", move |text: &str, timeout: i64| {
        s.borrow_mut().wait_for(text, seconds(timeout as f64))
    });
    let s = state.clone();
    engine.register_fn("sleep", move |duration: f64| {
        s.borrow().sleep(seconds(duration))
    });
    let s = state.clone();
    engine.register_fn("sleep", move |duration: i64| {
        s.borrow().sleep(seconds(duration as f64))
    });
    let s = state.clone();
    engine.register_fn("notify", move |message: &str| {
        s.borrow().emit(ScriptEvent::Notify(message.to_owned()))
    });
    let s = state.clone();
    engine.register_fn("ask", move |question: &str| s.borrow().ask(question, false));
    let s = state.clone();
    engine.register_fn("ask_password", move |question: &str| {
        s.borrow().ask(question, true)
    });

    let mut scope = Scope::new();
    scope.push_constant("SESSION_NAME", info.name);
    scope.push_constant("HOST", info.host);
    scope.push_constant("USER", info.user);
    engine
        .run_with_scope(&mut scope, source)
        .map_err(|e| match *e {
            EvalAltResult::ErrorTerminated(..) => "cancelled".to_owned(),
            other => other.to_string(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn script(dir: &Path, source: &str) -> PathBuf {
        let path = dir.join("test.rhai");
        std::fs::write(&path, source).unwrap();
        path
    }

    fn info() -> SessionInfo {
        SessionInfo {
            name: "web-1".into(),
            host: "web1.example.com".into(),
            user: "deploy".into(),
        }
    }

    /// Collect events until the script finishes, answering questions with
    /// `answer`.
    fn finish(handle: &ScriptHandle, answer: Option<&str>) -> Vec<ScriptEvent> {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut all = Vec::new();
        loop {
            for event in handle.events() {
                if let ScriptEvent::Ask { reply, .. } = event {
                    let _ = reply.send(answer.map(str::to_owned));
                    continue;
                }
                let done = matches!(event, ScriptEvent::Finished(_));
                all.push(event);
                if done {
                    return all;
                }
            }
            assert!(Instant::now() < deadline, "script did not finish");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn sends_waits_and_notifies() {
        let dir = tempfile::tempdir().unwrap();
        let path = script(
            dir.path(),
            r#"
            notify(`${SESSION_NAME} ${USER}@${HOST}`);
            send_line("show version");
            if wait_for("Version 9", 5) { notify("found"); }
            if !wait_for("never shows up", 0.2) { notify("timed out"); }
            send("no newline");
            "#,
        );
        let handle = start(&path, info(), Arc::new(|| {}));
        handle.output("Cisco IOS Software, Ver");
        handle.output("sion 9.1\r\n");
        let events = finish(&handle, None);
        let summary: Vec<String> = events
            .iter()
            .map(|e| match e {
                ScriptEvent::Send(bytes) => format!("send {}", String::from_utf8_lossy(bytes)),
                ScriptEvent::Notify(m) => format!("notify {m}"),
                ScriptEvent::Finished(r) => format!("finished {r:?}"),
                ScriptEvent::Ask { .. } => unreachable!(),
            })
            .collect();
        assert_eq!(
            summary,
            [
                "notify web-1 deploy@web1.example.com",
                "send show version\r",
                "notify found",
                "notify timed out",
                "send no newline",
                "finished Ok(())",
            ]
        );
    }

    #[test]
    fn asks_the_user() {
        let dir = tempfile::tempdir().unwrap();
        let path = script(
            dir.path(),
            r#"let secret = ask_password("Enable password"); send_line(secret);"#,
        );
        let handle = start(&path, info(), Arc::new(|| {}));
        let events = finish(&handle, Some("hunter2"));
        assert!(matches!(&events[0], ScriptEvent::Send(b) if b == b"hunter2\r"));

        // Cancelling the question ends the script.
        let handle = start(&path, info(), Arc::new(|| {}));
        let events = finish(&handle, None);
        assert!(matches!(&events[0], ScriptEvent::Finished(Err(e)) if e == "cancelled"));
    }

    #[test]
    fn endless_loops_and_waits_can_be_cancelled() {
        let dir = tempfile::tempdir().unwrap();
        for source in ["loop { }", r#"wait_for("x", 3600);"#, "sleep(3600);"] {
            let path = script(dir.path(), source);
            let handle = start(&path, info(), Arc::new(|| {}));
            std::thread::sleep(Duration::from_millis(50));
            handle.cancel();
            let events = finish(&handle, None);
            assert!(
                matches!(&events[0], ScriptEvent::Finished(Err(e)) if e == "cancelled"),
                "{source}: {events:?}"
            );
        }
    }

    #[test]
    fn errors_are_reported() {
        let dir = tempfile::tempdir().unwrap();
        let path = script(dir.path(), "let x = ;");
        let handle = start(&path, info(), Arc::new(|| {}));
        let events = finish(&handle, None);
        assert!(matches!(&events[0], ScriptEvent::Finished(Err(e)) if !e.is_empty()));

        // No file access: the sandbox has no such function.
        let path = script(dir.path(), r#"open_file("/etc/passwd");"#);
        let handle = start(&path, info(), Arc::new(|| {}));
        let events = finish(&handle, None);
        assert!(matches!(&events[0], ScriptEvent::Finished(Err(e)) if e.contains("open_file")));
    }

    #[test]
    fn scripts_folder() {
        let dir = tempfile::tempdir().unwrap();
        let scripts = dir.path().join("scripts");
        ensure_scripts_dir(&scripts).unwrap();
        std::fs::write(scripts.join("Backup.rhai"), "").unwrap();
        std::fs::write(scripts.join("notes.txt"), "").unwrap();
        assert_eq!(list_scripts(&scripts), ["Backup.rhai", "example.rhai"]);
        assert_eq!(
            scripts_dir(Some(&dir.path().join("config.toml"))),
            Some(scripts)
        );
    }
}
