//! Box-drawing (U+2500–U+257F) and block (U+2580–U+259F) characters are
//! drawn as shapes instead of font glyphs, so lines join seamlessly across
//! cells regardless of the font's metrics.

use egui::{Color32, Painter, Pos2, Rect, Stroke, pos2, vec2};

/// Line weight in one direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Weight {
    None,
    Light,
    Heavy,
    Double,
}

/// Arms from the cell center: up, down, left, right.
pub type Arms = [Weight; 4];

/// The line arms of a box-drawing character, if it is one we draw.
pub fn arms(ch: char) -> Option<Arms> {
    use Weight::{Double as D, Heavy as H, Light as L, None as N};
    Some(match ch {
        // Light and heavy lines (dashed variants are drawn solid).
        '─' | '┄' | '┈' | '╌' => [N, N, L, L],
        '━' | '┅' | '┉' | '╍' => [N, N, H, H],
        '│' | '┆' | '┊' | '╎' => [L, L, N, N],
        '┃' | '┇' | '┋' | '╏' => [H, H, N, N],
        '┌' | '╭' => [N, L, N, L],
        '┐' | '╮' => [N, L, L, N],
        '└' | '╰' => [L, N, N, L],
        '┘' | '╯' => [L, N, L, N],
        '├' => [L, L, N, L],
        '┤' => [L, L, L, N],
        '┬' => [N, L, L, L],
        '┴' => [L, N, L, L],
        '┼' => [L, L, L, L],
        '┏' => [N, H, N, H],
        '┓' => [N, H, H, N],
        '┗' => [H, N, N, H],
        '┛' => [H, N, H, N],
        '┣' => [H, H, N, H],
        '┫' => [H, H, H, N],
        '┳' => [N, H, H, H],
        '┻' => [H, N, H, H],
        '╋' => [H, H, H, H],
        // Mixed light/heavy (the common ones).
        '┝' => [L, L, N, H],
        '┥' => [L, L, H, N],
        '┠' => [H, H, N, L],
        '┨' => [H, H, L, N],
        '┯' => [N, L, H, H],
        '┷' => [L, N, H, H],
        '┰' => [N, H, L, L],
        '┸' => [H, N, L, L],
        '┿' => [L, L, H, H],
        '╂' => [H, H, L, L],
        // Double lines.
        '═' => [N, N, D, D],
        '║' => [D, D, N, N],
        '╔' => [N, D, N, D],
        '╗' => [N, D, D, N],
        '╚' => [D, N, N, D],
        '╝' => [D, N, D, N],
        '╠' => [D, D, N, D],
        '╣' => [D, D, D, N],
        '╦' => [N, D, D, D],
        '╩' => [D, N, D, D],
        '╬' => [D, D, D, D],
        '╒' => [N, L, N, D],
        '╕' => [N, L, D, N],
        '╘' => [L, N, N, D],
        '╛' => [L, N, D, N],
        '╓' => [N, D, N, L],
        '╖' => [N, D, L, N],
        '╙' => [D, N, N, L],
        '╜' => [D, N, L, N],
        '╞' => [L, L, N, D],
        '╡' => [L, L, D, N],
        '╟' => [D, D, N, L],
        '╢' => [D, D, L, N],
        '╤' => [N, L, D, D],
        '╧' => [L, N, D, D],
        '╥' => [N, D, L, L],
        '╨' => [D, N, L, L],
        '╪' => [L, L, D, D],
        '╫' => [D, D, L, L],
        // Half lines.
        '╴' => [N, N, L, N],
        '╵' => [L, N, N, N],
        '╶' => [N, N, N, L],
        '╷' => [N, L, N, N],
        '╸' => [N, N, H, N],
        '╹' => [H, N, N, N],
        '╺' => [N, N, N, H],
        '╻' => [N, H, N, N],
        _ => return None,
    })
}

