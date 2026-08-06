//! Talking to the hardware.
//!
//! Connects to `herdr-deckd`'s frontend socket exactly like the macOS plugin does, then paints
//! whatever the daemon sends onto the device and forwards input back. Keeping it a *client* of
//! the daemon rather than a special in-process path means there is one protocol, one set of
//! semantics, and one place where key meaning is decided.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as _;
use base64::Engine as _;
use elgato_streamdeck::asynchronous::AsyncStreamDeck;
use elgato_streamdeck::images::ImageRect;
use elgato_streamdeck::info::Kind;
use elgato_streamdeck::{list_devices, new_hidapi, DeviceStateUpdate};
use herdr_deck_core::protocol::{DaemonMessage, FrontendMessage, FRONTEND_PROTOCOL};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

use crate::translate::{capabilities_for, strip_segment_x, translate, DeckEvent, DeckGeometry};

/// How often to poll the device for input, in seconds.
///
/// 30Hz: fast enough that a key press feels instant, slow enough to stay invisible in `top`.
const POLL_RATE: f32 = 30.0;

/// How long to wait before looking for a deck again after one goes away.
const RESCAN_INTERVAL: Duration = Duration::from_secs(3);

/// Every deck currently attached, as (human name, serial).
pub fn list_decks() -> anyhow::Result<Vec<(String, String)>> {
    let hidapi = new_hidapi().context("could not open HID; is a udev rule installed?")?;
    Ok(list_devices(&hidapi)
        .into_iter()
        .map(|(kind, serial)| (describe_kind(kind), serial))
        .collect())
}

/// Drive the first attached deck, reconnecting as hardware and the daemon come and go.
///
/// Never returns under normal operation.
pub async fn run(socket_path: PathBuf) -> anyhow::Result<()> {
    loop {
        match attach_once(&socket_path).await {
            Ok(()) => tracing::info!("deck detached; waiting for it to come back"),
            Err(e) => tracing::debug!(error = %e, "hid frontend stopped; retrying"),
        }
        tokio::time::sleep(RESCAN_INTERVAL).await;
    }
}

async fn attach_once(socket_path: &Path) -> anyhow::Result<()> {
    let hidapi = new_hidapi().context("could not open HID")?;
    let devices = list_devices(&hidapi);
    let Some((kind, serial)) = devices.into_iter().next() else {
        anyhow::bail!("no Stream Deck found");
    };

    let deck = AsyncStreamDeck::connect(&hidapi, kind, &serial)
        .with_context(|| format!("could not open {} ({serial})", describe_kind(kind)))?;
    let deck = Arc::new(deck);

    let geometry = geometry_of(kind);
    let name = describe_kind(kind);
    tracing::info!(
        deck = %name,
        keys = geometry.columns as u16 * geometry.rows as u16,
        dials = geometry.encoders,
        "opened deck"
    );

    // A deck left showing a previous session's tiles would be actively misleading.
    let _ = deck.reset().await;
    let _ = deck.set_brightness(80).await;

    let stream = UnixStream::connect(socket_path)
        .await
        .with_context(|| format!("could not reach herdr-deckd at {}", socket_path.display()))?;
    let (read_half, mut write_half) = stream.into_split();

    let report = capabilities_for(&name, geometry);
    let hello = FrontendMessage::Hello {
        frontend: "linux-hid".to_string(),
        device: report,
        protocol: Some(FRONTEND_PROTOCOL),
    };
    write_message(&mut write_half, &hello).await?;

    // Input: device → daemon.
    let input_deck = Arc::clone(&deck);
    let strip_width = geometry.lcd.map(|(w, _)| w).unwrap_or(0);
    let dials = geometry.encoders;
    let input = tokio::spawn(async move {
        let reader = input_deck.get_reader();
        loop {
            let updates = match reader.read(POLL_RATE).await {
                Ok(updates) => updates,
                Err(e) => {
                    tracing::debug!(error = %e, "deck read failed");
                    return;
                }
            };
            for update in updates {
                let Some(event) = to_event(update) else { continue };
                let Some(message) = translate(event, dials, strip_width) else {
                    continue;
                };
                if write_message(&mut write_half, &message).await.is_err() {
                    return;
                }
            }
        }
    });

    // Output: daemon → device.
    let paint = paint_loop(read_half, Arc::clone(&deck), geometry).await;

    input.abort();
    paint
}

