//! Generating the Stream Deck plugin's icon set.
//!
//! Elgato requires a specific set of images at specific sizes, each in 1x and 2x. Rather than
//! commit a dozen opaque PNGs nobody can regenerate, they are rendered from SVG by the same
//! renderer that draws the deck — so the icons and the tiles share one visual language, and
//! changing the palette updates both.
//!
//! Run `herdr-deck icons --out plugin/com.sneakytowelsuit.herdr-deck.sdPlugin/imgs` after touching the theme.

use std::path::Path;

use herdr_deck_core::render::TileRenderer;
use herdr_deck_core::theme::Theme;

/// One image the manifest refers to, and the size Elgato wants it at.
struct IconSpec {
    /// Path relative to the output directory, without extension or `@2x` suffix.
    name: &'static str,
    size: u32,
    kind: Kind,
}

enum Kind {
    /// The plugin tile in the Stream Deck app.
    Plugin,
    /// A monochrome-ish glyph for the actions list and category.
    Glyph,
    /// The default image shown on an unconfigured key.
    KeyState,
    /// The circular canvas representing a dial.
    Encoder,
}

const ICONS: &[IconSpec] = &[
    IconSpec {
        name: "plugin",
        size: 288,
        kind: Kind::Plugin,
    },
    IconSpec {
        name: "category",
        size: 28,
        kind: Kind::Glyph,
    },
    IconSpec {
        name: "actions/key",
        size: 20,
        kind: Kind::Glyph,
    },
    IconSpec {
        name: "actions/dial",
        size: 20,
        kind: Kind::Glyph,
    },
    IconSpec {
        name: "actions/key-state",
        size: 72,
        kind: Kind::KeyState,
    },
    IconSpec {
        name: "actions/dial-encoder",
        size: 72,
        kind: Kind::Encoder,
    },
];

/// Write every icon, at 1x and 2x, under `out`.
pub fn generate(out: &Path) -> anyhow::Result<Vec<String>> {
    let renderer = TileRenderer::new(Theme::Dark);
    let mut written = Vec::new();

    for spec in ICONS {
        // Elgato wants both a 1x and a @2x for every image.
        for (suffix, scale) in [("", 1), ("@2x", 2)] {
            let size = spec.size * scale;
            let svg = match spec.kind {
                Kind::Plugin => plugin_svg(size),
                Kind::Glyph => glyph_svg(size),
                Kind::KeyState => key_state_svg(size),
                Kind::Encoder => encoder_svg(size),
            };
            let png = renderer.render_svg(&svg, size, size)?;
            let path = out.join(format!("{}{suffix}.png", spec.name));
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&path, png)?;
            written.push(path.display().to_string());
        }
    }
    Ok(written)
}

/// The mark: a dark rounded tile with the "needs you" dot. It is the one state the whole
/// product exists to surface, so it is what the icon shows.
fn plugin_svg(size: u32) -> String {
    let s = size as f32;
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{s}" height="{s}" viewBox="0 0 {s} {s}">
<rect width="{s}" height="{s}" rx="{r:.2}" fill="#15161A"/>
<rect x="{gx:.2}" y="{gy:.2}" width="{gw:.2}" height="{gh:.2}" rx="{gr:.2}" fill="#B4242A"/>
<rect x="{gx2:.2}" y="{gy:.2}" width="{gw:.2}" height="{gh:.2}" rx="{gr:.2}" fill="#8A5A00"/>
<rect x="{gx:.2}" y="{gy2:.2}" width="{gw:.2}" height="{gh:.2}" rx="{gr:.2}" fill="#2A2D34"/>
<rect x="{gx2:.2}" y="{gy2:.2}" width="{gw:.2}" height="{gh:.2}" rx="{gr:.2}" fill="#2A2D34"/>
<circle cx="{cx:.2}" cy="{cy:.2}" r="{cr:.2}" fill="#15161A"/>
<circle cx="{cx:.2}" cy="{cy:.2}" r="{cr2:.2}" fill="#FF8A8F"/>
</svg>"##,
        s = s,
        r = s * 0.22,
        gx = s * 0.17,
        gx2 = s * 0.53,
        gy = s * 0.17,
        gy2 = s * 0.53,
        gw = s * 0.30,
        gh = s * 0.30,
        gr = s * 0.07,
        cx = s * 0.5,
        cy = s * 0.5,
        cr = s * 0.19,
        cr2 = s * 0.115,
    )
}

