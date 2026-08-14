//! herdr wire types.
//!
//! These mirror herdr's API schema (`herdr api schema --json`) for socket protocol 19. They are
//! deliberately **tolerant**: every field herdr documents as nullable is an `Option`, unknown
//! fields are ignored, and unknown enum variants fall back rather than failing the whole
//! response. A status display that renders slightly stale data is far better than one that
//! refuses to start because herdr grew a field.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// An agent's state, as classified by herdr.
///
/// Two of these are "attention" states and drive everything herdr-deck does:
/// - [`AgentStatus::Blocked`] — the agent needs input, approval, or a decision **right now**.
/// - [`AgentStatus::Done`] — the agent finished and you have not looked at it yet.
///
/// `Done` is derived by herdr from "idle and unseen"; focusing the pane marks it seen and it
/// becomes `Idle`. Plugins cannot report `Done` themselves.
///
/// Caveat worth knowing: herdr only marks `Blocked` when a live screen snapshot matches a known
/// approval/question/permission UI. An unrecognised prompt shape falls back to `Idle`. The deck
/// can only ever be as accurate as herdr's detection; `herdr agent explain <target>` diagnoses it.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum AgentStatus {
    Idle,
    Working,
    Blocked,
    Done,
    /// herdr could not confidently classify the state. Also the landing spot for any status
    /// string a future herdr adds that we do not know about yet — and therefore the safest
    /// default.
    #[default]
    #[serde(other)]
    Unknown,
}

impl AgentStatus {
    /// Does this state want a human?
    pub fn needs_attention(self) -> bool {
        matches!(self, AgentStatus::Blocked | AgentStatus::Done)
    }

    /// Ranking for the attention queue: lower sorts first.
    pub fn attention_rank(self) -> u8 {
        match self {
            AgentStatus::Blocked => 0,
            AgentStatus::Done => 1,
            AgentStatus::Working => 2,
            AgentStatus::Idle => 3,
            AgentStatus::Unknown => 4,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            AgentStatus::Idle => "idle",
            AgentStatus::Working => "working",
            AgentStatus::Blocked => "blocked",
            AgentStatus::Done => "done",
            AgentStatus::Unknown => "unknown",
        }
    }
}

impl std::fmt::Display for AgentStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A recognised agent process inside a pane.
///
/// In herdr's model an agent is a *property of a pane*, not an object with its own lifetime.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentInfo {
    /// Stable across cross-workspace pane moves — **bind deck slots to this**, not `pane_id`.
    pub terminal_id: String,
    #[serde(default)]
    pub agent_status: AgentStatus,
    #[serde(default)]
    pub workspace_id: String,
    #[serde(default)]
    pub tab_id: String,
    /// Workspace-qualified, e.g. `w1:p1`. Changes when a pane moves between workspaces.
    #[serde(default)]
    pub pane_id: String,
    #[serde(default)]
    pub focused: bool,
    #[serde(default)]
    pub revision: u64,

    /// The agent kind, e.g. `claude`, `codex`, `cursor`.
    #[serde(default)]
    pub agent: Option<String>,
    /// A user-assigned agent name (`agent start --name`), unique among live agents.
    #[serde(default)]
    pub name: Option<String>,
    /// A richer display string herdr composes, e.g. `Claude: auth`.
    #[serde(default)]
    pub display_agent: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub state_labels: Option<BTreeMap<String, String>>,
    /// Monotonic counter bumped on each state change — used to sort by recency.
    #[serde(default)]
    pub state_change_seq: Option<u64>,
}

impl AgentInfo {
    /// The best short human label for this agent, in decreasing order of specificity.
    pub fn label(&self) -> &str {
        self.name
            .as_deref()
            .or(self.display_agent.as_deref())
            .or(self.agent.as_deref())
            .filter(|s| !s.is_empty())
            .unwrap_or(&self.terminal_id)
    }

    /// The last component of the agent's working directory, which is nearly always the project.
    ///
    /// The leading components are shared by every agent on the machine (`~/src/…`), so they cost
    /// horizontal room without distinguishing anything — and a key is only 72px wide at worst.
    pub fn cwd_basename(&self) -> Option<&str> {
        // Trailing slashes turn up in anything a human typed or a shell completed; splitting
        // without stripping them first hands back an empty name and loses one we actually have.
        self.cwd
            .as_deref()?
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .filter(|s| !s.is_empty())
    }

