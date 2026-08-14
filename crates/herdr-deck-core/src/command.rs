//! What the deck asks herdr to do.
//!
//! # One command, carried end to end
//!
//! A control decides *which* command it means, the daemon performs it, and the audit log records
//! it. All three handle the same value. Adding a command is therefore a variant here, an arm
//! where commands are performed, and whatever binds it to a control — not a parallel enum in
//! every layer it passes through, each with its own match statement to keep in step.
//!
//! Nothing in here is a herdr method name. herdr's wire protocol lives in `herdr-deck-herdr` and
//! nowhere else; these are intents, and that crate decides what each one becomes on the socket.
//!
//! # Writes only
//!
//! This is the deck's entire *write* surface. Reading herdr is the watcher's job and happens on
//! a timer whether anyone presses anything or not, which is why every command issued from here
//! is worth recording and no read ever is.

use herdr_deck_herdr::wire::{PaneDirection, SplitDirection, ZoomMode};
use serde::{Deserialize, Serialize};

/// One thing the deck asks herdr to do.
///
/// Every target is an id herdr's API will accept as it stands, or a word from a closed set. That
/// is deliberate: a command that still needed resolving could be handed to the socket by mistake,
/// and herdr would either refuse it or — worse — act on whatever now answers to that id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "verb")]
pub enum DeckCommand {
    /// Take the user to a pane.
    ///
    /// herdr walks the whole path itself — workspace, then tab, then pane — from this single
    /// call, and follows zoom while it does. Sending a workspace or tab focus around it makes
    /// three round trips out of a journey herdr does atomically, and fights its own logic.
    FocusPane { pane_id: String },
    /// Take the user to a workspace, landing on whichever tab it had active.
    FocusWorkspace { workspace_id: String },
    /// Take the user to one named tab, which is not the same as landing on its workspace's
    /// current one.
    FocusTab { tab_id: String },

    /// Move one pane that way, inside the tab herdr is already on.
    ///
    /// The most deck-shaped thing herdr offers: one key, one direction, no target to resolve and
    /// nothing to type. It names no pane on purpose — see
    /// [`HerdrClient::pane_focus_direction`](herdr_deck_herdr::HerdrClient::pane_focus_direction).
    MovePaneFocus { direction: PaneDirection },

    /// Make the focused pane fill its tab, or put it back.
    ///
    /// Two commands rather than one toggle, because a toggle would be a claim about which way the
    /// zoom is going to go, and the deck cannot see that: zoom is a per-tab flag that herdr's own
    /// snapshot carries but the deck only ever holds a second or two late. Stating the wanted end
    /// state makes the key idempotent instead — press it twice and you are still zoomed.
    ZoomPane { zoom: ZoomMode },

    /// Put a new shell beside or below the focused pane.
    SplitPane { direction: SplitDirection },

    /// Close a pane, killing what is running in it.
    ///
    /// **Destructive**, and further-reaching than the name suggests: the last pane of a tab takes
    /// the tab with it and the last tab takes the workspace, and herdr reports the same bare `ok`
    /// for all three. The deck can therefore say what it asked for but never what it did, which is
    /// the whole reason this one waits for a hold.
    ClosePane { pane_id: String },
}

