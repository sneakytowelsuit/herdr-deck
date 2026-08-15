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
/// The envelope `session.snapshot` actually returns.
///
/// Note the absence of `#[serde(default)]`: a missing `snapshot` key must be a hard error. This
/// type exists because the tolerance that is right for *fields* is wrong for *shape* — a snapshot
/// parsed one level too high silently became an empty one, and an empty snapshot is
/// indistinguishable from a herdr with nothing running.
#[derive(Debug, Clone, Deserialize)]
pub struct SessionSnapshotResult {
    pub snapshot: SessionSnapshot,
}

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

// ----- structure -------------------------------------------------------------------------------
//
// Worktrees, layouts and the parameters that create a workspace or a tab. Like the pane
// vocabularies above, these live here because they are herdr's shapes rather than the deck's —
// and because two of them double as *config* shapes, which means the spelling a user types and
// the spelling herdr reads are pinned side by side in one file rather than drifting apart in two.

/// One checkout from `worktree.list`.
///
/// herdr builds this by shelling out to `git worktree list --porcelain`, so every field here is
/// git's opinion rather than herdr's, with one exception: `open_workspace_id`, which is herdr
/// saying "I already have this one open as a workspace".
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorktreeInfo {
    /// The absolute path of the checkout. The only field guaranteed to be present, and therefore
    /// the one the deck opens by.
    pub path: String,
    /// Absent on a detached checkout.
    #[serde(default)]
    pub branch: Option<String>,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub is_bare: bool,
    #[serde(default)]
    pub is_detached: bool,
    /// git has lost the checkout — the directory is gone but the administrative file remains.
    #[serde(default)]
    pub is_prunable: bool,
    /// False for the source checkout the other worktrees hang off.
    #[serde(default)]
    pub is_linked_worktree: bool,
    /// The workspace this checkout is already open as, when it is.
    #[serde(default)]
    pub open_workspace_id: Option<String>,
}

impl WorktreeInfo {
    /// The best short human name, in decreasing order of specificity.
    ///
    /// A branch name beats a path because that is what the work is called; the last path component
    /// is the fallback because a worktree directory is nearly always named after its branch anyway.
    pub fn label(&self) -> &str {
        self.label
            .as_deref()
            .or(self.branch.as_deref())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| {
                self.path
                    .trim_end_matches('/')
                    .rsplit('/')
                    .next()
                    .filter(|s| !s.is_empty())
                    .unwrap_or(&self.path)
            })
    }

    /// Can `worktree.open` actually open this one?
    ///
    /// herdr refuses bare and prunable entries with `worktree_not_found`. Offering them on a key
    /// would be offering a press that cannot work, so they never reach the deck at all.
    pub fn openable(&self) -> bool {
        !self.is_bare && !self.is_prunable && !self.path.is_empty()
    }
}

/// `worktree.list`'s envelope. The `source` block it also carries is not modelled: the deck shows
/// the checkouts, and the repository they belong to is the one the user is already looking at.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WorktreeListResult {
    #[serde(default)]
    pub worktrees: Vec<WorktreeInfo>,
}

/// What to put in a new workspace or a new tab.
///
/// Every field is free text, which is exactly why this is a *config* type: a deck has no keyboard,
/// so the only way any of it can be supplied is by being written down in advance and given a name.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CreateSpec {
    pub label: Option<String>,
    pub cwd: Option<String>,
    pub env: BTreeMap<String, String>,
}

impl CreateSpec {
    /// The parameters herdr wants, with everything unset left out rather than sent as null.
    pub(crate) fn to_params(&self, focus: bool) -> serde_json::Value {
        let mut params = serde_json::json!({ "focus": focus });
        if let Some(label) = &self.label {
            params["label"] = serde_json::json!(label);
        }
        if let Some(cwd) = &self.cwd {
            params["cwd"] = serde_json::json!(cwd);
        }
        if !self.env.is_empty() {
            params["env"] = serde_json::json!(self.env);
        }
        params
    }
}

/// How many panes a layout tree may describe, and how deeply it may nest.
///
/// herdr's own limits. Checked here so a preset that breaks them is refused when the config is
/// read, rather than accepted and then rejected by herdr the first time somebody presses the key.
pub const MAX_LAYOUT_PANES: usize = 24;
pub const MAX_LAYOUT_DEPTH: usize = 16;

/// One node of a layout tree: either a pane, or a split holding two more nodes.
///
/// Flat rather than an enum because this is a *TOML* shape before it is a JSON one, and a tagged
/// enum would put `type = "pane"` on every leaf of a tree that is mostly leaves. `split` is the
/// discriminator: with it the node is a split and needs both children, without it the node is a
/// pane. [`LayoutNode::validate`] is what makes that rule an error rather than a surprise.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LayoutNode {
    /// Set to make this a split.
    pub split: Option<SplitDirection>,
    /// How much of the space the first child gets, as a percentage.
    ///
    /// A whole number rather than herdr's fraction, for two reasons. `50` is what a person writing
    /// a config means, and an integer keeps every command the deck can issue comparable by value —
    /// a float in here would make two identical keys unable to agree that they are identical.
    pub ratio: Option<u8>,
    pub first: Option<Box<LayoutNode>>,
    pub second: Option<Box<LayoutNode>>,

    /// Pane fields. All free text, all optional, and all only reachable from a named preset.
    pub cwd: Option<String>,
    pub command: Option<Vec<String>>,
    pub label: Option<String>,
}

