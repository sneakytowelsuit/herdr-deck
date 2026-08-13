//! Hardware self-test: talk to the deck directly, no daemon and no herdr.
//!
//! # Why this exists
//!
//! The rest of this crate deliberately knows nothing about what a key *means* — it is a dumb
//! frontend of `herdr-deckd`. That is exactly the wrong shape for first-time bring-up: if a
//! freshly-plugged deck does nothing, the failure could be udev permissions, the HID driver, the
//! image format, key numbering, dial polling, or the touchstrip placement, and going through the
//! full herdr -> daemon -> frontend stack changes several of those variables at once. This module
//! collapses the stack to one thing: open the device, paint a known pattern, read raw events, so
//! each variable can be checked in isolation.
//!
//! # Why it still calls into `herdr-deck-core`
//!
//! Painting a *second*, hand-rolled way to rasterise text onto a tile would mean the self-test
//! could pass while the real rendering path was broken, which defeats the point. `TileRenderer`
//! exposes `render_svg` for exactly this — see its doc comment — so the self-test draws through
//! the same font, the same rasteriser, the same code path the daemon uses in production. Only the
//! SVG *content* here is test-pattern-specific.
//!
//! # What this cannot tell you
//!
//! This talks to hidapi and the device directly; it says nothing about the daemon, the frontend
//! protocol, or herdr. A clean self-test does not guarantee the full stack works — see
//! `docs/src/help/hardware-bringup.md` for what to check next.

use std::sync::Arc;

use anyhow::Context as _;
use elgato_streamdeck::asynchronous::{AsyncDeviceStateReader, AsyncStreamDeck};
use elgato_streamdeck::images::ImageRect;
use elgato_streamdeck::info::Kind;
use elgato_streamdeck::{list_devices, new_hidapi, DeviceStateUpdate};
use herdr_deck_core::render::TileRenderer;
use herdr_deck_core::theme::Theme;

use crate::translate::{dial_at_x, strip_segment_x, DeckGeometry};

/// How often to poll the device for input, in Hz.
///
/// Matches the driver's normal poll rate: fast enough that a press feels instant, and there is
/// no reason for the self-test to behave differently from production here.
const POLL_RATE: f32 = 30.0;

/// A handful of high-contrast, easily-named colours.
///
/// These exist only so adjacent keys and strip segments don't blur into each other at a glance —
/// the number printed on the tile is what actually identifies it, never the colour alone.
const PALETTE: [&str; 8] = [
    "#C0392B", "#2471A3", "#1E8449", "#B7950B", "#7D3C98", "#117A65", "#B9770E", "#5D6D7E",
];

