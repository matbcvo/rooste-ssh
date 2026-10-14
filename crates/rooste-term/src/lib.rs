//! Terminal emulation, independent of the UI toolkit.
//!
//! [`Terminal`] parses output from the remote side (via `alacritty_terminal`)
//! and exposes a resolved [`Screen`] for rendering; [`input`] encodes key
//! presses, mouse events and pastes for the remote side.

pub mod input;
pub mod palette;
mod terminal;

pub use palette::{Palette, Rgb};
pub use terminal::{
    CursorShape, MouseMode, MouseReport, Screen, ScreenCell, SelectionKind, TermEvent, Terminal,
};
