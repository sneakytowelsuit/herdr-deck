//! The daemon's view of herdr.
//!
//! Built from `session.snapshot` and refreshed whenever an event says something moved. The one
//! interesting piece of logic here is the **attention order**, which decides what lands on the
//! deck when nothing is pinned.

use herdr_deck_herdr::wire::{AgentInfo, AgentStatus, SessionSnapshot, TabInfo, WorkspaceInfo};

/// A point-in-time view of every agent and workspace herdr knows about.
#[derive(Debug, Clone, Default)]
pub struct DeckState {
    pub agents: Vec<AgentInfo>,
    pub workspaces: Vec<WorkspaceInfo>,
    pub tabs: Vec<TabInfo>,
    pub focused_workspace_id: Option<String>,
    pub focused_pane_id: Option<String>,
    /// `None` until we have successfully talked to herdr at least once.
    pub protocol: Option<u32>,
    /// Set when herdr is unreachable, so the deck can say so instead of going blank.
    pub offline_reason: Option<String>,
}

impl DeckState {
    /// Replace the whole state from a fresh snapshot.
    ///
    /// Wholesale replacement rather than merging is deliberate: the snapshot *is* the truth, and
    /// a merge would let a stale agent linger on the deck after herdr forgot about it.
    pub fn from_snapshot(snapshot: SessionSnapshot) -> Self {
        let mut state = Self {
            agents: snapshot.agents,
            workspaces: snapshot.workspaces,
            tabs: snapshot.tabs,
            focused_workspace_id: snapshot.focused_workspace_id,
            focused_pane_id: snapshot.focused_pane_id,
            protocol: snapshot.protocol,
            offline_reason: None,
        };
        state.agents.sort_by(attention_order);
        state
    }

    /// Mark herdr unreachable, preserving nothing — a stale tile is worse than an honest one.
    pub fn offline(reason: impl Into<String>) -> Self {
        Self {
            offline_reason: Some(reason.into()),
            ..Default::default()
        }
    }

    pub fn is_offline(&self) -> bool {
        self.offline_reason.is_some()
    }

    /// Agents in attention order: blocked first, then done, then working, then idle.
    ///
    /// Within a status band, the most recently changed agent comes first — when three agents
    /// are blocked, the one that *just* blocked is the one you are most likely reaching for.
    pub fn attention_order(&self) -> &[AgentInfo] {
        &self.agents
    }

    /// Only the agents that actually want a human: `blocked` and `done`.
    pub fn needing_attention(&self) -> impl Iterator<Item = &AgentInfo> {
        self.agents
            .iter()
            .filter(|a| a.agent_status.needs_attention())
    }

    pub fn attention_count(&self) -> usize {
        self.needing_attention().count()
    }

    /// The single agent a "next attention" key should jump to.
    pub fn top_attention(&self) -> Option<&AgentInfo> {
        self.needing_attention().next()
    }

    /// The Nth agent in attention order, for a dynamic slot.
    pub fn agent_at(&self, index: usize) -> Option<&AgentInfo> {
        self.agents.get(index)
    }

    pub fn agent_by_terminal_id(&self, terminal_id: &str) -> Option<&AgentInfo> {
        self.agents.iter().find(|a| a.terminal_id == terminal_id)
    }

    pub fn workspace_by_id(&self, workspace_id: &str) -> Option<&WorkspaceInfo> {
        self.workspaces
            .iter()
            .find(|w| w.workspace_id == workspace_id)
    }

