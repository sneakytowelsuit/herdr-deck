//! What the attached hardware can actually do.
//!
//! Everything above this module is written against [`DeckCapabilities`] rather than against a
//! model name, so a deck we have never heard of still gets a sensible layout as long as the
//! frontend can report its key count and image size. That is the whole point: the layout engine
//! asks "how many keys, any dials, any touchstrip", never "is this a Stream Deck +".

use serde::{Deserialize, Serialize};

/// Known Stream Deck models.
///
/// [`DeckModel::Unknown`] is a first-class citizen: the Linux HID frontend can enumerate a
/// device we do not recognise, and the macOS frontend reports a numeric device type that may be
/// newer than this list. In both cases the frontend supplies the raw geometry and we lay out
/// against that.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeckModel {
    /// 15 keys, 5x3. The classic, and the MK.2.
    Original,
    /// 6 keys, 3x2.
    Mini,
    /// 32 keys, 8x4.
    Xl,
    /// 8 keys, 4x2, plus 4 dials and a touchstrip. The only model with rotary encoders.
    Plus,
    /// 8 keys, 4x2, plus two touch points.
    Neo,
    /// 3 pedals, no screens at all.
    Pedal,
    Unknown,
}

impl DeckModel {
    pub fn as_str(self) -> &'static str {
        match self {
            DeckModel::Original => "Stream Deck",
            DeckModel::Mini => "Stream Deck Mini",
            DeckModel::Xl => "Stream Deck XL",
            DeckModel::Plus => "Stream Deck +",
            DeckModel::Neo => "Stream Deck Neo",
            DeckModel::Pedal => "Stream Deck Pedal",
            DeckModel::Unknown => "Stream Deck (unrecognised model)",
        }
    }

    /// The stock geometry for this model.
    pub fn capabilities(self) -> DeckCapabilities {
        let (columns, rows, key_px, dials, strip) = match self {
            DeckModel::Original => (5, 3, 72, 0, None),
            DeckModel::Mini => (3, 2, 80, 0, None),
            DeckModel::Xl => (8, 4, 96, 0, None),
            DeckModel::Plus => (4, 2, 120, 4, Some(TouchStrip { width: 800, height: 100 })),
            DeckModel::Neo => (4, 2, 96, 0, None),
            DeckModel::Pedal => (3, 1, 0, 0, None),
            DeckModel::Unknown => (5, 3, 72, 0, None),
        };
        DeckCapabilities {
            model: self,
            model_name: self.as_str().to_string(),
            columns,
            rows,
            key_image_px: key_px,
            dials,
            touchstrip: strip,
        }
    }
}

/// The touchstrip LCD above the dials on a Stream Deck +.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TouchStrip {
    pub width: u32,
    pub height: u32,
}

impl TouchStrip {
    /// The strip is divided into one segment per dial.
    pub fn segment_width(&self, dials: u8) -> u32 {
        if dials == 0 {
            self.width
        } else {
            self.width / dials as u32
        }
    }
}

/// The geometry and controls of an attached deck.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeckCapabilities {
    pub model: DeckModel,
    pub model_name: String,
    pub columns: u8,
    pub rows: u8,
    /// Native key image edge length in pixels. Keys are square on every current model.
    pub key_image_px: u32,
    pub dials: u8,
    pub touchstrip: Option<TouchStrip>,
}

impl DeckCapabilities {
    pub fn key_count(&self) -> usize {
        self.columns as usize * self.rows as usize
    }

    pub fn has_dials(&self) -> bool {
        self.dials > 0
    }

    pub fn has_touchstrip(&self) -> bool {
        self.touchstrip.is_some()
    }

    /// Can this device display anything at all? A Pedal cannot.
    pub fn has_display(&self) -> bool {
        self.key_image_px > 0
    }