    /// A per-state descriptive label if herdr supplied one for the current state.
    pub fn state_label(&self) -> Option<&str> {
        self.state_labels
            .as_ref()?
            .get(self.agent_status.as_str())
            .map(String::as_str)
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WorkspaceInfo {
    pub workspace_id: String,
    #[serde(default)]
    pub number: Option<u32>,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub focused: bool,
    #[serde(default)]
    pub pane_count: Option<u32>,
    #[serde(default)]
    pub tab_count: Option<u32>,
    #[serde(default)]
    pub active_tab_id: Option<String>,
    /// Rolls up from the agents inside: a blocked agent makes its workspace look blocked.
    #[serde(default)]
    pub agent_status: AgentStatus,
}

impl WorkspaceInfo {
    pub fn label(&self) -> &str {
        self.explicit_label().unwrap_or(&self.workspace_id)
    }

    /// The name herdr actually knows for this workspace, or `None` when it only knows the id.
    ///
    /// [`Self::label`] papers over that difference because a tile must draw *something*; a
    /// caller with a better fallback of its own needs to be able to tell the two apart.
    pub fn explicit_label(&self) -> Option<&str> {
        self.label.as_deref().filter(|s| !s.is_empty())
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TabInfo {
    pub tab_id: String,
    #[serde(default)]
    pub workspace_id: String,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub focused: bool,
    #[serde(default)]
    pub agent_status: AgentStatus,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PaneInfo {
    pub pane_id: String,
    #[serde(default)]
    pub terminal_id: Option<String>,
    #[serde(default)]
    pub workspace_id: String,
    #[serde(default)]
    pub tab_id: String,
    #[serde(default)]
    pub focused: bool,
    #[serde(default)]
    pub label: Option<String>,
}

/// The bootstrap snapshot from `session.snapshot`.
///
/// herdr's documented pattern is: read this once, then subscribe to events and keep a local
/// cache. We additionally re-read it on a slow timer and after every reconnect — see
/// [`crate::client`] for why.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionSnapshot {
    #[serde(default)]
    pub version: Option<String>,
    /// The socket protocol version. Compare against [`crate::EXPECTED_PROTOCOL`].
    #[serde(default)]
    pub protocol: Option<u32>,
    #[serde(default)]
    pub workspaces: Vec<WorkspaceInfo>,
    #[serde(default)]
    pub tabs: Vec<TabInfo>,
    #[serde(default)]
    pub panes: Vec<PaneInfo>,
    #[serde(default)]
    pub agents: Vec<AgentInfo>,
    #[serde(default)]
    pub focused_workspace_id: Option<String>,
    #[serde(default)]
    pub focused_tab_id: Option<String>,
    #[serde(default)]
    pub focused_pane_id: Option<String>,
}

// ----- panes ---------------------------------------------------------------------------------
//
// The four types below are herdr's parameter vocabularies, not the deck's. They live here for
// the same reason [`AgentStatus`] does: callers above this crate need to *name* a direction, and
// naming it as a Rust variant rather than a string is what keeps the JSON spelling — and the fact
// that herdr calls it `no_neighbor` and not `no_neighbour` — inside this crate.

/// Which way to move, in the layout of the tab herdr is currently on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PaneDirection {
    Left,
    Right,
    Up,
    Down,
}

impl PaneDirection {
    pub const ALL: [PaneDirection; 4] = [
        PaneDirection::Left,
        PaneDirection::Right,
        PaneDirection::Up,
        PaneDirection::Down,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            PaneDirection::Left => "left",
            PaneDirection::Right => "right",
            PaneDirection::Up => "up",
            PaneDirection::Down => "down",
        }
    }
}

/// Where a new pane goes.
///
/// Two variants and not four: herdr splits right and down only, so there is no "split left" for a
/// key to offer and no point pretending otherwise.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SplitDirection {
    Right,
    Down,
}

impl SplitDirection {
    pub const ALL: [SplitDirection; 2] = [SplitDirection::Right, SplitDirection::Down];

    pub fn as_str(self) -> &'static str {
        match self {
            SplitDirection::Right => "right",
            SplitDirection::Down => "down",
        }
    }
}

/// Zoom, stated rather than toggled.
///
/// herdr also accepts `toggle`, and this enum deliberately cannot express it. A toggle asks herdr
/// to flip a boolean the deck cannot see, so the same key press means different things depending
/// on state that may have moved since the deck last looked. Stating the wanted end state makes the
/// key idempotent and self-correcting: press it twice and you are still zoomed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ZoomMode {
    On,
    Off,
}

impl ZoomMode {
    pub const ALL: [ZoomMode; 2] = [ZoomMode::On, ZoomMode::Off];