async fn paint_loop(
    read_half: tokio::net::unix::OwnedReadHalf,
    deck: Arc<AsyncStreamDeck>,
    geometry: DeckGeometry,
) -> anyhow::Result<()> {
    let mut lines = BufReader::new(read_half).lines();
    while let Some(line) = lines.next_line().await? {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let message: DaemonMessage = match serde_json::from_str(line) {
            Ok(message) => message,
            Err(e) => {
                // One unparseable line must not take the deck down.
                tracing::warn!(error = %e, "ignoring unparseable daemon message");
                continue;
            }
        };

        match message {
            DaemonMessage::Ready { device, keys, .. } => {
                tracing::info!(%device, keys, "daemon ready");
            }

            DaemonMessage::SetKeyImage { index, png } => {
                if let Err(e) = draw_key(&deck, index, &png).await {
                    tracing::warn!(index, error = %e, "could not draw key");
                }
            }

            DaemonMessage::SetDialFeedback { dial, png, .. } => {
                // Only the touchstrip renders dial feedback here; a deck with encoders but no
                // strip simply has nothing to draw on.
                if let (Some(png), Some((strip_width, _))) = (png, geometry.lcd) {
                    if let Err(e) =
                        draw_strip(&deck, dial, &png, geometry.encoders, strip_width).await
                    {
                        tracing::warn!(dial, error = %e, "could not draw touchstrip");
                    }
                }
            }

            // The deck has no "flash ok" affordance of its own, and blinking a tile would fight
            // with the status the key is showing. The daemon already logs these.
            DaemonMessage::Ok { .. } => {}
            DaemonMessage::Alert { index, message } => {
                tracing::warn!(index, %message, "action did not fully succeed");
            }

            DaemonMessage::ProtocolMismatch { expected, got } => {
                anyhow::bail!(
                    "herdr-deckd speaks frontend protocol {expected}, this frontend speaks {got}"
                );
            }

            DaemonMessage::Pong => {}
        }
    }
    Ok(())
}

async fn draw_key(deck: &AsyncStreamDeck, index: usize, png_base64: &str) -> anyhow::Result<()> {
    let bytes = base64::engine::general_purpose::STANDARD.decode(png_base64)?;
    let image = image::load_from_memory(&bytes)?;
    deck.set_button_image(index as u8, image).await?;
    Ok(())
}

async fn draw_strip(
    deck: &AsyncStreamDeck,
    dial: usize,
    png_base64: &str,
    dials: u8,
    strip_width: u32,
) -> anyhow::Result<()> {
    let bytes = base64::engine::general_purpose::STANDARD.decode(png_base64)?;
    let image = image::load_from_memory(&bytes)?;
    let rect = ImageRect::from_image(image)?;
    let x = strip_segment_x(dial, dials, strip_width);
    deck.write_lcd(x as u16, 0, &rect).await?;
    Ok(())
}

async fn write_message(
    writer: &mut tokio::net::unix::OwnedWriteHalf,
    message: &FrontendMessage,
) -> anyhow::Result<()> {
    let mut line = serde_json::to_string(message)?;
    line.push('\n');
    writer.write_all(line.as_bytes()).await?;
    writer.flush().await?;
    Ok(())
}

/// Map the driver's update enum onto our platform-independent one.
fn to_event(update: DeviceStateUpdate) -> Option<DeckEvent> {
    match update {
        DeviceStateUpdate::ButtonDown(key) => Some(DeckEvent::ButtonDown(key)),
        DeviceStateUpdate::ButtonUp(key) => Some(DeckEvent::ButtonUp(key)),
        DeviceStateUpdate::EncoderDown(dial) => Some(DeckEvent::EncoderDown(dial)),
        DeviceStateUpdate::EncoderUp(dial) => Some(DeckEvent::EncoderUp(dial)),
        DeviceStateUpdate::EncoderTwist(dial, ticks) => Some(DeckEvent::EncoderTwist(dial, ticks)),
        DeviceStateUpdate::TouchScreenPress(x, y) => Some(DeckEvent::TouchPress(x, y)),
        // A long press and a swipe are not bound to anything, and guessing at a meaning would
        // be worse than ignoring them.
        _ => None,
    }
}

/// Geometry straight off the device.
fn geometry_of(kind: Kind) -> DeckGeometry {
    let format = kind.key_image_format();
    DeckGeometry {
        columns: kind.column_count(),
        rows: kind.row_count(),
        // Keys are square on every current model; take the width.
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
