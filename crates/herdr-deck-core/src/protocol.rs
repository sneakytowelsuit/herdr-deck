//! The protocol between `herdr-deckd` and its frontends.
//!
//! Newline-delimited JSON over a unix socket. Deliberately not a TCP port: there is nothing to
//! authenticate, no reason to be reachable off-box, and filesystem permissions already express
//! exactly the access we want.
//!
//! # The division of labour
//!
//! The frontend is dumb on purpose. It reports what hardware it has, forwards button and dial
//! events, and draws the PNGs it is handed. **Every decision** — what a key means, what it
//! shows, what pressing it does — happens in the daemon. That is what keeps the macOS
//! TypeScript plugin thin and guarantees both platforms behave identically.

use serde::{Deserialize, Serialize};

use crate::capabilities::{DeckCapabilities, DeckModel, TouchStrip};

/// Bumped when the frontend protocol changes incompatibly.
///
/// The macOS plugin ships separately from the daemon (different install mechanisms, different
/// update cadences), so a version mismatch is a real possibility and needs a clear error rather
/// than mysterious silence.
pub const FRONTEND_PROTOCOL: u32 = 1;

/// Hardware a frontend reports in its `hello`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceReport {
    /// A recognised model, when the frontend knows it.
    #[serde(default)]
    pub model: Option<DeckModel>,
    #[serde(default)]
    pub model_name: Option<String>,
    pub columns: u8,
    pub rows: u8,
    pub key_image_px: u32,
    #[serde(default)]
    pub dials: u8,
    #[serde(default)]
    pub touchstrip: Option<TouchStrip>,
}

impl DeviceReport {
    /// Turn a report into capabilities.
    ///
    /// A frontend that names a model we know gets that model's stock geometry, but any field it
    /// reports explicitly still wins — so a firmware revision with a different key size works
    /// without a code change.
    pub fn to_capabilities(&self) -> DeckCapabilities {
        let mut caps = match self.model {
            Some(model) if model != DeckModel::Unknown => model.capabilities(),
            _ => DeckCapabilities::from_geometry(
                self.model_name
                    .clone()
                    .unwrap_or_else(|| "Stream Deck".to_string()),
                self.columns,
                self.rows,
                self.key_image_px,
                self.dials,
                self.touchstrip,
            ),
        };
        if self.columns > 0 {
            caps.columns = self.columns;
        }
        if self.rows > 0 {
            caps.rows = self.rows;
        }
        if self.key_image_px > 0 {
            caps.key_image_px = self.key_image_px;
        }
        if let Some(name) = &self.model_name {
            if !name.is_empty() {
                caps.model_name = name.clone();
            }
        }
        // Dials and the touchstrip are reported as-is: a frontend that says "no dials" must be
        // believed even if we think the model has them, because that is what it can drive.
        caps.dials = self.dials;
        caps.touchstrip = self.touchstrip;
        caps
    }
}

/// Frontend → daemon.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FrontendMessage {
    /// First message on every connection.
    Hello {
        /// Which frontend this is, for logs and `doctor`.
        frontend: String,
        device: DeviceReport,
        #[serde(default)]
        protocol: Option<u32>,
    },
    KeyDown {
        index: usize,
    },
    KeyUp {
        index: usize,
    },
    DialRotate {
        dial: usize,
        ticks: i32,
    },
    DialDown {
        dial: usize,
    },
    DialUp {
        dial: usize,
    },
    TouchTap {
        #[serde(default)]
        dial: Option<usize>,
    },
    /// Ask for a full redraw, e.g. after the Stream Deck app wakes from sleep.
    Refresh,
    Ping,
}

/// Daemon → frontend.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DaemonMessage {
    /// Sent once after a successful `hello`.
    Ready {
        protocol: u32,
        keys: usize,
        dials: usize,
        /// Echoed back so the frontend can log what the daemon believes it is driving.
        device: String,
    },
    /// Draw a PNG on a key. `png` is base64.
    SetKeyImage {
        index: usize,
        png: String,
    },
    /// Draw a touchstrip segment above a dial.
    SetDialFeedback {
        dial: usize,
        title: String,
        value: String,
        /// Base64 PNG for frontends that render the strip as an image.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        png: Option<String>,
    },
    /// Flash the "that worked" indicator.
    Ok {
        index: usize,
    },
    /// Flash the "that did not fully work" indicator, with a reason for the logs.
    Alert {
        index: usize,
        message: String,
    },
    /// The frontend spoke a protocol we do not implement.
    ProtocolMismatch {
        expected: u32,
        got: u32,
    },
    Pong,
}

