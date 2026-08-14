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
use crate::command::DeckCommand;
use crate::config::{Config, Presets};
use crate::render::{KeyGlyph, Tile};
use crate::state::{Acknowledged, DeckState};
use herdr_deck_herdr::wire::{AgentInfo, PaneDirection, SplitDirection, WorktreeInfo, ZoomMode};
use serde::{Deserialize, Serialize};

/// Which list the deck is currently showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    #[default]
    Agents,
    Workspaces,
    /// The git worktrees of the repository the focused workspace belongs to.
    ///
    /// Only ever reached when there are some — see [`ResolvedDeck::next_mode`]. A third stop on
    /// the mode key that showed an empty grid to everybody who does not use worktrees would be a
    /// press taken from the people who do.
    Worktrees,
}

impl Mode {
    pub fn label(self) -> &'static str {
        match self {
            Mode::Agents => "agents",
            Mode::Workspaces => "spaces",
            Mode::Worktrees => "trees",
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
    /// The git worktrees of the repository the focused workspace belongs to.
    Worktrees,
}

impl ScrubTarget {
    pub fn label(self) -> &'static str {
        match self {
            ScrubTarget::Agents => "agent",
            ScrubTarget::Workspaces => "space",
            ScrubTarget::Tabs => "tab",
            ScrubTarget::Attention => "needs you",
            ScrubTarget::Worktrees => "worktree",
        }
    }
}

/// What a key does.
///
/// `deny_unknown_fields` for the same reason the config as a whole has it: a field that does not
/// belong to the binding it was written on is a misunderstanding, and the deck saying so beats the
/// user staring at a key that quietly ignores half of what they asked for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", deny_unknown_fields)]
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
    /// One herdr command, spelled out in config.
    ///
    /// The escape hatch for everything the derived layout does not offer: a key bound straight to
    /// a [`DeckCommand`], with no list, no page and no cursor behind it. It is also how a
    /// destructive command would ever reach a key, which is why the guard lives on the action
    /// rather than on the bindings that happen to produce one today.
    Command {
        command: DeckCommand,
    },
    /// Close whichever pane herdr says is focused.
    ///
    /// Its own binding rather than a [`KeyBinding::Command`] carrying a pane id, because a pane id
    /// written into a config file is a promise nobody can keep: pane ids are renumbered when a
    /// pane moves workspaces, so `w1:p2` in a file written last month names whatever is second
    /// there now. The deck resolves it against live state instead, and refuses when herdr reports
    /// nothing focused.
    ClosePane,
    /// Build a new tab from a layout named in config.
    ///
    /// The tree itself lives in `[layouts.<name>]` rather than on the key, because a key is a
    /// place to say *which* and a config file is the place to say *what*. A key naming a layout
    /// that is not defined is refused when the config is read; the dimmed face it would otherwise
    /// draw exists only for the layout a herdr-deck upgrade might one day take away.
    Layout {
        preset: String,
    },
    /// Make a new workspace, from `[workspaces.<name>]` when one is named.
    NewWorkspace {
        #[serde(default)]
        preset: Option<String>,
    },
    /// Make a new tab in the workspace you are in, from `[tabs.<name>]` when one is named.
    NewTab {
        #[serde(default)]
        preset: Option<String>,
    },
    /// Make a new git worktree and go to it.
    ///
    /// Takes nothing, and needs nothing: herdr invents the branch name. This is the only key on
    /// the deck that starts a piece of work rather than navigating to one.
    NewWorktree,
    /// Close whichever tab herdr says is focused.
    ///
    /// Argument-less for the same reason [`KeyBinding::ClosePane`] is: an id written into a config
    /// file names whatever answers to it now, not what it named when it was written.
    CloseTab,
    /// Give the focused workspace's worktree back to git.
    ///
    /// Only offers itself when the focused workspace really is a linked worktree the deck has seen
    /// in herdr's listing — otherwise herdr would refuse it, and a key that draws as usable and
    /// then refuses is a key that has to be tried to be understood.
    RemoveWorktree,
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
///
/// Two kinds of thing live here, and the split is the point. [`SlotAction::Command`] leaves the
/// daemon and reaches herdr; everything else is the deck rearranging its own view and never
/// touches herdr at all. Only the first kind is audited, and only the first kind can be
/// destructive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlotAction {
    /// Ask herdr for something. One variant, however many commands there come to be.
    Command(DeckCommand),
    /// Focus an agent: herdr focus **and** raise the terminal window.
    ///
    /// Deliberately not a [`DeckCommand`]: the deck binds agents by the stable `terminal_id` and
    /// herdr focuses *panes*, so this is the one action carrying a target herdr would not accept.
    /// The daemon resolves it against live state at the moment the key is released, which is what
    /// lets a vanished agent be reported rather than guessed at — and what stops an unresolved
    /// target ever reaching the socket.
    FocusAgent {
        terminal_id: String,
    },
    /// Dismiss an agent from the attention queue. Pointedly **not** a focus: herdr is never
    /// called and no window moves, which is the entire reason this gesture exists.
    AcknowledgeAgent {
        terminal_id: String,
    },
    ToggleMode,
    ChangePage {
        delta: i32,
    },
    Scrub {
        target: ScrubTarget,
        delta: i32,
    },
    /// The control was pressed, will not act, and says why on its own face.
    ///
    /// A key that goes quiet reads as broken hardware. This is the difference between "nothing is
    /// bound here" and "I heard you, and here is what you have to do instead".
    Refuse {
        message: String,
    },
    /// Nothing bound, or nothing there right now.
    None,
}

impl SlotAction {
    /// The command this action would send to herdr, if any.
    fn command(&self) -> Option<&DeckCommand> {
        match self {
            SlotAction::Command(command) => Some(command),
            _ => None,
        }
    }

    fn is_destructive(&self) -> bool {
        self.command().is_some_and(DeckCommand::is_destructive)
    }
}

/// The destructive-action guard, on the way to a control that acts the instant it is pressed.
///
/// Anything that can destroy work is taken off the tap and put on the hold — the gesture this
/// deck already uses for "I mean this" — and the tap says so out loud. There is exactly one
/// confirmation idiom on this hardware and this is it; a second one would be a second thing for
/// the user to learn and a second thing to get wrong under the finger.
///
/// Applied to the *action*, not to the bindings that produce one. A command added later is
/// guarded on the day it is added, whether it arrives from a derived layout, a pinned key or a
/// hand-written config.
fn guard_tap(action: SlotAction) -> SlotAction {
    match &action {
        SlotAction::Command(command) if command.is_destructive() => SlotAction::Refuse {
            message: format!("hold to {}", command.label()),
        },
        _ => action,
    }
}

/// How long the list behind each scrub target is right now.
///
/// A named struct rather than a tuple because there are five of them and they are all `usize`:
/// two of them swapped at a call site would be a bug nothing could catch.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ListLengths {
    pub agents: usize,
    pub workspaces: usize,
    pub tabs: usize,
    pub attention: usize,
    pub worktrees: usize,
}

/// Where each scrub cursor currently sits.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Selection {
    pub agents: usize,
    pub workspaces: usize,
    pub tabs: usize,
    pub attention: usize,
    pub worktrees: usize,
}

impl Selection {
    pub fn get(&self, target: ScrubTarget) -> usize {
        match target {
            ScrubTarget::Agents => self.agents,
            ScrubTarget::Workspaces => self.workspaces,
            ScrubTarget::Tabs => self.tabs,
            ScrubTarget::Attention => self.attention,
            ScrubTarget::Worktrees => self.worktrees,
        }
    }

    /// Move a cursor, wrapping at both ends so a dial never gets stuck.
    pub fn scrub(&mut self, target: ScrubTarget, delta: i32, len: usize) {
        let slot = match target {
            ScrubTarget::Agents => &mut self.agents,
            ScrubTarget::Workspaces => &mut self.workspaces,
            ScrubTarget::Tabs => &mut self.tabs,
            ScrubTarget::Attention => &mut self.attention,
            ScrubTarget::Worktrees => &mut self.worktrees,
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
    pub fn clamp(&mut self, lengths: ListLengths) {
        let fix = |v: &mut usize, len: usize| {
            if len == 0 {
                *v = 0;
            } else if *v >= len {
                *v = len - 1;
            }
        };
        fix(&mut self.agents, lengths.agents);
        fix(&mut self.workspaces, lengths.workspaces);
        fix(&mut self.tabs, lengths.tabs);
        fix(&mut self.attention, lengths.attention);
        fix(&mut self.worktrees, lengths.worktrees);
    }
}

/// A concrete assignment of bindings to physical controls.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Profile {
    pub keys: Vec<KeyBinding>,
    pub dials: Vec<DialBinding>,
    /// The named layouts, workspaces and tabs a key here may refer to.
    ///
    /// Carried by the profile rather than fetched from the config at press time, so that resolving
    /// a key needs nothing the layout engine does not already hold — and so a key naming a preset
    /// that has gone missing degrades to a dimmed face that says so, rather than to a panic.
    #[serde(default)]
    pub presets: Presets,
}

impl Profile {
    /// Build the default layout for this hardware alone.
    pub fn for_capabilities(caps: &DeckCapabilities) -> Self {
        Self::derive(caps, &Config::default())
    }