/// A small, high-contrast glyph for lists where the image is tiny.
fn glyph_svg(size: u32) -> String {
    let s = size as f32;
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{s}" height="{s}" viewBox="0 0 {s} {s}">
<rect x="{x:.2}" y="{y:.2}" width="{w:.2}" height="{w:.2}" rx="{r:.2}" fill="none" stroke="#D8DBE2" stroke-width="{sw:.2}"/>
<circle cx="{cx:.2}" cy="{cy:.2}" r="{cr:.2}" fill="#D8DBE2"/>
</svg>"##,
        s = s,
        x = s * 0.14,
        y = s * 0.14,
        w = s * 0.72,
        r = s * 0.16,
        sw = (s * 0.075).max(1.0),
        cx = s * 0.5,
        cy = s * 0.5,
        cr = s * 0.17,
    )
}

/// What an unconfigured key shows before the daemon paints it.
///
/// It says the daemon is not connected yet, because that is the most likely reason a key is
/// still showing this image.
fn key_state_svg(size: u32) -> String {
    let s = size as f32;
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{s}" height="{s}" viewBox="0 0 {s} {s}">
<rect width="{s}" height="{s}" fill="#0E0F12"/>
<circle cx="{cx:.2}" cy="{cy:.2}" r="{cr:.2}" fill="none" stroke="#3A3E46" stroke-width="{sw:.2}"/>
<text x="{cx:.2}" y="{ty:.2}" font-family="DejaVu Sans" font-size="{fs:.2}" fill="#71767F" text-anchor="middle">herdr</text>
</svg>"##,
        s = s,
        cx = s * 0.5,
        cy = s * 0.40,
        cr = s * 0.17,
        sw = (s * 0.04).max(1.0),
        ty = s * 0.82,
        fs = s * 0.17,
    )
}

/// The circular canvas shown for a dial in the Stream Deck app.
fn encoder_svg(size: u32) -> String {
    let s = size as f32;
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{s}" height="{s}" viewBox="0 0 {s} {s}">
<circle cx="{cx:.2}" cy="{cx:.2}" r="{outer:.2}" fill="#15161A"/>
<circle cx="{cx:.2}" cy="{cx:.2}" r="{ring:.2}" fill="none" stroke="#4C9AFF" stroke-width="{sw:.2}"
        stroke-dasharray="{dash:.2} {gap:.2}"/>
<circle cx="{cx:.2}" cy="{cx:.2}" r="{inner:.2}" fill="#B4242A"/>
</svg>"##,
        s = s,
        cx = s * 0.5,
        outer = s * 0.46,
        ring = s * 0.34,
        sw = (s * 0.08).max(1.0),
        dash = s * 0.09,
        gap = s * 0.06,
        inner = s * 0.16,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn is_png(bytes: &[u8]) -> bool {
        bytes.starts_with(&[0x89, b'P', b'N', b'G'])
    }

    #[test]
    fn generates_every_icon_at_one_and_two_x() {
        let dir = tempfile::tempdir().unwrap();
        let written = generate(dir.path()).unwrap();
        // Two files (1x and @2x) for every spec.
        assert_eq!(written.len(), ICONS.len() * 2);
        for spec in ICONS {
            for suffix in ["", "@2x"] {
                let path = dir.path().join(format!("{}{suffix}.png", spec.name));
                assert!(path.exists(), "missing {}", path.display());
                let bytes = std::fs::read(&path).unwrap();
                assert!(is_png(&bytes), "{} is not a PNG", path.display());
            }
        }
    }

    #[test]
    fn the_two_x_image_is_twice_the_size() {
        // Elgato scales by filename convention; a wrongly-sized @2x renders blurry.
        let dir = tempfile::tempdir().unwrap();
        generate(dir.path()).unwrap();
        let one = std::fs::read(dir.path().join("plugin.png")).unwrap();
        let two = std::fs::read(dir.path().join("plugin@2x.png")).unwrap();
        assert_eq!(png_size(&one), (288, 288));
        assert_eq!(png_size(&two), (576, 576));
    }

    #[test]
    fn nested_action_icons_get_their_directory_created() {
        let dir = tempfile::tempdir().unwrap();
        generate(dir.path()).unwrap();
        assert!(dir.path().join("actions").join("key.png").exists());
    }

    /// Read width and height out of a PNG's IHDR.
    fn png_size(bytes: &[u8]) -> (u32, u32) {
        let width = u32::from_be_bytes(bytes[16..20].try_into().unwrap());
        let height = u32::from_be_bytes(bytes[20..24].try_into().unwrap());
        (width, height)
    }
}