    /// Build capabilities from raw geometry reported by a frontend, for hardware we do not
    /// have a model entry for.
    pub fn from_geometry(
        model_name: impl Into<String>,
        columns: u8,
        rows: u8,
        key_image_px: u32,
        dials: u8,
        touchstrip: Option<TouchStrip>,
    ) -> Self {
        Self {
            model: DeckModel::Unknown,
            model_name: model_name.into(),
            columns,
            rows,
            key_image_px,
            dials,
            touchstrip,
        }
    }

    /// Map the Elgato SDK's numeric device type to a model.
    ///
    /// The macOS frontend receives this in `deviceDidConnect`. Types we do not recognise fall
    /// through to `Unknown`, and the frontend's reported `size.columns`/`size.rows` are used
    /// instead — so a deck released after this code was written still works.
    pub fn from_sdk_device_type(device_type: u32) -> Option<DeckModel> {
        match device_type {
            0 => Some(DeckModel::Original),
            1 => Some(DeckModel::Mini),
            2 => Some(DeckModel::Xl),
            5 => Some(DeckModel::Pedal),
            7 => Some(DeckModel::Plus),
            9 => Some(DeckModel::Neo),
            _ => None,
        }
    }

    /// A one-line summary for `herdr-deck doctor`.
    pub fn describe(&self) -> String {
        let mut parts = vec![format!(
            "{} — {}x{} keys ({}px)",
            self.model_name, self.columns, self.rows, self.key_image_px
        )];
        if self.has_dials() {
            parts.push(format!("{} dials", self.dials));
        }
        if let Some(strip) = self.touchstrip {
            parts.push(format!("touchstrip {}x{}", strip.width, strip.height));
        }
        parts.join(", ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_deck_plus_has_the_controls_we_build_the_default_profile_around() {
        let caps = DeckModel::Plus.capabilities();
        assert_eq!(caps.key_count(), 8);
        assert_eq!(caps.dials, 4);
        assert!(caps.has_touchstrip());
    }

    #[test]
    fn dial_less_models_report_no_dials() {
        for model in [DeckModel::Original, DeckModel::Mini, DeckModel::Xl, DeckModel::Neo] {
            let caps = model.capabilities();
            assert!(!caps.has_dials(), "{model:?} should have no dials");
            assert!(!caps.has_touchstrip(), "{model:?} should have no touchstrip");
        }
    }

    #[test]
    fn key_counts_match_the_published_hardware() {
        assert_eq!(DeckModel::Original.capabilities().key_count(), 15);
        assert_eq!(DeckModel::Mini.capabilities().key_count(), 6);
        assert_eq!(DeckModel::Xl.capabilities().key_count(), 32);
        assert_eq!(DeckModel::Plus.capabilities().key_count(), 8);
        assert_eq!(DeckModel::Neo.capabilities().key_count(), 8);
    }

    #[test]
    fn a_pedal_has_no_display_so_nothing_should_try_to_render_to_it() {
        assert!(!DeckModel::Pedal.capabilities().has_display());
    }

    #[test]
    fn touchstrip_splits_evenly_across_dials() {
        let caps = DeckModel::Plus.capabilities();
        let strip = caps.touchstrip.unwrap();
        assert_eq!(strip.segment_width(caps.dials), 200);
    }

    #[test]
    fn unknown_hardware_can_still_be_described_by_raw_geometry() {
        let caps = DeckCapabilities::from_geometry("Future Deck", 6, 3, 128, 2, None);
        assert_eq!(caps.key_count(), 18);
        assert_eq!(caps.model, DeckModel::Unknown);
        assert!(caps.has_dials());
    }

    #[test]
    fn sdk_device_types_map_to_known_models_and_degrade_otherwise() {
        assert_eq!(
            DeckCapabilities::from_sdk_device_type(7),
            Some(DeckModel::Plus)
        );
        assert_eq!(
            DeckCapabilities::from_sdk_device_type(2),
            Some(DeckModel::Xl)
        );
        // A device type from a model released after this code was written.
        assert_eq!(DeckCapabilities::from_sdk_device_type(99), None);
    }
}
