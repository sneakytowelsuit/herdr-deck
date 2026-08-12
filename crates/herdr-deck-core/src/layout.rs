//! Turning hardware capabilities into a concrete deck layout.
//!
//! # The adaptation rule
//!
//! The engine never asks "is this a Stream Deck +". It asks how many keys there are, whether
//! there are dials, and how much room is left after the fixed controls. A model we have never
//! seen gets a working layout as long as the frontend reports its geometry.
//!
//! Degradation, in order:
//! - **Dials present** → scrubbing lives on the dials, all keys show agents.
//! - **No dials, plenty of keys** → no scrubbing needed; more agents are visible at once.
//! - **No dials, few keys** → paging keys appear so every agent is still reachable.
//! - **No display at all** (Pedal) → keys act, nothing renders.

use crate::capabilities::DeckCapabilities;
use crate::render::Tile;
use crate::state::DeckState;
use serde::{Deserialize, Serialize};

/// Which list the deck is currently showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    #[default]
    Agents,
    Workspaces,
}

impl Mode {
    pub fn toggled(self) -> Self {
        match self {
            Mode::Agents => Mode::Workspaces,
            Mode::Workspaces => Mode::Agents,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Mode::Agents => "agents",
            Mode::Workspaces => "spaces",
        }
    }
}

/// What a dial (or a degraded pair of keys) scrubs through.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScrubTarget {
    /// Every agent, in attention order.
    Agents,
    /// Workspaces.
    Workspaces,
    /// Tabs within the focused workspace.
    Tabs,
    /// Only agents that are blocked or done.
    Attention,
}

impl ScrubTarget {
    pub fn label(self) -> &'static str {
        match self {
            ScrubTarget::Agents => "agent",
            ScrubTarget::Workspaces => "space",
            ScrubTarget::Tabs => "tab",
            ScrubTarget::Attention => "needs you",
        }
    }
}

/// What a key does.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum KeyBinding {
    /// The Nth entry of the current mode's list. Contents change as agents come and go —
    /// this is what makes the deck useful with zero configuration.
    Dynamic {
        rank: usize,
    },
    /// Always this agent, whatever it is doing.
    PinnedAgent {
        terminal_id: String,
    },
    /// Always this workspace.
    PinnedWorkspace {
        workspace_id: String,
    },
    /// Jump straight to the top-ranked agent that wants you.
    NextAttention,
    /// Switch between agents and workspaces.
    ModeToggle,
    PagePrev,
    PageNext,
    /// Dial degradation for hardware without encoders.
    Scrub {
        target: ScrubTarget,
        delta: i32,
    },
    Empty,
}

/// What a dial does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum DialBinding {
    Scrub { target: ScrubTarget },
    Unused,
}

/// The action to perform when a control is pressed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlotAction {
    /// Focus an agent: herdr focus **and** raise the terminal window.
    FocusAgent {
        terminal_id: String,
    },
    FocusWorkspace {
        workspace_id: String,
    },
    /// Focus one tab. Distinct from [`SlotAction::FocusWorkspace`] because a workspace holds
    /// many tabs, and landing on the workspace's current one is not where the user pointed.
    FocusTab {
        tab_id: String,
    },
    ToggleMode,
    ChangePage {
        delta: i32,
    },
    Scrub {
        target: ScrubTarget,
        delta: i32,
    },
    /// Nothing bound, or nothing there right now.
    None,
}

/// Where each scrub cursor currently sits.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Selection {
    pub agents: usize,
    pub workspaces: usize,
    pub tabs: usize,
    pub attention: usize,
}

impl Selection {
    pub fn get(&self, target: ScrubTarget) -> usize {
        match target {
            ScrubTarget::Agents => self.agents,
            ScrubTarget::Workspaces => self.workspaces,
            ScrubTarget::Tabs => self.tabs,
            ScrubTarget::Attention => self.attention,
        }
    }

    /// Move a cursor, wrapping at both ends so a dial never gets stuck.
    pub fn scrub(&mut self, target: ScrubTarget, delta: i32, len: usize) {
        let slot = match target {
            ScrubTarget::Agents => &mut self.agents,
            ScrubTarget::Workspaces => &mut self.workspaces,
            ScrubTarget::Tabs => &mut self.tabs,
            ScrubTarget::Attention => &mut self.attention,
        };
        if len == 0 {
            *slot = 0;
            return;
        }
        let len_i = len as i64;
        let next = (*slot as i64 + delta as i64).rem_euclid(len_i);
        *slot = next as usize;
    }

