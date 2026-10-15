//! Terminal behavior, modeled on vttest/esctest cases. Screens are compared
//! as text; files under `snapshots/` are reviewed `insta` snapshots.

use insta::assert_snapshot;
use rooste_terminal::input::InputMode;
use rooste_terminal::{
    MouseReport, Palette, Rgb, SearchDirection, SelectionKind, TermEvent, Terminal,
};

fn term(cols: u16, rows: u16, input: &str) -> Terminal {
    let mut t = Terminal::new(cols, rows);
    t.advance(input.as_bytes());
    t
}

fn screen(cols: u16, rows: u16, input: &str) -> String {
    term(cols, rows, input).screen_text()
}

#[test]
fn text_newlines_and_autowrap() {
    assert_snapshot!(screen(10, 4, "hello\r\nworld\r\n0123456789abc"));
}

#[test]
fn autowrap_off() {
    assert_snapshot!(screen(10, 2, "\x1b[?7l0123456789abc"));
}

#[test]
fn cursor_positioning() {
    // CUP, CUU/CUD/CUF/CUB, CHA, VPA.
    let input = "\x1b[3;5HA\x1b[1;1HB\x1b[2B\x1b[3CC\x1b[A\x1b[2DD\x1b[10GE\x1b[4dF";
    assert_snapshot!(screen(12, 5, input));
}

#[test]
fn erase_display_and_line() {
    let fill = "aaaaaaaa\r\nbbbbbbbb\r\ncccccccc\r\ndddddddd";
    // EL to end, EL to start, EL whole line, ED below.
    let input = format!("{fill}\x1b[1;4H\x1b[K\x1b[2;4H\x1b[1K\x1b[3;4H\x1b[2K\x1b[4;1H\x1b[J");
    assert_snapshot!(screen(8, 4, &input));
    assert_eq!(screen(8, 2, &format!("{fill}\x1b[2J")), "\n\n");
}

#[test]
fn insert_and_delete_characters_and_lines() {
    let fill = "abcdefgh\r\n11111111\r\n22222222\r\n33333333";
    // ICH, DCH on row 1; IL on row 2; DL on row 4 (after the insert).
    let input = format!("{fill}\x1b[1;3H\x1b[2@\x1b[1;7H\x1b[1P\x1b[2;1H\x1b[L\x1b[4;1H\x1b[M");
    assert_snapshot!(screen(8, 4, &input));
}

#[test]
fn scroll_region() {
    // DECSTBM rows 2-3: line feeds at the bottom of the region scroll only it.
    let input = "top\r\n1\r\n2\r\nbottom\x1b[2;3r\x1b[3;1H\nX\nY";
    assert_snapshot!(screen(8, 4, input));
}

#[test]
fn origin_mode_is_relative_to_scroll_region() {
    let input = "\x1b[2;3r\x1b[?6h\x1b[1;1HO";
    assert_eq!(screen(4, 4, input), "\nO\n\n\n");
}

#[test]
fn reverse_index_scrolls_down_at_top() {
    assert_eq!(screen(4, 3, "a\r\nb\x1b[1;1H\x1bMz"), "z\na\nb\n");
}

#[test]
fn alternate_screen_restores_main_screen() {
    let mut t = term(10, 3, "main text");
    t.advance(b"\x1b[?1049h\x1b[2J\x1b[Hfull screen");
    assert!(t.alt_screen());
    assert_eq!(t.screen_text(), "full scree\nn\n\n");
    t.advance(b"\x1b[?1049l");
    assert!(!t.alt_screen());
    assert_eq!(t.screen_text(), "main text\n\n\n");
}

#[test]
fn save_and_restore_cursor() {
    assert_eq!(screen(8, 2, "ab\x1b7\x1b[2;5Hx\x1b8c"), "abc\n    x\n");
}

#[test]
fn tab_stops() {
    // Default stops every 8 columns; HTS sets one, TBC clears all.
    assert_eq!(screen(20, 1, "a\tb\tc"), "a       b       c\n");
    assert_eq!(screen(20, 1, "\x1b[3g\x1b[1;4H\x1bH\ra\tb"), "a  b\n");
}