    pub fn as_str(self) -> &'static str {
        match self {
            ZoomMode::On => "on",
            ZoomMode::Off => "off",
        }
    }
}

/// Why herdr changed nothing, having understood the request perfectly well.
///
/// herdr answers these as *successes* carrying a reason, not as errors, and that distinction is
/// the whole reason this type exists: pressing "left" at the left-hand edge of a layout is not a
/// failure, and a key that flashed an alert for it would teach its owner to stop reading alerts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NothingToDo {
    /// The edge of the layout: there is no pane that way.
    NoNeighbour,
    /// The tab holds one pane, so there is nothing to zoom into or out of.
    OnlyPane,
    /// It was already the way it was asked to be.
    AlreadySo,
    /// herdr gave a reason this version does not recognise — treated as "nothing happened",
    /// because that is the one thing `changed: false` definitely tells us.
    Unrecognised,
}

impl NothingToDo {
    fn from_reason(reason: Option<&str>) -> Self {
        match reason {
            Some("no_neighbor") => NothingToDo::NoNeighbour,
            Some("single_pane") => NothingToDo::OnlyPane,
            Some("already_zoomed") | Some("already_unzoomed") => NothingToDo::AlreadySo,
            _ => NothingToDo::Unrecognised,
        }
    }

    /// A short line for a log or a key's alert text.
    pub fn describe(self) -> &'static str {
        match self {
            NothingToDo::NoNeighbour => "there is no pane that way",
            NothingToDo::OnlyPane => "this tab has only the one pane",
            NothingToDo::AlreadySo => "it was already like that",
            NothingToDo::Unrecognised => "herdr changed nothing and did not say why",
        }
    }
}

/// What came of asking herdr to move or reshape a pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneOutcome {
    /// herdr did it.
    Changed,
    /// herdr understood, and there was nothing to do.
    Unchanged(NothingToDo),
}

/// The body herdr returns from a pane command: did anything move, and if not, why not.
///
/// Both `pane.focus_direction` and `pane.zoom` answer in this shape, nested one level down under
/// a name of their own. The extra fields they carry — the layout snapshot, the source pane — are
/// deliberately not modelled: see [`crate::client::HerdrClient::pane_focus_direction`].
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PaneChange {
    #[serde(default)]
    pub changed: bool,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub focused_pane_id: Option<String>,
}

impl PaneChange {
    pub fn outcome(&self) -> PaneOutcome {
        if self.changed {
            PaneOutcome::Changed
        } else {
            PaneOutcome::Unchanged(NothingToDo::from_reason(self.reason.as_deref()))
        }
    }
}

/// `pane.focus_direction`'s envelope.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PaneFocusDirectionResult {
    #[serde(default)]
    pub focus: Option<PaneChange>,
}

/// `pane.zoom`'s envelope.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PaneZoomResult {
    #[serde(default)]
    pub zoom: Option<PaneChange>,
}

/// A request line on the herdr socket.
#[derive(Debug, Clone, Serialize)]
pub struct Request<'a> {
    pub id: &'a str,
    pub method: &'a str,
    pub params: serde_json::Value,
}

/// A response line: exactly one of `result` or `error` is present.
#[derive(Debug, Clone, Deserialize)]
pub struct Response {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub result: Option<serde_json::Value>,
    #[serde(default)]
    pub error: Option<ResponseError>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ResponseError {
    #[serde(default)]
    pub code: String,
    #[serde(default)]
    pub message: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_documented_status_enum() {
        for (raw, want) in [
            ("\"idle\"", AgentStatus::Idle),
            ("\"working\"", AgentStatus::Working),
            ("\"blocked\"", AgentStatus::Blocked),
            ("\"done\"", AgentStatus::Done),
            ("\"unknown\"", AgentStatus::Unknown),
        ] {
            let got: AgentStatus = serde_json::from_str(raw).unwrap();
            assert_eq!(got, want, "parsing {raw}");
        }
    }

    #[test]
    fn an_unrecognised_status_degrades_to_unknown_instead_of_failing() {
        // A future herdr adding a state must not take the whole deck down.
        let got: AgentStatus = serde_json::from_str("\"hibernating\"").unwrap();
        assert_eq!(got, AgentStatus::Unknown);
    }

    #[test]
    fn attention_ranking_puts_blocked_first_and_done_second() {
        let mut all = vec![
            AgentStatus::Idle,
            AgentStatus::Unknown,
            AgentStatus::Blocked,
            AgentStatus::Working,
            AgentStatus::Done,
        ];
        all.sort_by_key(|s| s.attention_rank());
        assert_eq!(
            all,
            vec![
                AgentStatus::Blocked,
                AgentStatus::Done,
                AgentStatus::Working,
                AgentStatus::Idle,
                AgentStatus::Unknown,
            ]
        );
    }

    #[test]
    fn only_blocked_and_done_want_a_human() {
        assert!(AgentStatus::Blocked.needs_attention());
        assert!(AgentStatus::Done.needs_attention());
        assert!(!AgentStatus::Working.needs_attention());
        assert!(!AgentStatus::Idle.needs_attention());
        assert!(!AgentStatus::Unknown.needs_attention());
    }

    #[test]
    fn agent_info_tolerates_a_response_with_only_required_fields() {
        let raw = r#"{
            "terminal_id": "term_abc123",
            "agent_status": "blocked",
            "workspace_id": "w1",
            "tab_id": "w1:t1",
            "pane_id": "w1:p1",
            "focused": false,
            "revision": 7
        }"#;
        let a: AgentInfo = serde_json::from_str(raw).unwrap();
        assert_eq!(a.agent_status, AgentStatus::Blocked);
        assert_eq!(a.terminal_id, "term_abc123");
        // With no name/display_agent/agent, the label falls back to the stable id.
        assert_eq!(a.label(), "term_abc123");
    }

