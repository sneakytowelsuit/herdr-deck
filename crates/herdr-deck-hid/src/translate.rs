//! Hardware description and event translation.
//!
//! Kept free of any HID types so it compiles and tests on every platform. The driver module
//! feeds it geometry read from the device; everything here is pure.

use herdr_deck_core::capabilities::TouchStrip;
use herdr_deck_core::protocol::{DeviceReport, FrontendMessage};

/// The geometry the HID layer reads off a connected deck.
///
/// Deliberately plain numbers rather than the driver's `Kind` enum: a deck the crate does not
/// know about still reports counts, and that is all the daemon needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeckGeometry {
    pub columns: u8,
    pub rows: u8,
    pub key_image_px: u32,
    pub encoders: u8,
    /// Width and height of the touchstrip LCD, when there is one.
    pub lcd: Option<(u32, u32)>,
}

/// Describe a connected deck for the daemon.
pub fn capabilities_for(name: &str, geometry: DeckGeometry) -> DeviceReport {
    DeviceReport {
        // The HID layer identifies hardware by its own `Kind`, which does not map cleanly onto
        // the daemon's model list. Reporting geometry and letting the daemon lay out from that
        // is the whole point of the capability model — so we do not guess a model name.
        model: None,
        model_name: Some(name.to_string()),
        columns: geometry.columns,
        rows: geometry.rows,
        key_image_px: geometry.key_image_px,
        dials: geometry.encoders,
        touchstrip: geometry
            .lcd
            .map(|(width, height)| TouchStrip { width, height }),
    }
}

/// Which touchstrip segment belongs to a dial, as an x offset in pixels.
///
/// The daemon renders one image per dial sized to a segment; the driver has to place it at the
/// right x. Getting this wrong stacks every dial's feedback on top of dial 0.
pub fn strip_segment_x(dial: usize, dials: u8, strip_width: u32) -> u32 {
    if dials == 0 {
        return 0;
    }
    let segment = strip_width / dials as u32;
    segment * dial as u32
}

/// Which dial a touchstrip press at `x` belongs to.
pub fn dial_at_x(x: u32, dials: u8, strip_width: u32) -> Option<usize> {
    if dials == 0 || strip_width == 0 {
        return None;
    }
    let segment = strip_width / dials as u32;
    if segment == 0 {
        return None;
    }
    // Clamp rather than return None for a press on the last pixel column.
    Some(((x / segment) as usize).min(dials as usize - 1))
}

/// A device event, in the form the driver reports it.
///
/// Mirrors the driver crate's update enum but without depending on it, so translation is
/// testable everywhere.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeckEvent {
    ButtonDown(u8),
    ButtonUp(u8),
    EncoderDown(u8),
    EncoderUp(u8),
    EncoderTwist(u8, i8),
    TouchPress(u16, u16),
}

/// Turn a device event into a frontend message.
///
/// Returns `None` for events the daemon has no use for, rather than inventing a message.
pub fn translate(event: DeckEvent, dials: u8, strip_width: u32) -> Option<FrontendMessage> {
    match event {
        DeckEvent::ButtonDown(key) => Some(FrontendMessage::KeyDown {
            index: key as usize,
        }),
        DeckEvent::ButtonUp(key) => Some(FrontendMessage::KeyUp {
            index: key as usize,
        }),
        DeckEvent::EncoderDown(dial) => Some(FrontendMessage::DialDown {
            dial: dial as usize,
        }),
        DeckEvent::EncoderUp(dial) => Some(FrontendMessage::DialUp {
            dial: dial as usize,
        }),
        // A twist of zero carries no information; forwarding it would make the daemon repaint
        // for nothing.
        DeckEvent::EncoderTwist(_, 0) => None,
        DeckEvent::EncoderTwist(dial, ticks) => Some(FrontendMessage::DialRotate {
            dial: dial as usize,
            ticks: ticks as i32,
        }),
        DeckEvent::TouchPress(x, _) => Some(FrontendMessage::TouchTap {
            dial: dial_at_x(x as u32, dials, strip_width),
        }),
    }
}