/// Run the self-test against the first attached deck.
///
/// Enumerates and opens the device, paints a numbered test pattern on every key and touchstrip
/// segment, then prints every input event until Ctrl-C, at which point it resets the deck and
/// returns. Never touches the daemon socket or herdr.
pub async fn run() -> anyhow::Result<()> {
    println!("herdr-deck self-test: talking to the hardware directly, no daemon, no herdr.");
    println!();

    let hidapi = new_hidapi().context(
        "could not open HID — is the udev rule installed? see docs/help/hardware-bringup.md",
    )?;
    let devices = list_devices(&hidapi);
    if devices.is_empty() {
        anyhow::bail!(
            "no Stream Deck found on USB. Check the cable and that the device shows up in \
             `lsusb`, then see docs/help/hardware-bringup.md for the udev rule."
        );
    }

    println!("found {} device(s):", devices.len());
    for (kind, serial) in &devices {
        println!("  - {} (serial {serial})", describe_kind(*kind));
    }

    let (kind, serial) = devices.into_iter().next().expect("just checked non-empty");
    println!();
    println!("opening {} (serial {serial})...", describe_kind(kind));
    let deck = AsyncStreamDeck::connect(&hidapi, kind, &serial)
        .with_context(|| format!("could not open {} ({serial})", describe_kind(kind)))?;

    let firmware = deck
        .firmware_version()
        .await
        .unwrap_or_else(|_| "unknown".to_string());
    println!("firmware: {firmware}");

    let geometry = geometry_of(kind);
    let key_count = geometry.columns as usize * geometry.rows as usize;
    println!(
        "geometry: {}x{} keys ({} total, {}px), {} dial(s){}",
        geometry.columns,
        geometry.rows,
        key_count,
        geometry.key_image_px,
        geometry.encoders,
        geometry
            .lcd
            .map(|(w, h)| format!(", touchstrip {w}x{h}"))
            .unwrap_or_default()
    );
    println!();

    // A deck left showing whatever it last had on it would be actively misleading here.
    let _ = deck.reset().await;
    let _ = deck.set_brightness(80).await;

    let renderer = TileRenderer::new(Theme::default());

    if geometry.key_image_px > 0 {
        println!("painting {key_count} key(s)...");
        for index in 0..key_count {
            let svg = key_test_svg(index, geometry.columns, geometry.key_image_px);
            let png = renderer
                .render_svg(&svg, geometry.key_image_px, geometry.key_image_px)
                .with_context(|| format!("could not render test tile for key {index}"))?;
            let image = image::load_from_memory(&png)
                .with_context(|| format!("could not decode rendered tile for key {index}"))?;
            deck.set_button_image(index as u8, image)
                .await
                .with_context(|| format!("could not paint key {index}"))?;
        }
        deck.flush()
            .await
            .context("could not flush key images to the device")?;
        println!(
            "done. Keys are numbered 0..{}, left-to-right then top-to-bottom — the same order \
             the daemon addresses them in. Check each physical key shows the number you expect.",
            key_count - 1
        );
    } else {
        println!("this deck has no display (e.g. the Pedal) — nothing to paint on the keys.");
    }

    if let Some((strip_width, strip_height)) = geometry.lcd {
        if geometry.encoders > 0 {
            println!();
            println!("painting {} touchstrip segment(s)...", geometry.encoders);
            for dial in 0..geometry.encoders as usize {
                let segment_width = strip_width / geometry.encoders as u32;
                let svg = dial_test_svg(dial, segment_width, strip_height);
                let png = renderer
                    .render_svg(&svg, segment_width, strip_height)
                    .with_context(|| format!("could not render test tile for dial {dial}"))?;
                let image = image::load_from_memory(&png)
                    .with_context(|| format!("could not decode rendered tile for dial {dial}"))?;
                let rect = ImageRect::from_image(image)
                    .with_context(|| format!("could not prepare LCD rect for dial {dial}"))?;
                let x = strip_segment_x(dial, geometry.encoders, strip_width);
                deck.write_lcd(x as u16, 0, &rect)
                    .await
                    .with_context(|| format!("could not paint touchstrip segment {dial}"))?;
            }
            println!(
                "done. If two dial numbers overlap, or one segment is missing, the segments are \
                 stacking on top of each other rather than tiling — that is exactly the bug this \
                 step exists to catch."
            );
        } else {
            println!();
            println!(
                "this deck reports a touchstrip ({strip_width}x{strip_height}) but no dials — \
                 there is nothing to segment it by, so nothing was painted there. If that is a \
                 surprise for this model, the geometry read off the device is probably wrong."
            );
        }
    }

    println!();
    println!(
        "now press every key, press and twist every dial, and tap the touchstrip. \
         Ctrl-C to exit and reset the deck."
    );
    println!();

    let reader = deck.get_reader();
    tokio::select! {
        result = input_loop(reader, geometry) => {
            // The reader only returns on a read error — the device went away.
            if let Err(e) = result {
                println!();
                println!("device read failed: {e:#}");
            }
        }
        _ = tokio::signal::ctrl_c() => {
            println!();
            println!("Ctrl-C received, resetting the deck...");
        }
    }

    // A diagnostic tool that reports a success it did not have is worse than one that reports
    // nothing — the whole point of this mode is honest hardware reporting.
    match deck.reset().await {
        Ok(()) => println!("deck reset. self-test complete."),
        Err(e) => println!(
            "self-test complete, but the deck did not reset ({e}). \
             It may still be showing the test pattern; unplug and replug it."
        ),
    }
    Ok(())
}

