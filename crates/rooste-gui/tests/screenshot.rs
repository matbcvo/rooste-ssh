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
use rooste_term::Terminal;

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
    terminal.select_start(0, 7, false, rooste_term::SelectionKind::Simple);
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