/// A block element as a filled fraction of the cell: (x0, y0, x1, y1) in
/// 0..=1, plus an opacity for shades.
pub fn block(ch: char) -> Option<([f32; 4], f32)> {
    let eighth = |n: u32| n as f32 / 8.0;
    Some(match ch {
        '█' => ([0.0, 0.0, 1.0, 1.0], 1.0),
        '▀' => ([0.0, 0.0, 1.0, 0.5], 1.0),
        '▄' => ([0.0, 0.5, 1.0, 1.0], 1.0),
        '▌' => ([0.0, 0.0, 0.5, 1.0], 1.0),
        '▐' => ([0.5, 0.0, 1.0, 1.0], 1.0),
        '░' => ([0.0, 0.0, 1.0, 1.0], 0.25),
        '▒' => ([0.0, 0.0, 1.0, 1.0], 0.5),
        '▓' => ([0.0, 0.0, 1.0, 1.0], 0.75),
        '▔' => ([0.0, 0.0, 1.0, eighth(1)], 1.0),
        '▕' => ([eighth(7), 0.0, 1.0, 1.0], 1.0),
        // Lower eighths ▁▂▃▄▅▆▇ and left eighths ▏▎▍▌▋▊▉.
        '\u{2581}'..='\u{2587}' => {
            let n = ch as u32 - 0x2580;
            ([0.0, 1.0 - eighth(n), 1.0, 1.0], 1.0)
        }
        '\u{2589}'..='\u{258f}' => {
            let n = 0x2590 - ch as u32;
            ([0.0, 0.0, eighth(n), 1.0], 1.0)
        }
        _ => return None,
    })
}

/// Draw `ch` into `rect` if it is a box or block character.
pub fn draw(painter: &Painter, rect: Rect, ch: char, color: Color32) -> bool {
    if let Some((frac, alpha)) = block(ch) {
        let min = rect.min + vec2(frac[0] * rect.width(), frac[1] * rect.height());
        let max = rect.min + vec2(frac[2] * rect.width(), frac[3] * rect.height());
        painter.rect_filled(
            Rect::from_min_max(min, max),
            0.0,
            color.gamma_multiply(alpha),
        );
        return true;
    }
    let Some(arms) = arms(ch) else {
        return false;
    };
    let light = (rect.width() / 8.0).round().max(1.0);
    let heavy = light * 2.0;
    let gap = light; // half-distance between the strokes of a double line
    let center = rect.center();
    let ends: [Pos2; 4] = [
        pos2(center.x, rect.top()),
        pos2(center.x, rect.bottom()),
        pos2(rect.left(), center.y),
        pos2(rect.right(), center.y),
    ];
    let has_double = arms.contains(&Weight::Double);
    for (i, weight) in arms.iter().enumerate() {
        let vertical = i < 2;
        let end = ends[i];
        match weight {
            Weight::None => {}
            Weight::Light | Weight::Heavy => {
                let width = if *weight == Weight::Heavy {
                    heavy
                } else {
                    light
                };
                // Extend past the center so joins have no notch; with double
                // arms present, reach the outer double stroke.
                let reach = if has_double {
                    gap + light / 2.0
                } else {
                    width / 2.0
                };
                let start = if vertical {
                    pos2(center.x, center.y + if i == 0 { reach } else { -reach })
                } else {
                    pos2(center.x + if i == 2 { reach } else { -reach }, center.y)
                };
                painter.line_segment([start, end], Stroke::new(width, color));
            }
            Weight::Double => {
                // Unit vector along the arm and across it.
                let dir = (end - center).normalized();
                let across = vec2(-dir.y, dir.x).abs();
                for side in [-1.0f32, 1.0] {
                    // The perpendicular arm on this stroke's side: up/left
                    // for side -1, down/right for side +1.
                    let perpendicular = if vertical {
                        arms[if side < 0.0 { 2 } else { 3 }]
                    } else {
                        arms[if side < 0.0 { 0 } else { 1 }]
                    };
                    // Stop short of the center to leave room for that arm
                    // (inner corner), or run past it (outer corner/straight).
                    let t = if perpendicular == Weight::None {
                        -gap
                    } else {
                        gap
                    };
                    let offset = across * side * gap;
                    let start = center + dir * t + offset;
                    painter.line_segment([start, end + offset], Stroke::new(light, color));
                }
            }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_characters() {
        use Weight::*;
        assert_eq!(arms('─'), Some([None, None, Light, Light]));
        assert_eq!(arms('╋'), Some([Heavy; 4]));
        assert_eq!(arms('╔'), Some([None, Double, None, Double]));
        assert_eq!(arms('a'), Option::None);
        assert_eq!(block('█'), Some(([0.0, 0.0, 1.0, 1.0], 1.0)));
        assert_eq!(block('▁'), Some(([0.0, 0.875, 1.0, 1.0], 1.0)));
        assert_eq!(block('▏'), Some(([0.0, 0.0, 0.125, 1.0], 1.0)));
        assert_eq!(block('▒').unwrap().1, 0.5);
        assert_eq!(block('x'), Option::None);
    }

    #[test]
    fn every_box_char_in_range_is_known_or_skipped() {
        // Any character we claim must have at least one arm.
        for c in '\u{2500}'..='\u{257f}' {
            if let Some(a) = arms(c) {
                assert!(a.iter().any(|w| *w != Weight::None), "{c}");
            }
        }
    }
}
