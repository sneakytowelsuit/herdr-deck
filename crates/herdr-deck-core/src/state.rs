//! The daemon's view of herdr.
//!
//! Built from `session.snapshot` and refreshed whenever an event says something moved. The one
//! interesting piece of logic here is the **attention order**, which decides what lands on the
//! deck when nothing is pinned.

use std::collections::HashMap;
use std::time::{Duration, Instant};

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
    /// How long each waiting agent has been waiting, stamped on by a [`WaitClock`].
    ///
    /// Empty unless something stamped it, and deliberately not derived from the snapshot: herdr
    /// has no idea. Only the clock may write here, which is why the field is private — a
    /// duration nobody timed would be a lie about the one number this feature exists to report.
    waits: HashMap<String, WaitBucket>,
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
            waits: HashMap::new(),
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

    /// The same order, with acknowledged agents demoted out of the attention band.
    ///
    /// Acknowledgement is per-frontend and the state is shared, so this cannot be folded into the
    /// sort in [`Self::from_snapshot`]: two decks looking at the same herdr may legitimately
    /// disagree about what has been dismissed.
    pub fn attention_order_with<'a>(&'a self, acked: &Acknowledged) -> Vec<&'a AgentInfo> {
        let mut order: Vec<&AgentInfo> = self.agents.iter().collect();
        // `self.agents` is already in attention order, so a *stable* sort on the acknowledged-aware
        // band moves the dismissed agents down without disturbing anything else about the order.
        order.sort_by_key(|agent| acked.effective_rank(agent));
        order
    }

    /// Only the agents that actually want a human: `blocked` and `done`.
    ///
    /// This and the three below are herdr's view, before any frontend has dismissed anything.
    /// Anything driving a deck wants [`crate::layout::ResolvedDeck`] instead, which applies that
    /// frontend's acknowledgements — counting a dismissed agent here would put it back on the
    /// attention key the user just cleared.
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

    /// How long this agent has been asking for you, if anything has been timing it.
    ///
    /// `None` covers three honest cases: nothing stamped this state, the agent is not in an
    /// attention state so there is nothing to escalate, or herdr gave no `state_change_seq` and
    /// the clock had no key to hang a start time on.
    pub fn wait_bucket(&self, agent: &AgentInfo) -> Option<WaitBucket> {
        self.waits.get(&agent.terminal_id).copied()
    }

    pub fn workspace_by_id(&self, workspace_id: &str) -> Option<&WorkspaceInfo> {
        self.workspaces
            .iter()
            .find(|w| w.workspace_id == workspace_id)
    }

    /// The best human name for the project an agent is working on.
    ///
    /// A raw workspace id is honest but useless: when four agents are blocked across four
    /// repositories, `w1`/`w2`/`w3`/`w4` does not tell you which one is asking for you. herdr's
    /// own workspace label is the intended answer; failing that the directory the agent is
    /// sitting in names the project just as well, because that is how people organise work.
    /// The id survives as the last resort so a tile is never left with nothing at all.
    pub fn project_label<'a>(&'a self, agent: &'a AgentInfo) -> Option<&'a str> {
        self.workspace_by_id(&agent.workspace_id)
            .and_then(WorkspaceInfo::explicit_label)
            .or_else(|| agent.cwd_basename())
            .or(Some(agent.workspace_id.as_str()))
            .filter(|label| !label.is_empty())
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

/// Agents the user has dismissed from the attention queue without focusing them.
///
/// # Why this is keyed on the state, not the agent
///
/// An entry is `terminal_id -> state_change_seq`: the agent *and the exact state it was in* when
/// it was acknowledged. herdr bumps that sequence on every state change, so the moment an
/// acknowledged agent blocks again — or finishes, or starts working — the key stops matching and
/// the agent is back in the queue on its own.
///
/// Keying on the id alone would be a false negative machine: an agent dismissed while `done`
/// would stay silently hidden the next time it blocked, which is the worst thing a tool whose
/// entire job is "tell me which agent needs me" can do.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Acknowledged {
    entries: HashMap<String, u64>,
}