impl DaemonMessage {
    pub fn to_line(&self) -> String {
        let mut line = serde_json::to_string(self).expect("daemon message serialises");
        line.push('\n');
        line
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plus_report() -> DeviceReport {
        DeviceReport {
            model: Some(DeckModel::Plus),
            model_name: Some("Stream Deck +".into()),
            columns: 4,
            rows: 2,
            key_image_px: 120,
            dials: 4,
            touchstrip: Some(TouchStrip {
                width: 800,
                height: 100,
            }),
        }
    }

    #[test]
    fn a_hello_from_the_macos_plugin_parses() {
        let raw = r#"{"type":"hello","frontend":"streamdeck-macos","protocol":1,
            "device":{"model":"plus","model_name":"Stream Deck +","columns":4,"rows":2,
            "key_image_px":120,"dials":4,"touchstrip":{"width":800,"height":100}}}"#;
        let msg: FrontendMessage = serde_json::from_str(raw).unwrap();
        match msg {
            FrontendMessage::Hello {
                frontend,
                device,
                protocol,
            } => {
                assert_eq!(frontend, "streamdeck-macos");
                assert_eq!(protocol, Some(1));
                assert_eq!(device.dials, 4);
            }
            other => panic!("expected hello, got {other:?}"),
        }
    }

    #[test]
    fn a_hello_omitting_optional_fields_still_parses() {
        // A minimal frontend should not need to send model metadata it does not have.
        let raw = r#"{"type":"hello","frontend":"custom","device":{"columns":5,"rows":3,"key_image_px":72}}"#;
        let msg: FrontendMessage = serde_json::from_str(raw).unwrap();
        match msg {
            FrontendMessage::Hello { device, .. } => {
                assert_eq!(device.dials, 0);
                assert!(device.touchstrip.is_none());
                let caps = device.to_capabilities();
                assert_eq!(caps.key_count(), 15);
            }
            other => panic!("expected hello, got {other:?}"),
        }
    }

    #[test]
    fn a_known_model_yields_its_stock_capabilities() {
        let caps = plus_report().to_capabilities();
        assert_eq!(caps.model, DeckModel::Plus);
        assert_eq!(caps.key_count(), 8);
        assert_eq!(caps.dials, 4);
        assert!(caps.has_touchstrip());
    }

    #[test]
    fn explicitly_reported_geometry_overrides_the_stock_model() {
        // A firmware revision with a different key size must work without a code change.
        let mut report = plus_report();
        report.key_image_px = 144;
        report.columns = 5;
        let caps = report.to_capabilities();
        assert_eq!(caps.key_image_px, 144);
        assert_eq!(caps.columns, 5);
    }

    #[test]
    fn a_frontend_that_cannot_drive_dials_is_believed_even_for_a_model_that_has_them() {
        let mut report = plus_report();
        report.dials = 0;
        report.touchstrip = None;
        let caps = report.to_capabilities();
        assert_eq!(caps.dials, 0);
        assert!(!caps.has_touchstrip());
    }

    #[test]
    fn unknown_hardware_falls_back_to_reported_geometry() {
        let report = DeviceReport {
            model: None,
            model_name: Some("Deck 9000".into()),
            columns: 6,
            rows: 3,
            key_image_px: 128,
            dials: 2,
            touchstrip: None,
        };
        let caps = report.to_capabilities();
        assert_eq!(caps.model, DeckModel::Unknown);
        assert_eq!(caps.model_name, "Deck 9000");
        assert_eq!(caps.key_count(), 18);
        assert_eq!(caps.dials, 2);
    }

    #[test]
    fn every_input_event_round_trips() {
        let messages = vec![
            FrontendMessage::KeyDown { index: 3 },
            FrontendMessage::KeyUp { index: 3 },
            FrontendMessage::DialRotate { dial: 1, ticks: -2 },
            FrontendMessage::DialDown { dial: 0 },
            FrontendMessage::DialUp { dial: 0 },
            FrontendMessage::TouchTap { dial: Some(2) },
            FrontendMessage::Refresh,
            FrontendMessage::Ping,
        ];
        for msg in messages {
            let text = serde_json::to_string(&msg).unwrap();
            let back: FrontendMessage = serde_json::from_str(&text).unwrap();
            assert_eq!(msg, back);
        }
    }

    #[test]
    fn daemon_messages_serialise_as_one_line_each() {
        // The transport is newline-delimited: an embedded newline would desynchronise it.
        let msg = DaemonMessage::SetKeyImage {
            index: 0,
            png: "AAAA".into(),
        };
        let line = msg.to_line();
        assert!(line.ends_with('\n'));
        assert_eq!(line.matches('\n').count(), 1);
    }

    #[test]
    fn daemon_messages_use_a_type_tag_the_typescript_plugin_can_switch_on() {
        let line = DaemonMessage::Ok { index: 2 }.to_line();
        let value: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(value["type"], "ok");
        assert_eq!(value["index"], 2);
    }

    #[test]
    fn dial_feedback_omits_the_image_field_when_there_is_none() {
        // The macOS plugin renders the strip from title/value via Elgato's layout system and
        // has no use for a PNG; sending a null would just be noise.
        let line = DaemonMessage::SetDialFeedback {
            dial: 0,
            title: "agent".into(),
            value: "refactor".into(),
            png: None,
        }
        .to_line();
        assert!(!line.contains("png"));
    }

    #[test]
    fn an_unknown_message_type_is_rejected_rather_than_silently_ignored() {
        let result = serde_json::from_str::<FrontendMessage>(r#"{"type":"launch_missiles"}"#);
        assert!(result.is_err());
    }
}
