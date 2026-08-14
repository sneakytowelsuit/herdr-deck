//! User configuration (`~/.config/herdr-deck/config.toml`).
//!
//! Every field has a working default, so the file is entirely optional — the deck should be
//! useful the moment it is plugged in. The config exists to override, pin, and troubleshoot.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use herdr_deck_herdr::wire::{CreateSpec, LayoutPreset};
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

    /// Named arrangements of panes, each of which gets a key.
    ///
    /// The one place in this file where a *table* is also a layout decision: a deck with room to
    /// spare grows one key per entry here, because a preset nobody can press is a preset nobody
    /// wrote on purpose.
    pub layouts: BTreeMap<String, LayoutPreset>,

    /// Named workspaces to create. Bound by hand — see [`KeyBinding::NewWorkspace`].
    pub workspaces: BTreeMap<String, CreateSpec>,

    /// Named tabs to create. Bound by hand — see [`KeyBinding::NewTab`].
    ///
    /// Presets are the *only* way a working directory, a label or an environment reaches herdr
    /// from this deck. There is no keyboard on a Stream Deck and this file is the substitute: a
    /// name written once, and then a key that means it.
    pub tabs: BTreeMap<String, CreateSpec>,
}

/// The named things a key can be bound to.
///
/// Held by a [`Profile`](crate::layout::Profile) rather than looked up through the config, so that
/// resolving a key never needs anything the layout engine does not already have — and so a key
/// naming a preset that is not there can say so on its own face instead of panicking somewhere.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Presets {
    pub layouts: BTreeMap<String, LayoutPreset>,
    pub workspaces: BTreeMap<String, CreateSpec>,
    pub tabs: BTreeMap<String, CreateSpec>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            theme: Theme::default(),
            reconcile_interval_ms: DEFAULT_RECONCILE_MS,
            herdr_session: None,
            focus: FocusConfig::default(),
            layout: None,
            layouts: BTreeMap::new(),
            workspaces: BTreeMap::new(),
            tabs: BTreeMap::new(),
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

/// Preset names have to be safe to put on a key and safe to write to the audit trail.
///
/// The second half is the load-bearing one. A command records what it acted on, and a preset name
/// is the only thing it records that a person wrote — so holding those names to a spelling this
/// repository chose is what keeps the rule "nothing out of the user's world reaches the log" true
/// by construction rather than by hoping nobody names a preset after a client.
fn check_preset_name(kind: &str, name: &str) -> anyhow::Result<()> {
    let ok = !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '_' | '-'));
    anyhow::ensure!(
        ok,
        "{kind} preset name `{name}` must be lower-case letters, digits, `-` or `_` — \
         it goes on a key face and into the command log"
    );
    Ok(())
}