/// Flat key index from a coordinate, matching the daemon's numbering.
pub fn key_index_of(column: u8, row: u8, columns: u8) -> usize {
    row as usize * columns as usize + column as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plus() -> DeckGeometry {
        DeckGeometry {
            columns: 4,
            rows: 2,
            key_image_px: 120,
            encoders: 4,
            lcd: Some((800, 100)),
        }
    }

    #[test]
    fn a_stream_deck_plus_is_reported_with_its_dials_and_strip() {
        let report = capabilities_for("Stream Deck +", plus());
        assert_eq!(report.columns, 4);
        assert_eq!(report.rows, 2);
        assert_eq!(report.dials, 4);
        assert_eq!(report.key_image_px, 120);
        assert_eq!(
            report.touchstrip,
            Some(TouchStrip {
                width: 800,
                height: 100
            })
        );
    }

    #[test]
    fn geometry_alone_is_enough_for_the_daemon_to_lay_out_unknown_hardware() {
        let report = capabilities_for(
            "Some New Deck",
            DeckGeometry {
                columns: 6,
                rows: 3,
                key_image_px: 128,
                encoders: 2,
                lcd: None,
            },
        );
        let caps = report.to_capabilities();
        assert_eq!(caps.key_count(), 18);
        assert_eq!(caps.dials, 2);
        assert!(!caps.has_touchstrip());
    }

    #[test]
    fn a_deck_with_no_lcd_reports_no_touchstrip() {
        let report = capabilities_for(
            "Stream Deck XL",
            DeckGeometry {
                columns: 8,
                rows: 4,
                key_image_px: 96,
                encoders: 0,
                lcd: None,
            },
        );
        assert_eq!(report.dials, 0);
        assert_eq!(report.touchstrip, None);
    }

    #[test]
    fn strip_segments_tile_the_lcd_without_overlapping() {
        // Getting this wrong stacks every dial's feedback on top of dial 0.
        let xs: Vec<_> = (0..4).map(|d| strip_segment_x(d, 4, 800)).collect();
        assert_eq!(xs, vec![0, 200, 400, 600]);
    }

    #[test]
    fn a_touch_maps_back_to_the_dial_above_it() {
        assert_eq!(dial_at_x(10, 4, 800), Some(0));
        assert_eq!(dial_at_x(250, 4, 800), Some(1));
        assert_eq!(dial_at_x(450, 4, 800), Some(2));
        assert_eq!(dial_at_x(799, 4, 800), Some(3));
    }

    #[test]
    fn a_touch_on_the_last_pixel_clamps_rather_than_falling_off_the_end() {
        assert_eq!(dial_at_x(800, 4, 800), Some(3));
        assert_eq!(dial_at_x(100_000, 4, 800), Some(3));
    }

    #[test]
    fn a_deck_with_no_dials_has_no_strip_segments() {
        assert_eq!(dial_at_x(10, 0, 800), None);
        assert_eq!(strip_segment_x(0, 0, 800), 0);
    }

    #[test]
    fn button_events_become_key_messages_with_the_same_index() {
        assert_eq!(
            translate(DeckEvent::ButtonDown(3), 4, 800),
            Some(FrontendMessage::KeyDown { index: 3 })
        );
        assert_eq!(
            translate(DeckEvent::ButtonUp(3), 4, 800),
            Some(FrontendMessage::KeyUp { index: 3 })
        );
    }

    #[test]
    fn encoder_events_become_dial_messages() {
        assert_eq!(
            translate(DeckEvent::EncoderDown(2), 4, 800),
            Some(FrontendMessage::DialDown { dial: 2 })
        );
        assert_eq!(
            translate(DeckEvent::EncoderTwist(1, -3), 4, 800),
            Some(FrontendMessage::DialRotate { dial: 1, ticks: -3 })
        );
    }

    #[test]
    fn a_zero_tick_twist_is_dropped_rather_than_causing_a_pointless_repaint() {
        assert_eq!(translate(DeckEvent::EncoderTwist(1, 0), 4, 800), None);
    }

    #[test]
    fn a_touchstrip_press_targets_the_dial_it_sits_above() {
        assert_eq!(
            translate(DeckEvent::TouchPress(450, 50), 4, 800),
            Some(FrontendMessage::TouchTap { dial: Some(2) })
        );
    }

    #[test]
    fn key_indices_match_the_daemons_reading_order() {
        assert_eq!(key_index_of(0, 0, 4), 0);
        assert_eq!(key_index_of(3, 1, 4), 7);
        assert_eq!(key_index_of(7, 3, 8), 31);
    }
}
