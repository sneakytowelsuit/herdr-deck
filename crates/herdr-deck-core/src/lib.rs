//! Core logic for herdr-deck: state, hardware capabilities, layout, and tile rendering.
//!
//! Everything platform-specific lives elsewhere. This crate knows nothing about USB HID, the
//! Elgato app, or window managers — it turns "what does herdr look like right now" plus "what
//! hardware is attached" into "what pixels go on which key, and what happens when it is
//! pressed". That is what makes the macOS and Linux frontends thin, and what lets both produce
//! identical output.

pub mod capabilities;
pub mod config;
pub mod layout;
pub mod protocol;
pub mod render;
pub mod state;
pub mod theme;

pub use capabilities::{DeckCapabilities, DeckModel, TouchStrip};
pub use config::Config;
pub use layout::{
    DialBinding, KeyBinding, Mode, Profile, ResolvedDeck, ScrubTarget, Selection, SlotAction,
};
pub use protocol::{DaemonMessage, DeviceReport, FrontendMessage, FRONTEND_PROTOCOL};
pub use render::{Tile, TileRenderer};
pub use state::{DeckState, StatusCounts};
pub use theme::Theme;