/// Print every input event as it arrives, forever (or until the device goes away).
async fn input_loop(
    reader: Arc<AsyncDeviceStateReader>,
    geometry: DeckGeometry,
) -> anyhow::Result<()> {
    let strip_width = geometry.lcd.map(|(w, _)| w).unwrap_or(0);
    loop {
        let updates = reader.read(POLL_RATE).await?;
        for update in updates {
            match update {
                DeviceStateUpdate::ButtonDown(key) => println!("key {key} down"),
                DeviceStateUpdate::ButtonUp(key) => println!("key {key} up"),
                DeviceStateUpdate::EncoderDown(dial) => println!("dial {dial} down"),
                DeviceStateUpdate::EncoderUp(dial) => println!("dial {dial} up"),
                DeviceStateUpdate::EncoderTwist(dial, ticks) => {
                    // The sign convention is whatever hidapi hands back; print the raw ticks
                    // alongside a label so bring-up can confirm which physical direction is
                    // positive on this particular device, rather than trusting an assumption.
                    let direction = if ticks > 0 {
                        "clockwise"
                    } else {
                        "counter-clockwise"
                    };
                    println!("dial {dial} twist {ticks:+} ({direction})");
                }
                DeviceStateUpdate::TouchPointDown(point) => println!("touch point {point} down"),
                DeviceStateUpdate::TouchPointUp(point) => println!("touch point {point} up"),
                DeviceStateUpdate::TouchScreenPress(x, y) => {
                    match dial_at_x(x as u32, geometry.encoders, strip_width) {
                        Some(dial) => println!("touch ({x}, {y}) -> segment for dial {dial}"),
                        None => println!(
                            "touch ({x}, {y}) -> no dial segment (no touchstrip, or no dials)"
                        ),
                    }
                }
                DeviceStateUpdate::TouchScreenLongPress(x, y) => {
                    println!("touch long-press ({x}, {y})")
                }
                DeviceStateUpdate::TouchScreenSwipe((sx, sy), (ex, ey)) => {
                    println!("touch swipe ({sx}, {sy}) -> ({ex}, {ey})")
                }
            }
        }
    }
}

/// Geometry straight off the device.
///
/// Deliberately duplicated from the driver rather than shared: the driver's copy is private
/// (`driver.rs` is not part of this module's job), and the self-test's whole point is to depend
/// on as little else as possible.
fn geometry_of(kind: Kind) -> DeckGeometry {
    let format = kind.key_image_format();
    DeckGeometry {
        columns: kind.column_count(),
        rows: kind.row_count(),
        key_image_px: format.size.0 as u32,
        encoders: kind.encoder_count(),
        lcd: kind
            .lcd_strip_size()
            .map(|(width, height)| (width as u32, height as u32)),
    }
}

fn describe_kind(kind: Kind) -> String {
    format!("{kind:?}")
}

/// A test tile for one key: its flat index, large and centred, plus row/column so the reading
/// order is unambiguous, on a background colour that changes every key so neighbours are easy to
/// tell apart at a glance.
fn key_test_svg(index: usize, columns: u8, size: u32) -> String {
    let s = size as f32;
    let bg = PALETTE[index % PALETTE.len()];
    let columns = columns.max(1) as usize;
    let row = index / columns;
    let col = index % columns;
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{s}" height="{s}" viewBox="0 0 {s} {s}">
<rect width="{s}" height="{s}" fill="{bg}"/>
<rect x="2" y="2" width="{iw:.2}" height="{iw:.2}" fill="none" stroke="#FFFFFF" stroke-width="2"/>
<text x="{cx:.2}" y="{ny:.2}" font-family="DejaVu Sans" font-size="{nfs:.2}" font-weight="bold" fill="#FFFFFF" text-anchor="middle">{index}</text>
<text x="{cx:.2}" y="{ry:.2}" font-family="DejaVu Sans" font-size="{rfs:.2}" fill="#FFFFFF" text-anchor="middle" opacity="0.85">r{row}c{col}</text>
</svg>"##,
        s = s,
        bg = bg,
        iw = s - 4.0,
        cx = s / 2.0,
        ny = s * 0.55,
        nfs = s * 0.42,
        index = index,
        ry = s * 0.82,
        rfs = s * 0.13,
        row = row,
        col = col,
    )
}