    /// Keep cursors in range after the underlying lists change.
    pub fn clamp(&mut self, agents: usize, workspaces: usize, tabs: usize, attention: usize) {
        let fix = |v: &mut usize, len: usize| {
            if len == 0 {
                *v = 0;
            } else if *v >= len {
                *v = len - 1;
            }
        };
        fix(&mut self.agents, agents);
        fix(&mut self.workspaces, workspaces);
        fix(&mut self.tabs, tabs);
        fix(&mut self.attention, attention);
    }
}

/// A concrete assignment of bindings to physical controls.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Profile {
    pub keys: Vec<KeyBinding>,
    pub dials: Vec<DialBinding>,
}

impl Profile {
    /// Build the default layout for this hardware.
    pub fn for_capabilities(caps: &DeckCapabilities) -> Self {
        let dials = default_dials(caps.dials);
        let keys = default_keys(caps, !dials.is_empty());
        Self { keys, dials }
    }

    /// How many keys show list entries.
    pub fn dynamic_slots(&self) -> usize {
        self.keys
            .iter()
            .filter(|k| matches!(k, KeyBinding::Dynamic { .. }))
            .count()
    }

    pub fn has_paging(&self) -> bool {
        self.keys
            .iter()
            .any(|k| matches!(k, KeyBinding::PagePrev | KeyBinding::PageNext))
    }
}

/// Dials get the four most useful scrub targets, in the order you reach for them.
fn default_dials(count: u8) -> Vec<DialBinding> {
    const ORDER: [ScrubTarget; 4] = [
        ScrubTarget::Agents,
        ScrubTarget::Workspaces,
        ScrubTarget::Tabs,
        ScrubTarget::Attention,
    ];
    (0..count as usize)
        .map(|i| match ORDER.get(i) {
            Some(target) => DialBinding::Scrub { target: *target },
            None => DialBinding::Unused,
        })
        .collect()
}

fn default_keys(caps: &DeckCapabilities, has_dials: bool) -> Vec<KeyBinding> {
    let total = caps.key_count();
    if total == 0 {
        return vec![];
    }

    // A Pedal has no screen: give it the three actions that make sense blind.
    if !caps.has_display() {
        let mut keys = vec![KeyBinding::NextAttention];
        if total > 1 {
            keys.push(KeyBinding::Scrub {
                target: ScrubTarget::Attention,
                delta: -1,
            });
        }
        if total > 2 {
            keys.push(KeyBinding::Scrub {
                target: ScrubTarget::Attention,
                delta: 1,
            });
        }
        keys.resize(total, KeyBinding::Empty);
        return keys;
    }

    // Fixed controls, allocated from the end of the deck.
    let mut reserved: Vec<KeyBinding> = Vec::new();
    if total >= 4 {
        reserved.push(KeyBinding::NextAttention);
        reserved.push(KeyBinding::ModeToggle);
    }
    // Without dials, small decks need a way to reach agents past the first page.
    if !has_dials && total >= 8 {
        reserved.push(KeyBinding::PagePrev);
        reserved.push(KeyBinding::PageNext);
    }
    // Never let fixed controls crowd out the agents they are meant to navigate.
    let max_reserved = total.saturating_sub(1).min(total / 2);
    reserved.truncate(max_reserved);

    let dynamic = total - reserved.len();
    let mut keys: Vec<KeyBinding> = (0..dynamic)
        .map(|rank| KeyBinding::Dynamic { rank })
        .collect();
    keys.extend(reserved);
    keys
}

/// Resolves a profile against live state to produce what to draw and what to do.
#[derive(Debug, Clone)]
pub struct ResolvedDeck<'a> {
    profile: &'a Profile,
    state: &'a DeckState,
    mode: Mode,
    page: usize,
    selection: Selection,
}

