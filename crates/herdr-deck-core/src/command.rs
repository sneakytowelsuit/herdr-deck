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

use herdr_deck_herdr::wire::{CreateSpec, LayoutPreset, PaneDirection, SplitDirection, ZoomMode};
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

    /// Build a new tab from an arrangement of panes named in config.
    ///
    /// Always additive — a new tab, never a rearrangement of an existing one. The command carries
    /// the whole tree rather than the preset's name so that a key holding one cannot exist without
    /// the layout it applies: an unresolvable preset is caught where it is written, not where it
    /// is pressed.
    ApplyLayout {
        preset: String,
        layout: LayoutPreset,
    },

    /// Open a git worktree as a workspace and go there.
    ///
    /// The nearest thing in herdr's structural API to focusing an agent — one call, idempotent,
    /// non-destructive, and it takes you somewhere — which is why it is the one command here that
    /// also fetches the window.
    OpenWorktree { path: String },

    /// Make a new git worktree and open it.
    ///
    /// Takes nothing: herdr generates the branch name. Slow, because git is slow.
    CreateWorktree,

    /// Give a worktree's checkout back to git, closing the workspace it was open as.
    ///
    /// **Destructive**, and never forced — see
    /// [`HerdrClient::worktree_remove`](herdr_deck_herdr::HerdrClient::worktree_remove). git
    /// refuses to remove a checkout with uncommitted or untracked work in it, and that refusal is
    /// the confirmation this hardware has no screen to show. Committed work is never at risk: the
    /// branch survives.
    RemoveWorktree { workspace_id: String },

    /// Make a new workspace, optionally from a named preset.
    CreateWorkspace {
        preset: Option<String>,
        spec: CreateSpec,
    },

    /// Make a new tab in the workspace the user is in, optionally from a named preset.
    CreateTab {
        preset: Option<String>,
        spec: CreateSpec,
    },

    /// Close a tab, killing everything running in it.
    ///
    /// **Destructive**, and it cascades the way [`DeckCommand::ClosePane`] does one level up: the
    /// last tab of a workspace takes the workspace with it, and herdr answers the same bare `ok`
    /// for both.
    CloseTab { tab_id: String },
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
            DeckCommand::ApplyLayout { .. } => "apply_layout",
            DeckCommand::OpenWorktree { .. } => "open_worktree",
            DeckCommand::CreateWorktree => "create_worktree",
            DeckCommand::RemoveWorktree { .. } => "remove_worktree",
            DeckCommand::CreateWorkspace { .. } => "create_workspace",
            DeckCommand::CreateTab { .. } => "create_tab",
            DeckCommand::CloseTab { .. } => "close_tab",
        }
    }

    /// Short words for a key face.
    ///
    /// A key that carries one of the pane commands also carries a shape (see
    /// [`crate::render::KeyGlyph`]) and is meant to be read by that shape alone; this is the
    /// caption under it, and the words `herdr-deck layout` prints.
    pub fn label(&self) -> &str {
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
            // A preset key is one of several that look alike, so its name is the only thing
            // telling them apart and the caption is the whole message.
            DeckCommand::ApplyLayout { preset, .. } => preset,
            DeckCommand::CreateWorkspace { preset, .. } => preset.as_deref().unwrap_or("new space"),
            DeckCommand::CreateTab { preset, .. } => preset.as_deref().unwrap_or("new tab"),
            DeckCommand::OpenWorktree { .. } => "worktree",
            DeckCommand::CreateWorktree => "new tree",
            DeckCommand::RemoveWorktree { .. } => "remove tree",
            DeckCommand::CloseTab { .. } => "close tab",
        }
    }

    /// What this command names, when that is a thing safe to write down.
    ///
    /// Three kinds of answer qualify. A herdr id (`w1:p2`), which herdr minted. A word from a
    /// closed set (`left`), which came from an enum in this repository. And the name of a config
    /// preset, which the user chose as the name of a *key* — the deck's own vocabulary, just one
    /// they wrote rather than we did, and checked against a safe spelling when the config is read.
    ///
    /// `None` is the fourth answer and the reason this returns an `Option` at all: some commands
    /// can only identify their target by a name out of the user's own repository — a filesystem
    /// path, a branch. Those belong to the user, not to the deck, and an audit trail that quoted
    /// them would be a file that had to be guarded like a secret instead of read like a log. The
    /// command's own name still says what was done; only *which one* is withheld.
    pub fn target(&self) -> Option<&str> {
        match self {
            DeckCommand::FocusPane { pane_id } => Some(pane_id),
            DeckCommand::FocusWorkspace { workspace_id } => Some(workspace_id),
            DeckCommand::FocusTab { tab_id } => Some(tab_id),
            DeckCommand::MovePaneFocus { direction } => Some(direction.as_str()),
            DeckCommand::ZoomPane { zoom } => Some(zoom.as_str()),
            DeckCommand::SplitPane { direction } => Some(direction.as_str()),
            DeckCommand::ClosePane { pane_id } => Some(pane_id),
            DeckCommand::ApplyLayout { preset, .. } => Some(preset),
            DeckCommand::CreateWorkspace { preset, .. } | DeckCommand::CreateTab { preset, .. } => {
                preset.as_deref()
            }
            DeckCommand::RemoveWorktree { workspace_id } => Some(workspace_id),
            DeckCommand::CloseTab { tab_id } => Some(tab_id),
            // A worktree is a directory on the user's disk, named after a branch they invented.
            // Both halves of that are theirs.
            DeckCommand::OpenWorktree { .. } | DeckCommand::CreateWorktree => None,
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
            | DeckCommand::ClosePane { .. }
            | DeckCommand::RemoveWorktree { .. }
            | DeckCommand::CloseTab { .. } => true,
            // A preset key's label already *is* its preset name, so printing the target after it
            // would read "dev dev".
            DeckCommand::MovePaneFocus { .. }
            | DeckCommand::ZoomPane { .. }
            | DeckCommand::SplitPane { .. }
            | DeckCommand::ApplyLayout { .. }
            | DeckCommand::CreateWorkspace { .. }
            | DeckCommand::CreateTab { .. }
            | DeckCommand::OpenWorktree { .. }
            | DeckCommand::CreateWorktree => false,
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
            | DeckCommand::SplitPane { .. }
            // Everything that makes something is safe by construction: a new tab, a new workspace,
            // a new worktree and a layout applied additively all leave what was there alone. That
            // is the whole reason this half of herdr's structural API is on keys at all.
            | DeckCommand::ApplyLayout { .. }
            | DeckCommand::CreateWorkspace { .. }
            | DeckCommand::CreateTab { .. }
            | DeckCommand::CreateWorktree
            | DeckCommand::OpenWorktree { .. } => false,
            DeckCommand::ClosePane { .. }
            | DeckCommand::CloseTab { .. }
            // Not because git will let it destroy anything — sent unforced, git refuses a dirty
            // checkout — but because it closes a workspace and kills the shells in it, and because
            // the hold is what makes the refusal land on someone who meant to press this.
            | DeckCommand::RemoveWorktree { .. } => true,
        }
    }

    /// Will herdr keep the deck waiting while it answers this?
    ///
    /// True for exactly the two commands herdr defers until git has finished: making a worktree
    /// and giving one back. Every other method in its API answers at once. The daemon carries
    /// these out alongside its own event loop rather than inside it, so a `git worktree add` on a
    /// large repository costs the key its feedback for a few seconds and costs the rest of the
    /// deck nothing at all.
    pub fn may_take_a_while(&self) -> bool {
        matches!(
            self,
            DeckCommand::CreateWorktree | DeckCommand::RemoveWorktree { .. }
        )
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
            | DeckCommand::FocusTab { .. }
            // Opening a worktree is the same promise as focusing an agent — notice it, go there —
            // and it is the one command here you might press while looking at something that is
            // not a terminal at all. Half of going there is the window.
            | DeckCommand::OpenWorktree { .. } => true,
            // Making something is not a journey. You are already at the terminal, because you just
            // asked it for a new tab; fetching the window would spend a round trip nobody asked
            // for and, where raising cannot work, an alert about a failure that never happened.
            DeckCommand::MovePaneFocus { .. }
            | DeckCommand::ZoomPane { .. }
            | DeckCommand::SplitPane { .. }
            | DeckCommand::ClosePane { .. }
            | DeckCommand::ApplyLayout { .. }
            | DeckCommand::CreateWorkspace { .. }
            | DeckCommand::CreateTab { .. }
            | DeckCommand::CreateWorktree
            | DeckCommand::RemoveWorktree { .. }
            | DeckCommand::CloseTab { .. } => false,
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
            DeckCommand::ApplyLayout {
                preset: "dev".into(),
                layout: LayoutPreset::default(),
            },
            DeckCommand::OpenWorktree {
                path: "/src/.worktrees/api/fix".into(),
            },
            DeckCommand::CreateWorktree,
            DeckCommand::RemoveWorktree {
                workspace_id: "w3".into(),
            },
            DeckCommand::CreateWorkspace {
                preset: Some("notes".into()),
                spec: CreateSpec::default(),
            },
            DeckCommand::CreateTab {
                preset: Some("logs".into()),
                spec: CreateSpec::default(),
            },
            DeckCommand::CloseTab {
                tab_id: "w1:t2".into(),
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
    fn every_target_that_is_recorded_is_an_id_or_a_word_we_chose() {
        // The audit log copies `target` verbatim, so anything richer than this ends up on disk —
        // and a trail full of directory names and window titles is one that has to be guarded
        // like a secret rather than read like a log. herdr ids, enum spellings and preset names
        // all pass; anything else a person could have typed does not.
        for command in DeckCommand::every() {
            let Some(target) = command.target() else {
                continue;
            };
            assert!(
                !target.is_empty(),
                "{} recorded an empty target, which says less than recording none",
                command.name()
            );
            assert!(
                target.chars().all(|c| c.is_ascii_lowercase()
                    || c.is_ascii_digit()
                    || matches!(c, ':' | '_' | '-')),
                "{} named {target:?}, which is neither a herdr id nor a word we chose",
                command.name()
            );
        }
    }

    #[test]
    fn the_only_commands_that_decline_to_name_a_target_are_the_ones_whose_target_is_the_users_own()
    {
        // A worktree is a directory on somebody's disk named after a branch they invented, and
        // that is theirs rather than ours. Every other command can identify what it acted on
        // without copying anything out of the user's world, and this is what stops a future one
        // quietly deciding it cannot.
        let anonymous: Vec<_> = DeckCommand::every()
            .into_iter()
            .filter(|c| c.target().is_none())
            .map(|c| c.name())
            .collect();
        assert_eq!(anonymous, vec!["open_worktree", "create_worktree"]);
    }

    #[test]
    fn a_create_key_with_no_preset_behind_it_has_nothing_to_name_and_says_so() {
        // Distinct from the case above: this one is not withholding anything, there simply is no
        // name. The command's own name is the whole of the record, and that is enough — nothing
        // was destroyed and herdr's own state shows what appeared.
        assert_eq!(
            DeckCommand::CreateTab {
                preset: None,
                spec: CreateSpec::default(),
            }
            .target(),
            None
        );
    }

    #[test]
    fn only_closing_and_removing_can_destroy_work() {
        // Stated exhaustively so that wiring the *next* destructive command is a deliberate act
        // with a failing assertion attached, rather than something that slips in unnoticed. Note
        // what is absent: nothing here closes a workspace, and nothing here removes a worktree by
        // force.
        let mut destructive: Vec<_> = DeckCommand::every()
            .into_iter()
            .filter(DeckCommand::is_destructive)
            .map(|c| c.name())
            .collect();
        destructive.sort_unstable();
        assert_eq!(
            destructive,
            vec!["close_pane", "close_tab", "remove_worktree"]
        );
    }

    #[test]
    fn the_deck_has_no_way_at_all_to_close_a_workspace() {
        // Deliberate, and the sharpest omission in the vocabulary. `workspace.close` has no
        // confirmation, and when the target is the source-repo member of a worktree group it
        // closes *every* workspace in that group — then answers a bare `ok`, so the key could not
        // even report what it had destroyed. See docs/src/reference/herdr-protocol.md.
        assert!(
            !DeckCommand::every()
                .iter()
                .any(|c| c.name().contains("close_workspace")),
            "closing a workspace is not something this deck may offer"
        );
    }

    #[test]
    fn only_the_commands_that_take_you_somewhere_ask_for_the_window() {
        // Rearranging the terminal you are already sitting at is not a journey, and a key that
        // fetched the window every time you stepped one pane left would spend a round trip — and,
        // where raising cannot work at all, an alert — on something nobody asked for. Opening a
        // worktree *is* a journey, and is the only structural command that counts as one.
        let raises: Vec<_> = DeckCommand::every()
            .into_iter()
            .filter(DeckCommand::raises_the_window)
            .map(|c| c.name())
            .collect();
        assert_eq!(
            raises,
            vec![
                "focus_pane",
                "focus_workspace",
                "focus_tab",
                "open_worktree"
            ]
        );
    }

    #[test]
    fn the_only_commands_the_deck_waits_on_are_the_two_that_wait_on_git() {
        // Everything else in herdr's API answers at once. Marking a fast command slow would cost
        // it nothing; marking a slow one fast freezes the whole deck until git finishes.
        let slow: Vec<_> = DeckCommand::every()
            .into_iter()
            .filter(DeckCommand::may_take_a_while)
            .map(|c| c.name())
            .collect();
        assert_eq!(slow, vec!["create_worktree", "remove_worktree"]);
    }

    #[test]
    fn no_two_commands_share_a_name_in_the_audit_log() {
        // The log is read months later with `grep`, so two commands answering to one word would
        // make it impossible to tell which of them was pressed.
        let every = DeckCommand::every();
        let names: std::collections::HashSet<_> = every.iter().map(DeckCommand::name).collect();
        assert_eq!(
            names.len(),
            14,
            "one name per command, got {} for {} commands",
            names.len(),
            every.len()
        );
    }

    #[test]
    fn each_direction_a_key_can_offer_is_labelled_with_its_own_word() {
        // The shape on the tile is what these keys are read by, but the caption still has to
        // distinguish them — two keys captioned "pane" would be a coin toss.
        let every = DeckCommand::every();
        let labels: std::collections::HashSet<_> = every.iter().map(DeckCommand::label).collect();
        for word in ["left", "right", "up", "down", "zoom", "unzoom", "close"] {
            assert!(labels.contains(word), "no key would ever say {word:?}");
        }
    }
}