/// A test tile for one touchstrip segment: just its dial number, large, on a background colour
/// distinct from its neighbours — segments stacking on top of each other shows up immediately as
/// overlapping numbers and colours instead of an evenly tiled strip.
fn dial_test_svg(dial: usize, width: u32, height: u32) -> String {
    let w = width as f32;
    let h = height as f32;
    let bg = PALETTE[dial % PALETTE.len()];
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {w} {h}">
<rect width="{w}" height="{h}" fill="{bg}"/>
<rect x="2" y="2" width="{iw:.2}" height="{ih:.2}" fill="none" stroke="#FFFFFF" stroke-width="2"/>
<text x="{cx:.2}" y="{cy:.2}" font-family="DejaVu Sans" font-size="{fs:.2}" font-weight="bold" fill="#FFFFFF" text-anchor="middle">dial {dial}</text>
</svg>"##,
        w = w,
        h = h,
        bg = bg,
        iw = w - 4.0,
        ih = h - 4.0,
        cx = w / 2.0,
        cy = h * 0.6,
        fs = h * 0.35,
        dial = dial,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn is_png(bytes: &[u8]) -> bool {
        bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A])
    }

    #[test]
    fn every_key_tile_renders_through_the_real_renderer_and_contains_its_own_index() {
        // Rendered through `TileRenderer::render_svg`, the exact path the self-test uses on
        // hardware — if this SVG were malformed, that is where it would fail with no way to see
        // why on a machine with a deck plugged in but no way to inspect the intermediate PNG.
        let renderer = TileRenderer::new(Theme::default());
        for index in 0..8 {
            let svg = key_test_svg(index, 4, 120);
            assert!(svg.contains(&format!(">{index}<")), "{svg}");
            let png = renderer
                .render_svg(&svg, 120, 120)
                .unwrap_or_else(|e| panic!("key {index} tile failed to render: {e}"));
            assert!(is_png(&png), "key {index} tile did not produce a PNG");
        }
    }

    #[test]
    fn adjacent_keys_get_different_background_colours() {
        // The whole point of colouring the pattern is that neighbours read as distinct at a
        // glance. Comparing whole SVGs would prove nothing — they always differ by the index
        // number drawn on them — so this compares the fill colours themselves.
        for index in 0..7 {
            let a = background_of(&key_test_svg(index, 4, 120));
            let b = background_of(&key_test_svg(index + 1, 4, 120));
            assert_ne!(
                a,
                b,
                "keys {index} and {} share the background {a}",
                index + 1
            );
        }
    }

    /// The `fill` of the full-bleed background rect, which is the first fill in the document.
    fn background_of(svg: &str) -> String {
        let start = svg.find("fill=\"").expect("the tile has a background fill") + 6;
        let rest = &svg[start..];
        rest[..rest.find('"').expect("unterminated fill")].to_string()
    }

    #[test]
    fn the_palette_itself_has_no_adjacent_duplicates() {
        // The guarantee above only holds while the palette does; a copy-paste slip that
        // repeated a colour would otherwise only surface on real hardware.
        for pair in PALETTE.windows(2) {
            assert_ne!(pair[0], pair[1], "adjacent palette entries must differ");
        }
    }

    #[test]
    fn key_rows_and_columns_match_the_daemons_reading_order() {
        // Mirrors `translate::key_index_of`: row-major, matching how the daemon numbers keys.
        let svg = key_test_svg(7, 4, 120);
        assert!(svg.contains(">r1c3<"), "{svg}");
    }

    #[test]
    fn dial_tiles_render_through_the_real_renderer_and_are_labelled_with_their_own_number() {
        let renderer = TileRenderer::new(Theme::default());
        for dial in 0..4 {
            let svg = dial_test_svg(dial, 200, 100);
            assert!(svg.contains(&format!("dial {dial}")), "{svg}");
            let png = renderer
                .render_svg(&svg, 200, 100)
                .unwrap_or_else(|e| panic!("dial {dial} tile failed to render: {e}"));
            assert!(is_png(&png), "dial {dial} tile did not produce a PNG");
        }
    }

    #[test]
    fn a_plus_reports_geometry_matching_the_daemons_model_table() {
        // Cross-check against `herdr_deck_core::capabilities::DeckModel::Plus`, since a mismatch
        // here would mean the self-test is exercising different geometry than production.
        let geometry = geometry_of(Kind::Plus);
        assert_eq!(geometry.columns, 4);
        assert_eq!(geometry.rows, 2);
        assert_eq!(geometry.encoders, 4);
        assert_eq!(geometry.lcd, Some((800, 100)));
    }
}