    /// Build the default layout for this hardware and this config's presets.
    ///
    /// The presets are an input to the *layout* and not only to the keys: a deck with room to
    /// spare grows one key per named layout, because a layout nobody can reach is one nobody
    /// meant to write down.
    pub fn derive(caps: &DeckCapabilities, config: &Config) -> Self {
        let dials = default_dials(caps.dials);
        let keys = default_keys(caps, !dials.is_empty(), &config.layouts);
        Self {
            keys,
            dials,
            presets: config.presets(),
        }
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

/// How many keys a deck needs before pane control appears without being asked for.
///
/// Chosen as the point where the agent slots stop being the scarce thing: below it a key spent on
/// a pane arrow is a key taken off an agent, and above it the deck already shows more agents at
/// once than anybody runs.
const PANE_CLUSTER_MIN_KEYS: usize = 24;

/// Pane control, in the order a hand reaches for it.
///
/// Directions first because they are the ones pressed by feel and in sequence; the pair that
/// changes what is on screen next; the pair that adds a shell last. Every one of them is a single
/// unambiguous outcome with nothing to type — which is what makes them worth a physical key at all.
fn pane_cluster() -> Vec<KeyBinding> {
    let mut keys: Vec<KeyBinding> = PaneDirection::ALL
        .into_iter()
        .map(|direction| KeyBinding::Command {
            command: DeckCommand::MovePaneFocus { direction },
        })
        .collect();
    keys.extend(ZoomMode::ALL.into_iter().map(|zoom| KeyBinding::Command {
        command: DeckCommand::ZoomPane { zoom },
    }));
    keys.extend(
        SplitDirection::ALL
            .into_iter()
            .map(|direction| KeyBinding::Command {
                command: DeckCommand::SplitPane { direction },
            }),
    );
    keys
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

fn default_keys<V>(
    caps: &DeckCapabilities,
    has_dials: bool,
    layouts: &std::collections::BTreeMap<String, V>,
) -> Vec<KeyBinding> {
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
    // A key per named layout, ahead of pane control in the queue because these were asked for by
    // name in a config file and pane control was not. Alphabetical, so the same config always
    // produces the same deck and a key does not move under a thumb because a table was reordered.
    reserved.extend(layouts.keys().map(|preset| KeyBinding::Layout {
        preset: preset.clone(),
    }));
    // Pane control, but only where it is free. On an eight-key deck four arrows would be half the
    // surface, and the agents are the reason the deck is on the desk at all — so the Plus, the
    // Mini and the 15-key get none of this by default and bind it by hand if they want it. Past
    // twenty-four keys there are already more agent slots than anyone has agents, and the cluster
    // costs nothing anybody was using.
    //
    // Nothing destructive is in here. Closing a pane is available, guarded, to anyone who asks for
    // it in their config; it is not something a deck should offer to someone who never did.
    if total >= PANE_CLUSTER_MIN_KEYS {
        reserved.extend(pane_cluster());
    }
    // Never let fixed controls crowd out the agents they are meant to navigate. Pane control is
    // last in the list so that, if anything has to go, it goes before the paging keys that make
    // the rest of the agents reachable at all.
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
    acked: &'a Acknowledged,
    /// Agents in attention order with the acknowledged ones demoted. Held rather than recomputed
    /// because every key, dial and count on the deck asks for it.
    order: Vec<&'a AgentInfo>,
}

impl<'a> ResolvedDeck<'a> {
    pub fn new(
        profile: &'a Profile,
        state: &'a DeckState,
        mode: Mode,
        page: usize,
        selection: Selection,
        acked: &'a Acknowledged,
    ) -> Self {
        Self {
            profile,
            state,
            mode,
            page,
            selection,
            acked,
            order: state.attention_order_with(acked),
        }
    }

    /// The list index a dynamic slot of this rank refers to, accounting for the page.
    fn list_index(&self, rank: usize) -> usize {
        self.page * self.profile.dynamic_slots().max(1) + rank
    }

    fn list_len(&self) -> usize {
        match self.mode {
            Mode::Agents => self.order.len(),
            Mode::Workspaces => self.state.workspaces.len(),
            Mode::Worktrees => self.state.worktrees.len(),
        }
    }

    /// Where the mode key goes next.
    ///
    /// Worktrees are skipped when there are none, so the third stop only exists for the people who
    /// have a third list — everybody else keeps the two-press cycle they had. Leaving worktree
    /// mode is unconditional, so a list that empties underneath you never traps the key.
    pub fn next_mode(&self) -> Mode {
        match self.mode {
            Mode::Agents => Mode::Workspaces,
            Mode::Workspaces if !self.state.worktrees.is_empty() => Mode::Worktrees,
            Mode::Workspaces | Mode::Worktrees => Mode::Agents,
        }
    }

    /// The Nth agent, in the order this deck is actually showing them.
    fn agent_at(&self, index: usize) -> Option<&'a AgentInfo> {
        self.order.get(index).copied()
    }

    /// Only the agents still asking for a human — acknowledged ones have stopped asking.
    fn needing_attention(&self) -> impl Iterator<Item = &'a AgentInfo> + '_ {
        self.order
            .iter()
            .copied()
            .filter(|agent| self.acked.wants_attention(agent))
    }

    /// How many agents the attention key should be counting.
    pub fn attention_count(&self) -> usize {
        self.needing_attention().count()
    }

    /// Lengths for every scrub target.
    ///
    /// Cursors have to be clamped against the same lists the keys resolve against, so handing
    /// them out together is what stops the two drifting apart.
    pub fn list_lengths(&self) -> ListLengths {
        ListLengths {
            agents: self.order.len(),
            workspaces: self.state.workspaces.len(),
            tabs: self.state.tabs.len(),
            attention: self.attention_count(),
            worktrees: self.state.worktrees.len(),
        }
    }

    /// How long the list behind one scrub target is right now.
    pub fn scrub_len(&self, target: ScrubTarget) -> usize {
        let lengths = self.list_lengths();
        match target {
            ScrubTarget::Agents => lengths.agents,
            ScrubTarget::Workspaces => lengths.workspaces,
            ScrubTarget::Tabs => lengths.tabs,
            ScrubTarget::Attention => lengths.attention,
            ScrubTarget::Worktrees => lengths.worktrees,
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
                .map(|agent| agent_tile(self.state, agent, self.acked))
                .unwrap_or(Tile::Empty),
            KeyBinding::PinnedWorkspace { workspace_id } => self
                .state
                .workspace_by_id(workspace_id)
                .map(workspace_tile)
                .unwrap_or(Tile::Empty),
            KeyBinding::Command { command } => command_tile(command, true),
            // Drawn even when herdr has nothing focused, dimmed rather than blank: a key that
            // vanished would take its neighbours' positions with it, and this deck is meant to be
            // found by feel.
            KeyBinding::ClosePane => command_tile(
                &DeckCommand::ClosePane {
                    pane_id: String::new(),
                },
                self.state.focused_pane_id.is_some(),
            ),
            // Every one of these draws the same face whether or not it can act right now, dimmed
            // when it cannot. The alternative — a key that disappears — takes its neighbours'
            // positions with it, and this deck is meant to be found by feel.
            KeyBinding::Layout { preset } => match self.layout_preset(preset) {
                Some(command) => command_tile(&command, true),
                None => Tile::Command {
                    glyph: KeyGlyph::Layout,
                    label: preset.clone(),
                    hold: false,
                    enabled: false,
                },
            },
            KeyBinding::NewWorkspace { preset } => {
                command_tile(&self.create_workspace(preset), true)
            }
            KeyBinding::NewTab { preset } => command_tile(&self.create_tab(preset), true),
            KeyBinding::NewWorktree => command_tile(&DeckCommand::CreateWorktree, true),
            KeyBinding::CloseTab => command_tile(
                &DeckCommand::CloseTab {
                    tab_id: String::new(),
                },
                self.state.focused_tab_id.is_some(),
            ),
            KeyBinding::RemoveWorktree => command_tile(
                &DeckCommand::RemoveWorktree {
                    workspace_id: String::new(),
                },
                self.removable_worktree().is_some(),
            ),
            KeyBinding::NextAttention => Tile::Attention {
                count: self.attention_count(),
            },
            KeyBinding::ModeToggle => Tile::Mode {
                label: self.next_mode().label().to_string(),
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
                .agent_at(index)
                .map(|agent| agent_tile(self.state, agent, self.acked))
                .unwrap_or(Tile::Empty),
            Mode::Workspaces => self
                .state
                .workspaces
                .get(index)
                .map(workspace_tile)
                .unwrap_or(Tile::Empty),
            Mode::Worktrees => self
                .state
                .worktrees
                .get(index)
                .map(|tree| worktree_tile(self.state, tree))
                .unwrap_or(Tile::Empty),
        }
    }

    /// The command a layout key means, when the layout it names is still there.
    fn layout_preset(&self, preset: &str) -> Option<DeckCommand> {
        self.profile
            .presets
            .layouts
            .get(preset)
            .map(|layout| DeckCommand::ApplyLayout {
                preset: preset.to_string(),
                layout: layout.clone(),
            })
    }

    /// What a "new workspace" key means. An unnamed one still works — herdr picks the working
    /// directory from the pane you are in, which is nearly always the one you meant.
    fn create_workspace(&self, preset: &Option<String>) -> DeckCommand {
        DeckCommand::CreateWorkspace {
            preset: preset.clone(),
            spec: self.spec(&self.profile.presets.workspaces, preset),
        }
    }

    fn create_tab(&self, preset: &Option<String>) -> DeckCommand {
        DeckCommand::CreateTab {
            preset: preset.clone(),
            spec: self.spec(&self.profile.presets.tabs, preset),
        }
    }

    fn spec(
        &self,
        book: &std::collections::BTreeMap<String, herdr_deck_herdr::wire::CreateSpec>,
        preset: &Option<String>,
    ) -> herdr_deck_herdr::wire::CreateSpec {
        preset
            .as_ref()
            .and_then(|name| book.get(name))
            .cloned()
            .unwrap_or_default()
    }

    /// The worktree a remove key would give back to git, when there is one.
    ///
    /// Only a *linked* checkout that the deck has actually seen in herdr's listing qualifies.
    /// herdr refuses to remove the source repository — and finding that out by pressing a key
    /// that looked usable is exactly the experience the dimming exists to avoid.
    fn removable_worktree(&self) -> Option<&'a WorktreeInfo> {
        let workspace_id = self.state.focused_workspace_id.as_deref()?;
        self.state.linked_worktree_of(workspace_id)
    }

    /// What pressing key `index` should do.
    pub fn key_action(&self, index: usize) -> SlotAction {
        guard_tap(self.bound_key_action(index))
    }

    /// What key `index` is bound to, before the guard has had its say.
    fn bound_key_action(&self, index: usize) -> SlotAction {
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
            KeyBinding::PinnedWorkspace { workspace_id } => {
                SlotAction::Command(DeckCommand::FocusWorkspace {
                    workspace_id: workspace_id.clone(),
                })
            }
            KeyBinding::Command { command } => SlotAction::Command(command.clone()),
            // herdr has no "close whatever is focused" form, so the deck has to name the pane —
            // and the only pane it may name is the one herdr last told it was focused. Guessing
            // one when herdr reports none would be the worst possible guess, so the key says so
            // instead.
            KeyBinding::ClosePane => match &self.state.focused_pane_id {
                Some(pane_id) => SlotAction::Command(DeckCommand::ClosePane {
                    pane_id: pane_id.clone(),
                }),
                None => SlotAction::Refuse {
                    message: "herdr reports no focused pane".to_string(),
                },
            },
            KeyBinding::Layout { preset } => match self.layout_preset(preset) {
                Some(command) => SlotAction::Command(command),
                None => SlotAction::Refuse {
                    message: format!("no layout named {preset}"),
                },
            },
            KeyBinding::NewWorkspace { preset } => {
                SlotAction::Command(self.create_workspace(preset))
            }
            KeyBinding::NewTab { preset } => SlotAction::Command(self.create_tab(preset)),
            KeyBinding::NewWorktree => SlotAction::Command(DeckCommand::CreateWorktree),
            KeyBinding::CloseTab => match &self.state.focused_tab_id {
                Some(tab_id) => SlotAction::Command(DeckCommand::CloseTab {
                    tab_id: tab_id.clone(),
                }),
                None => SlotAction::Refuse {
                    message: "herdr reports no focused tab".to_string(),
                },
            },
            // Named by the workspace it is open as, which is a herdr id — not by its path, which
            // would be a string from a config file pointing at a directory that has since moved.
            KeyBinding::RemoveWorktree => match self.removable_worktree() {
                Some(tree) => match &tree.open_workspace_id {
                    Some(workspace_id) => SlotAction::Command(DeckCommand::RemoveWorktree {
                        workspace_id: workspace_id.clone(),
                    }),
                    None => SlotAction::Refuse {
                        message: "herdr has not opened that worktree".to_string(),
                    },
                },
                None => SlotAction::Refuse {
                    message: "this workspace is not a worktree".to_string(),
                },
            },
            KeyBinding::NextAttention => self
                .needing_attention()
                .next()
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
                .agent_at(index)
                .map(|a| SlotAction::FocusAgent {
                    terminal_id: a.terminal_id.clone(),
                })
                .unwrap_or(SlotAction::None),
            Mode::Workspaces => self
                .state
                .workspaces
                .get(index)
                .map(|w| {
                    SlotAction::Command(DeckCommand::FocusWorkspace {
                        workspace_id: w.workspace_id.clone(),
                    })
                })
                .unwrap_or(SlotAction::None),
            // Always an open, never a workspace focus, even for a checkout herdr already has
            // open: `worktree.open` is idempotent and focuses what it finds, so one path covers
            // both cases and there is no state the deck could be wrong about.
            Mode::Worktrees => self
                .state
                .worktrees
                .get(index)
                .map(|tree| {
                    SlotAction::Command(DeckCommand::OpenWorktree {
                        path: tree.path.clone(),
                    })
                })
                .unwrap_or(SlotAction::None),
        }
    }

    /// What *holding* key `index` should do.
    ///
    /// Two things claim a hold. A destructive command, which the guard moved here off the tap;
    /// and acknowledging an agent, which is a statement about one agent — there is nothing a page
    /// key or a mode toggle could mean by it. Every other binding deliberately reports nothing
    /// rather than growing a gesture nobody asked for.
    ///
    /// A [`SlotAction::None`] here is also what tells the daemon a key is a plain one, and so may
    /// keep acting the instant it is pressed.
    pub fn key_long_press_action(&self, index: usize) -> SlotAction {
        if self.state.is_offline() {
            return SlotAction::None;
        }
        // The hold is where the guard put a destructive command, so it outranks anything else the
        // key might have offered — and a key showing one is not showing an agent anyway.
        let bound = self.bound_key_action(index);
        if bound.is_destructive() {
            return bound;
        }
        // Only an agent that is actually asking for attention can be dismissed. Storing an
        // acknowledgement for a working agent would do nothing visible now but arm a mute for
        // the moment it blocks — a dismissal the user never made, for a state they never saw.
        let acknowledge = |agent: &AgentInfo| {
            if agent.agent_status.needs_attention() {
                SlotAction::AcknowledgeAgent {
                    terminal_id: agent.terminal_id.clone(),
                }
            } else {
                SlotAction::None
            }
        };
        match self.profile.keys.get(index) {
            Some(KeyBinding::Dynamic { rank }) if self.mode == Mode::Agents => self
                .agent_at(self.list_index(*rank))
                .map(acknowledge)
                .unwrap_or(SlotAction::None),
            Some(KeyBinding::PinnedAgent { terminal_id }) => self
                .state
                .agent_by_terminal_id(terminal_id)
                .map(acknowledge)
                .unwrap_or(SlotAction::None),
            _ => SlotAction::None,
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
    ///
    /// A dial has no hold — the daemon ignores its release — so the guard here can only refuse.
    /// That is the honest answer: there is no gesture on an encoder that means "I am sure".
    pub fn dial_press_action(&self, dial: usize) -> SlotAction {
        guard_tap(self.bound_dial_press_action(dial))
    }

    fn bound_dial_press_action(&self, dial: usize) -> SlotAction {
        if self.state.is_offline() {
            return SlotAction::None;
        }
        let Some(DialBinding::Scrub { target }) = self.profile.dials.get(dial) else {
            return SlotAction::None;
        };
        let cursor = self.selection.get(*target);
        match target {
            ScrubTarget::Agents => self
                .agent_at(cursor)
                .map(|a| SlotAction::FocusAgent {
                    terminal_id: a.terminal_id.clone(),
                })
                .unwrap_or(SlotAction::None),
            ScrubTarget::Attention => self
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
                .map(|w| {
                    SlotAction::Command(DeckCommand::FocusWorkspace {
                        workspace_id: w.workspace_id.clone(),
                    })
                })
                .unwrap_or(SlotAction::None),
            ScrubTarget::Tabs => self
                .state
                .tabs
                .get(cursor)
                .map(|t| {
                    SlotAction::Command(DeckCommand::FocusTab {
                        tab_id: t.tab_id.clone(),
                    })
                })
                .unwrap_or(SlotAction::None),
            ScrubTarget::Worktrees => self
                .state
                .worktrees
                .get(cursor)
                .map(|tree| {
                    SlotAction::Command(DeckCommand::OpenWorktree {
                        path: tree.path.clone(),
                    })
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
                .agent_at(cursor)
                .map(|a| (a.label().to_string(), Some(a.agent_status)))
                .unwrap_or_else(|| ("—".to_string(), None)),
            ScrubTarget::Attention => self
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
            // No status: a worktree is a checkout, not something that can be blocked. Handing the
            // strip a colour here would be inventing a state herdr never reported.
            ScrubTarget::Worktrees => self
                .state
                .worktrees
                .get(cursor)
                .map(|tree| (tree.label().to_string(), None))
                .unwrap_or_else(|| ("—".to_string(), None)),
        };
        (target.label().to_string(), value, status)
    }
}

/// Needs the whole state, not just the agent: the footer names the agent's *project*, and only
/// the state knows what herdr calls the workspace the agent happens to be in.
fn agent_tile(state: &DeckState, agent: &AgentInfo, acked: &Acknowledged) -> Tile {
    // Only an *attention* state can be dismissed. Marking a working agent acknowledged would
    // repaint a perfectly ordinary tile for no reason a user could explain.
    let acknowledged = agent.agent_status.needs_attention() && acked.contains(agent);
    Tile::Agent {
        label: agent.label().to_string(),
        sublabel: agent.state_label().map(str::to_string),
        workspace: state.project_label(agent).map(str::to_string),
        status: agent.agent_status,
        focused: agent.focused,
        acknowledged,
        // A dismissed agent has stopped asking, so it has nothing left to escalate: keeping its
        // marker would go on shouting the exact number the user just said they had seen.
        waiting: if acknowledged {
            None
        } else {
            state.wait_bucket(agent)
        },
    }
}

/// What a key bound to one command draws.
///
/// The `hold` flag comes off the command rather than off the key, exactly as the guard does — a
/// tile that advertised the gesture separately could disagree with the gesture the key actually
/// has, and the first anyone would know of it is a key that appears broken when tapped.
fn command_tile(command: &DeckCommand, enabled: bool) -> Tile {
    let label = command.label().to_string();
    let hold = command.is_destructive();
    match command_glyph(command) {
        Some(glyph) => Tile::Command {
            glyph,
            label,
            hold,
            enabled,
        },
        // A focus takes you to a named thing and no shape can say *which*, so the caption is the
        // whole message and it gets the plain labelled key. An arrow here would be decoration
        // pretending to be information.
        None => Tile::Mode {
            label: if hold {
                format!("hold: {label}")
            } else {
                label
            },
            active: enabled,
        },
    }
}

/// The shape a command wears on a key, where it has one.
///
/// Lives here rather than on [`DeckCommand`] so the vocabulary stays about intent and knows
/// nothing about pixels.
fn command_glyph(command: &DeckCommand) -> Option<KeyGlyph> {
    match command {
        DeckCommand::MovePaneFocus { direction } => Some(KeyGlyph::Arrow(*direction)),
        DeckCommand::ZoomPane { zoom } => Some(match zoom {
            ZoomMode::On => KeyGlyph::ZoomIn,
            ZoomMode::Off => KeyGlyph::ZoomOut,
        }),
        DeckCommand::SplitPane { direction } => Some(match direction {
            SplitDirection::Right => KeyGlyph::SplitRight,
            SplitDirection::Down => KeyGlyph::SplitDown,
        }),
        DeckCommand::ClosePane { .. } => Some(KeyGlyph::Close),
        DeckCommand::ApplyLayout { .. } => Some(KeyGlyph::Layout),
        DeckCommand::CreateWorkspace { .. } => Some(KeyGlyph::NewWorkspace),
        DeckCommand::CreateTab { .. } => Some(KeyGlyph::NewTab),
        DeckCommand::CreateWorktree => Some(KeyGlyph::NewWorktree),
        DeckCommand::RemoveWorktree { .. } => Some(KeyGlyph::RemoveWorktree),
        DeckCommand::CloseTab { .. } => Some(KeyGlyph::CloseTab),
        // A focus takes you to a named thing and no shape can say which. Opening a worktree is
        // the same: the list is what says which one, so these arrive on a dynamic tile with a
        // name on it rather than on a key with a shape.
        DeckCommand::FocusPane { .. }
        | DeckCommand::FocusWorkspace { .. }
        | DeckCommand::FocusTab { .. }
        | DeckCommand::OpenWorktree { .. } => None,
    }
}

/// Needs the whole state to answer the one question a worktree key is pressed to settle: am I in
/// this one already?
fn worktree_tile(state: &DeckState, tree: &WorktreeInfo) -> Tile {
    Tile::Worktree {
        label: tree.label().to_string(),
        open: tree.open_workspace_id.is_some(),
        focused: tree.open_workspace_id.is_some()
            && tree.open_workspace_id == state.focused_workspace_id,
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
    use herdr_deck_herdr::wire::{
        AgentInfo, AgentStatus, CreateSpec, LayoutNode, LayoutPreset, SessionSnapshot, TabInfo,
        WorkspaceInfo,
    };

    /// A linked worktree, optionally already open as a workspace.
    fn worktree(branch: &str, open_as: Option<&str>) -> WorktreeInfo {
        WorktreeInfo {
            path: format!("/src/.worktrees/api/{branch}"),
            branch: Some(branch.to_string()),
            is_linked_worktree: true,
            open_workspace_id: open_as.map(str::to_string),
            ..Default::default()
        }
    }

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
        let acked = Acknowledged::default();
        let deck = ResolvedDeck::new(
            &profile,
            &state,
            Mode::Agents,
            0,
            Selection::default(),
            &acked,
        );
        // Blocked sorts first, so key 0 shows it.
        assert_eq!(
            deck.key_action(0),
            SlotAction::FocusAgent {
                terminal_id: "stuck".into()
            }
        );
    }

    // --- The second action on a key --------------------------------------------------------
    //
    // Holding an agent key dismisses it from the attention queue. Nothing else on the deck has a
    // second action, and these pin that it stays that way.

    /// The deck a Stream Deck + would resolve to, with `acked` applied.
    fn plus_deck<'a>(
        profile: &'a Profile,
        state: &'a DeckState,
        acked: &'a Acknowledged,
    ) -> ResolvedDeck<'a> {
        ResolvedDeck::new(profile, state, Mode::Agents, 0, Selection::default(), acked)
    }

    #[test]
    fn holding_a_dynamic_key_acknowledges_the_agent_it_shows_rather_than_focusing_it() {
        let caps = DeckModel::Plus.capabilities();
        let profile = Profile::for_capabilities(&caps);
        let state = state_with(vec![agent("stuck", AgentStatus::Blocked, 2)]);
        let acked = Acknowledged::default();
        let deck = plus_deck(&profile, &state, &acked);
        assert_eq!(
            deck.key_long_press_action(0),
            SlotAction::AcknowledgeAgent {
                terminal_id: "stuck".into()
            }
        );
    }

    #[test]
    fn holding_a_pinned_agent_key_acknowledges_that_agent_too() {
        // A pinned key names an agent just as squarely as a dynamic one does; there is no reason
        // for the gesture to work on one and not the other.
        let profile = Profile {
            keys: vec![KeyBinding::PinnedAgent {
                terminal_id: "pinned".into(),
            }],
            dials: vec![],
            presets: Presets::default(),
        };
        let state = state_with(vec![agent("pinned", AgentStatus::Done, 4)]);
        let acked = Acknowledged::default();
        let deck = plus_deck(&profile, &state, &acked);
        assert_eq!(
            deck.key_long_press_action(0),
            SlotAction::AcknowledgeAgent {
                terminal_id: "pinned".into()
            }
        );
    }

    #[test]
    fn holding_anything_that_is_not_an_agent_key_does_nothing_at_all() {
        // Every one of these keys must keep acting the instant it is pressed, so none of them may
        // claim a long press — a page key that went dead when you held it would read as broken.
        let caps = DeckModel::Xl.capabilities();
        let profile = Profile::for_capabilities(&caps);
        let state = state_with(vec![agent("stuck", AgentStatus::Blocked, 1)]);
        let acked = Acknowledged::default();
        let deck = plus_deck(&profile, &state, &acked);
        for binding in [
            KeyBinding::NextAttention,
            KeyBinding::ModeToggle,
            KeyBinding::PagePrev,
            KeyBinding::PageNext,
        ] {
            let index = profile
                .keys
                .iter()
                .position(|k| *k == binding)
                .unwrap_or_else(|| panic!("this profile has a {binding:?} key"));
            assert_eq!(
                deck.key_long_press_action(index),
                SlotAction::None,
                "{binding:?} must have no second action"
            );
        }
    }

    #[test]
    fn holding_a_key_in_workspace_mode_does_nothing_because_workspaces_do_not_want_you() {
        let caps = DeckModel::Plus.capabilities();
        let profile = Profile::for_capabilities(&caps);
        let mut state = state_with(vec![]);
        state.workspaces = vec![WorkspaceInfo {
            workspace_id: "w2".into(),
            ..Default::default()
        }];
        let acked = Acknowledged::default();
        let deck = ResolvedDeck::new(
            &profile,
            &state,
            Mode::Workspaces,
            0,
            Selection::default(),
            &acked,
        );
        assert_eq!(deck.key_long_press_action(0), SlotAction::None);
    }

    #[test]
    fn holding_an_empty_slot_does_nothing_rather_than_acknowledging_something_random() {
        let caps = DeckModel::Plus.capabilities();
        let profile = Profile::for_capabilities(&caps);
        let state = state_with(vec![agent("only", AgentStatus::Blocked, 1)]);
        let acked = Acknowledged::default();
        let deck = plus_deck(&profile, &state, &acked);
        assert_eq!(deck.key_long_press_action(3), SlotAction::None);
    }

    #[test]
    fn an_offline_deck_has_no_second_action_either() {
        let caps = DeckModel::Plus.capabilities();
        let profile = Profile::for_capabilities(&caps);
        let state = DeckState::offline("herdr socket not found");
        let acked = Acknowledged::default();
        let deck = plus_deck(&profile, &state, &acked);
        for index in 0..profile.keys.len() {
            assert_eq!(deck.key_long_press_action(index), SlotAction::None);
        }
    }

    // --- The destructive-action guard --------------------------------------------------------
    //
    // Closing a pane is the one thing this deck can do that takes work away, so it is what these
    // run against. They are about the *gesture*, not about the command: whatever destructive
    // command is added next inherits every one of them without being named here.

    /// A deck whose only key closes the focused pane, with a pane for it to close.
    fn destructive_deck() -> (Profile, DeckState) {
        let profile = Profile {
            keys: vec![KeyBinding::ClosePane],
            dials: vec![DialBinding::Scrub {
                target: ScrubTarget::Workspaces,
            }],
            presets: Presets::default(),
        };
        let mut state = state_with(vec![]);
        state.focused_pane_id = Some("w1:p2".into());
        (profile, state)
    }

    #[test]
    fn tapping_a_destructive_key_refuses_out_loud_instead_of_doing_it() {
        let (profile, state) = destructive_deck();
        let acked = Acknowledged::default();
        let deck = plus_deck(&profile, &state, &acked);

        match deck.key_action(0) {
            SlotAction::Refuse { message } => assert!(
                message.contains("hold"),
                "the refusal has to name the gesture that would work, got {message:?}"
            ),
            other => panic!("a tap must not carry the command, got {other:?}"),
        }
    }

    #[test]
    fn holding_a_destructive_key_is_what_actually_issues_the_command() {
        // The other half of the same bargain: guarding a command is only defensible if there is
        // still a way to mean it.
        let (profile, state) = destructive_deck();
        let acked = Acknowledged::default();
        let deck = plus_deck(&profile, &state, &acked);

        assert_eq!(
            deck.key_long_press_action(0),
            SlotAction::Command(DeckCommand::ClosePane {
                pane_id: "w1:p2".into()
            })
        );
    }

    #[test]
    fn a_destructive_key_says_so_before_it_is_ever_pressed() {
        // A guard nobody can see is a key that appears to be broken the first time it is tapped.
        let (profile, state) = destructive_deck();
        let acked = Acknowledged::default();
        let deck = plus_deck(&profile, &state, &acked);

        match deck.tile(0) {
            Tile::Command { hold, enabled, .. } => {
                assert!(hold, "the tile has to advertise the gesture");
                assert!(enabled, "and there is a pane to close");
            }
            other => panic!("expected a command key, got {other:?}"),
        }
    }

    // --- Pane control -------------------------------------------------------------------------

    /// A deck whose keys are the whole pane cluster, in the order the layout engine lays it out.
    fn pane_deck() -> Profile {
        Profile {
            keys: pane_cluster(),
            dials: vec![],
            presets: Presets::default(),
        }
    }

    #[test]
    fn each_direction_key_asks_herdr_to_move_that_way_and_no_other() {
        // Four keys that differ only in one enum value is exactly the shape a copy-paste mistake
        // hides in, and a "left" key that moved right would be indistinguishable from a layout
        // the user misremembered.
        let profile = pane_deck();
        let state = state_with(vec![]);
        let acked = Acknowledged::default();
        let deck = plus_deck(&profile, &state, &acked);

        for (index, direction) in PaneDirection::ALL.into_iter().enumerate() {
            assert_eq!(
                deck.key_action(index),
                SlotAction::Command(DeckCommand::MovePaneFocus { direction }),
                "key {index} should move {}",
                direction.as_str()
            );
        }
    }

    #[test]
    fn a_direction_key_wears_an_arrow_and_acts_the_instant_it_is_pressed() {
        // Nothing about stepping one pane sideways is ambiguous, so nothing about it should wait —
        // and the arrow is the point: these keys are found by silhouette, not by reading them.
        let profile = pane_deck();
        let state = state_with(vec![]);
        let acked = Acknowledged::default();
        let deck = plus_deck(&profile, &state, &acked);

        match deck.tile(0) {
            Tile::Command { glyph, hold, .. } => {
                assert_eq!(glyph, KeyGlyph::Arrow(PaneDirection::Left));
                assert!(!hold, "a move takes nothing away and must not need a hold");
            }
            other => panic!("expected a command key, got {other:?}"),
        }
        assert_eq!(deck.key_long_press_action(0), SlotAction::None);
    }

    #[test]
    fn the_two_zoom_keys_state_opposite_ends_and_neither_asks_herdr_to_toggle() {
        // Two keys rather than one, because a toggle would be a claim about which way the zoom is
        // going to go and the deck cannot see that. `DeckCommand` cannot express a toggle at all,
        // so this is really a test that the pair are wired the way round they claim to be.
        let profile = pane_deck();
        let state = state_with(vec![]);
        let acked = Acknowledged::default();
        let deck = plus_deck(&profile, &state, &acked);

        assert_eq!(
            deck.key_action(4),
            SlotAction::Command(DeckCommand::ZoomPane { zoom: ZoomMode::On })
        );
        assert_eq!(
            deck.key_action(5),
            SlotAction::Command(DeckCommand::ZoomPane {
                zoom: ZoomMode::Off
            })
        );
    }

    #[test]
    fn splitting_right_and_splitting_down_are_two_different_keys_with_two_different_shapes() {
        let profile = pane_deck();
        let state = state_with(vec![]);
        let acked = Acknowledged::default();
        let deck = plus_deck(&profile, &state, &acked);

        assert_eq!(
            deck.key_action(6),
            SlotAction::Command(DeckCommand::SplitPane {
                direction: SplitDirection::Right
            })
        );
        assert_eq!(
            deck.key_action(7),
            SlotAction::Command(DeckCommand::SplitPane {
                direction: SplitDirection::Down
            })
        );
        assert_ne!(
            deck.tile(6),
            deck.tile(7),
            "two keys that do different things must not draw the same face"
        );
    }

    #[test]
    fn every_key_in_the_pane_cluster_draws_a_shape_no_other_one_draws() {
        // Colour is never the only signal on this deck, and neither is a caption: these keys sit
        // next to each other in a block and are pressed by feel, so each has to be distinguishable
        // by silhouette alone.
        let profile = pane_deck();
        let state = state_with(vec![]);
        let acked = Acknowledged::default();
        let deck = plus_deck(&profile, &state, &acked);

        let mut glyphs: Vec<KeyGlyph> = (0..profile.keys.len())
            .map(|index| match deck.tile(index) {
                Tile::Command { glyph, .. } => glyph,
                other => panic!("key {index} is not a command key: {other:?}"),
            })
            .collect();
        let count = glyphs.len();
        glyphs.dedup();
        assert_eq!(glyphs.len(), count, "two pane keys share a shape");
    }

    #[test]
    fn a_close_key_names_the_pane_herdr_says_is_focused_rather_than_one_from_a_config_file() {
        // Pane ids are renumbered when a pane moves workspaces, so an id written down last month
        // names whatever is second there now. The only id safe to close is the one herdr just gave
        // us for the pane the user is in.
        let profile = Profile {
            keys: vec![KeyBinding::ClosePane],
            dials: vec![],
            presets: Presets::default(),
        };
        let mut state = state_with(vec![]);
        state.focused_pane_id = Some("w3:p7".into());
        let acked = Acknowledged::default();
        let deck = plus_deck(&profile, &state, &acked);

        assert_eq!(
            deck.key_long_press_action(0),
            SlotAction::Command(DeckCommand::ClosePane {
                pane_id: "w3:p7".into()
            })
        );
    }

    #[test]
    fn a_close_key_with_nothing_focused_says_so_instead_of_guessing_a_pane() {
        // The worst possible guess. It refuses on the tap *and* on the hold, and draws dimmed, so
        // there is no gesture that quietly closes something the user never pointed at.
        let profile = Profile {
            keys: vec![KeyBinding::ClosePane],
            dials: vec![],
            presets: Presets::default(),
        };
        let state = state_with(vec![]);
        let acked = Acknowledged::default();
        let deck = plus_deck(&profile, &state, &acked);

        assert!(matches!(deck.key_action(0), SlotAction::Refuse { .. }));
        assert_eq!(deck.key_long_press_action(0), SlotAction::None);
        match deck.tile(0) {
            Tile::Command { enabled, .. } => assert!(!enabled, "the key must look unusable"),
            other => panic!("expected a command key, got {other:?}"),
        }
    }

    #[test]
    fn pane_control_never_costs_an_eight_key_deck_a_single_agent() {
        // Four arrows would be half a Stream Deck +, and the agents are the reason the deck is on
        // the desk at all. Anybody who wants them there can say so in their config.
        for model in [DeckModel::Plus, DeckModel::Mini, DeckModel::Original] {
            let profile = Profile::for_capabilities(&model.capabilities());
            assert!(
                !profile
                    .keys
                    .iter()
                    .any(|k| matches!(k, KeyBinding::Command { .. })),
                "{model:?} gave keys away to pane control"
            );
        }
    }

    #[test]
    fn a_deck_with_keys_to_spare_gets_the_whole_pane_cluster_and_still_shows_more_agents() {
        // Past two dozen keys there are already more agent slots than anyone has agents, so the
        // cluster costs nothing that was being used.
        let profile = Profile::for_capabilities(&DeckModel::Xl.capabilities());
        let commands: Vec<_> = profile
            .keys
            .iter()
            .filter_map(|k| match k {
                KeyBinding::Command { command } => Some(command.name()),
                _ => None,
            })
            .collect();
        assert_eq!(commands.len(), pane_cluster().len());
        assert!(
            profile.dynamic_slots() > commands.len(),
            "the agents must still have the larger share"
        );
    }

    #[test]
    fn no_deck_is_given_a_key_that_closes_things_without_being_asked_for_one() {
        // The guard makes closing a pane safe to *offer*; it does not make it something to put in
        // front of somebody who never went looking for it. This is the whole list of destructive
        // bindings, checked against every deck and against a config that names presets — because
        // the derived layout grew a way to add keys, and that must not be a way to add these.
        let mut config = Config::default();
        config.layouts.insert("dev".into(), LayoutPreset::default());
        for model in [
            DeckModel::Mini,
            DeckModel::Original,
            DeckModel::Plus,
            DeckModel::Xl,
            DeckModel::Pedal,
        ] {
            let profile = Profile::derive(&model.capabilities(), &config);
            for key in &profile.keys {
                let destructive = matches!(
                    key,
                    KeyBinding::ClosePane | KeyBinding::CloseTab | KeyBinding::RemoveWorktree
                ) || matches!(key, KeyBinding::Command { command } if command.is_destructive());
                assert!(
                    !destructive,
                    "{model:?} was handed {key:?}, a destructive key nobody asked for"
                );
            }
        }
    }

    // --- Structure: layouts, worktrees, and the things that make them --------------------------

    /// A config with one named layout, one named workspace and one named tab.
    fn config_with_presets() -> Config {
        let mut config = Config::default();
        config.layouts.insert(
            "dev".into(),
            LayoutPreset {
                label: Some("dev".into()),
                root: LayoutNode {
                    split: Some(SplitDirection::Down),
                    ratio: Some(70),
                    first: Some(Box::default()),
                    second: Some(Box::default()),
                    ..Default::default()
                },
            },
        );
        config.workspaces.insert(
            "notes".into(),
            CreateSpec {
                label: Some("notes".into()),
                cwd: Some("/home/dev/notes".into()),
                ..Default::default()
            },
        );
        config.tabs.insert(
            "logs".into(),
            CreateSpec {
                label: Some("logs".into()),
                ..Default::default()
            },
        );
        config
    }

    fn deck_of(profile: &Profile, state: &DeckState, acked: &Acknowledged) -> SlotAction {
        ResolvedDeck::new(profile, state, Mode::Agents, 0, Selection::default(), acked)
            .key_action(0)
    }

    #[test]
    fn a_layout_key_carries_the_whole_arrangement_so_the_preset_cannot_go_missing_later() {
        // The command holds the tree, not the preset's name. A key that only held the name would
        // have to look it up at the moment it is pressed, and would have to have an answer for
        // "it is not there any more" on the deck rather than in the config file.
        let config = config_with_presets();
        let profile = Profile {
            keys: vec![KeyBinding::Layout {
                preset: "dev".into(),
            }],
            dials: vec![],
            presets: config.presets(),
        };
        let state = state_with(vec![]);
        let acked = Acknowledged::default();

        match deck_of(&profile, &state, &acked) {
            SlotAction::Command(DeckCommand::ApplyLayout { preset, layout }) => {
                assert_eq!(preset, "dev");
                assert_eq!(layout, config.layouts["dev"]);
            }
            other => panic!("expected a layout command, got {other:?}"),
        }
    }

    #[test]
    fn a_layout_key_naming_something_that_is_not_there_refuses_out_loud_and_draws_unusable() {
        // Unreachable through a config file, which is checked at load. Reachable if a herdr-deck
        // upgrade ever drops a preset out from under a running daemon — and a key that silently
        // did nothing then would be indistinguishable from broken hardware.
        let profile = Profile {
            keys: vec![KeyBinding::Layout {
                preset: "gone".into(),
            }],
            dials: vec![],
            presets: Presets::default(),
        };
        let state = state_with(vec![]);
        let acked = Acknowledged::default();

        match deck_of(&profile, &state, &acked) {
            SlotAction::Refuse { message } => assert!(message.contains("gone"), "got {message:?}"),
            other => panic!("expected a refusal, got {other:?}"),
        }
        let deck = plus_deck(&profile, &state, &acked);
        match deck.tile(0) {
            Tile::Command { enabled, .. } => assert!(!enabled, "the key must look unusable"),
            other => panic!("expected a command key, got {other:?}"),
        }
    }

    #[test]
    fn a_deck_with_room_grows_one_key_per_named_layout_and_a_deck_without_grows_none() {
        // A layout nobody can press is a layout nobody meant to write down. But the agents are
        // still why the deck is on the desk, so the same cap that keeps pane control off a small
        // deck applies here too.
        let config = config_with_presets();
        let big = Profile::derive(&DeckModel::Xl.capabilities(), &config);
        let layouts: Vec<_> = big
            .keys
            .iter()
            .filter_map(|k| match k {
                KeyBinding::Layout { preset } => Some(preset.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(layouts, vec!["dev"]);
        assert!(
            big.dynamic_slots() > layouts.len(),
            "the agents must still have the larger share"
        );

        // ...and a deck with no presets configured is exactly the deck it was before.
        let plain = Profile::derive(&DeckModel::Xl.capabilities(), &Config::default());
        assert!(!plain
            .keys
            .iter()
            .any(|k| matches!(k, KeyBinding::Layout { .. })));
    }

    #[test]
    fn a_preset_key_means_the_preset_and_a_bare_one_means_herdrs_own_defaults() {
        // Both are useful: a named workspace opens somewhere specific, an unnamed one opens
        // wherever you already are. Neither needs anything typed.
        let config = config_with_presets();
        let profile = Profile {
            keys: vec![
                KeyBinding::NewWorkspace {
                    preset: Some("notes".into()),
                },
                KeyBinding::NewTab { preset: None },
            ],
            dials: vec![],
            presets: config.presets(),
        };
        let state = state_with(vec![]);
        let acked = Acknowledged::default();
        let deck = plus_deck(&profile, &state, &acked);

        assert_eq!(
            deck.key_action(0),
            SlotAction::Command(DeckCommand::CreateWorkspace {
                preset: Some("notes".into()),
                spec: config.workspaces["notes"].clone(),
            })
        );
        assert_eq!(
            deck.key_action(1),
            SlotAction::Command(DeckCommand::CreateTab {
                preset: None,
                spec: CreateSpec::default(),
            })
        );
    }

    #[test]
    fn a_worktree_key_opens_the_checkout_it_shows_by_path() {
        // By path and not by branch, because a detached checkout has no branch — and a list where
        // some entries work and others do not is worse than one that shows fewer.
        let profile = Profile::for_capabilities(&DeckModel::Plus.capabilities());
        let state = state_with(vec![]).with_worktrees(vec![
            worktree("fix-auth", None),
            worktree("spike", Some("w4")),
        ]);
        let acked = Acknowledged::default();
        let deck = ResolvedDeck::new(
            &profile,
            &state,
            Mode::Worktrees,
            0,
            Selection::default(),
            &acked,
        );

        assert_eq!(
            deck.key_action(0),
            SlotAction::Command(DeckCommand::OpenWorktree {
                path: "/src/.worktrees/api/fix-auth".into()
            })
        );
        // Already open is the same command, not a workspace focus: `worktree.open` is idempotent
        // and focuses what it finds, so one key covers both and there is no state to be wrong.
        assert_eq!(
            deck.key_action(1),
            SlotAction::Command(DeckCommand::OpenWorktree {
                path: "/src/.worktrees/api/spike".into()
            })
        );
    }

    #[test]
    fn a_worktree_tile_says_whether_herdr_already_has_it_and_whether_you_are_in_it() {
        let profile = Profile::for_capabilities(&DeckModel::Plus.capabilities());
        let mut state = state_with(vec![]).with_worktrees(vec![
            worktree("here", Some("w2")),
            worktree("elsewhere", Some("w5")),
            worktree("closed", None),
        ]);
        state.focused_workspace_id = Some("w2".into());
        let acked = Acknowledged::default();
        let deck = ResolvedDeck::new(
            &profile,
            &state,
            Mode::Worktrees,
            0,
            Selection::default(),
            &acked,
        );

        assert_eq!(
            deck.tile(0),
            Tile::Worktree {
                label: "here".into(),
                open: true,
                focused: true
            }
        );
        assert_eq!(
            deck.tile(1),
            Tile::Worktree {
                label: "elsewhere".into(),
                open: true,
                focused: false
            }
        );
        assert_eq!(
            deck.tile(2),
            Tile::Worktree {
                label: "closed".into(),
                open: false,
                focused: false
            }
        );
    }

    #[test]
    fn a_checkout_herdr_would_refuse_to_open_never_reaches_a_key() {
        // Bare and prunable entries answer `worktree_not_found`. Filtering them in the state keeps
        // every count, cursor and key agreeing about how many worktrees there are.
        let state = state_with(vec![]).with_worktrees(vec![
            WorktreeInfo {
                path: "/src/api.git".into(),
                is_bare: true,
                ..Default::default()
            },
            worktree("real", None),
        ]);
        assert_eq!(state.worktrees.len(), 1);
        assert_eq!(state.worktrees[0].label(), "real");
    }

    #[test]
    fn a_remove_key_names_the_workspace_herdr_has_the_worktree_open_as() {
        // Not the path: a workspace id is a herdr id and safe to write to the command log, and it
        // is what `worktree.remove` actually takes.
        let profile = Profile {
            keys: vec![KeyBinding::RemoveWorktree],
            dials: vec![],
            presets: Presets::default(),
        };
        let mut state = state_with(vec![]).with_worktrees(vec![worktree("fix-auth", Some("w3"))]);
        state.focused_workspace_id = Some("w3".into());
        let acked = Acknowledged::default();
        let deck = plus_deck(&profile, &state, &acked);

        // On the hold, because it is destructive — the tap only says so.
        assert_eq!(
            deck.key_long_press_action(0),
            SlotAction::Command(DeckCommand::RemoveWorktree {
                workspace_id: "w3".into()
            })
        );
        assert!(matches!(deck.key_action(0), SlotAction::Refuse { .. }));
    }

    #[test]
    fn a_remove_key_pointed_at_a_repository_rather_than_a_worktree_says_so_before_it_is_pressed() {
        // herdr refuses to remove the source checkout, and the deck already holds the listing that
        // says which one that is. Letting the key look usable and then fail would make the user
        // press it to find out.
        let profile = Profile {
            keys: vec![KeyBinding::RemoveWorktree],
            dials: vec![],
            presets: Presets::default(),
        };
        let mut state = state_with(vec![]).with_worktrees(vec![WorktreeInfo {
            path: "/src/api".into(),
            branch: Some("main".into()),
            is_linked_worktree: false,
            open_workspace_id: Some("w1".into()),
            ..Default::default()
        }]);
        state.focused_workspace_id = Some("w1".into());
        let acked = Acknowledged::default();
        let deck = plus_deck(&profile, &state, &acked);

        assert!(matches!(deck.key_action(0), SlotAction::Refuse { .. }));
        assert_eq!(deck.key_long_press_action(0), SlotAction::None);
        match deck.tile(0) {
            Tile::Command { enabled, .. } => assert!(!enabled, "the key must look unusable"),
            other => panic!("expected a command key, got {other:?}"),
        }
    }

    #[test]
    fn a_close_tab_key_names_the_tab_herdr_says_is_focused_and_only_acts_on_a_hold() {
        let profile = Profile {
            keys: vec![KeyBinding::CloseTab],
            dials: vec![],
            presets: Presets::default(),
        };
        let mut state = state_with(vec![]);
        state.focused_tab_id = Some("w1:t2".into());
        let acked = Acknowledged::default();
        let deck = plus_deck(&profile, &state, &acked);

        assert!(matches!(deck.key_action(0), SlotAction::Refuse { .. }));
        assert_eq!(
            deck.key_long_press_action(0),
            SlotAction::Command(DeckCommand::CloseTab {
                tab_id: "w1:t2".into()
            })
        );
    }

    #[test]
    fn every_structural_key_draws_a_shape_no_other_one_draws() {
        // These sit next to each other in a block and are pressed by feel. Colour is never the
        // only signal on this deck, and neither is a caption.
        let config = config_with_presets();
        let profile = Profile {
            keys: vec![
                KeyBinding::Layout {
                    preset: "dev".into(),
                },
                KeyBinding::NewWorkspace { preset: None },
                KeyBinding::NewTab { preset: None },
                KeyBinding::NewWorktree,
                KeyBinding::CloseTab,
                KeyBinding::RemoveWorktree,
                KeyBinding::ClosePane,
            ],
            dials: vec![],
            presets: config.presets(),
        };
        let state = state_with(vec![]);
        let acked = Acknowledged::default();
        let deck = plus_deck(&profile, &state, &acked);

        let mut glyphs: Vec<KeyGlyph> = (0..profile.keys.len())
            .map(|index| match deck.tile(index) {
                Tile::Command { glyph, .. } => glyph,
                other => panic!("key {index} is not a command key: {other:?}"),
            })
            .collect();
        let count = glyphs.len();
        glyphs.sort_by_key(|g| format!("{g:?}"));
        glyphs.dedup();
        assert_eq!(glyphs.len(), count, "two structural keys share a shape");
    }

    #[test]
    fn an_ordinary_command_key_still_acts_on_the_tap_and_claims_no_hold() {
        // The guard must cost nothing to everything that is not dangerous, or every key on the
        // deck slowly becomes a hold.
        let profile = Profile {
            keys: vec![KeyBinding::Command {
                command: DeckCommand::FocusWorkspace {
                    workspace_id: "w2".into(),
                },
            }],
            dials: vec![],
            presets: Presets::default(),
        };
        let state = state_with(vec![]);
        let acked = Acknowledged::default();
        let deck = plus_deck(&profile, &state, &acked);

        assert_eq!(
            deck.key_action(0),
            SlotAction::Command(DeckCommand::FocusWorkspace {
                workspace_id: "w2".into()
            })
        );
        assert_eq!(deck.key_long_press_action(0), SlotAction::None);
    }

    #[test]
    fn a_dial_press_refuses_a_destructive_command_because_an_encoder_has_no_hold() {
        // The daemon ignores a dial's release, so there is no gesture here that could mean "I am
        // sure". Performing it anyway would put the one unguarded path in the product on the
        // control that is easiest to knock.
        let profile = Profile {
            keys: vec![],
            dials: vec![DialBinding::Scrub {
                target: ScrubTarget::Workspaces,
            }],
            presets: Presets::default(),
        };
        let mut state = state_with(vec![]);
        state.workspaces = vec![WorkspaceInfo {
            workspace_id: "w2".into(),
            ..Default::default()
        }];
        let acked = Acknowledged::default();
        let deck = plus_deck(&profile, &state, &acked);

        // The dial's own binding is harmless, so it still acts...
        assert_eq!(
            deck.dial_press_action(0),
            SlotAction::Command(DeckCommand::FocusWorkspace {
                workspace_id: "w2".into()
            })
        );
        // ...and the guard is what a destructive one would meet.
        assert!(matches!(
            guard_tap(SlotAction::Command(DeckCommand::ClosePane {
                pane_id: "w1:p1".into()
            })),
            SlotAction::Refuse { .. }
        ));
    }

    #[test]
    fn an_acknowledged_agent_stops_counting_towards_the_attention_key() {
        let caps = DeckModel::Plus.capabilities();
        let profile = Profile::for_capabilities(&caps);
        let state = state_with(vec![
            agent("finished", AgentStatus::Done, 2),
            agent("stuck", AgentStatus::Blocked, 1),
        ]);
        let next_key = profile
            .keys
            .iter()
            .position(|k| *k == KeyBinding::NextAttention)
            .unwrap();

        let mut acked = Acknowledged::default();
        acked.acknowledge(state.agent_by_terminal_id("finished").unwrap());
        let deck = plus_deck(&profile, &state, &acked);

        assert_eq!(deck.tile(next_key), Tile::Attention { count: 1 });
        assert_eq!(
            deck.key_action(next_key),
            SlotAction::FocusAgent {
                terminal_id: "stuck".into()
            },
            "the attention key must skip what you already dismissed"
        );
    }

    #[test]
    fn an_acknowledged_agent_keeps_its_key_and_is_drawn_calm() {
        // Dropping it off the deck would be the wrong trade entirely: you dismissed the alarm,
        // not the agent, and you still need to be able to reach it.
        let caps = DeckModel::Plus.capabilities();
        let profile = Profile::for_capabilities(&caps);
        let state = state_with(vec![agent("finished", AgentStatus::Done, 2)]);
        let mut acked = Acknowledged::default();
        acked.acknowledge(state.agent_by_terminal_id("finished").unwrap());
        let deck = plus_deck(&profile, &state, &acked);

        match deck.tile(0) {
            Tile::Agent {
                label,
                status,
                acknowledged,
                ..
            } => {
                assert_eq!(label, "finished");
                assert!(acknowledged, "the tile must draw calm");
                assert_eq!(
                    status,
                    AgentStatus::Done,
                    "and must still carry what herdr actually reports"
                );
            }
            other => panic!("the agent should still be on its key, got {other:?}"),
        }
        assert_eq!(
            deck.key_action(0),
            SlotAction::FocusAgent {
                terminal_id: "finished".into()
            },
            "a short press must still take you there"
        );
    }

    #[test]
    fn an_acknowledged_agent_drops_below_the_ones_still_asking_for_you() {
        let caps = DeckModel::Plus.capabilities();
        let profile = Profile::for_capabilities(&caps);
        let state = state_with(vec![
            agent("dismissed", AgentStatus::Blocked, 9),
            agent("stuck", AgentStatus::Blocked, 8),
        ]);
        let mut acked = Acknowledged::default();
        acked.acknowledge(state.agent_by_terminal_id("dismissed").unwrap());
        let deck = plus_deck(&profile, &state, &acked);

        assert_eq!(
            deck.key_action(0),
            SlotAction::FocusAgent {
                terminal_id: "stuck".into()
            },
            "key 0 belongs to whichever agent is loudest, and it is no longer the dismissed one"
        );
    }

    #[test]
    fn the_attention_dial_skips_acknowledged_agents_the_same_way_the_keys_do() {
        // The dial and the keys read the same queue; letting them disagree would mean the deck
        // told you two different things about the same agent at the same time.
        let caps = DeckModel::Plus.capabilities();
        let profile = Profile::for_capabilities(&caps);
        let state = state_with(vec![
            agent("dismissed", AgentStatus::Blocked, 9),
            agent("stuck", AgentStatus::Blocked, 8),
        ]);
        let mut acked = Acknowledged::default();
        acked.acknowledge(state.agent_by_terminal_id("dismissed").unwrap());
        let deck = plus_deck(&profile, &state, &acked);

        assert_eq!(deck.scrub_len(ScrubTarget::Attention), 1);
        assert_eq!(
            deck.dial_press_action(3),
            SlotAction::FocusAgent {
                terminal_id: "stuck".into()
            }
        );
        let (_, value, _) = deck.dial_feedback(3);
        assert_eq!(value, "stuck");
    }

    // --- How long an agent has been asking ---------------------------------------------------
    //
    // The clock itself is pinned in `state.rs`; these are about the bucket reaching the tile,
    // which is the only place a user can ever see it.

    /// A deck whose single blocked agent has been waiting `waited`.
    fn state_waiting_for(waited: std::time::Duration) -> DeckState {
        let start = std::time::Instant::now();
        let mut clock = crate::state::WaitClock::default();
        let agents = vec![agent("stuck", AgentStatus::Blocked, 1)];
        let mut first = state_with(agents.clone());
        clock.stamp(&mut first, start);
        let mut later = state_with(agents);
        clock.stamp(&mut later, start + waited);
        later
    }

    fn tile_wait(tile: &Tile) -> Option<crate::state::WaitBucket> {
        match tile {
            Tile::Agent { waiting, .. } => *waiting,
            other => panic!("expected an agent tile, got {other:?}"),
        }
    }

    #[test]
    fn an_agent_tile_says_how_long_it_has_been_asking_for_you() {
        let caps = DeckModel::Plus.capabilities();
        let profile = Profile::for_capabilities(&caps);
        let state = state_waiting_for(std::time::Duration::from_secs(360));
        let acked = Acknowledged::default();
        let deck = plus_deck(&profile, &state, &acked);
        assert_eq!(
            tile_wait(&deck.tile(0)),
            Some(crate::state::WaitBucket::Overdue)
        );
    }

    #[test]
    fn a_dismissed_agent_stops_reporting_how_long_it_has_waited() {
        // Dismissing is the user saying they have seen it. Going on to display a number that
        // only grows would be arguing with them about the one thing they just settled.
        let caps = DeckModel::Plus.capabilities();
        let profile = Profile::for_capabilities(&caps);
        let state = state_waiting_for(std::time::Duration::from_secs(360));
        let mut acked = Acknowledged::default();
        acked.acknowledge(state.agent_by_terminal_id("stuck").unwrap());
        let deck = plus_deck(&profile, &state, &acked);

        let tile = deck.tile(0);
        assert_eq!(tile_wait(&tile), None);
        assert!(
            matches!(
                tile,
                Tile::Agent {
                    acknowledged: true,
                    status: AgentStatus::Blocked,
                    ..
                }
            ),
            "and it must still carry the status herdr reports, got {tile:?}"
        );
    }

    #[test]
    fn a_workspace_tile_never_claims_a_wait_because_it_is_a_rollup_of_many_agents() {
        let caps = DeckModel::Plus.capabilities();
        let profile = Profile::for_capabilities(&caps);
        let mut state = state_waiting_for(std::time::Duration::from_secs(360));
        state.workspaces = vec![WorkspaceInfo {
            workspace_id: "w1".into(),
            agent_status: AgentStatus::Blocked,
            ..Default::default()
        }];
        let acked = Acknowledged::default();
        let deck = ResolvedDeck::new(
            &profile,
            &state,
            Mode::Workspaces,
            0,
            Selection::default(),
            &acked,
        );
        assert!(matches!(deck.tile(0), Tile::Workspace { .. }));
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
        let acked = Acknowledged::default();
        let deck = ResolvedDeck::new(
            &profile,
            &state,
            Mode::Agents,
            0,
            Selection::default(),
            &acked,
        );
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
        let acked = Acknowledged::default();
        let deck = ResolvedDeck::new(
            &profile,
            &state,
            Mode::Agents,
            0,
            Selection::default(),
            &acked,
        );
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
        let acked = Acknowledged::default();
        let deck = ResolvedDeck::new(
            &profile,
            &state,
            Mode::Agents,
            0,
            Selection::default(),
            &acked,
        );
        assert_eq!(deck.tile(3), Tile::Empty);
        assert_eq!(deck.key_action(3), SlotAction::None);
    }

    #[test]
    fn when_herdr_is_offline_every_key_says_so_and_nothing_acts() {
        let caps = DeckModel::Plus.capabilities();
        let profile = Profile::for_capabilities(&caps);
        let state = DeckState::offline("herdr socket not found");
        let acked = Acknowledged::default();
        let deck = ResolvedDeck::new(
            &profile,
            &state,
            Mode::Agents,
            0,
            Selection::default(),
            &acked,
        );
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
        let acked = Acknowledged::default();
        let deck = ResolvedDeck::new(
            &profile,
            &state,
            Mode::Workspaces,
            0,
            Selection::default(),
            &acked,
        );
        assert_eq!(
            deck.key_action(0),
            SlotAction::Command(DeckCommand::FocusWorkspace {
                workspace_id: "w2".into()
            })
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
        let acked = Acknowledged::default();
        let deck = ResolvedDeck::new(
            &profile,
            &state,
            Mode::Agents,
            1,
            Selection::default(),
            &acked,
        );
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
        let acked = Acknowledged::default();
        let deck = ResolvedDeck::new(&profile, &state, Mode::Agents, 0, selection, &acked);
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
        let acked = Acknowledged::default();
        let deck = ResolvedDeck::new(&profile, &state, Mode::Agents, 0, selection, &acked);
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
        let acked = Acknowledged::default();
        let deck = ResolvedDeck::new(&profile, &state, Mode::Agents, 0, selection, &acked);
        assert_eq!(
            deck.dial_press_action(2),
            SlotAction::Command(DeckCommand::FocusTab {
                tab_id: "w1:t2".into()
            })
        );
    }

    #[test]
    fn rotating_a_dial_produces_a_scrub_for_its_own_target() {
        let caps = DeckModel::Plus.capabilities();
        let profile = Profile::for_capabilities(&caps);
        let state = state_with(vec![]);
        let acked = Acknowledged::default();
        let deck = ResolvedDeck::new(
            &profile,
            &state,
            Mode::Agents,
            0,
            Selection::default(),
            &acked,
        );
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
            worktrees: 6,
        };
        sel.clamp(ListLengths {
            agents: 2,
            workspaces: 0,
            tabs: 5,
            attention: 1,
            worktrees: 0,
        });
        assert_eq!(sel.agents, 1);
        assert_eq!(sel.workspaces, 0);
        assert_eq!(sel.tabs, 2);
        assert_eq!(sel.attention, 0);
        assert_eq!(sel.worktrees, 0);
    }

    #[test]
    fn touchstrip_feedback_names_the_target_and_the_selection() {
        let caps = DeckModel::Plus.capabilities();
        let profile = Profile::for_capabilities(&caps);
        let mut blocked = agent("refactor", AgentStatus::Blocked, 1);
        blocked.name = Some("refactor".into());
        let state = state_with(vec![blocked]);
        let acked = Acknowledged::default();
        let deck = ResolvedDeck::new(
            &profile,
            &state,
            Mode::Agents,
            0,
            Selection::default(),
            &acked,
        );
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
        let acked = Acknowledged::default();
        let deck = ResolvedDeck::new(
            &profile,
            &state,
            Mode::Agents,
            0,
            Selection::default(),
            &acked,
        );
        let (_, value, _) = deck.dial_feedback(3);
        assert_eq!(value, "all clear");
    }

    #[test]
    fn the_mode_key_cycles_agents_and_workspaces_when_there_are_no_worktrees() {
        // Which is most sessions. A third stop showing an empty grid would cost everybody who
        // does not use worktrees an extra press to get back to their agents.
        let profile = Profile::for_capabilities(&DeckModel::Plus.capabilities());
        let state = state_with(vec![]);
        let acked = Acknowledged::default();
        let at = |mode| {
            ResolvedDeck::new(&profile, &state, mode, 0, Selection::default(), &acked).next_mode()
        };
        assert_eq!(at(Mode::Agents), Mode::Workspaces);
        assert_eq!(at(Mode::Workspaces), Mode::Agents);
    }

    #[test]
    fn a_session_with_worktrees_gets_a_third_stop_on_the_same_key() {
        // No new key, no configuration: the deck that has worktrees grows a place to see them and
        // the deck that does not is unchanged.
        let profile = Profile::for_capabilities(&DeckModel::Plus.capabilities());
        let state = state_with(vec![]).with_worktrees(vec![worktree("fix-auth", None)]);
        let acked = Acknowledged::default();
        let at = |mode| {
            ResolvedDeck::new(&profile, &state, mode, 0, Selection::default(), &acked).next_mode()
        };
        assert_eq!(at(Mode::Agents), Mode::Workspaces);
        assert_eq!(at(Mode::Workspaces), Mode::Worktrees);
        assert_eq!(at(Mode::Worktrees), Mode::Agents);
    }

    #[test]
    fn leaving_worktree_mode_always_works_even_once_the_list_has_emptied() {
        // The list is refreshed underneath the deck, so it can empty while somebody is looking at
        // it. Leaving has to be unconditional or the mode key becomes a trap.
        let profile = Profile::for_capabilities(&DeckModel::Plus.capabilities());
        let state = state_with(vec![]);
        let acked = Acknowledged::default();
        let deck = ResolvedDeck::new(
            &profile,
            &state,
            Mode::Worktrees,
            0,
            Selection::default(),
            &acked,
        );
        assert_eq!(deck.next_mode(), Mode::Agents);
    }

    // --- The project footer ------------------------------------------------------------------
    //
    // The bottom row is the only thing on an agent tile that says *where* the work is, so these
    // pin the whole fallback chain rather than just its happy path.

    /// The footer the deck would draw for the state's single agent.
    fn project_footer(state: &DeckState) -> Option<String> {
        let agent = state.agents.first().expect("state has an agent");
        match agent_tile(state, agent, &Acknowledged::default()) {
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