impl DeckCommand {
    /// A stable identifier for the audit log.
    ///
    /// Separate from [`DeckCommand::label`] because the two answer to different readers: this one
    /// is grepped months later and must never change spelling to suit a key face.
    pub fn name(&self) -> &'static str {
        match self {
            DeckCommand::FocusPane { .. } => "focus_pane",
            DeckCommand::FocusWorkspace { .. } => "focus_workspace",
            DeckCommand::FocusTab { .. } => "focus_tab",
            DeckCommand::MovePaneFocus { .. } => "move_pane_focus",
            DeckCommand::ZoomPane { .. } => "zoom_pane",
            DeckCommand::SplitPane { .. } => "split_pane",
            DeckCommand::ClosePane { .. } => "close_pane",
        }
    }

    /// Short words for a key face.
    ///
    /// A key that carries one of the pane commands also carries a shape (see
    /// [`crate::render::KeyGlyph`]) and is meant to be read by that shape alone; this is the
    /// caption under it, and the words `herdr-deck layout` prints.
    pub fn label(&self) -> &'static str {
        match self {
            DeckCommand::FocusPane { .. } => "focus",
            DeckCommand::FocusWorkspace { .. } => "space",
            DeckCommand::FocusTab { .. } => "tab",
            DeckCommand::MovePaneFocus { direction } => direction.as_str(),
            DeckCommand::ZoomPane { zoom } => match zoom {
                ZoomMode::On => "zoom",
                ZoomMode::Off => "unzoom",
            },
            DeckCommand::SplitPane { direction } => match direction {
                SplitDirection::Right => "split right",
                SplitDirection::Down => "split down",
            },
            DeckCommand::ClosePane { .. } => "close",
        }
    }

    /// What this command names: a herdr id, or a word from a closed set when it names no object.
    ///
    /// Never anything a human wrote. Labels, window titles and working directories all belong to
    /// the user or their agents, and an audit trail that quoted them would be a log file that had
    /// to be guarded like a secret instead of read like a log. A direction is safe by the same
    /// test — `left` came from this enum, not from anybody's keyboard.
    pub fn target(&self) -> &str {
        match self {
            DeckCommand::FocusPane { pane_id } => pane_id,
            DeckCommand::FocusWorkspace { workspace_id } => workspace_id,
            DeckCommand::FocusTab { tab_id } => tab_id,
            DeckCommand::MovePaneFocus { direction } => direction.as_str(),
            DeckCommand::ZoomPane { zoom } => zoom.as_str(),
            DeckCommand::SplitPane { direction } => direction.as_str(),
            DeckCommand::ClosePane { pane_id } => pane_id,
        }
    }

    /// Does [`DeckCommand::target`] name a thing in herdr, or merely say how?
    ///
    /// The audit log does not care — an id and a direction are equally worth recording — but
    /// anything printing a key for a human does: "split right right" is what you get from writing
    /// out a parameter that the verb already said.
    pub fn names_an_object(&self) -> bool {
        match self {
            DeckCommand::FocusPane { .. }
            | DeckCommand::FocusWorkspace { .. }
            | DeckCommand::FocusTab { .. }
            | DeckCommand::ClosePane { .. } => true,
            DeckCommand::MovePaneFocus { .. }
            | DeckCommand::ZoomPane { .. }
            | DeckCommand::SplitPane { .. } => false,
        }
    }

    /// Could performing this destroy work the user cannot get back?
    ///
    /// Asked of the command rather than of the key, so the guard holds however the command
    /// reaches a control. A hand-written layout that bound one directly must not be able to opt
    /// out of the hold by accident.
    pub fn is_destructive(&self) -> bool {
        match self {
            DeckCommand::FocusPane { .. }
            | DeckCommand::FocusWorkspace { .. }
            | DeckCommand::FocusTab { .. }
            | DeckCommand::MovePaneFocus { .. }
            // Splitting adds a shell and takes nothing away, and a zoom is a view of a tab rather
            // than a change to what is in it. Both are undone by pressing something else.
            | DeckCommand::ZoomPane { .. }
            | DeckCommand::SplitPane { .. } => false,
            DeckCommand::ClosePane { .. } => true,
        }
    }

    /// Does carrying this out mean bringing the terminal window forward?
    ///
    /// The line is between taking someone somewhere and rearranging where they already are. A
    /// focus is the deck's whole promise — notice it, go there — and half of going there is the
    /// window. Splitting, zooming, closing and stepping between panes are all things you press
    /// while sitting at the terminal doing them, so fetching the window would spend a round trip,
    /// and on hardware that cannot raise windows an alert, on something nobody asked for.
    pub fn raises_the_window(&self) -> bool {
        match self {
            DeckCommand::FocusPane { .. }
            | DeckCommand::FocusWorkspace { .. }
            | DeckCommand::FocusTab { .. } => true,
            DeckCommand::MovePaneFocus { .. }
            | DeckCommand::ZoomPane { .. }
            | DeckCommand::SplitPane { .. }
            | DeckCommand::ClosePane { .. } => false,
        }
    }

    /// Every command the deck knows how to issue, with a stand-in target where one is needed.
    ///
    /// Exists so the tests that must cover *all* of them — that each reaches herdr, that none
    /// records anything a human wrote — cannot quietly fall behind the enum. Adding a variant
    /// without adding it here is the mistake this is here to make loud.
    ///
    /// Public rather than test-only because the crate that actually issues these lives above this
    /// one, and its "every command reaches herdr" test is the one that matters most.
    pub fn every() -> Vec<DeckCommand> {
        let mut all = vec![
            DeckCommand::FocusPane {
                pane_id: "w1:p2".into(),
            },
            DeckCommand::FocusWorkspace {
                workspace_id: "w1".into(),
            },
            DeckCommand::FocusTab {
                tab_id: "w1:t2".into(),
            },
            DeckCommand::ClosePane {
                pane_id: "w1:p2".into(),
            },
        ];
        all.extend(
            PaneDirection::ALL
                .into_iter()
                .map(|direction| DeckCommand::MovePaneFocus { direction }),
        );
        all.extend(
            ZoomMode::ALL
                .into_iter()
                .map(|zoom| DeckCommand::ZoomPane { zoom }),
        );
        all.extend(
            SplitDirection::ALL
                .into_iter()
                .map(|direction| DeckCommand::SplitPane { direction }),
        );
        all
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_command_round_trips_through_config_so_a_key_can_be_bound_to_one_by_hand() {
        let toml = r#"verb = "focus_workspace"
workspace_id = "w2"
"#;
        let command: DeckCommand = toml::from_str(toml).unwrap();
        assert_eq!(
            command,
            DeckCommand::FocusWorkspace {
                workspace_id: "w2".into()
            }
        );
    }

    #[test]
    fn a_pane_command_round_trips_through_config_too_so_a_cluster_can_be_hand_written() {
        let toml = r#"verb = "move_pane_focus"
direction = "left"
"#;
        let command: DeckCommand = toml::from_str(toml).unwrap();
        assert_eq!(
            command,
            DeckCommand::MovePaneFocus {
                direction: PaneDirection::Left
            }
        );
    }

    #[test]
    fn every_command_names_its_target_by_id_or_by_a_word_from_a_closed_set() {
        // The audit log copies `target` verbatim, so anything richer than this ends up on disk —
        // and a trail full of directory names and window titles is one that has to be guarded
        // like a secret rather than read like a log. Ids and enum spellings both pass; anything a
        // person could have typed does not.
        for command in DeckCommand::every() {
            let target = command.target();
            assert!(
                !target.is_empty(),
                "{} recorded nothing at all",
                command.name()
            );
            assert!(
                target
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == ':'),
                "{} named {target:?}, which is neither a herdr id nor a word we chose",
                command.name()
            );
        }
    }

    #[test]
    fn the_only_thing_the_deck_can_do_that_destroys_work_is_close_a_pane() {
        // Stated exhaustively so that wiring the *next* destructive command is a deliberate act
        // with a failing assertion attached, rather than something that slips in unnoticed.
        let destructive: Vec<_> = DeckCommand::every()
            .into_iter()
            .filter(DeckCommand::is_destructive)
            .map(|c| c.name())
            .collect();
        assert_eq!(destructive, vec!["close_pane"]);
    }

    #[test]
    fn only_the_commands_that_take_you_somewhere_ask_for_the_window() {
        // Rearranging the terminal you are already sitting at is not a journey, and a key that
        // fetched the window every time you stepped one pane left would spend a round trip — and,
        // where raising cannot work at all, an alert — on something nobody asked for.
        let raises: Vec<_> = DeckCommand::every()
            .into_iter()
            .filter(DeckCommand::raises_the_window)
            .map(|c| c.name())
            .collect();
        assert_eq!(raises, vec!["focus_pane", "focus_workspace", "focus_tab"]);
    }

    #[test]
    fn no_two_commands_share_a_name_in_the_audit_log() {
        // The log is read months later with `grep`, so two commands answering to one word would
        // make it impossible to tell which of them was pressed.
        let mut names: Vec<_> = DeckCommand::every()
            .iter()
            .map(DeckCommand::name)
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();
        names.sort_unstable();
        assert_eq!(names.len(), 7, "one name per command, got {names:?}");
    }

    #[test]
    fn each_direction_a_key_can_offer_is_labelled_with_its_own_word() {
        // The shape on the tile is what these keys are read by, but the caption still has to
        // distinguish them — two keys captioned "pane" would be a coin toss.
        let labels: std::collections::HashSet<_> = DeckCommand::every()
            .iter()
            .map(DeckCommand::label)
            .collect();
        for word in ["left", "right", "up", "down", "zoom", "unzoom", "close"] {
            assert!(labels.contains(word), "no key would ever say {word:?}");
        }
    }
}
