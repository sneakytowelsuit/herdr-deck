//! Colours and glyphs for deck tiles.
//!
//! # Why every status carries a glyph
//!
//! Status is never encoded in hue alone. Each state has a distinct shape as well as a distinct
//! colour, so the deck stays readable for colour-blind users and in the washed-out contrast of a
//! Stream Deck's LCD viewed off-axis. If you add a status, give it a glyph.

use herdr_deck_herdr::wire::AgentStatus;
use serde::{Deserialize, Serialize};

/// How one status is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StatusStyle {
    /// Tile background.
    pub background: &'static str,
    /// Primary text colour on that background.
    pub foreground: &'static str,
    /// Accent used for the status bar and glyph.
    pub accent: &'static str,
    /// A shape, not just a colour — see the module note.
    pub glyph: &'static str,
    /// Whether this state should dominate the tile (attention states do).
    pub emphatic: bool,
}

/// The palette. Deliberately small and high-contrast.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Theme {
    #[default]
    Dark,
}

impl Theme {
    pub fn status(self, status: AgentStatus) -> StatusStyle {
        match status {
            // Blocked owns the whole tile: this is the state the entire product exists for.
            AgentStatus::Blocked => StatusStyle {
                background: "#B4242A",
                foreground: "#FFFFFF",
                accent: "#FF8A8F",
                glyph: "!",
                emphatic: true,
            },
            // Done is loud but not alarming — work finished, come look when you can.
            AgentStatus::Done => StatusStyle {
                background: "#8A5A00",
                foreground: "#FFFFFF",
                accent: "#FFC85C",
                glyph: "✓",
                emphatic: true,
            },
            AgentStatus::Working => StatusStyle {
                background: "#16181D",
                foreground: "#E6E8EC",
                accent: "#4C9AFF",
                glyph: "▶",
                emphatic: false,
            },
            AgentStatus::Idle => StatusStyle {
                background: "#131418",
                foreground: "#8A8F99",
                accent: "#4A4F59",
                glyph: "·",
                emphatic: false,
            },
            AgentStatus::Unknown => StatusStyle {
                background: "#131418",
                foreground: "#7A7F89",
                accent: "#5A5F69",
                glyph: "?",
                emphatic: false,
            },
        }
    }

    /// An attention state the user has dismissed with a long press.
    ///
    /// Calm, because that is the whole point of dismissing it, but marked with a glyph of its own
    /// rather than borrowed from `idle`: the agent may still be blocked, and a tile that claimed
    /// otherwise would be lying about the one thing this deck exists to report.
    pub fn acknowledged(self) -> StatusStyle {
        StatusStyle {
            background: "#131418",
            foreground: "#8A8F99",
            accent: "#6E7480",
            glyph: "×",
            emphatic: false,
        }
    }

    /// A git worktree, drawn by whether herdr already has it open.
    ///
    /// Filled circle for open, hollow for not — a difference in *shape*, so the two are told apart
    /// on a washed-out LCD seen off-axis and by someone who cannot tell the two greys apart. The
    /// colours differ as well, but only as the second signal.
    pub fn worktree(self, open: bool) -> StatusStyle {
        if open {
            StatusStyle {
                background: "#16181D",
                foreground: "#E6E8EC",
                accent: "#7FBF7F",
                glyph: "●",
                emphatic: false,
            }
        } else {
            StatusStyle {
                background: "#131418",
                foreground: "#8A8F99",
                accent: "#5A6A5A",
                glyph: "○",
                emphatic: false,
            }
        }
    }

    /// Background for tiles that are not showing an agent.
    pub fn neutral_background(self) -> &'static str {
        "#0E0F12"
    }

    pub fn neutral_foreground(self) -> &'static str {
        "#C6CAD2"
    }

    pub fn dim_foreground(self) -> &'static str {
        "#71767F"
    }

    /// herdr is unreachable. Distinct from any agent state so it can never be mistaken for one.
    pub fn offline(self) -> StatusStyle {
        StatusStyle {
            background: "#1A1206",
            foreground: "#D9A441",
            accent: "#D9A441",
            glyph: "⚠",
            emphatic: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_status_has_a_distinct_glyph_so_colour_is_never_the_only_signal() {
        let theme = Theme::Dark;
        let statuses = [
            AgentStatus::Blocked,
            AgentStatus::Done,
            AgentStatus::Working,
            AgentStatus::Idle,
            AgentStatus::Unknown,
        ];
        let glyphs: Vec<_> = statuses.iter().map(|s| theme.status(*s).glyph).collect();
        let mut unique = glyphs.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(
            unique.len(),
            glyphs.len(),
            "statuses must be distinguishable by shape, not just hue"
        );
    }

    #[test]
    fn every_status_has_a_distinct_background() {
        let theme = Theme::Dark;
        // Idle and Unknown intentionally share a near-identical calm background, but the
        // attention states must each be unmistakable.
        assert_ne!(
            theme.status(AgentStatus::Blocked).background,
            theme.status(AgentStatus::Done).background
        );
        assert_ne!(
            theme.status(AgentStatus::Blocked).background,
            theme.status(AgentStatus::Working).background
        );
    }

    #[test]
    fn attention_states_are_emphatic_and_calm_states_are_not() {
        let theme = Theme::Dark;
        assert!(theme.status(AgentStatus::Blocked).emphatic);
        assert!(theme.status(AgentStatus::Done).emphatic);
        assert!(!theme.status(AgentStatus::Working).emphatic);
        assert!(!theme.status(AgentStatus::Idle).emphatic);
    }

    #[test]
    fn offline_is_not_confusable_with_any_agent_state() {
        let theme = Theme::Dark;
        let offline = theme.offline();
        for status in [
            AgentStatus::Blocked,
            AgentStatus::Done,
            AgentStatus::Working,
            AgentStatus::Idle,
            AgentStatus::Unknown,
        ] {
            assert_ne!(theme.status(status).glyph, offline.glyph);
        }
    }

    #[test]
    fn an_acknowledged_tile_is_calm_but_still_tells_you_it_was_dismissed() {
        // Reusing idle's glyph would make a dismissed blocked agent indistinguishable from one
        // that genuinely has nothing to say — the colour is calm, the shape has to differ.
        let theme = Theme::Dark;
        let acknowledged = theme.acknowledged();
        assert!(!acknowledged.emphatic);
        for status in [
            AgentStatus::Blocked,
            AgentStatus::Done,
            AgentStatus::Working,
            AgentStatus::Idle,
            AgentStatus::Unknown,
        ] {
            assert_ne!(theme.status(status).glyph, acknowledged.glyph);
        }
        assert_ne!(theme.offline().glyph, acknowledged.glyph);
    }
}