    #[test]
    fn agent_info_ignores_fields_we_do_not_model() {
        let raw = r#"{
            "terminal_id": "term_1", "agent_status": "working",
            "workspace_id": "w1", "tab_id": "w1:t1", "pane_id": "w1:p1",
            "focused": true, "revision": 1,
            "some_future_field": {"nested": [1,2,3]},
            "interactive_ready": true
        }"#;
        let a: AgentInfo = serde_json::from_str(raw).unwrap();
        assert_eq!(a.agent_status, AgentStatus::Working);
    }

    #[test]
    fn label_prefers_name_then_display_agent_then_kind() {
        let mut a = AgentInfo {
            terminal_id: "term_1".into(),
            ..Default::default()
        };
        a.agent = Some("claude".into());
        assert_eq!(a.label(), "claude");
        a.display_agent = Some("Claude: auth".into());
        assert_eq!(a.label(), "Claude: auth");
        a.name = Some("refactor".into());
        assert_eq!(a.label(), "refactor");
    }

    #[test]
    fn state_label_is_looked_up_by_current_status() {
        let mut labels = BTreeMap::new();
        labels.insert("working".to_string(), "refactoring auth".to_string());
        let a = AgentInfo {
            agent_status: AgentStatus::Working,
            state_labels: Some(labels),
            ..Default::default()
        };
        assert_eq!(a.state_label(), Some("refactoring auth"));

        let b = AgentInfo {
            agent_status: AgentStatus::Blocked,
            state_labels: a.state_labels.clone(),
            ..Default::default()
        };
        assert_eq!(b.state_label(), None);
    }

    #[test]
    fn snapshot_parses_a_realistic_payload() {
        let raw = r#"{
            "type": "session_snapshot",
            "version": "0.8.0",
            "protocol": 19,
            "workspaces": [
              {"workspace_id":"w1","number":1,"label":"api","focused":true,
               "pane_count":2,"tab_count":1,"active_tab_id":"w1:t1","agent_status":"blocked"}
            ],
            "tabs": [{"tab_id":"w1:t1","workspace_id":"w1","focused":true,"agent_status":"blocked"}],
            "panes": [{"pane_id":"w1:p1","terminal_id":"term_a","workspace_id":"w1","tab_id":"w1:t1","focused":true}],
            "agents": [
              {"terminal_id":"term_a","agent_status":"blocked","workspace_id":"w1",
               "tab_id":"w1:t1","pane_id":"w1:p1","focused":true,"revision":3,
               "agent":"claude","state_change_seq":42}
            ],
            "focused_workspace_id": "w1",
            "focused_tab_id": "w1:t1",
            "focused_pane_id": "w1:p1"
        }"#;
        let s: SessionSnapshot = serde_json::from_str(raw).unwrap();
        assert_eq!(s.protocol, Some(19));
        assert_eq!(s.agents.len(), 1);
        assert_eq!(s.agents[0].state_change_seq, Some(42));
        assert_eq!(s.workspaces[0].label(), "api");
    }

    #[test]
    fn cwd_basename_survives_trailing_slashes_and_refuses_to_invent_a_name() {
        let basename = |cwd: Option<&str>| {
            AgentInfo {
                cwd: cwd.map(str::to_string),
                ..Default::default()
            }
            .cwd_basename()
            .map(str::to_string)
        };
        assert_eq!(basename(Some("/home/dev/src/api")).as_deref(), Some("api"));
        assert_eq!(basename(Some("/home/dev/src/api/")).as_deref(), Some("api"));
        assert_eq!(basename(Some("api")).as_deref(), Some("api"));
        // Nothing here is a project name, so callers must be free to fall back.
        assert_eq!(basename(Some("/")), None);
        assert_eq!(basename(Some("")), None);
        assert_eq!(basename(None), None);
    }

    #[test]
    fn an_unlabelled_workspace_admits_it_rather_than_passing_its_id_off_as_a_name() {
        let w = WorkspaceInfo {
            workspace_id: "w3".into(),
            label: Some(String::new()),
            ..Default::default()
        };
        assert_eq!(w.explicit_label(), None);
        assert_eq!(w.label(), "w3");
    }

    // --- Pane commands -----------------------------------------------------------------------

    #[test]
    fn a_pane_command_that_moved_something_reads_as_changed() {
        let raw = r#"{"type":"pane_focus_direction","focus":{"changed":true,
            "source_pane_id":"w1:p1","focused_pane_id":"w1:p2",
            "layout":{"zoomed":false,"panes":[]}}}"#;
        let result: PaneFocusDirectionResult = serde_json::from_str(raw).unwrap();
        let focus = result.focus.expect("herdr reported the move");
        assert_eq!(focus.outcome(), PaneOutcome::Changed);
        assert_eq!(focus.focused_pane_id.as_deref(), Some("w1:p2"));
    }

    #[test]
    fn reaching_the_edge_of_a_layout_is_a_success_that_says_nothing_moved() {
        // herdr answers this as a result, not an error, and everything above depends on that
        // staying true: a key that treated the edge as a failure would flash an alert every time
        // someone ran their thumb along the arrows.
        let raw = r#"{"type":"pane_focus_direction","focus":{"changed":false,
            "reason":"no_neighbor","source_pane_id":"w1:p1"}}"#;
        let result: PaneFocusDirectionResult = serde_json::from_str(raw).unwrap();
        assert_eq!(
            result.focus.unwrap().outcome(),
            PaneOutcome::Unchanged(NothingToDo::NoNeighbour)
        );
    }

    #[test]
    fn each_reason_herdr_gives_for_doing_nothing_is_recognised_and_distinct() {
        let outcome = |reason: &str| {
            PaneChange {
                changed: false,
                reason: Some(reason.to_string()),
                ..Default::default()
            }
            .outcome()
        };
        assert_eq!(
            outcome("single_pane"),
            PaneOutcome::Unchanged(NothingToDo::OnlyPane)
        );
        assert_eq!(
            outcome("already_zoomed"),
            PaneOutcome::Unchanged(NothingToDo::AlreadySo)
        );
        assert_eq!(
            outcome("already_unzoomed"),
            PaneOutcome::Unchanged(NothingToDo::AlreadySo)
        );
        // A reason a future herdr invents must not be mistaken for one we understand.
        assert_eq!(
            outcome("teleported"),
            PaneOutcome::Unchanged(NothingToDo::Unrecognised)
        );
    }

    #[test]
    fn a_zoom_response_reports_whether_the_zoom_actually_flipped() {
        let raw = r#"{"type":"pane_zoom","zoom":{"changed":false,"zoom_changed":false,
            "reason":"already_zoomed","pane_id":"w1:p1","zoomed":true}}"#;
        let result: PaneZoomResult = serde_json::from_str(raw).unwrap();
        assert_eq!(
            result.zoom.unwrap().outcome(),
            PaneOutcome::Unchanged(NothingToDo::AlreadySo)
        );
    }

    #[test]
    fn the_wire_spelling_of_every_pane_parameter_is_the_one_herdr_documents() {
        // These strings are the entire contract with herdr for these commands, and they are the
        // one thing in this file no test above this crate can ever notice going wrong.
        assert_eq!(
            serde_json::to_value(PaneDirection::ALL).unwrap(),
            serde_json::json!(["left", "right", "up", "down"])
        );
        assert_eq!(
            serde_json::to_value(SplitDirection::ALL).unwrap(),
            serde_json::json!(["right", "down"])
        );
        assert_eq!(
            serde_json::to_value(ZoomMode::ALL).unwrap(),
            serde_json::json!(["on", "off"]),
        );
    }

    #[test]
    fn workspace_label_falls_back_to_its_id() {
        let w = WorkspaceInfo {
            workspace_id: "w3".into(),
            ..Default::default()
        };
        assert_eq!(w.label(), "w3");
    }
}