impl LayoutNode {
    pub fn is_split(&self) -> bool {
        self.split.is_some()
    }

    /// herdr's smallest and largest ratios. It clamps silently rather than complaining, so
    /// anything outside this is refused here instead — a key that asked for 95% and quietly got
    /// 90% is a key whose config lies about what it does.
    const RATIO_RANGE: std::ops::RangeInclusive<u8> = 10..=90;

    /// Check the tree against herdr's rules and this crate's own.
    ///
    /// Returns the reason as prose, because the only place it can go is a config error in front of
    /// somebody who is editing the file right now.
    pub fn validate(&self) -> std::result::Result<(), String> {
        let mut panes = 0usize;
        self.check(1, &mut panes)?;
        if panes > MAX_LAYOUT_PANES {
            return Err(format!(
                "describes {panes} panes; herdr accepts at most {MAX_LAYOUT_PANES}"
            ));
        }
        Ok(())
    }

    fn check(&self, depth: usize, panes: &mut usize) -> std::result::Result<(), String> {
        if depth > MAX_LAYOUT_DEPTH {
            return Err(format!(
                "nests more than {MAX_LAYOUT_DEPTH} levels deep, which herdr will not accept"
            ));
        }
        match self.split {
            Some(_) => {
                if self.cwd.is_some() || self.command.is_some() || self.label.is_some() {
                    return Err(
                        "a `split` also carries pane settings; move `cwd`, `command` or `label` \
                         onto one of its children"
                            .to_string(),
                    );
                }
                let ratio = self.ratio.unwrap_or(50);
                if !Self::RATIO_RANGE.contains(&ratio) {
                    return Err(format!(
                        "has ratio {ratio}; herdr only honours {}..={}",
                        Self::RATIO_RANGE.start(),
                        Self::RATIO_RANGE.end()
                    ));
                }
                let (Some(first), Some(second)) = (&self.first, &self.second) else {
                    return Err("a `split` needs both a `first` and a `second` child".to_string());
                };
                first.check(depth + 1, panes)?;
                second.check(depth + 1, panes)
            }
            None => {
                if self.first.is_some() || self.second.is_some() || self.ratio.is_some() {
                    return Err(
                        "has children or a ratio but no `split` saying which way it divides"
                            .to_string(),
                    );
                }
                *panes += 1;
                Ok(())
            }
        }
    }

    /// The JSON herdr wants. Only reached after [`Self::validate`] has passed.
    pub(crate) fn to_params(&self) -> serde_json::Value {
        match self.split {
            Some(direction) => {
                let empty = LayoutNode::default();
                serde_json::json!({
                    "type": "split",
                    "direction": direction,
                    // Divided as a double, not a float: `60f32 / 100.0` serialises as
                    // 0.6000000238418579, which is a true value of the float and a startling thing
                    // to find in a request when you wrote `ratio = 60`.
                    "ratio": f64::from(self.ratio.unwrap_or(50)) / 100.0,
                    "first": self.first.as_deref().unwrap_or(&empty).to_params(),
                    "second": self.second.as_deref().unwrap_or(&empty).to_params(),
                })
            }
            None => {
                let mut pane = serde_json::json!({ "type": "pane" });
                if let Some(cwd) = &self.cwd {
                    pane["cwd"] = serde_json::json!(cwd);
                }
                if let Some(command) = &self.command {
                    pane["command"] = serde_json::json!(command);
                }
                if let Some(label) = &self.label {
                    pane["label"] = serde_json::json!(label);
                }
                pane
            }
        }
    }
}

/// A named arrangement of panes, from config.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LayoutPreset {
    /// What to call the tab this makes. Optional; herdr names it itself otherwise.
    pub label: Option<String>,
    pub root: LayoutNode,
}

impl LayoutPreset {
    pub fn validate(&self) -> std::result::Result<(), String> {
        self.root.validate()
    }
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

    // --- Structure ---------------------------------------------------------------------------