/// A key naming a preset that is not there, said in a way that fixes it.
fn unknown_preset<V>(kind: &str, wanted: &str, known: &BTreeMap<String, V>) -> String {
    if known.is_empty() {
        return format!("a key wants the {kind} preset `{wanted}`, but none are defined");
    }
    format!(
        "a key wants the {kind} preset `{wanted}`, which is not defined. Known: {}",
        known.keys().cloned().collect::<Vec<_>>().join(", ")
    )
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
        config.validate()?;
        Ok(config)
    }

    /// Clamp anything that would misbehave if taken literally.
    fn normalise(&mut self) {
        // A sub-250ms reconcile would hammer herdr with snapshots for no perceptible gain.
        self.reconcile_interval_ms = self.reconcile_interval_ms.max(250);
    }

    /// Refuse anything that would only fail later, on a key, in front of somebody.
    ///
    /// Everything checked here could instead be discovered at press time: herdr would reject an
    /// oversized layout tree, and a key naming a preset that does not exist would simply refuse.
    /// Both are worse places to learn it. The person who can fix a config error is the person
    /// editing the config, and they are here now.
    fn validate(&self) -> anyhow::Result<()> {
        for (name, preset) in &self.layouts {
            check_preset_name("layout", name)?;
            preset
                .validate()
                .map_err(|why| anyhow::anyhow!("layout preset `{name}` {why}"))?;
        }
        for name in self.workspaces.keys() {
            check_preset_name("workspace", name)?;
        }
        for name in self.tabs.keys() {
            check_preset_name("tab", name)?;
        }

        let Some(layout) = &self.layout else {
            return Ok(());
        };
        for key in &layout.keys {
            match key {
                KeyBinding::Layout { preset } if !self.layouts.contains_key(preset) => {
                    anyhow::bail!("{}", unknown_preset("layout", preset, &self.layouts));
                }
                KeyBinding::NewWorkspace {
                    preset: Some(preset),
                } if !self.workspaces.contains_key(preset) => {
                    anyhow::bail!("{}", unknown_preset("workspace", preset, &self.workspaces));
                }
                KeyBinding::NewTab {
                    preset: Some(preset),
                } if !self.tabs.contains_key(preset) => {
                    anyhow::bail!("{}", unknown_preset("tab", preset, &self.tabs));
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// The named things a layout engine may bind a key to.
    pub fn presets(&self) -> Presets {
        Presets {
            layouts: self.layouts.clone(),
            workspaces: self.workspaces.clone(),
            tabs: self.tabs.clone(),
        }
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
    fn a_pane_control_cluster_can_be_written_by_hand_on_a_deck_too_small_to_be_given_one() {
        // Small decks keep every key for agents, so this is the only way pane control reaches a
        // Stream Deck + — which makes it the path most likely to be typed by a person and most
        // deserving of a test that the spellings are the ones documented.
        let config = Config::from_toml(
            r#"
            [layout]
            keys = [
              { kind = "command", command = { verb = "move_pane_focus", direction = "left" } },
              { kind = "command", command = { verb = "move_pane_focus", direction = "right" } },
              { kind = "command", command = { verb = "zoom_pane", zoom = "on" } },
              { kind = "command", command = { verb = "zoom_pane", zoom = "off" } },
              { kind = "command", command = { verb = "split_pane", direction = "down" } },
              { kind = "close_pane" },
            ]
            "#,
        )
        .unwrap();
        let keys = config.layout.expect("layout parsed").keys;
        assert_eq!(
            keys[0],
            KeyBinding::Command {
                command: crate::command::DeckCommand::MovePaneFocus {
                    direction: herdr_deck_herdr::wire::PaneDirection::Left
                }
            }
        );
        assert_eq!(
            keys[3],
            KeyBinding::Command {
                command: crate::command::DeckCommand::ZoomPane {
                    zoom: herdr_deck_herdr::wire::ZoomMode::Off
                }
            }
        );
        assert_eq!(keys[5], KeyBinding::ClosePane);
    }

    #[test]
    fn a_field_written_on_the_wrong_kind_of_key_is_an_error_like_any_other_typo() {
        // The same bargain the rest of the file makes: a key that quietly ignored half of what was
        // asked of it would leave someone editing their config and wondering why nothing changed.
        //
        // Serde can only enforce this on bindings that take arguments — a unit binding such as
        // `close_pane` swallows stray fields whatever we ask for — so the guarantee is real but
        // not total, and a stray `pane_id` on a close key is ignored rather than refused. It would
        // have been ignored anyway: that key closes whatever herdr says is focused, by design.
        let err = Config::from_toml(
            r#"
            [layout]
            keys = [{ kind = "dynamic", rank = 0, ranks = 1 }]
            "#,
        )
        .unwrap_err();
        assert!(err.to_string().contains("ranks"), "got: {err}");
    }

    // --- Presets -------------------------------------------------------------------------------

    #[test]
    fn a_layout_preset_and_the_key_that_applies_it_are_written_separately_and_found_together() {
        let config = Config::from_toml(
            r#"
            [layouts.dev]
            label = "dev"

            [layouts.dev.root]
            split = "right"
            ratio = 60

            [layouts.dev.root.first]

            [layouts.dev.root.second]
            command = ["cargo", "watch", "-x", "test"]

            [layout]
            keys = [{ kind = "layout", preset = "dev" }]
            "#,
        )
        .unwrap();

        assert_eq!(
            config.layout.as_ref().unwrap().keys[0],
            KeyBinding::Layout {
                preset: "dev".into()
            }
        );
        let dev = &config.layouts["dev"];
        assert_eq!(dev.label.as_deref(), Some("dev"));
        assert_eq!(dev.root.ratio, Some(60));
        assert_eq!(
            dev.root.second.as_ref().unwrap().command.as_deref(),
            Some(
                ["cargo", "watch", "-x", "test"]
                    .map(String::from)
                    .as_slice()
            )
        );
    }

    #[test]
    fn a_key_naming_a_preset_that_does_not_exist_fails_the_config_rather_than_the_key() {
        // The alternative is a key that draws dimmed and refuses when pressed — which is correct
        // behaviour for a preset that vanishes at runtime, and a terrible way to learn about a
        // typo. The person who can fix this is reading the file right now.
        let err = Config::from_toml(
            r#"
            [layouts.dev]
            [layouts.dev.root]

            [layout]
            keys = [{ kind = "layout", preset = "dvel" }]
            "#,
        )
        .unwrap_err();
        assert!(err.to_string().contains("dvel"), "got: {err}");
        assert!(
            err.to_string().contains("dev"),
            "and it must say what does exist: {err}"
        );
    }

    #[test]
    fn a_workspace_or_tab_key_naming_a_missing_preset_fails_the_same_way() {
        for (kind, key) in [
            (
                "workspace",
                r#"{ kind = "new_workspace", preset = "notes" }"#,
            ),
            ("tab", r#"{ kind = "new_tab", preset = "notes" }"#),
        ] {
            let err = Config::from_toml(&format!("[layout]\nkeys = [{key}]\n")).unwrap_err();
            assert!(
                err.to_string().contains(kind) && err.to_string().contains("notes"),
                "a missing {kind} preset must say so: {err}"
            );
        }
    }

    #[test]
    fn a_layout_herdr_would_refuse_is_refused_here_first() {
        // herdr silently clamps a ratio outside 0.1..0.9, so a config saying 95 would quietly do
        // 90 forever. Refusing is the only way the file and the deck agree.
        let err = Config::from_toml(
            r#"
            [layouts.wide.root]
            split = "right"
            ratio = 95
            [layouts.wide.root.first]
            [layouts.wide.root.second]
            "#,
        )
        .unwrap_err();
        assert!(err.to_string().contains("wide"), "got: {err}");
        assert!(err.to_string().contains("95"), "got: {err}");
    }

    #[test]
    fn a_preset_name_that_could_not_be_written_to_the_command_log_is_refused() {
        // Preset names are the one thing a command records that a person wrote. Holding them to a
        // spelling this repository chose is what keeps "nothing out of the user's world reaches
        // the log" true by construction rather than by hoping nobody names one after a client.
        let err = Config::from_toml(
            r#"
            [workspaces."Acme Corp/notes"]
            cwd = "/srv/acme"
            "#,
        )
        .unwrap_err();
        assert!(err.to_string().contains("Acme Corp"), "got: {err}");
    }

    #[test]
    fn a_key_with_no_preset_named_is_fine_because_herdr_has_its_own_defaults() {
        let config = Config::from_toml(
            r#"
            [layout]
            keys = [{ kind = "new_workspace" }, { kind = "new_tab" }, { kind = "new_worktree" }]
            "#,
        )
        .unwrap();
        let keys = config.layout.expect("layout parsed").keys;
        assert_eq!(keys[0], KeyBinding::NewWorkspace { preset: None });
        assert_eq!(keys[1], KeyBinding::NewTab { preset: None });
        assert_eq!(keys[2], KeyBinding::NewWorktree);
    }

    #[test]
    fn config_serialises_back_to_toml_so_install_can_write_a_starter_file() {
        let config = Config::default();
        let text = toml::to_string_pretty(&config).unwrap();
        let round_tripped = Config::from_toml(&text).unwrap();
        assert_eq!(config, round_tripped);
    }
}