    /// Count of agents in each status, for summary tiles.
    pub fn status_counts(&self) -> StatusCounts {
        let mut counts = StatusCounts::default();
        for agent in &self.agents {
            match agent.agent_status {
                AgentStatus::Blocked => counts.blocked += 1,
                AgentStatus::Done => counts.done += 1,
                AgentStatus::Working => counts.working += 1,
                AgentStatus::Idle => counts.idle += 1,
                AgentStatus::Unknown => counts.unknown += 1,
            }
        }
        counts
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StatusCounts {
    pub blocked: usize,
    pub done: usize,
    pub working: usize,
    pub idle: usize,
    pub unknown: usize,
}

impl StatusCounts {
    pub fn total(&self) -> usize {
        self.blocked + self.done + self.working + self.idle + self.unknown
    }

    pub fn needing_attention(&self) -> usize {
        self.blocked + self.done
    }
}

/// Sort agents so the ones that want a human float to the top.
///
/// Ordering, in priority order:
/// 1. status band (`blocked` < `done` < `working` < `idle` < `unknown`)
/// 2. most recent state change first, so a freshly blocked agent outranks a long-blocked one
/// 3. terminal id, purely so the order is stable when the first two tie — without this, tiles
///    would shuffle between renders and the deck would be unusable.
fn attention_order(a: &AgentInfo, b: &AgentInfo) -> std::cmp::Ordering {
    a.agent_status
        .attention_rank()
        .cmp(&b.agent_status.attention_rank())
        .then_with(|| {
            b.state_change_seq
                .unwrap_or(0)
                .cmp(&a.state_change_seq.unwrap_or(0))
        })
        .then_with(|| a.terminal_id.cmp(&b.terminal_id))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent(terminal_id: &str, status: AgentStatus, seq: u64) -> AgentInfo {
        AgentInfo {
            terminal_id: terminal_id.into(),
            agent_status: status,
            workspace_id: "w1".into(),
            tab_id: "w1:t1".into(),
            pane_id: format!("w1:{terminal_id}"),
            state_change_seq: Some(seq),
            ..Default::default()
        }
    }

    fn snapshot(agents: Vec<AgentInfo>) -> SessionSnapshot {
        SessionSnapshot {
            protocol: Some(19),
            agents,
            ..Default::default()
        }
    }

    #[test]
    fn blocked_agents_come_before_everything_else() {
        let state = DeckState::from_snapshot(snapshot(vec![
            agent("idle_one", AgentStatus::Idle, 1),
            agent("working_one", AgentStatus::Working, 2),
            agent("blocked_one", AgentStatus::Blocked, 3),
            agent("done_one", AgentStatus::Done, 4),
        ]));
        let order: Vec<_> = state
            .attention_order()
            .iter()
            .map(|a| a.terminal_id.as_str())
            .collect();
        assert_eq!(
            order,
            vec!["blocked_one", "done_one", "working_one", "idle_one"]
        );
    }

    #[test]
    fn within_a_status_band_the_most_recent_change_wins() {
        let state = DeckState::from_snapshot(snapshot(vec![
            agent("old", AgentStatus::Blocked, 10),
            agent("newest", AgentStatus::Blocked, 99),
            agent("middle", AgentStatus::Blocked, 50),
        ]));
        let order: Vec<_> = state
            .attention_order()
            .iter()
            .map(|a| a.terminal_id.as_str())
            .collect();
        assert_eq!(order, vec!["newest", "middle", "old"]);
    }

    #[test]
    fn ordering_is_stable_when_nothing_distinguishes_two_agents() {
        // Without a tiebreak, tiles would swap places between renders and the deck would be
        // impossible to hit reliably.
        let agents = vec![
            agent("zebra", AgentStatus::Idle, 0),
            agent("alpha", AgentStatus::Idle, 0),
        ];
        let first = DeckState::from_snapshot(snapshot(agents.clone()));
        let second = DeckState::from_snapshot(snapshot(agents.into_iter().rev().collect()));
        let ids = |s: &DeckState| -> Vec<String> {
            s.attention_order()
                .iter()
                .map(|a| a.terminal_id.clone())
                .collect()
        };
        assert_eq!(ids(&first), ids(&second));
        assert_eq!(ids(&first), vec!["alpha", "zebra"]);
    }

    #[test]
    fn agents_with_no_state_change_seq_still_sort_deterministically() {
        let mut a = agent("a", AgentStatus::Blocked, 0);
        let mut b = agent("b", AgentStatus::Blocked, 0);
        a.state_change_seq = None;
        b.state_change_seq = None;
        let state = DeckState::from_snapshot(snapshot(vec![b, a]));
        let order: Vec<_> = state
            .attention_order()
            .iter()
            .map(|x| x.terminal_id.as_str())
            .collect();
        assert_eq!(order, vec!["a", "b"]);
    }

    #[test]
    fn only_blocked_and_done_count_as_needing_attention() {
        let state = DeckState::from_snapshot(snapshot(vec![
            agent("b", AgentStatus::Blocked, 1),
            agent("d", AgentStatus::Done, 2),
            agent("w", AgentStatus::Working, 3),
            agent("i", AgentStatus::Idle, 4),
            agent("u", AgentStatus::Unknown, 5),
        ]));
        assert_eq!(state.attention_count(), 2);
        assert_eq!(state.top_attention().unwrap().terminal_id, "b");
    }

    #[test]
    fn top_attention_is_none_when_everything_is_calm() {
        let state = DeckState::from_snapshot(snapshot(vec![
            agent("w", AgentStatus::Working, 1),
            agent("i", AgentStatus::Idle, 2),
        ]));
        assert!(state.top_attention().is_none());
        assert_eq!(state.attention_count(), 0);
    }

    #[test]
    fn status_counts_tally_every_band() {
        let state = DeckState::from_snapshot(snapshot(vec![
            agent("b1", AgentStatus::Blocked, 1),
            agent("b2", AgentStatus::Blocked, 2),
            agent("d", AgentStatus::Done, 3),
            agent("w", AgentStatus::Working, 4),
        ]));
        let counts = state.status_counts();
        assert_eq!(counts.blocked, 2);
        assert_eq!(counts.done, 1);
        assert_eq!(counts.working, 1);
        assert_eq!(counts.total(), 4);
        assert_eq!(counts.needing_attention(), 3);
    }

    #[test]
    fn lookups_by_stable_terminal_id_survive_a_pane_moving_workspace() {
        // pane_id changes on a cross-workspace move; terminal_id does not. A key bound to an
        // agent must keep working after the user reorganises.
        let before =
            DeckState::from_snapshot(snapshot(vec![agent("term_x", AgentStatus::Idle, 1)]));
        assert_eq!(
            before.agent_by_terminal_id("term_x").unwrap().pane_id,
            "w1:term_x"
        );

        let mut moved = agent("term_x", AgentStatus::Idle, 2);
        moved.pane_id = "w7:p4".into();
        moved.workspace_id = "w7".into();
        let after = DeckState::from_snapshot(snapshot(vec![moved]));
        assert!(after.agent_by_terminal_id("term_x").is_some());
    }

    #[test]
    fn an_offline_state_reports_itself_rather_than_showing_stale_agents() {
        let state = DeckState::offline("herdr socket not found");
        assert!(state.is_offline());
        assert!(state.agents.is_empty());
        assert_eq!(state.attention_count(), 0);
    }
}