#[test]
fn wide_characters() {
    let t = term(6, 2, "日本語!");
    assert_eq!(t.screen_text(), "日本語\n!\n");
    let row = t.screen().row(0).to_vec();
    assert!(row[0].wide && row[1].spacer);
}

#[test]
fn screen_alignment_pattern() {
    // DECALN fills the screen with `E`.
    assert_eq!(screen(3, 2, "\x1b#8"), "EEE\nEEE\n");
}

#[test]
fn sgr_colors_and_attributes() {
    let p = Palette::default();
    let t = term(
        8,
        1,
        "\x1b[31mr\x1b[1mb\x1b[0;38;5;196mi\x1b[38;2;1;2;3mt\x1b[0;7mv\x1b[0;4;9;3mu\x1b[2;32md",
    );
    let s = t.screen();
    let row = s.row(0);
    assert_eq!(row[0].fg, p.ansi[1], "red");
    assert_eq!(row[1].fg, p.ansi[9], "bold red is bright red");
    assert!(row[1].bold);
    assert_eq!(row[2].fg, Rgb::new(255, 0, 0), "256-color");
    assert_eq!(row[3].fg, Rgb::new(1, 2, 3), "truecolor");
    assert_eq!(
        (row[4].fg, row[4].bg),
        (p.background, p.foreground),
        "inverse"
    );
    assert!(row[5].underline && row[5].strikeout && row[5].italic);
    assert_eq!(row[6].fg, p.ansi[2].dimmed(), "dim green");
    assert_eq!(row[7].bg, p.background);
}

#[test]
fn status_queries_get_replies() {
    let mut t = term(10, 5, "\x1b[3;4H\x1b[6n\x1b[c");
    let replies: Vec<_> = t
        .take_events()
        .into_iter()
        .filter_map(|e| match e {
            TermEvent::Reply(bytes) => Some(String::from_utf8(bytes).unwrap()),
            _ => None,
        })
        .collect();
    assert_eq!(replies[0], "\x1b[3;4R", "cursor position report");
    assert!(replies[1].starts_with("\x1b[?"), "device attributes");
    assert!(t.take_events().is_empty(), "events are drained");
}

#[test]
fn title_bell_and_clipboard_events() {
    let mut t = term(10, 2, "\x1b]0;my title\x07\x07\x1b]52;c;aGVsbG8=\x07");
    let events = t.take_events();
    assert!(events.contains(&TermEvent::Title("my title".into())));
    assert!(events.contains(&TermEvent::Bell));
    assert!(events.contains(&TermEvent::Clipboard("hello".into())));
}

#[test]
fn modes_reported_to_input_encoding() {
    let mut t = Terminal::new(10, 2);
    assert_eq!(t.input_mode(), InputMode { app_cursor: false });
    t.advance(b"\x1b[?1h\x1b[?2004h\x1b[?1002h\x1b[?1006h");
    assert!(t.input_mode().app_cursor);
    assert!(t.bracketed_paste());
    assert_eq!(t.mouse_mode().report, MouseReport::Drag);
    assert!(t.mouse_mode().sgr);
    t.advance(b"\x1b[?1l\x1b[?2004l\x1b[?1002l");
    assert!(!t.input_mode().app_cursor);
    assert!(!t.bracketed_paste());
    assert_eq!(t.mouse_mode().report, MouseReport::None);
}

#[test]
fn scrollback_history_and_view() {
    let lines: String = (1..=12).map(|i| format!("line {i}\r\n")).collect();
    let mut t = term(10, 4, &lines);
    assert_eq!(t.screen_text(), "line 10\nline 11\nline 12\n\n");
    assert_eq!(t.history_size(), 9);

    t.scroll(3);
    assert_eq!(t.display_offset(), 3);
    assert_eq!(t.screen_text(), "line 7\nline 8\nline 9\nline 10\n");
    assert_eq!(t.screen().cursor, None, "cursor scrolled out of view");

    t.scroll_to_bottom();
    assert_eq!(t.display_offset(), 0);
}