impl<'a> ResolvedDeck<'a> {
    pub fn new(
        profile: &'a Profile,
        state: &'a DeckState,
        mode: Mode,
        page: usize,
        selection: Selection,
    ) -> Self {
        Self {
            profile,
            state,
            mode,
            page,
            selection,
        }
    }

    /// The list index a dynamic slot of this rank refers to, accounting for the page.
    fn list_index(&self, rank: usize) -> usize {
        self.page * self.profile.dynamic_slots().max(1) + rank
    }

    fn list_len(&self) -> usize {
        match self.mode {
            Mode::Agents => self.state.agents.len(),
            Mode::Workspaces => self.state.workspaces.len(),
        }
    }

    pub fn page_count(&self) -> usize {
        let per_page = self.profile.dynamic_slots().max(1);
        self.list_len().div_ceil(per_page).max(1)
    }

    /// What to draw on key `index`.
    pub fn tile(&self, index: usize) -> Tile {
        if let Some(reason) = &self.state.offline_reason {
            // Every key says the same thing when herdr is gone — a half-live deck is worse
            // than an obviously dead one.
            return Tile::Offline {
                message: reason.clone(),
            };
        }
        let Some(binding) = self.profile.keys.get(index) else {
            return Tile::Empty;
        };
        match binding {
            KeyBinding::Dynamic { rank } => self.list_tile(self.list_index(*rank)),
            KeyBinding::PinnedAgent { terminal_id } => self
                .state
                .agent_by_terminal_id(terminal_id)
                .map(|agent| agent_tile(self.state, agent))
                .unwrap_or(Tile::Empty),
            KeyBinding::PinnedWorkspace { workspace_id } => self
                .state
                .workspace_by_id(workspace_id)
                .map(workspace_tile)
                .unwrap_or(Tile::Empty),
            KeyBinding::NextAttention => Tile::Attention {
                count: self.state.attention_count(),
            },
            KeyBinding::ModeToggle => Tile::Mode {
                label: self.mode.toggled().label().to_string(),
                active: false,
            },
            KeyBinding::PagePrev => Tile::Mode {
                label: "◀ page".to_string(),
                active: self.page > 0,
            },
            KeyBinding::PageNext => Tile::Mode {
                label: "page ▶".to_string(),
                active: self.page + 1 < self.page_count(),
            },
            KeyBinding::Scrub { target, delta } => Tile::Mode {
                label: format!("{} {}", if *delta < 0 { "◀" } else { "▶" }, target.label()),
                active: true,
            },
            KeyBinding::Empty => Tile::Empty,
        }
    }

    fn list_tile(&self, index: usize) -> Tile {
        match self.mode {
            Mode::Agents => self
                .state
                .agent_at(index)
                .map(|agent| agent_tile(self.state, agent))
                .unwrap_or(Tile::Empty),
            Mode::Workspaces => self
                .state
                .workspaces
                .get(index)
                .map(workspace_tile)
                .unwrap_or(Tile::Empty),
        }
    }

    /// What pressing key `index` should do.
    pub fn key_action(&self, index: usize) -> SlotAction {
        if self.state.is_offline() {
            return SlotAction::None;
        }
        let Some(binding) = self.profile.keys.get(index) else {
            return SlotAction::None;
        };
        match binding {
            KeyBinding::Dynamic { rank } => self.list_action(self.list_index(*rank)),
            KeyBinding::PinnedAgent { terminal_id } => {
                if self.state.agent_by_terminal_id(terminal_id).is_some() {
                    SlotAction::FocusAgent {
                        terminal_id: terminal_id.clone(),
                    }
                } else {
                    SlotAction::None
                }
            }
            KeyBinding::PinnedWorkspace { workspace_id } => SlotAction::FocusWorkspace {
                workspace_id: workspace_id.clone(),
            },
            KeyBinding::NextAttention => self
                .state
                .top_attention()
                .map(|a| SlotAction::FocusAgent {
                    terminal_id: a.terminal_id.clone(),
                })
                .unwrap_or(SlotAction::None),
            KeyBinding::ModeToggle => SlotAction::ToggleMode,
            KeyBinding::PagePrev => SlotAction::ChangePage { delta: -1 },
            KeyBinding::PageNext => SlotAction::ChangePage { delta: 1 },
            KeyBinding::Scrub { target, delta } => SlotAction::Scrub {
                target: *target,
                delta: *delta,
            },
            KeyBinding::Empty => SlotAction::None,
        }
    }

