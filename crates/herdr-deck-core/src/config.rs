//! User configuration (`~/.config/herdr-deck/config.toml`).
//!
//! Every field has a working default, so the file is entirely optional — the deck should be
//! useful the moment it is plugged in. The config exists to override, pin, and troubleshoot.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::layout::{DialBinding, KeyBinding};
use crate::theme::Theme;

/// How often to reconcile with herdr even if no event arrives.
///
/// Events are the fast path; this is the safety net that makes a dropped event cost latency
/// instead of correctness.
pub const DEFAULT_RECONCILE_MS: u64 = 2000;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub theme: Theme,
    /// Reconcile interval in milliseconds. Clamped to a sane floor on load.
    pub reconcile_interval_ms: u64,
    /// An explicit herdr session name, equivalent to `herdr --session <name>`.
    pub herdr_session: Option<String>,
    pub focus: FocusConfig,
    /// Optional hand-written layout. When absent, the layout engine derives one from the
    /// attached hardware.
    pub layout: Option<LayoutOverride>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            theme: Theme::default(),
            reconcile_interval_ms: DEFAULT_RECONCILE_MS,
            herdr_session: None,
            focus: FocusConfig::default(),
            layout: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct FocusConfig {
    /// Also raise the terminal window, not just switch herdr's internal focus.
    pub raise_window: bool,
    /// macOS bundle identifier of the terminal hosting herdr.
    ///
    /// Defaults to Ghostty. Override if you run herdr somewhere else; `herdr-deck doctor`
    /// prints what it detected.
    pub macos_bundle_id: String,
    /// Linux window class / `app_id` of the terminal hosting herdr.
    pub linux_app_id: String,
    /// Force a specific window-raise backend instead of detecting one. Mostly a debugging aid;
    /// values are the same names `doctor` prints.
    pub backend: Option<String>,
    /// Stamp a unique marker into herdr's window title so the raiser can find the exact window
    /// rather than merely the right application.
    pub use_title_marker: bool,
}

impl Default for FocusConfig {
    fn default() -> Self {
        Self {
            raise_window: true,
            macos_bundle_id: "com.mitchellh.ghostty".to_string(),
            linux_app_id: "com.mitchellh.ghostty".to_string(),
            backend: None,
            use_title_marker: true,
        }
    }
}

/// A hand-written layout, replacing the derived one.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LayoutOverride {
    pub keys: Vec<KeyBinding>,
    pub dials: Vec<DialBinding>,
}

impl Config {
    /// The default config path: `~/.config/herdr-deck/config.toml`.
    pub fn default_path() -> Option<PathBuf> {
        directories::BaseDirs::new()
            .map(|dirs| dirs.config_dir().join("herdr-deck").join("config.toml"))
    }

    /// Load from `path`, falling back to defaults when the file does not exist.
    ///
    /// A *missing* config is normal. A *malformed* config is an error — silently ignoring a
    /// typo would leave the user staring at a deck that quietly ignores their settings.
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => Self::from_toml(&text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(anyhow::anyhow!("could not read {}: {e}", path.display())),
        }
    }

    pub fn from_toml(text: &str) -> anyhow::Result<Self> {
        let mut config: Config = toml::from_str(text)?;
        config.normalise();
        Ok(config)
    }

    /// Clamp anything that would misbehave if taken literally.
    fn normalise(&mut self) {
        // A sub-250ms reconcile would hammer herdr with snapshots for no perceptible gain.
        self.reconcile_interval_ms = self.reconcile_interval_ms.max(250);
    }

    pub fn reconcile_interval(&self) -> std::time::Duration {
        std::time::Duration::from_millis(self.reconcile_interval_ms)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::ScrubTarget;

    #[test]
    fn an_empty_config_is_valid_and_yields_defaults() {
        let config = Config::from_toml("").unwrap();
        assert_eq!(config, Config::default());
        assert!(config.focus.raise_window);
        assert_eq!(config.focus.macos_bundle_id, "com.mitchellh.ghostty");
    }

    #[test]
    fn a_missing_file_is_not_an_error_because_config_is_optional() {
        let config = Config::load(Path::new("/nonexistent/herdr-deck/config.toml")).unwrap();
        assert_eq!(config, Config::default());
    }

    #[test]
    fn a_typo_is_an_error_rather_than_being_silently_ignored() {
        // Without deny_unknown_fields the user would edit `raise_windows` and never learn why
        // nothing changed.
        let err = Config::from_toml("[focus]\nraise_windows = false\n").unwrap_err();
        assert!(err.to_string().contains("raise_windows"), "got: {err}");
    }

    #[test]
    fn overrides_are_applied() {
        let config = Config::from_toml(
            r#"
            reconcile_interval_ms = 5000
            herdr_session = "work"

            [focus]
            raise_window = false
            macos_bundle_id = "com.googlecode.iterm2"
            linux_app_id = "kitty"
            "#,
        )
        .unwrap();
        assert_eq!(config.reconcile_interval_ms, 5000);
        assert_eq!(config.herdr_session.as_deref(), Some("work"));
        assert!(!config.focus.raise_window);
        assert_eq!(config.focus.linux_app_id, "kitty");
    }

    #[test]
    fn an_absurdly_fast_reconcile_interval_is_clamped() {
        let config = Config::from_toml("reconcile_interval_ms = 1").unwrap();
        assert_eq!(config.reconcile_interval_ms, 250);
    }

    #[test]
    fn a_hand_written_layout_round_trips_through_toml() {
        let config = Config::from_toml(
            r#"
            [layout]
            keys = [
              { kind = "dynamic", rank = 0 },
              { kind = "pinned_agent", terminal_id = "term_abc" },
              { kind = "next_attention" },
            ]
            dials = [
              { kind = "scrub", target = "attention" },
            ]
            "#,
        )
        .unwrap();
        let layout = config.layout.expect("layout parsed");
        assert_eq!(layout.keys[0], KeyBinding::Dynamic { rank: 0 });
        assert_eq!(
            layout.keys[1],
            KeyBinding::PinnedAgent {
                terminal_id: "term_abc".into()
            }
        );
        assert_eq!(layout.keys[2], KeyBinding::NextAttention);
        assert_eq!(
            layout.dials[0],
            DialBinding::Scrub {
                target: ScrubTarget::Attention
            }
        );
    }

    #[test]
    fn config_serialises_back_to_toml_so_install_can_write_a_starter_file() {
        let config = Config::default();
        let text = toml::to_string_pretty(&config).unwrap();
        let round_tripped = Config::from_toml(&text).unwrap();
        assert_eq!(config, round_tripped);
    }
}
