//! The Linux frontend: USB HID.
//!
//! # Why this exists at all
//!
//! Elgato's Stream Deck application runs on macOS and Windows only, and Linux Stream Deck hosts
//! cannot load Elgato-format `.sdPlugin` packages. So on Linux there is no app to plug into —
//! we drive the hardware directly over HID.
//!
//! # Why it looks identical to macOS
//!
//! This frontend speaks the **same protocol** to `herdr-deckd` as the macOS plugin does, and
//! draws the **same PNGs** the daemon renders. It reports hardware, forwards input, and paints
//! pixels; every decision about what a key means stays in the daemon. That is what makes "it
//! behaves the same on both machines" true by construction.
//!
//! # Permissions
//!
//! Reaching the device without root needs a udev rule granting the logged-in user access to
//! Elgato's vendor id. `herdr-deck install` writes one; see the docs for the manual version.

pub mod translate;

pub use translate::{capabilities_for, key_index_of, DeckGeometry};

#[cfg(target_os = "linux")]
mod driver;

#[cfg(target_os = "linux")]
pub use driver::{list_decks, run};

/// Hardware bring-up self-test: talks to the deck directly, bypassing the daemon and herdr.
/// See the module for why it exists and what it can and cannot tell you.
#[cfg(target_os = "linux")]
pub mod selftest;

/// Elgato's USB vendor id, used by the udev rule.
pub const ELGATO_VENDOR_ID: &str = "0fd9";

/// The udev rule that makes a deck usable without root.
///
/// `uaccess` hands the device to whoever is logged in at the seat, which is what you want for a
/// desktop peripheral — better than a fixed group the user must be added to and re-login for.
pub const UDEV_RULE: &str = concat!(
    "# herdr-deck: let the logged-in user talk to Elgato Stream Deck hardware.\n",
    "SUBSYSTEM==\"usb\", ATTRS{idVendor}==\"0fd9\", TAG+=\"uaccess\"\n",
    "SUBSYSTEM==\"hidraw\", ATTRS{idVendor}==\"0fd9\", TAG+=\"uaccess\"\n",
);

/// Where the rule belongs.
pub const UDEV_RULE_PATH: &str = "/etc/udev/rules.d/70-herdr-deck.rules";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_udev_rule_covers_both_usb_and_hidraw() {
        // hidapi may bind through either subsystem depending on how it was built; a rule
        // covering only one leaves a permission failure that looks like "no deck found".
        assert!(UDEV_RULE.contains("SUBSYSTEM==\"usb\""));
        assert!(UDEV_RULE.contains("SUBSYSTEM==\"hidraw\""));
        assert_eq!(UDEV_RULE.matches(ELGATO_VENDOR_ID).count(), 2);
    }

    #[test]
    fn the_udev_rule_uses_uaccess_rather_than_a_fixed_group() {
        // A group would need the user added and a re-login; uaccess just works at the seat.
        assert!(UDEV_RULE.contains("TAG+=\"uaccess\""));
        assert!(!UDEV_RULE.contains("GROUP="));
    }
}
