//! Fallback fonts from the operating system for characters egui's bundled
//! fonts lack (CJK, many symbols). Only the first available font of each
//! group is loaded, since CJK fonts are large.

use std::sync::Arc;

use egui::{FontData, FontDefinitions, FontFamily};

/// Groups of candidate paths, most preferred first within each group.
#[cfg(windows)]
const GROUPS: &[&[&str]] = &[
    // Monospace with broad symbol coverage.
    &[
        r"C:\Windows\Fonts\consola.ttf",
        r"C:\Windows\Fonts\cour.ttf",
    ],
    // CJK.
    &[
        r"C:\Windows\Fonts\YuGothM.ttc",
        r"C:\Windows\Fonts\msyh.ttc",
        r"C:\Windows\Fonts\msgothic.ttc",
        r"C:\Windows\Fonts\malgun.ttf",
    ],
    // Symbols.
    &[r"C:\Windows\Fonts\seguisym.ttf"],
];

#[cfg(not(windows))]
const GROUPS: &[&[&str]] = &[
    &[
        "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
        "/usr/share/fonts/dejavu/DejaVuSansMono.ttf",
        "/usr/share/fonts/TTF/DejaVuSansMono.ttf",
    ],
    &[
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/google-noto-cjk/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/truetype/droid/DroidSansFallbackFull.ttf",
        "/usr/share/fonts/truetype/wqy/wqy-zenhei.ttc",
    ],
    &[
        "/usr/share/fonts/truetype/noto/NotoSansSymbols2-Regular.ttf",
        "/usr/share/fonts/noto/NotoSansSymbols2-Regular.ttf",
    ],
];

/// Font definitions with the first available font of each group appended
/// as a fallback to both families. Also returns the paths that were used.
pub fn with_system_fallbacks(mut fonts: FontDefinitions) -> (FontDefinitions, Vec<String>) {
    let mut used = Vec::new();
    for group in GROUPS {
        let Some((path, bytes)) = group
            .iter()
            .find_map(|path| std::fs::read(path).ok().map(|bytes| (*path, bytes)))
        else {
            continue;
        };
        let name = format!("system:{path}");
        fonts
            .font_data
            .insert(name.clone(), Arc::new(FontData::from_owned(bytes)));
        for family in [FontFamily::Monospace, FontFamily::Proportional] {
            fonts.families.entry(family).or_default().push(name.clone());
        }
        used.push(path.to_owned());
    }
    (fonts, used)
}

/// Install system fallbacks into an egui context.
pub fn install(ctx: &egui::Context) {
    let (fonts, _) = with_system_fallbacks(FontDefinitions::default());
    ctx.set_fonts(fonts);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fallbacks_come_after_bundled_fonts() {
        let defaults = FontDefinitions::default();
        let bundled = defaults.families[&FontFamily::Monospace].clone();
        let (fonts, used) = with_system_fallbacks(defaults);
        let mono = &fonts.families[&FontFamily::Monospace];
        assert_eq!(&mono[..bundled.len()], &bundled[..]);
        assert_eq!(mono.len(), bundled.len() + used.len());
        assert!(used.len() <= GROUPS.len());
    }
}