#[test]
fn resize_keeps_content_and_cursor() {
    let mut t = term(10, 3, "abc\r\ndef");
    t.resize(20, 5, 8, 16);
    assert_eq!(t.size(), (20, 5));
    assert!(t.screen_text().starts_with("abc\ndef\n"));
    assert_eq!(t.screen().cursor, Some((3, 1)));
}

#[test]
fn selection_text() {
    let mut t = term(12, 3, "hello world\r\nsecond line");
    assert_eq!(t.selection_text(), None);

    t.select_start(0, 0, false, SelectionKind::Simple);
    t.select_update(4, 0, true);
    assert_eq!(t.selection_text().as_deref(), Some("hello"));

    t.select_start(7, 0, false, SelectionKind::Word);
    assert_eq!(t.selection_text().as_deref(), Some("world"));

    t.select_start(3, 1, false, SelectionKind::Line);
    assert_eq!(t.selection_text().as_deref(), Some("second line\n"));

    let s = t.screen();
    assert_eq!(s.row(1)[0].bg, Palette::default().selection_background);

    t.clear_selection();
    assert_eq!(t.selection_text(), None);
}

#[test]
fn palette_and_scrollback_can_change() {
    let mut t = term(4, 2, "\x1b[31mr");
    assert_eq!(t.screen().row(0)[0].fg, Palette::default().ansi[1]);
    t.set_palette(Palette::light());
    assert_eq!(t.screen().row(0)[0].fg, Palette::light().ansi[1]);
    assert_eq!(t.screen().background, Palette::light().background);

    t.set_scrollback(2);
    t.advance(b"\r\n1\r\n2\r\n3\r\n4\r\n5");
    assert_eq!(t.history_size(), 2);
}

#[test]
fn hidden_cursor() {
    assert_eq!(term(4, 2, "\x1b[?25l").screen().cursor, None);
    assert_eq!(term(4, 2, "ab").screen().cursor, Some((2, 0)));
}

#[test]
fn garbage_input_does_not_panic() {
    let mut t = Terminal::new(5, 3);
    let noise: Vec<u8> = (0..20_000u32).map(|i| (i * 7919 % 256) as u8).collect();
    t.advance(&noise);
    let _ = t.screen();
}

#[test]
fn find_walks_through_matches_in_the_scrollback() {
    let mut term = Terminal::with_options(20, 3, rooste_terminal::Palette::default(), 100);
    for i in 0..10 {
        term.advance(format!("line {i} error\r\n").as_bytes());
    }
    term.advance(b"Error at the end\r\n$ ");

    // Up first: the newest match. Lower-case text ignores case.
    assert!(term.find("error", SearchDirection::Up));
    assert_eq!(term.selection_text().as_deref(), Some("Error"));
    assert_eq!(term.display_offset(), 0);

    // Further up goes into the scrollback.
    assert!(term.find("error", SearchDirection::Up));
    assert_eq!(term.selection_text().as_deref(), Some("error"));
    assert!(term.find("line 3", SearchDirection::Up));
    assert!(term.display_offset() > 0, "scrolled to the match");
    assert_eq!(term.selection_text().as_deref(), Some("line 3"));
    assert!(term.find("line 4", SearchDirection::Down));
    assert_eq!(term.selection_text().as_deref(), Some("line 4"));

    // Capitals make it case-sensitive; special characters are literal.
    term.clear_search();
    assert!(term.find("Error", SearchDirection::Up));
    assert!(
        term.find("Error", SearchDirection::Up),
        "wraps around to the same match"
    );
    assert!(!term.find("line 1.", SearchDirection::Up));
    assert!(!term.find("nothing like this", SearchDirection::Up));
    term.clear_search();
    assert_eq!(term.selection_text(), None);
}

/// Found by fuzzing: this left the cursor on a wide character spacer in the
/// first column, and drawing the screen panicked.
#[test]
fn cursor_on_spacer_in_first_column() {
    let mut t = Terminal::new(40, 12);
    t.advance(b"\xe7\x9b\x8c\x1b\r[$\xdb\x9b\x8c\x1b\r[\xe4\xaf\xb8\x11P");
    let screen = t.screen();
    let (col, row) = screen.cursor.expect("cursor visible");
    assert!(col < 40 && row < 12);
    t.resize(23, 7, 8, 16);
    let _ = t.screen();
}