    fn list_action(&self, index: usize) -> SlotAction {
        match self.mode {
            Mode::Agents => self
                .state
                .agent_at(index)
                .map(|a| SlotAction::FocusAgent {
                    terminal_id: a.terminal_id.clone(),
                })
                .unwrap_or(SlotAction::None),
            Mode::Workspaces => self
                .state
                .workspaces
                .get(index)
                .map(|w| SlotAction::FocusWorkspace {
                    workspace_id: w.workspace_id.clone(),
                })
                .unwrap_or(SlotAction::None),
        }
    }

    /// Rotating a dial scrubs its list.
    pub fn dial_rotate_action(&self, dial: usize, ticks: i32) -> SlotAction {
        match self.profile.dials.get(dial) {
            Some(DialBinding::Scrub { target }) => SlotAction::Scrub {
                target: *target,
                delta: ticks,
            },
            _ => SlotAction::None,
        }
    }

    /// Pressing a dial focuses whatever its cursor is on.
    pub fn dial_press_action(&self, dial: usize) -> SlotAction {
        if self.state.is_offline() {
            return SlotAction::None;
        }
        let Some(DialBinding::Scrub { target }) = self.profile.dials.get(dial) else {
            return SlotAction::None;
        };
        let cursor = self.selection.get(*target);
        match target {
            ScrubTarget::Agents => self
                .state
                .agent_at(cursor)
                .map(|a| SlotAction::FocusAgent {
                    terminal_id: a.terminal_id.clone(),
                })
                .unwrap_or(SlotAction::None),
            ScrubTarget::Attention => self
                .state
                .needing_attention()
                .nth(cursor)
                .map(|a| SlotAction::FocusAgent {
                    terminal_id: a.terminal_id.clone(),
                })
                .unwrap_or(SlotAction::None),
            ScrubTarget::Workspaces => self
                .state
                .workspaces
                .get(cursor)
                .map(|w| SlotAction::FocusWorkspace {
                    workspace_id: w.workspace_id.clone(),
                })
                .unwrap_or(SlotAction::None),
            ScrubTarget::Tabs => self
                .state
                .tabs
                .get(cursor)
                .map(|t| SlotAction::FocusTab {
                    tab_id: t.tab_id.clone(),
                })
                .unwrap_or(SlotAction::None),
        }
    }

    /// Title and value for the touchstrip segment above `dial`.
    pub fn dial_feedback(
        &self,
        dial: usize,
    ) -> (String, String, Option<herdr_deck_herdr::wire::AgentStatus>) {
        let Some(DialBinding::Scrub { target }) = self.profile.dials.get(dial) else {
            return (String::new(), String::new(), None);
        };
        if let Some(reason) = &self.state.offline_reason {
            return (target.label().to_string(), reason.clone(), None);
        }
        let cursor = self.selection.get(*target);
        let (value, status) = match target {
            ScrubTarget::Agents => self
                .state
                .agent_at(cursor)
                .map(|a| (a.label().to_string(), Some(a.agent_status)))
                .unwrap_or_else(|| ("—".to_string(), None)),
            ScrubTarget::Attention => self
                .state
                .needing_attention()
                .nth(cursor)
                .map(|a| (a.label().to_string(), Some(a.agent_status)))
                .unwrap_or_else(|| ("all clear".to_string(), None)),
            ScrubTarget::Workspaces => self
                .state
                .workspaces
                .get(cursor)
                .map(|w| (w.label().to_string(), Some(w.agent_status)))
                .unwrap_or_else(|| ("—".to_string(), None)),
            ScrubTarget::Tabs => self
                .state
                .tabs
                .get(cursor)
                .map(|t| {
                    (
                        t.label.clone().unwrap_or_else(|| t.tab_id.clone()),
                        Some(t.agent_status),
                    )
                })
                .unwrap_or_else(|| ("—".to_string(), None)),
        };
        (target.label().to_string(), value, status)
    }
}