    #[test]
    fn a_worktree_listing_parses_and_says_which_checkouts_are_already_open() {
        let raw = r#"{"type":"worktree_list",
            "source":{"repo_key":"k","repo_name":"api","repo_root":"/src/api"},
            "worktrees":[
              {"path":"/src/api","branch":"main","is_linked_worktree":false,
               "open_workspace_id":"w1","label":"api"},
              {"path":"/src/.worktrees/api/fix-auth","branch":"fix-auth",
               "is_linked_worktree":true}
            ]}"#;
        let result: WorktreeListResult = serde_json::from_str(raw).unwrap();
        assert_eq!(result.worktrees.len(), 2);
        assert_eq!(result.worktrees[0].open_workspace_id.as_deref(), Some("w1"));
        assert_eq!(result.worktrees[1].open_workspace_id, None);
    }

    #[test]
    fn a_worktree_names_itself_by_branch_and_falls_back_to_its_directory() {
        // A detached checkout has no branch at all, and a key showing its full path would show
        // nothing but the shared prefix every worktree on the machine has.
        let detached = WorktreeInfo {
            path: "/home/dev/.worktrees/api/spike/".into(),
            is_detached: true,
            ..Default::default()
        };
        assert_eq!(detached.label(), "spike");

        let named = WorktreeInfo {
            path: "/home/dev/.worktrees/api/spike".into(),
            branch: Some("fix-auth".into()),
            ..Default::default()
        };
        assert_eq!(named.label(), "fix-auth");
    }

    #[test]
    fn a_checkout_herdr_would_refuse_to_open_says_so_before_it_reaches_a_key() {
        // herdr answers `worktree_not_found` for bare and prunable entries. A key that offered one
        // would be a key that cannot work, which is worse than a key that is not there.
        let bare = WorktreeInfo {
            path: "/src/api.git".into(),
            is_bare: true,
            ..Default::default()
        };
        let gone = WorktreeInfo {
            path: "/src/gone".into(),
            is_prunable: true,
            ..Default::default()
        };
        let ordinary = WorktreeInfo {
            path: "/src/api".into(),
            ..Default::default()
        };
        assert!(!bare.openable());
        assert!(!gone.openable());
        assert!(ordinary.openable());
    }

    #[test]
    fn a_layout_preset_reaches_herdr_as_the_tree_it_describes() {
        let preset: LayoutPreset = toml::from_str(
            r#"
            label = "dev"
            [root]
            split = "right"
            ratio = 60
            [root.first]
            [root.second]
            command = ["cargo", "watch"]
            "#,
        )
        .unwrap();
        preset.validate().unwrap();

        assert_eq!(
            preset.root.to_params(),
            serde_json::json!({
                "type": "split",
                "direction": "right",
                "ratio": 0.6,
                "first": {"type": "pane"},
                "second": {"type": "pane", "command": ["cargo", "watch"]},
            })
        );
    }

    #[test]
    fn a_layout_with_a_ratio_herdr_would_silently_clamp_is_refused_instead() {
        // herdr accepts 0.95 and quietly gives you 0.9. A config that says one thing and does
        // another is worse than a config that will not load.
        let node = LayoutNode {
            split: Some(SplitDirection::Down),
            ratio: Some(95),
            first: Some(Box::default()),
            second: Some(Box::default()),
            ..Default::default()
        };
        let err = node.validate().unwrap_err();
        assert!(err.contains("95"), "got {err}");
    }

    #[test]
    fn a_split_missing_half_of_itself_is_an_error_and_not_an_empty_pane() {
        let node = LayoutNode {
            split: Some(SplitDirection::Right),
            first: Some(Box::default()),
            ..Default::default()
        };
        assert!(node.validate().unwrap_err().contains("second"));
    }

    #[test]
    fn a_pane_that_was_given_children_without_a_split_says_which_field_is_missing() {
        // The likeliest typo in the whole schema: writing `first`/`second` and forgetting to say
        // which way the divider goes. Without this it would load as a bare pane and silently drop
        // everything underneath it.
        let node = LayoutNode {
            first: Some(Box::default()),
            second: Some(Box::default()),
            ..Default::default()
        };
        assert!(node.validate().unwrap_err().contains("split"));
    }

    #[test]
    fn a_layout_larger_than_herdr_accepts_is_refused_when_the_config_is_read() {
        // Twenty-five panes, built as a spine of splits. herdr would reject this at press time
        // with `invalid_layout`; catching it at load means the user learns while they are looking
        // at the file.
        let mut node = LayoutNode::default();
        for _ in 0..MAX_LAYOUT_PANES {
            node = LayoutNode {
                split: Some(SplitDirection::Down),
                first: Some(Box::default()),
                second: Some(Box::new(node)),
                ..Default::default()
            };
        }
        let err = node.validate().unwrap_err();
        assert!(
            err.contains("deep") || err.contains("panes"),
            "an oversized layout must say so: {err}"
        );
    }

    #[test]
    fn a_create_preset_sends_only_what_it_was_given() {
        // Sending `label: null` is not the same as not sending `label`, and herdr's create methods
        // treat the absent form as "use your own default".
        let bare = CreateSpec::default();
        assert_eq!(bare.to_params(true), serde_json::json!({"focus": true}));

        let full = CreateSpec {
            label: Some("notes".into()),
            cwd: Some("/home/dev/notes".into()),
            env: BTreeMap::from([("EDITOR".to_string(), "hx".to_string())]),
        };
        assert_eq!(
            full.to_params(true),
            serde_json::json!({
                "focus": true,
                "label": "notes",
                "cwd": "/home/dev/notes",
                "env": {"EDITOR": "hx"},
            })
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