impl Acknowledged {
    /// Dismiss `agent` in its current state, reporting whether the acknowledgement took.
    ///
    /// It is refused when herdr gave us no `state_change_seq`, because nothing would then ever
    /// expire it — the map cannot hold an acknowledgement that outlives its state, by
    /// construction. Callers are expected to say so out loud rather than pretend it worked.
    pub fn acknowledge(&mut self, agent: &AgentInfo) -> bool {
        let Some(seq) = agent.state_change_seq else {
            return false;
        };
        // One entry per agent: acknowledging it again in a newer state replaces the old key,
        // which is the only sensible reading — you cannot have dismissed two states at once.
        self.entries.insert(agent.terminal_id.clone(), seq);
        true
    }

    /// Is this agent, in the state it is in *now*, acknowledged?
    pub fn contains(&self, agent: &AgentInfo) -> bool {
        match (self.entries.get(&agent.terminal_id), agent.state_change_seq) {
            (Some(acknowledged), Some(current)) => *acknowledged == current,
            _ => false,
        }
    }

    /// Does this agent still want a human?
    pub fn wants_attention(&self, agent: &AgentInfo) -> bool {
        agent.agent_status.needs_attention() && !self.contains(agent)
    }

    /// Where an agent sits in the attention order once acknowledgement is applied.
    ///
    /// A dismissed agent ranks with the calm ones, but only its *rank* moves: its tile still
    /// draws the status herdr reports, so the deck never claims an agent stopped being blocked
    /// just because you looked away from it.
    fn effective_rank(&self, agent: &AgentInfo) -> u8 {
        if self.contains(agent) && agent.agent_status.needs_attention() {
            AgentStatus::Idle.attention_rank()
        } else {
            agent.agent_status.attention_rank()
        }
    }

    /// Drop acknowledgements that can no longer match: the agent moved on, or vanished.
    ///
    /// Not required for correctness — [`Self::contains`] already answers `false` for both — but
    /// without it a long-lived connection accumulates an entry per agent it ever dismissed.
    pub fn forget_stale(&mut self, state: &DeckState) {
        self.entries.retain(|terminal_id, seq| {
            state
                .agent_by_terminal_id(terminal_id)
                .and_then(|agent| agent.state_change_seq)
                == Some(*seq)
        });
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// How long an agent has been waiting, at the only granularity worth painting.
///
/// # Why buckets and not a seconds counter
///
/// The daemon hashes each tile and resends only the keys whose hash moved; that diff is the
/// only reason the reconcile timer does not repaint the whole deck every two seconds forever.
/// A live counter would change the hash on every tick and defeat it completely. Three buckets
/// change at most twice per incident — two extra repaints per agent, and that is the entire
/// cost of knowing how long something has been red.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum WaitBucket {
    /// Under a minute. You may not have looked away yet.
    Fresh,
    /// One to five minutes.
    Waiting,
    /// Over five minutes: this is the agent costing you time.
    Overdue,
}

/// When [`WaitBucket::Fresh`] becomes [`WaitBucket::Waiting`], and that becomes
/// [`WaitBucket::Overdue`].
///
/// A minute is roughly how long it takes to notice something on your own; five is well past the
/// point where an agent has been waiting longer than the work it was asked to do.
const WAITING_AFTER: Duration = Duration::from_secs(60);
const OVERDUE_AFTER: Duration = Duration::from_secs(5 * 60);

impl WaitBucket {
    pub fn for_duration(waited: Duration) -> Self {
        if waited >= OVERDUE_AFTER {
            WaitBucket::Overdue
        } else if waited >= WAITING_AFTER {
            WaitBucket::Waiting
        } else {
            WaitBucket::Fresh
        }
    }