/// Needs the whole state, not just the agent: the footer names the agent's *project*, and only
/// the state knows what herdr calls the workspace the agent happens to be in.
fn agent_tile(state: &DeckState, agent: &herdr_deck_herdr::wire::AgentInfo) -> Tile {
    Tile::Agent {
        label: agent.label().to_string(),
        sublabel: agent.state_label().map(str::to_string),
        workspace: state.project_label(agent).map(str::to_string),
        status: agent.agent_status,
        focused: agent.focused,
    }
}

fn workspace_tile(ws: &herdr_deck_herdr::wire::WorkspaceInfo) -> Tile {
    Tile::Workspace {
        label: ws.label().to_string(),
        status: ws.agent_status,
        focused: ws.focused,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capabilities::DeckModel;
    use herdr_deck_herdr::wire::{AgentInfo, AgentStatus, SessionSnapshot, TabInfo, WorkspaceInfo};

    fn agent(id: &str, status: AgentStatus, seq: u64) -> AgentInfo {
        AgentInfo {
            terminal_id: id.into(),
            agent_status: status,
            workspace_id: "w1".into(),
            pane_id: format!("w1:{id}"),
            state_change_seq: Some(seq),
            ..Default::default()
        }
    }

    fn state_with(agents: Vec<AgentInfo>) -> DeckState {
        DeckState::from_snapshot(SessionSnapshot {
            agents,
            ..Default::default()
        })
    }

    #[test]
    fn stream_deck_plus_gets_four_scrub_dials_and_all_keys_for_agents() {
        let caps = DeckModel::Plus.capabilities();
        let profile = Profile::for_capabilities(&caps);
        assert_eq!(profile.dials.len(), 4);
        assert_eq!(
            profile.dials[0],
            DialBinding::Scrub {
                target: ScrubTarget::Agents
            }
        );
        assert_eq!(
            profile.dials[3],
            DialBinding::Scrub {
                target: ScrubTarget::Attention
            }
        );
        // 8 keys: 6 agents + next-attention + mode toggle. No paging keys — the dials scrub.
        assert_eq!(profile.dynamic_slots(), 6);
        assert!(!profile.has_paging());
    }

    #[test]
    fn dial_less_hardware_gets_paging_keys_instead_of_scrub_dials() {
        let caps = DeckModel::Xl.capabilities();
        let profile = Profile::for_capabilities(&caps);
        assert!(profile.dials.is_empty());
        assert!(
            profile.has_paging(),
            "without dials there must be another way to reach agents past page one"
        );
        assert_eq!(profile.keys.len(), 32);
    }

    #[test]
    fn a_tiny_deck_still_reserves_room_for_agents() {
        let caps = DeckModel::Mini.capabilities();
        let profile = Profile::for_capabilities(&caps);
        assert_eq!(profile.keys.len(), 6);
        assert!(
            profile.dynamic_slots() >= 3,
            "fixed controls must not crowd out the agents they navigate"
        );
    }

    #[test]
    fn unknown_hardware_gets_a_working_layout_from_geometry_alone() {
        let caps = DeckCapabilities::from_geometry("Future Deck", 6, 3, 128, 2, None);
        let profile = Profile::for_capabilities(&caps);
        assert_eq!(profile.keys.len(), 18);
        assert_eq!(profile.dials.len(), 2);
        assert!(profile.dynamic_slots() > 0);
    }

    #[test]
    fn a_pedal_gets_actions_and_never_a_dynamic_tile() {
        let caps = DeckModel::Pedal.capabilities();
        let profile = Profile::for_capabilities(&caps);
        assert_eq!(profile.keys.len(), 3);
        assert_eq!(profile.dynamic_slots(), 0);
        assert_eq!(profile.keys[0], KeyBinding::NextAttention);
    }

    #[test]
    fn pressing_a_dynamic_key_focuses_the_agent_it_shows() {
        let caps = DeckModel::Plus.capabilities();
        let profile = Profile::for_capabilities(&caps);
        let state = state_with(vec![
            agent("calm", AgentStatus::Idle, 1),
            agent("stuck", AgentStatus::Blocked, 2),
        ]);
        let deck = ResolvedDeck::new(&profile, &state, Mode::Agents, 0, Selection::default());
        // Blocked sorts first, so key 0 shows it.
        assert_eq!(
            deck.key_action(0),
            SlotAction::FocusAgent {
                terminal_id: "stuck".into()
            }
        );
    }

    #[test]
    fn next_attention_targets_the_top_ranked_blocked_agent() {
        let caps = DeckModel::Plus.capabilities();
        let profile = Profile::for_capabilities(&caps);
        let state = state_with(vec![
            agent("working", AgentStatus::Working, 5),
            agent("older_block", AgentStatus::Blocked, 1),
            agent("fresh_block", AgentStatus::Blocked, 9),
        ]);
        let deck = ResolvedDeck::new(&profile, &state, Mode::Agents, 0, Selection::default());
        let next_key = profile
            .keys
            .iter()
            .position(|k| *k == KeyBinding::NextAttention)
            .expect("profile has a next-attention key");
        assert_eq!(
            deck.key_action(next_key),
            SlotAction::FocusAgent {
                terminal_id: "fresh_block".into()
            }
        );
    }

    #[test]
    fn next_attention_does_nothing_when_nothing_needs_you() {
        let caps = DeckModel::Plus.capabilities();
        let profile = Profile::for_capabilities(&caps);
        let state = state_with(vec![agent("calm", AgentStatus::Idle, 1)]);
        let deck = ResolvedDeck::new(&profile, &state, Mode::Agents, 0, Selection::default());
        let next_key = profile
            .keys
            .iter()
            .position(|k| *k == KeyBinding::NextAttention)
            .unwrap();
        assert_eq!(deck.key_action(next_key), SlotAction::None);
        assert_eq!(deck.tile(next_key), Tile::Attention { count: 0 });
    }

    #[test]
    fn an_empty_slot_is_inert_rather_than_focusing_something_random() {
        let caps = DeckModel::Plus.capabilities();
        let profile = Profile::for_capabilities(&caps);
        let state = state_with(vec![agent("only", AgentStatus::Idle, 1)]);
        let deck = ResolvedDeck::new(&profile, &state, Mode::Agents, 0, Selection::default());
        assert_eq!(deck.tile(3), Tile::Empty);
        assert_eq!(deck.key_action(3), SlotAction::None);
    }

    #[test]
    fn when_herdr_is_offline_every_key_says_so_and_nothing_acts() {
        let caps = DeckModel::Plus.capabilities();
        let profile = Profile::for_capabilities(&caps);
        let state = DeckState::offline("herdr socket not found");
        let deck = ResolvedDeck::new(&profile, &state, Mode::Agents, 0, Selection::default());
        for index in 0..profile.keys.len() {
            assert!(matches!(deck.tile(index), Tile::Offline { .. }));
            assert_eq!(deck.key_action(index), SlotAction::None);
        }
    }

    #[test]
    fn workspace_mode_lists_workspaces_and_focuses_them() {
        let caps = DeckModel::Plus.capabilities();
        let profile = Profile::for_capabilities(&caps);
        let mut state = state_with(vec![]);
        state.workspaces = vec![WorkspaceInfo {
            workspace_id: "w2".into(),
            label: Some("web".into()),
            agent_status: AgentStatus::Working,
            ..Default::default()
        }];
        let deck = ResolvedDeck::new(&profile, &state, Mode::Workspaces, 0, Selection::default());
        assert_eq!(
            deck.key_action(0),
            SlotAction::FocusWorkspace {
                workspace_id: "w2".into()
            }
        );
        assert!(matches!(deck.tile(0), Tile::Workspace { .. }));
    }

    #[test]
    fn paging_moves_the_window_over_the_agent_list() {
        // 6 dynamic slots on a Plus; page 1 starts at index 6.
        let caps = DeckModel::Plus.capabilities();
        let profile = Profile::for_capabilities(&caps);
        let agents: Vec<_> = (0..10)
            .map(|i| agent(&format!("a{i:02}"), AgentStatus::Idle, 0))
            .collect();
        let state = state_with(agents);
        let deck = ResolvedDeck::new(&profile, &state, Mode::Agents, 1, Selection::default());
        assert_eq!(
            deck.key_action(0),
            SlotAction::FocusAgent {
                terminal_id: "a06".into()
            }
        );
        assert_eq!(deck.page_count(), 2);
    }

    #[test]
    fn a_dial_press_focuses_whatever_its_cursor_is_on() {
        let caps = DeckModel::Plus.capabilities();
        let profile = Profile::for_capabilities(&caps);
        let state = state_with(vec![
            agent("first", AgentStatus::Blocked, 9),
            agent("second", AgentStatus::Blocked, 5),
        ]);
        let selection = Selection {
            agents: 1,
            ..Default::default()
        };
        let deck = ResolvedDeck::new(&profile, &state, Mode::Agents, 0, selection);
        assert_eq!(
            deck.dial_press_action(0),
            SlotAction::FocusAgent {
                terminal_id: "second".into()
            }
        );
    }

    #[test]
    fn the_attention_dial_only_scrubs_agents_that_want_you() {
        let caps = DeckModel::Plus.capabilities();
        let profile = Profile::for_capabilities(&caps);
        let state = state_with(vec![
            agent("blocked", AgentStatus::Blocked, 9),
            agent("busy", AgentStatus::Working, 8),
            agent("finished", AgentStatus::Done, 7),
        ]);
        let selection = Selection {
            attention: 1,
            ..Default::default()
        };
        let deck = ResolvedDeck::new(&profile, &state, Mode::Agents, 0, selection);
        // Index 1 of the attention list is the `done` agent — the working one is skipped.
        assert_eq!(
            deck.dial_press_action(3),
            SlotAction::FocusAgent {
                terminal_id: "finished".into()
            }
        );
    }

    #[test]
    fn pressing_the_tab_dial_focuses_that_tab_and_not_the_workspace_holding_it() {
        // Focusing the workspace lands on whichever tab it happens to have active, which is
        // rarely the one the user just scrubbed to — the whole point of the dial.
        let caps = DeckModel::Plus.capabilities();
        let profile = Profile::for_capabilities(&caps);
        let mut state = state_with(vec![]);
        state.tabs = vec![
            TabInfo {
                tab_id: "w1:t1".into(),
                workspace_id: "w1".into(),
                ..Default::default()
            },
            TabInfo {
                tab_id: "w1:t2".into(),
                workspace_id: "w1".into(),
                ..Default::default()
            },
        ];
        let selection = Selection {
            tabs: 1,
            ..Default::default()
        };
        let deck = ResolvedDeck::new(&profile, &state, Mode::Agents, 0, selection);
        assert_eq!(
            deck.dial_press_action(2),
            SlotAction::FocusTab {
                tab_id: "w1:t2".into()
            }
        );
    }

    #[test]
    fn rotating_a_dial_produces_a_scrub_for_its_own_target() {
        let caps = DeckModel::Plus.capabilities();
        let profile = Profile::for_capabilities(&caps);
        let state = state_with(vec![]);
        let deck = ResolvedDeck::new(&profile, &state, Mode::Agents, 0, Selection::default());
        assert_eq!(
            deck.dial_rotate_action(1, 2),
            SlotAction::Scrub {
                target: ScrubTarget::Workspaces,
                delta: 2
            }
        );
    }

    #[test]
    fn scrubbing_wraps_at_both_ends_so_a_dial_never_gets_stuck() {
        let mut sel = Selection::default();
        sel.scrub(ScrubTarget::Agents, -1, 3);
        assert_eq!(
            sel.agents, 2,
            "scrolling back from the top wraps to the end"
        );
        sel.scrub(ScrubTarget::Agents, 1, 3);
        assert_eq!(sel.agents, 0, "scrolling past the end wraps to the top");
        sel.scrub(ScrubTarget::Agents, 7, 3);
        assert_eq!(sel.agents, 1, "large jumps wrap correctly");
    }

    #[test]
    fn scrubbing_an_empty_list_parks_at_zero_rather_than_dividing_by_zero() {
        let mut sel = Selection {
            agents: 4,
            ..Default::default()
        };
        sel.scrub(ScrubTarget::Agents, 1, 0);
        assert_eq!(sel.agents, 0);
    }

    #[test]
    fn cursors_are_clamped_when_agents_disappear() {
        let mut sel = Selection {
            agents: 9,
            workspaces: 4,
            tabs: 2,
            attention: 3,
        };
        sel.clamp(2, 0, 5, 1);
        assert_eq!(sel.agents, 1);
        assert_eq!(sel.workspaces, 0);
        assert_eq!(sel.tabs, 2);
        assert_eq!(sel.attention, 0);
    }

    #[test]
    fn touchstrip_feedback_names_the_target_and_the_selection() {
        let caps = DeckModel::Plus.capabilities();
        let profile = Profile::for_capabilities(&caps);
        let mut blocked = agent("refactor", AgentStatus::Blocked, 1);
        blocked.name = Some("refactor".into());
        let state = state_with(vec![blocked]);
        let deck = ResolvedDeck::new(&profile, &state, Mode::Agents, 0, Selection::default());
        let (title, value, status) = deck.dial_feedback(0);
        assert_eq!(title, "agent");
        assert_eq!(value, "refactor");
        assert_eq!(status, Some(AgentStatus::Blocked));
    }

    #[test]
    fn touchstrip_says_all_clear_when_the_attention_list_is_empty() {
        let caps = DeckModel::Plus.capabilities();
        let profile = Profile::for_capabilities(&caps);
        let state = state_with(vec![agent("busy", AgentStatus::Working, 1)]);
        let deck = ResolvedDeck::new(&profile, &state, Mode::Agents, 0, Selection::default());
        let (_, value, _) = deck.dial_feedback(3);
        assert_eq!(value, "all clear");
    }

    #[test]
    fn mode_toggles_back_and_forth() {
        assert_eq!(Mode::Agents.toggled(), Mode::Workspaces);
        assert_eq!(Mode::Workspaces.toggled(), Mode::Agents);
    }

    // --- The project footer ------------------------------------------------------------------
    //
    // The bottom row is the only thing on an agent tile that says *where* the work is, so these
    // pin the whole fallback chain rather than just its happy path.

    /// The footer the deck would draw for the state's single agent.
    fn project_footer(state: &DeckState) -> Option<String> {
        let agent = state.agents.first().expect("state has an agent");
        match agent_tile(state, agent) {
            Tile::Agent { workspace, .. } => workspace,
            other => panic!("expected an agent tile, got {other:?}"),
        }
    }

    fn state_with_workspace(agent: AgentInfo, workspace: WorkspaceInfo) -> DeckState {
        DeckState::from_snapshot(SessionSnapshot {
            agents: vec![agent],
            workspaces: vec![workspace],
            ..Default::default()
        })
    }

    #[test]
    fn a_tile_names_the_project_by_its_workspace_label_rather_than_the_raw_id() {
        let state = state_with_workspace(
            agent("a", AgentStatus::Blocked, 1),
            WorkspaceInfo {
                workspace_id: "w1".into(),
                label: Some("payments-api".into()),
                ..Default::default()
            },
        );
        assert_eq!(project_footer(&state).as_deref(), Some("payments-api"));
    }

    #[test]
    fn an_agent_in_an_unlabelled_workspace_is_named_by_the_directory_it_works_in() {
        let mut a = agent("a", AgentStatus::Blocked, 1);
        a.cwd = Some("/home/dev/src/payments-api".into());
        let state = state_with_workspace(
            a,
            WorkspaceInfo {
                workspace_id: "w1".into(),
                ..Default::default()
            },
        );
        assert_eq!(
            project_footer(&state).as_deref(),
            Some("payments-api"),
            "only the basename: a full path is illegible on a 72px key"
        );
    }

    #[test]
    fn the_raw_workspace_id_survives_as_a_footer_when_nothing_better_is_known() {
        // Better a bare id than a blank row — an empty footer would read as "no project", which
        // is a stronger and wronger claim than "the project is whatever w1 is".
        let state = state_with(vec![agent("a", AgentStatus::Blocked, 1)]);
        assert_eq!(project_footer(&state).as_deref(), Some("w1"));
    }

    #[test]
    fn a_working_directory_with_a_trailing_slash_still_names_its_project() {
        let mut a = agent("a", AgentStatus::Blocked, 1);
        a.cwd = Some("/home/dev/src/payments-api/".into());
        let state = state_with(vec![a]);
        assert_eq!(
            project_footer(&state).as_deref(),
            Some("payments-api"),
            "a naive split on `/` would hand back an empty name and fall through to the id"
        );
    }
}
