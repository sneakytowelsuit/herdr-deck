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

use serde::{Deserialize, Serialize};

/// One thing the deck asks herdr to do.
///
/// Every target is an id herdr's API will accept as it stands. That is deliberate: a command
/// that still needed resolving could be handed to the socket by mistake, and herdr would either
/// refuse it or — worse — act on whatever now answers to that id.
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

    /// A command that names nothing and does nothing, classified destructive.
    ///
    /// It exists so the guard that keeps destructive commands behind a deliberate hold can be
    /// proven before anything that actually destroys work is bound to a key. Test builds of this
    /// crate only: no released binary can construct it, and no other crate can even see it.
    #[cfg(test)]
    DestructiveStub,
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
            #[cfg(test)]
            DeckCommand::DestructiveStub => "destructive_stub",
        }
    }

    /// Short words for a key face.
    pub fn label(&self) -> &'static str {
        match self {
            DeckCommand::FocusPane { .. } => "focus",
            DeckCommand::FocusWorkspace { .. } => "space",
            DeckCommand::FocusTab { .. } => "tab",
            #[cfg(test)]
            DeckCommand::DestructiveStub => "stub",
        }
    }

    /// What this command names, as an id.
    ///
    /// Only ever an id. Labels, window titles and working directories are things the user or
    /// their agents wrote, and an audit trail that quotes them is a log file that has to be
    /// treated as secret.
    pub fn target(&self) -> &str {
        match self {
            DeckCommand::FocusPane { pane_id } => pane_id,
            DeckCommand::FocusWorkspace { workspace_id } => workspace_id,
            DeckCommand::FocusTab { tab_id } => tab_id,
            #[cfg(test)]
            DeckCommand::DestructiveStub => "",
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
            | DeckCommand::FocusTab { .. } => false,
            #[cfg(test)]
            DeckCommand::DestructiveStub => true,
        }
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
    fn every_command_names_its_target_by_id_and_never_by_anything_a_human_wrote() {
        // The audit log copies `target` verbatim, so anything richer than an id here would end up
        // on disk — and a trail full of directory names and window titles is one that has to be
        // guarded like a secret rather than read like a log.
        for command in [
            DeckCommand::FocusPane {
                pane_id: "w1:p2".into(),
            },
            DeckCommand::FocusWorkspace {
                workspace_id: "w1".into(),
            },
            DeckCommand::FocusTab {
                tab_id: "w1:t2".into(),
            },
        ] {
            let target = command.target();
            assert!(!target.is_empty());
            assert!(
                target.starts_with('w'),
                "{} named {target}, which is not a herdr id",
                command.name()
            );
        }
    }

    #[test]
    fn nothing_the_deck_can_currently_do_destroys_anything() {
        // Stated as a test so that wiring the first destructive command is a deliberate act with
        // a failing assertion attached, rather than something that slips in unnoticed.
        for command in [
            DeckCommand::FocusPane {
                pane_id: "w1:p2".into(),
            },
            DeckCommand::FocusWorkspace {
                workspace_id: "w1".into(),
            },
            DeckCommand::FocusTab {
                tab_id: "w1:t2".into(),
            },
        ] {
            assert!(!command.is_destructive(), "{} is not", command.name());
        }
    }
}