    /// The marker a tile draws. Three characters, because that is what fits opposite the status
    /// glyph on a 72px key without stealing room from the label.
    pub fn marker(self) -> &'static str {
        match self {
            WaitBucket::Fresh => "<1m",
            WaitBucket::Waiting => "1m+",
            WaitBucket::Overdue => "5m+",
        }
    }

    /// How far up the escalation this is, 0 to 2, for the renderer to scale its treatment by.
    pub fn level(self) -> u8 {
        match self {
            WaitBucket::Fresh => 0,
            WaitBucket::Waiting => 1,
            WaitBucket::Overdue => 2,
        }
    }
}

/// When each waiting agent started waiting.
///
/// # Why the deck has to own a clock at all
///
/// herdr records no timestamps: `state_change_seq` is a monotonic counter, not a time. So a key
/// that has been red for eight minutes is, as far as herdr is concerned, indistinguishable from
/// one that turned red a moment ago. The only way to tell them apart is to have been watching,
/// and this is the thing that was watching.
///
/// # Why it cannot live in [`DeckState`]
///
/// That struct is rebuilt wholesale from every snapshot — see [`DeckState::from_snapshot`] and
/// the reason it replaces rather than merges. A start time stored there would be discarded and
/// re-taken every reconcile, and would therefore always read zero. This table sits beside the
/// watcher instead and outlives every state it stamps.
///
/// # Why it is keyed on the state, not the agent
///
/// The same reason [`Acknowledged`] is: an entry is `terminal_id -> (state_change_seq, when)`.
/// herdr bumps that sequence on each state change, so an agent that unblocks and blocks again
/// is asking a *new* question, and the clock for it starts when that question was asked rather
/// than carrying over a wait the user already dealt with.
#[derive(Debug, Default)]
pub struct WaitClock {
    started: HashMap<String, (u64, Instant)>,
}

impl WaitClock {
    /// Note every agent now waiting, and stamp what it has waited so far onto `state`.
    ///
    /// Call this on each fresh snapshot, before publishing it. An offline state is deliberately
    /// *not* stamped: herdr briefly going away is not the agents calming down, and wiping the
    /// table there would reset every wait on a socket blip.
    pub fn stamp(&mut self, state: &mut DeckState, now: Instant) {
        let mut buckets = HashMap::new();
        for agent in &state.agents {
            if !agent.agent_status.needs_attention() {
                continue;
            }
            // No sequence, no clock. Keying on the terminal id alone would let a wait survive
            // the state it was measuring, so an agent that blocked, was dealt with, and blocked
            // again would be reported as having waited the whole time — overstating the one
            // number this exists to report. Saying nothing is the honest answer.
            let Some(seq) = agent.state_change_seq else {
                continue;
            };
            let since = match self.started.get(&agent.terminal_id) {
                Some((known, since)) if *known == seq => *since,
                _ => {
                    self.started.insert(agent.terminal_id.clone(), (seq, now));
                    now
                }
            };
            buckets.insert(
                agent.terminal_id.clone(),
                WaitBucket::for_duration(now.saturating_duration_since(since)),
            );
        }
        // Anything not bucketed just now stopped asking, moved to a new state, or vanished; its
        // start time can never be read again, so drop it rather than let a long-running daemon
        // hoard one entry per agent it ever saw block.
        self.started.retain(|id, _| buckets.contains_key(id));
        state.waits = buckets;
    }

    pub fn len(&self) -> usize {
        self.started.len()
    }

    pub fn is_empty(&self) -> bool {
        self.started.is_empty()
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

    // --- Acknowledgement -----------------------------------------------------------------------
    //
    // The rule these pin is the one the whole feature stands on: an acknowledgement is about a
    // *state*, not an agent, so it can never outlive the state it dismissed.

    #[test]
    fn an_acknowledged_agent_stops_wanting_a_human_but_keeps_its_reported_status() {
        let blocked = agent("stuck", AgentStatus::Blocked, 7);
        let mut acked = Acknowledged::default();
        assert!(acked.acknowledge(&blocked));
        assert!(acked.contains(&blocked));
        assert!(!acked.wants_attention(&blocked));
        assert_eq!(
            blocked.agent_status,
            AgentStatus::Blocked,
            "acknowledging must not rewrite what herdr says the agent is doing"
        );
    }

    #[test]
    fn an_acknowledgement_expires_the_moment_the_agent_changes_state() {
        // The whole safety property. If this ever passes for the wrong reason, an agent that
        // blocks again after being dismissed stays hidden and nobody ever knows.
        let done = agent("worker", AgentStatus::Done, 7);
        let mut acked = Acknowledged::default();
        acked.acknowledge(&done);

        let blocked_again = agent("worker", AgentStatus::Blocked, 8);
        assert!(!acked.contains(&blocked_again));
        assert!(acked.wants_attention(&blocked_again));
    }

    #[test]
    fn an_agent_with_no_state_sequence_cannot_be_acknowledged_at_all() {
        // Nothing would ever expire such an acknowledgement, so refusing it loudly is the only
        // safe answer — a permanently muted agent is worse than a gesture that did not take.
        let mut sequenceless = agent("mystery", AgentStatus::Blocked, 0);
        sequenceless.state_change_seq = None;
        let mut acked = Acknowledged::default();
        assert!(!acked.acknowledge(&sequenceless));
        assert!(!acked.contains(&sequenceless));
        assert!(acked.is_empty());
    }

    #[test]
    fn acknowledging_demotes_an_agent_out_of_the_attention_band_without_removing_it() {
        let state = DeckState::from_snapshot(snapshot(vec![
            agent("dismissed", AgentStatus::Blocked, 9),
            agent("still_blocked", AgentStatus::Blocked, 8),
            agent("busy", AgentStatus::Working, 7),
        ]));
        let mut acked = Acknowledged::default();
        acked.acknowledge(state.agent_by_terminal_id("dismissed").unwrap());

        let order: Vec<_> = state
            .attention_order_with(&acked)
            .iter()
            .map(|a| a.terminal_id.as_str())
            .collect();
        assert_eq!(
            order,
            vec!["still_blocked", "busy", "dismissed"],
            "a dismissed agent ranks with the calm ones, but it is still on the deck"
        );
    }

    #[test]
    fn acknowledging_one_agent_leaves_every_other_agents_place_alone() {
        let state = DeckState::from_snapshot(snapshot(vec![
            agent("first", AgentStatus::Blocked, 9),
            agent("second", AgentStatus::Blocked, 8),
            agent("third", AgentStatus::Done, 7),
        ]));
        let mut acked = Acknowledged::default();
        acked.acknowledge(state.agent_by_terminal_id("second").unwrap());

        let order: Vec<_> = state
            .attention_order_with(&acked)
            .iter()
            .map(|a| a.terminal_id.as_str())
            .collect();
        assert_eq!(order, vec!["first", "third", "second"]);
    }

    #[test]
    fn acknowledgements_for_agents_that_moved_on_are_forgotten_rather_than_accumulating() {
        let before =
            DeckState::from_snapshot(snapshot(vec![agent("worker", AgentStatus::Done, 7)]));
        let mut acked = Acknowledged::default();
        acked.acknowledge(before.agent_by_terminal_id("worker").unwrap());
        acked.forget_stale(&before);
        assert_eq!(acked.len(), 1, "a live, unchanged agent keeps its entry");

        let after =
            DeckState::from_snapshot(snapshot(vec![agent("worker", AgentStatus::Blocked, 8)]));
        acked.forget_stale(&after);
        assert!(acked.is_empty());
    }

    // --- How long an agent has been waiting ----------------------------------------------------
    //
    // herdr has no clock, so every one of these is about the deck's own. The two that matter most
    // are the one where the clock outlives the state it stamped — without that the whole feature
    // silently reports zero forever — and the one where a state change restarts it, without which
    // the deck would tell you an agent had been waiting ten minutes for a question it asked ten
    // seconds ago.

    /// The bucket the clock gives our single agent, `waited` after it first saw it.
    fn bucket_after(agents: Vec<AgentInfo>, waited: Duration) -> Option<WaitBucket> {
        let start = Instant::now();
        let mut clock = WaitClock::default();
        let mut first = DeckState::from_snapshot(snapshot(agents.clone()));
        clock.stamp(&mut first, start);
        // A *separate* state, exactly as the watcher builds one from every fresh snapshot. If
        // the clock lived in `DeckState` this second stamp would restart it and every assertion
        // below would read `Fresh`.
        let mut later = DeckState::from_snapshot(snapshot(agents));
        clock.stamp(&mut later, start + waited);
        later.wait_bucket(later.agents.first()?)
    }

    #[test]
    fn a_wait_is_fresh_until_the_moment_it_has_lasted_a_whole_minute() {
        let blocked = vec![agent("stuck", AgentStatus::Blocked, 1)];
        assert_eq!(
            bucket_after(blocked.clone(), Duration::from_secs(59)),
            Some(WaitBucket::Fresh)
        );
        assert_eq!(
            bucket_after(blocked, Duration::from_secs(60)),
            Some(WaitBucket::Waiting),
            "the boundary belongs to the louder bucket: rounding down would let an agent sit \
             one tick under a minute forever and never escalate"
        );
    }

    #[test]
    fn a_wait_becomes_the_longest_bucket_the_moment_it_has_lasted_five_minutes() {
        let blocked = vec![agent("stuck", AgentStatus::Blocked, 1)];
        assert_eq!(
            bucket_after(blocked.clone(), Duration::from_secs(299)),
            Some(WaitBucket::Waiting)
        );
        assert_eq!(
            bucket_after(blocked, Duration::from_secs(300)),
            Some(WaitBucket::Overdue)
        );
    }

    #[test]
    fn a_wait_survives_the_snapshot_it_was_measured_from_being_thrown_away() {
        // The reason the clock is a side table at all. `DeckState` is rebuilt wholesale every
        // reconcile — twice a second at worst — so a start time kept inside one would be
        // discarded and re-taken before it ever measured anything, and every agent would read
        // as freshly blocked forever.
        let start = Instant::now();
        let mut clock = WaitClock::default();
        let blocked = vec![agent("stuck", AgentStatus::Blocked, 1)];

        // Ten reconciles over six minutes, each throwing the previous state away.
        let mut latest = DeckState::default();
        for tick in 0..10 {
            latest = DeckState::from_snapshot(snapshot(blocked.clone()));
            clock.stamp(&mut latest, start + Duration::from_secs(tick * 40));
        }
        assert_eq!(
            latest.wait_bucket(&latest.agents[0]),
            Some(WaitBucket::Overdue)
        );
    }

    #[test]
    fn the_clock_starts_again_when_herdr_says_the_agent_entered_a_new_state() {
        // An agent that blocked, was dealt with, and blocked again is asking a *new* question.
        // Carrying the old wait over would report six minutes of waiting for something asked a
        // moment ago — and the number would be at its loudest exactly when it is most wrong.
        let start = Instant::now();
        let mut clock = WaitClock::default();

        let mut long_blocked =
            DeckState::from_snapshot(snapshot(vec![agent("stuck", AgentStatus::Blocked, 1)]));
        clock.stamp(&mut long_blocked, start);
        let mut still_blocked =
            DeckState::from_snapshot(snapshot(vec![agent("stuck", AgentStatus::Blocked, 1)]));
        clock.stamp(&mut still_blocked, start + Duration::from_secs(360));
        assert_eq!(
            still_blocked.wait_bucket(&still_blocked.agents[0]),
            Some(WaitBucket::Overdue)
        );

        // herdr bumps the sequence: same agent, new question.
        let mut asked_again =
            DeckState::from_snapshot(snapshot(vec![agent("stuck", AgentStatus::Blocked, 2)]));
        clock.stamp(&mut asked_again, start + Duration::from_secs(361));
        assert_eq!(
            asked_again.wait_bucket(&asked_again.agents[0]),
            Some(WaitBucket::Fresh)
        );
    }

    #[test]
    fn an_agent_that_is_only_working_has_no_wait_because_it_is_not_waiting_for_anyone() {
        for calm in [
            AgentStatus::Working,
            AgentStatus::Idle,
            AgentStatus::Unknown,
        ] {
            assert_eq!(
                bucket_after(vec![agent("busy", calm, 1)], Duration::from_secs(600)),
                None,
                "{calm} is not an attention state and has nothing to escalate"
            );
        }
    }

    #[test]
    fn an_agent_herdr_gave_no_state_sequence_for_gets_no_bucket_rather_than_an_invented_one() {
        // With no sequence there is no way to tell "still waiting on the same question" from
        // "waiting again on a new one", so any duration we reported could be arbitrarily too
        // large. Saying nothing is the only honest answer available.
        let mut sequenceless = agent("mystery", AgentStatus::Blocked, 0);
        sequenceless.state_change_seq = None;
        assert_eq!(
            bucket_after(vec![sequenceless], Duration::from_secs(600)),
            None
        );
    }

    #[test]
    fn a_wait_that_ended_is_forgotten_rather_than_accumulating_for_the_life_of_the_daemon() {
        let start = Instant::now();
        let mut clock = WaitClock::default();
        let mut blocked =
            DeckState::from_snapshot(snapshot(vec![agent("stuck", AgentStatus::Blocked, 1)]));
        clock.stamp(&mut blocked, start);
        assert_eq!(clock.len(), 1);

        let mut answered =
            DeckState::from_snapshot(snapshot(vec![agent("stuck", AgentStatus::Working, 2)]));
        clock.stamp(&mut answered, start + Duration::from_secs(1));
        assert!(
            clock.is_empty(),
            "an agent that stopped asking stops being timed"
        );
    }

    #[test]
    fn one_agents_wait_is_independent_of_every_other_agents() {
        let start = Instant::now();
        let mut clock = WaitClock::default();
        let mut first =
            DeckState::from_snapshot(snapshot(vec![agent("early", AgentStatus::Blocked, 1)]));
        clock.stamp(&mut first, start);

        // A second agent blocks six minutes later. The first is overdue; the second has only
        // just asked, and must not inherit the queue's age.
        let mut both = DeckState::from_snapshot(snapshot(vec![
            agent("early", AgentStatus::Blocked, 1),
            agent("late", AgentStatus::Blocked, 2),
        ]));
        clock.stamp(&mut both, start + Duration::from_secs(360));
        let bucket = |id: &str| both.wait_bucket(both.agent_by_terminal_id(id).unwrap());
        assert_eq!(bucket("early"), Some(WaitBucket::Overdue));
        assert_eq!(bucket("late"), Some(WaitBucket::Fresh));
    }

    #[test]
    fn every_bucket_has_its_own_marker_so_the_three_rungs_are_never_confusable() {
        let markers = [
            WaitBucket::Fresh.marker(),
            WaitBucket::Waiting.marker(),
            WaitBucket::Overdue.marker(),
        ];
        let mut unique = markers.to_vec();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), markers.len());
        for marker in markers {
            assert!(
                marker.chars().count() <= 3,
                "{marker:?} will not fit beside a status glyph on a 72px key"
            );
        }
    }

    #[test]
    fn an_offline_state_reports_itself_rather_than_showing_stale_agents() {
        let state = DeckState::offline("herdr socket not found");
        assert!(state.is_offline());
        assert!(state.agents.is_empty());
        assert_eq!(state.attention_count(), 0);
    }
}
