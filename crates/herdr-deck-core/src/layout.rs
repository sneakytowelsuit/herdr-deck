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
use herdr_deck_herdr::wire::{
    AgentInfo, CreateSpec, PaneDirection, SplitDirection, WorktreeInfo, ZoomMode,
};
use serde::{Deserialize, Serialize};

/// One page of the deck.
///
/// # Why pages, and why these
///
/// The deck started with two lists and a key that flipped between them. It now drives fifteen or
/// so herdr commands, and eight keys cannot hold them all at once — so the keys that used to show
/// "the current list" show *the current page*, and the page key walks the cycle.
///
/// A page is a **family of related controls**, not an arbitrary bank. Three of them are lists of
/// things herdr holds ([`Page::Agents`], [`Page::Spaces`], [`Page::Trees`]) and two are fixed sets
/// of commands ([`Page::Panes`], [`Page::Make`]). The split matters: a list page changes under you
/// as agents come and go, and a command page is the same six keys every time you reach it, which
/// is what lets it be pressed by feel.
///
/// The declaration order is the cycle order, and it is not arbitrary either: the pages that answer
/// *where is it* come before the pages that answer *do this*, because the first question is the
/// one this product exists for.
///
/// [`Page::Agents`] is first, is the default, and — uniquely — is never dropped from the cycle,
/// however empty it gets. Every other page can be skipped: an empty list has nothing to show, and
/// a page whose every entry is already pinned to keys of its own has nothing left to say (see
/// [`ResolvedDeck::pages`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Page {
    /// Every agent, in attention order. The reason the deck is on the desk.
    #[default]
    Agents,
    /// herdr's workspaces.
    Spaces,
    /// The git worktrees of the repository the focused workspace belongs to.
    Trees,
    /// Moving around, and between, the panes of the tab you are already in.
    Panes,
    /// The commands that bring something into being: splits, tabs, workspaces, worktrees, and any
    /// layout named in config.
    Make,
}

impl Page {
    /// Every page, in the order the page key walks them.
    pub const ALL: [Page; 5] = [
        Page::Agents,
        Page::Spaces,
        Page::Trees,
        Page::Panes,
        Page::Make,
    ];

    /// What the page key calls this page.
    ///
    /// One word, because it shares a 72px key with the name of the page after it.
    pub fn label(self) -> &'static str {
        match self {
            Page::Agents => "agents",
            Page::Spaces => "spaces",
            Page::Trees => "trees",
            Page::Panes => "panes",
            Page::Make => "make",
        }
    }

    /// The commands this page offers, in the order they are shown.
    ///
    /// Answerable without any state, which is the point: the layout engine has to know how long a
    /// page is *before* there is a herdr to ask, because that is what decides whether a deck has
    /// room to show the whole of it at once.
    ///
    /// Both command pages are ordered by how often a hand reaches for them, and neither contains
    /// anything destructive. Closing a pane, closing a tab and removing a worktree exist, are
    /// guarded by a hold, and have to be asked for by name in a config file; a deck should not
    /// offer to destroy work to somebody who never went looking for that.
    pub fn commands(self, presets: &Presets) -> Vec<DeckCommand> {
        match self {
            // Directions first — they are pressed in sequence and by feel — then the pair that
            // changes what fills the screen. Exactly six, which is what a Stream Deck + has room for
            // once the attention key and the page key have taken theirs.
            Page::Panes => {
                let mut commands: Vec<DeckCommand> = PaneDirection::ALL
                    .into_iter()
                    .map(|direction| DeckCommand::MovePaneFocus { direction })
                    .collect();
                commands.extend(
                    ZoomMode::ALL
                        .into_iter()
                        .map(|zoom| DeckCommand::ZoomPane { zoom }),
                );
                commands
            }
            // Splitting a pane belongs here rather than with the arrows: it does not move you around
            // an arrangement, it makes a new shell. Grouping it with the other four ways of starting
            // something also happens to leave the panes page at exactly the size a small deck holds.
            Page::Make => {
                let mut commands: Vec<DeckCommand> = SplitDirection::ALL
                    .into_iter()
                    .map(|direction| DeckCommand::SplitPane { direction })
                    .collect();
                commands.push(DeckCommand::CreateTab {
                    preset: None,
                    spec: CreateSpec::default(),
                });
                commands.push(DeckCommand::CreateWorkspace {
                    preset: None,
                    spec: CreateSpec::default(),
                });
                commands.push(DeckCommand::CreateWorktree);
                // Named layouts land last and in the order the config's map holds them, which is
                // alphabetical — so the same config always produces the same deck, and a key does not
                // move under a thumb because somebody reordered a table.
                commands.extend(presets.layouts.iter().map(|(preset, layout)| {
                    DeckCommand::ApplyLayout {
                        preset: preset.clone(),
                        layout: layout.clone(),
                    }
                }));
                commands
            }
            Page::Agents | Page::Spaces | Page::Trees => vec![],
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
    /// The Nth entry of a page. Contents change as agents come and go — this is what makes the
    /// deck useful with zero configuration.
    ///
    /// `page` is the whole of the page system on the key side. Left out, the key follows whichever
    /// page the deck is on, which is what every key on a small deck does. Named, the key shows
    /// that page whatever the deck is on — which is how a deck with keys to spare stops paging
    /// between control families and simply shows two of them at once.
    ///
    /// A pinned key is not paged: it shows entry `rank` of its page and nothing else. That is the
    /// point of pinning — a key that moved when you paged the *other* half of the deck would be
    /// worse than no key at all.
    Dynamic {
        rank: usize,
        #[serde(default)]
        page: Option<Page>,
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
    /// Come home to the agents page — and, if anything is asking for you, go to it.
    ///
    /// The one key that is on every page and means the same thing on all of them. It carries the
    /// whole of the promise this product makes: wherever you have wandered on the deck, one press
    /// puts you in front of the agent that needs you, and leaves the deck showing the agents. That
    /// is why the count lives on this key rather than on the page key — the thing that tells you
    /// something is wrong should be the thing that fixes it.
    ///
    /// Named `attention` and aliased from the older `next_attention`, which described only half of
    /// what it now does; a config written against the old name still loads.
    #[serde(alias = "next_attention")]
    Attention,
    /// Walk to the next page — agents, spaces, trees, panes, make.
    ///
    /// Aliased from `mode_toggle`, which is what it was called when there were two pages and the
    /// word "toggle" was still true.
    #[serde(alias = "mode_toggle")]
    PageCycle,
    /// Move the window the deck shows over a page that is longer than the keys it has.
    ///
    /// Aliased from `page_prev`/`page_next`, which is what these were called before a *page* meant
    /// a family of controls and this became a screen within one.
    #[serde(alias = "page_prev")]
    ScreenPrev,
    #[serde(alias = "page_next")]
    ScreenNext,
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
    /// Come home to the agents page, and take the agent that wants you most with it.
    ///
    /// One action rather than two because it is one press and one intention. The terminal id is
    /// resolved here, at the moment the key goes down, and is `None` when nothing is asking — in
    /// which case the press is purely a way home.
    Attention {
        terminal_id: Option<String>,
    },
    /// Walk the page cycle one stop.
    NextPage,
    /// Move the window over a page too long for the keys showing it.
    ChangeScreen {
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

    /// The layout this hardware would actually get from this config.
    ///
    /// A hand-written layout replaces the derived one entirely, but is never allowed to be shorter
    /// than the hardware — unbound trailing keys are explicitly empty rather than out of range.
    ///
    /// Lives here rather than in the daemon so that everything answering "what will my deck look
    /// like" gives the same answer: the running session, `herdr-deck layout`, and a dry run all go
    /// through this.
    pub fn for_config(caps: &DeckCapabilities, config: &Config) -> Self {
        match &config.layout {
            Some(written) if !written.keys.is_empty() => {
                let mut keys = written.keys.clone();
                keys.resize(caps.key_count(), KeyBinding::Empty);
                let mut dials = written.dials.clone();
                dials.resize(caps.dials as usize, DialBinding::Unused);
                Self {
                    keys,
                    dials,
                    presets: config.presets(),
                }
            }
            _ => Self::derive(caps, config),
        }
    }

    /// Build the default layout for this hardware and this config's presets, ignoring any
    /// hand-written one.
    ///
    /// The presets are an input to the *layout* and not only to the keys: they lengthen the
    /// [`Page::Make`] page, which is what decides whether a deck has room to show that page
    /// permanently or has to reach it through the cycle.
    pub fn derive(caps: &DeckCapabilities, config: &Config) -> Self {
        let presets = config.presets();
        let dials = default_dials(caps.dials);
        let keys = default_keys(caps, !dials.is_empty(), &presets);
        Self {
            keys,
            dials,
            presets,
        }
    }

    /// How many keys follow whichever page the deck is on.
    pub fn dynamic_slots(&self) -> usize {
        self.keys
            .iter()
            .filter(|k| matches!(k, KeyBinding::Dynamic { page: None, .. }))
            .count()
    }

    /// How many keys show `page` whatever page the deck is on.
    pub fn pinned_slots(&self, page: Page) -> usize {
        self.keys
            .iter()
            .filter(|k| matches!(k, KeyBinding::Dynamic { page: Some(p), .. } if *p == page))
            .count()
    }

    pub fn has_paging(&self) -> bool {
        self.keys
            .iter()
            .any(|k| matches!(k, KeyBinding::ScreenPrev | KeyBinding::ScreenNext))
    }
}

/// How many keys a deck needs before whole pages get keys of their own.
///
/// Chosen as the point where the agent slots stop being the scarce thing: below it a key spent on
/// a pane arrow is a key taken off an agent, and above it the deck already shows more agents at
/// once than anybody runs. A deck this size stops *paging* between control families and simply
/// shows two of them, which is the whole difference a big deck should buy you.
const PINNED_PAGE_MIN_KEYS: usize = 24;

/// The pages a deck with room gets permanently, in the order they are laid down.
///
/// Only the two command pages. The list pages are deliberately absent: pinning half of a list that
/// grows and shrinks would give you a row of keys whose meaning depends on how many agents happen
/// to exist, which is the opposite of a key you can press by feel.
const PINNED_PAGES: [Page; 2] = [Page::Panes, Page::Make];

/// The first `count` keys of one page, pinned so they are on the deck whatever page it is on.
fn pinned_keys(page: Page, count: usize) -> impl Iterator<Item = KeyBinding> {
    (0..count).map(move |rank| KeyBinding::Dynamic {
        rank,
        page: Some(page),
    })
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

fn default_keys(caps: &DeckCapabilities, has_dials: bool, presets: &Presets) -> Vec<KeyBinding> {
    let total = caps.key_count();
    if total == 0 {
        return vec![];
    }

    // A Pedal has no screen: give it the three actions that make sense blind.
    if !caps.has_display() {
        let mut keys = vec![KeyBinding::Attention];
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

    // The two keys that must survive whatever else has to go. The attention key is the way home
    // from every page and the only place a blocked agent is visible from one; the page key is the
    // only way to anywhere else. A deck without both is a deck you can get stranded on.
    let mut home: Vec<KeyBinding> = Vec::new();
    if total >= 4 {
        home.push(KeyBinding::Attention);
        home.push(KeyBinding::PageCycle);
    }
    // Without dials, a deck needs a way to reach a page longer than its keys. With dials, the
    // dials scrub the lists and a page that overruns grows a "more" key of its own instead — see
    // [`ResolvedDeck::needs_more_key`].
    let mut paging: Vec<KeyBinding> = Vec::new();
    if !has_dials && total >= 8 {
        paging.push(KeyBinding::ScreenPrev);
        paging.push(KeyBinding::ScreenNext);
    }

    // Never let fixed controls crowd out the agents they are meant to navigate.
    let budget = total.saturating_sub(1).min(total / 2);
    let mut room = budget.saturating_sub(home.len() + paging.len());

    // The control block, in the order it is laid out left to right after the agent keys. On an
    // eight-wide deck this lands as two whole rows: pane control and the paging pair, then the
    // make page and the two keys that are on every page.
    let mut controls: Vec<KeyBinding> = Vec::new();
    let pinned = total >= PINNED_PAGE_MIN_KEYS;
    if pinned {
        let shown = PINNED_PAGES[0].commands(presets).len().min(room);
        controls.extend(pinned_keys(PINNED_PAGES[0], shown));
        room -= shown;
    }
    controls.extend(paging);
    if pinned {
        let shown = PINNED_PAGES[1].commands(presets).len().min(room);
        controls.extend(pinned_keys(PINNED_PAGES[1], shown));
    }
    // A control block that stopped part-way along a row would have to be found by counting rather
    // than by feel, so on a deck big enough to have given whole rows away, the block is padded out
    // to a whole number of them. The dark key this can leave is where the first named layout goes.
    if pinned && caps.columns > 1 {
        let columns = caps.columns as usize;
        let short = (controls.len() + home.len()) % columns;
        if short != 0 {
            controls.resize(controls.len() + columns - short, KeyBinding::Empty);
        }
    }
    controls.extend(home);

    let dynamic = total.saturating_sub(controls.len());
    let mut keys: Vec<KeyBinding> = (0..dynamic)
        .map(|rank| KeyBinding::Dynamic { rank, page: None })
        .collect();
    keys.extend(controls);
    keys
}

/// Resolves a profile against live state to produce what to draw and what to do.
#[derive(Debug, Clone)]
pub struct ResolvedDeck<'a> {
    profile: &'a Profile,
    state: &'a DeckState,
    page: Page,
    screen: usize,
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
        page: Page,
        screen: usize,
        selection: Selection,
        acked: &'a Acknowledged,
    ) -> Self {
        Self {
            profile,
            state,
            page,
            screen,
            selection,
            acked,
            order: state.attention_order_with(acked),
        }
    }

    /// The index into the current page that a following key of this rank refers to.
    fn list_index(&self, rank: usize) -> usize {
        self.screen * self.visible_slots().max(1) + rank
    }

    /// How many entries a page has right now.
    pub fn page_len(&self, page: Page) -> usize {
        match page {
            Page::Agents => self.order.len(),
            Page::Spaces => self.state.workspaces.len(),
            Page::Trees => self.state.worktrees.len(),
            Page::Panes | Page::Make => page.commands(&self.profile.presets).len(),
        }
    }

    /// Whether the last following key has to become a "more" key to reach the rest of this page.
    ///
    /// The alternative is a deck that quietly shows six of nine things, which is exactly the
    /// silence this project refuses everywhere else. A deck with paging keys already has an
    /// answer and keeps all its slots; a deck without one — the Stream Deck +, whose dials scrub
    /// the *lists* but know nothing about a page of commands — spends a slot, and only on the page
    /// that needs it.
    ///
    /// A deck down to its last following key is the one case where this does not apply: spending
    /// that key would leave a deck showing nothing at all but a way to see nothing else. Only a
    /// hand-written layout can get there, and it is a layout that had already made its choice.
    fn needs_more_key(&self) -> bool {
        !self.profile.has_paging()
            && self.profile.dynamic_slots() > 1
            && self.page_len(self.page) > self.profile.dynamic_slots()
    }

    /// Following keys actually showing entries, once any "more" key has taken its slot.
    fn visible_slots(&self) -> usize {
        self.profile.dynamic_slots() - usize::from(self.needs_more_key())
    }

    /// Is this following key the one carrying the rest of the page?
    fn is_more_slot(&self, rank: usize) -> bool {
        self.needs_more_key() && rank + 1 >= self.profile.dynamic_slots()
    }

    /// The agent list index a dynamic key stands for, or `None` when it stands for no agent —
    /// because it is the "more" key, and a key that pages the list is not a key about an agent.
    fn agent_index(&self, rank: usize, page: Option<Page>) -> Option<usize> {
        match page {
            Some(_) => Some(rank),
            None if self.is_more_slot(rank) => None,
            None => Some(self.list_index(rank)),
        }
    }

    /// Is every entry of `page` already on a key of its own?
    ///
    /// A deck big enough to show the whole of a page permanently has nothing to gain from a cycle
    /// stop that would repaint the same six commands somewhere else. A page only *partly* pinned
    /// stays in the cycle, because the entries that did not fit have to be reachable somehow.
    fn shown_elsewhere(&self, page: Page) -> bool {
        let pinned = self.profile.pinned_slots(page);
        pinned > 0 && pinned >= self.page_len(page)
    }

    /// The pages this deck's page key actually stops at, in order.
    ///
    /// Three ways a page drops out, and the agents page is exempt from all of them: it is what the
    /// deck is for, and a cycle that could not reach it would be a bug wearing a rule's clothes.
    pub fn pages(&self) -> Vec<Page> {
        Page::ALL
            .into_iter()
            .filter(|page| {
                *page == Page::Agents || (self.page_len(*page) > 0 && !self.shown_elsewhere(*page))
            })
            .collect()
    }

    /// Where the page key goes next.
    pub fn next_page(&self) -> Page {
        let pages = self.pages();
        match pages.iter().position(|page| *page == self.page) {
            Some(index) => pages[(index + 1) % pages.len()],
            // The page we are on has stopped being offered — its list emptied underneath us, or
            // the hardware started showing it somewhere else. Home is the right answer to "where
            // next" from nowhere, and it is what stops an emptying list ever trapping the key.
            None => Page::Agents,
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

    /// How many screens the current page takes on this hardware.
    pub fn screen_count(&self) -> usize {
        let per_screen = self.visible_slots().max(1);
        self.page_len(self.page).div_ceil(per_screen).max(1)
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
            KeyBinding::Dynamic {
                rank,
                page: Some(page),
            } => self.entry_tile(*page, *rank),
            KeyBinding::Dynamic { rank, page: None } if self.is_more_slot(*rank) => Tile::Label {
                label: format!("more {}/{}", self.screen + 1, self.screen_count()),
                active: true,
            },
            KeyBinding::Dynamic { rank, page: None } => {
                self.entry_tile(self.page, self.list_index(*rank))
            }
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
            KeyBinding::Attention => Tile::Attention {
                count: self.attention_count(),
                // The key is also the way back, and on a page that is not the agents page it has
                // to say so — a key whose second job is invisible is a key nobody presses for it.
                away: self.page != Page::Agents || self.screen > 0,
            },
            // Where you are, in the size it is read at, and where the next press goes underneath
            // it. Naming only the destination — which is what this key used to do — answers
            // "what happens if I press this" and leaves "which page am I on" to be worked out by
            // counting presses, on a deck whose whole point is not making you count anything.
            KeyBinding::PageCycle => {
                let next = self.next_page();
                Tile::Page {
                    current: self.page.label().to_string(),
                    next: (next != self.page).then(|| next.label().to_string()),
                    screen: (self.screen_count() > 1)
                        .then(|| (self.screen + 1, self.screen_count())),
                }
            }
            KeyBinding::ScreenPrev => Tile::Label {
                label: "◀ more".to_string(),
                active: self.screen > 0,
            },
            KeyBinding::ScreenNext => Tile::Label {
                label: "more ▶".to_string(),
                active: self.screen + 1 < self.screen_count(),
            },
            KeyBinding::Scrub { target, delta } => Tile::Label {
                label: format!("{} {}", if *delta < 0 { "◀" } else { "▶" }, target.label()),
                active: true,
            },
            KeyBinding::Empty => Tile::Empty,
        }
    }

    /// What the `index`th entry of `page` draws.
    fn entry_tile(&self, page: Page, index: usize) -> Tile {
        match page {
            Page::Agents => self
                .agent_at(index)
                .map(|agent| agent_tile(self.state, agent, self.acked))
                .unwrap_or(Tile::Empty),
            Page::Spaces => self
                .state
                .workspaces
                .get(index)
                .map(workspace_tile)
                .unwrap_or(Tile::Empty),
            Page::Trees => self
                .state
                .worktrees
                .get(index)
                .map(|tree| worktree_tile(self.state, tree))
                .unwrap_or(Tile::Empty),
            Page::Panes | Page::Make => page
                .commands(&self.profile.presets)
                .get(index)
                .map(|command| command_tile(command, true))
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
            KeyBinding::Dynamic {
                rank,
                page: Some(page),
            } => self.entry_action(*page, *rank),
            KeyBinding::Dynamic { rank, page: None } if self.is_more_slot(*rank) => {
                SlotAction::ChangeScreen { delta: 1 }
            }
            KeyBinding::Dynamic { rank, page: None } => {
                self.entry_action(self.page, self.list_index(*rank))
            }
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
            KeyBinding::Attention => SlotAction::Attention {
                terminal_id: self
                    .needing_attention()
                    .next()
                    .map(|agent| agent.terminal_id.clone()),
            },
            KeyBinding::PageCycle => SlotAction::NextPage,
            KeyBinding::ScreenPrev => SlotAction::ChangeScreen { delta: -1 },
            KeyBinding::ScreenNext => SlotAction::ChangeScreen { delta: 1 },
            KeyBinding::Scrub { target, delta } => SlotAction::Scrub {
                target: *target,
                delta: *delta,
            },
            KeyBinding::Empty => SlotAction::None,
        }
    }

    /// What pressing the `index`th entry of `page` does.
    fn entry_action(&self, page: Page, index: usize) -> SlotAction {
        match page {
            Page::Agents => self
                .agent_at(index)
                .map(|a| SlotAction::FocusAgent {
                    terminal_id: a.terminal_id.clone(),
                })
                .unwrap_or(SlotAction::None),
            Page::Spaces => self
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
            Page::Trees => self
                .state
                .worktrees
                .get(index)
                .map(|tree| {
                    SlotAction::Command(DeckCommand::OpenWorktree {
                        path: tree.path.clone(),
                    })
                })
                .unwrap_or(SlotAction::None),
            Page::Panes | Page::Make => page
                .commands(&self.profile.presets)
                .get(index)
                .cloned()
                .map(SlotAction::Command)
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
            Some(KeyBinding::Dynamic { rank, page })
                if page.unwrap_or(self.page) == Page::Agents =>
            {
                match self.agent_index(*rank, *page) {
                    Some(index) => self
                        .agent_at(index)
                        .map(acknowledge)
                        .unwrap_or(SlotAction::None),
                    None => SlotAction::None,
                }
            }
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
                .unwrap_or_else(|| ("no agents".to_string(), None)),
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
                .unwrap_or_else(|| ("no spaces".to_string(), None)),
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
                .unwrap_or_else(|| ("no tabs".to_string(), None)),
            // No status: a worktree is a checkout, not something that can be blocked. Handing the
            // strip a colour here would be inventing a state herdr never reported.
            ScrubTarget::Worktrees => self
                .state
                .worktrees
                .get(cursor)
                .map(|tree| (tree.label().to_string(), None))
                .unwrap_or_else(|| ("no worktrees".to_string(), None)),
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
        None => Tile::Label {
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
        assert_eq!(profile.keys[0], KeyBinding::Attention);
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
            Page::Agents,
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
        ResolvedDeck::new(profile, state, Page::Agents, 0, Selection::default(), acked)
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
            KeyBinding::Attention,
            KeyBinding::PageCycle,
            KeyBinding::ScreenPrev,
            KeyBinding::ScreenNext,
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
            Page::Spaces,
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

    /// A deck whose keys are every pane command, in the order the pages lay them out: the panes
    /// page entire, then the two splits that live on the make page because they make something.
    fn pane_deck() -> Profile {
        let mut commands = Page::Panes.commands(&Presets::default());
        commands.extend(
            SplitDirection::ALL
                .into_iter()
                .map(|direction| DeckCommand::SplitPane { direction }),
        );
        Profile {
            keys: commands
                .into_iter()
                .map(|command| KeyBinding::Command { command })
                .collect(),
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
        // the desk at all. On a small deck pane control lives one press away, on a page, and every
        // key the deck has is still an agent when it is showing agents.
        for model in [DeckModel::Plus, DeckModel::Mini, DeckModel::Original] {
            let caps = model.capabilities();
            let profile = Profile::for_capabilities(&caps);
            assert_eq!(
                profile.pinned_slots(Page::Panes),
                0,
                "{model:?} gave keys away to pane control"
            );
            assert_eq!(
                profile.pinned_slots(Page::Make),
                0,
                "{model:?} gave keys away to the make page"
            );
        }
    }

    #[test]
    fn a_deck_with_keys_to_spare_shows_the_command_pages_instead_of_paging_to_them() {
        // Past two dozen keys there are already more agent slots than anyone has agents, so whole
        // pages cost nothing that was being used — and a page permanently on the deck is a page
        // the user never has to walk the cycle to reach.
        let caps = DeckModel::Xl.capabilities();
        let profile = Profile::for_capabilities(&caps);
        let presets = Presets::default();

        assert_eq!(
            profile.pinned_slots(Page::Panes),
            Page::Panes.commands(&presets).len(),
            "the whole panes page has to be there, or a key would be unreachable"
        );
        assert_eq!(
            profile.pinned_slots(Page::Make),
            Page::Make.commands(&presets).len()
        );
        assert!(
            profile.dynamic_slots() > profile.pinned_slots(Page::Panes),
            "the agents must still have the larger share"
        );
        // Two whole rows for the agents, two for the controls: a block found by feel, not counted.
        assert_eq!(profile.dynamic_slots() % caps.columns as usize, 0);
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
        ResolvedDeck::new(profile, state, Page::Agents, 0, Selection::default(), acked)
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
    fn a_named_layout_joins_the_make_page_rather_than_taking_a_key_off_the_agents() {
        // A layout nobody can press is a layout nobody meant to write down — but it reaching a key
        // must not be at the agents' expense, and ten presets must not be able to swallow a deck.
        // Putting them on a page rather than in the reserved block is what makes both true: the
        // page grows, the number of keys does not, and a deck too small to show it all pages.
        let config = config_with_presets();
        let commands = Page::Make.commands(&config.presets());
        assert!(
            commands.iter().any(|command| matches!(
                command,
                DeckCommand::ApplyLayout { preset, .. } if preset == "dev"
            )),
            "the dev layout has to be somewhere a key can reach it: {commands:?}"
        );

        // The Plus has six slots and no paging keys, so a preset is exactly what tips the make
        // page over into needing a "more" key — and the deck says so rather than dropping one.
        let plus = Profile::derive(&DeckModel::Plus.capabilities(), &config);
        assert_eq!(
            plus.dynamic_slots(),
            6,
            "the agents keep every key they had"
        );

        // A deck with room shows the whole page, preset and all, without a cycle stop for it.
        let big = Profile::derive(&DeckModel::Xl.capabilities(), &config);
        assert_eq!(big.pinned_slots(Page::Make), commands.len());
        assert!(
            big.dynamic_slots() > big.pinned_slots(Page::Make),
            "the agents must still have the larger share"
        );
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
            Page::Trees,
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
            Page::Trees,
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
            .position(|k| *k == KeyBinding::Attention)
            .unwrap();

        let mut acked = Acknowledged::default();
        acked.acknowledge(state.agent_by_terminal_id("finished").unwrap());
        let deck = plus_deck(&profile, &state, &acked);

        assert_eq!(
            deck.tile(next_key),
            Tile::Attention {
                count: 1,
                away: false
            }
        );
        assert_eq!(
            deck.key_action(next_key),
            SlotAction::Attention {
                terminal_id: Some("stuck".into())
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
            Page::Spaces,
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
            Page::Agents,
            0,
            Selection::default(),
            &acked,
        );
        let next_key = profile
            .keys
            .iter()
            .position(|k| *k == KeyBinding::Attention)
            .expect("profile has a next-attention key");
        assert_eq!(
            deck.key_action(next_key),
            SlotAction::Attention {
                terminal_id: Some("fresh_block".into())
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
            Page::Agents,
            0,
            Selection::default(),
            &acked,
        );
        let next_key = profile
            .keys
            .iter()
            .position(|k| *k == KeyBinding::Attention)
            .unwrap();
        // Still an action, and still worth pressing: with nothing asking it is purely the way
        // home. Reporting `None` would make the key inert on the page where it matters most.
        assert_eq!(
            deck.key_action(next_key),
            SlotAction::Attention { terminal_id: None }
        );
        assert_eq!(
            deck.tile(next_key),
            Tile::Attention {
                count: 0,
                away: false
            }
        );
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
            Page::Agents,
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
            Page::Agents,
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
            Page::Spaces,
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
        // A Plus has six following keys and no paging keys, so ten agents cost the last of them:
        // five agents and a key that says there are more. Screen 1 therefore starts at index 5.
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
            Page::Agents,
            1,
            Selection::default(),
            &acked,
        );
        assert_eq!(
            deck.key_action(0),
            SlotAction::FocusAgent {
                terminal_id: "a05".into()
            }
        );
        assert_eq!(deck.screen_count(), 2);
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
        let deck = ResolvedDeck::new(&profile, &state, Page::Agents, 0, selection, &acked);
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
        let deck = ResolvedDeck::new(&profile, &state, Page::Agents, 0, selection, &acked);
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
        let deck = ResolvedDeck::new(&profile, &state, Page::Agents, 0, selection, &acked);
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
            Page::Agents,
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
            Page::Agents,
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
            Page::Agents,
            0,
            Selection::default(),
            &acked,
        );
        let (_, value, _) = deck.dial_feedback(3);
        assert_eq!(value, "all clear");
    }

    // --- Pages ----------------------------------------------------------------------------------

    /// A session as most people have one: an agent, the workspace it lives in, no worktrees.
    fn ordinary_state() -> DeckState {
        DeckState::from_snapshot(SessionSnapshot {
            agents: vec![agent("a1", AgentStatus::Blocked, 1)],
            workspaces: vec![WorkspaceInfo {
                workspace_id: "w1".into(),
                ..Default::default()
            }],
            ..Default::default()
        })
    }

    /// Every stop the page key makes, starting from the agents page and going all the way round.
    fn cycle(profile: &Profile, state: &DeckState) -> Vec<Page> {
        let acked = Acknowledged::default();
        let mut seen = vec![Page::Agents];
        let mut page = Page::Agents;
        // Bounded rather than "until it repeats", so a cycle that never came home fails here
        // instead of hanging the suite.
        for _ in 0..Page::ALL.len() + 1 {
            page = ResolvedDeck::new(profile, state, page, 0, Selection::default(), &acked)
                .next_page();
            if page == Page::Agents {
                return seen;
            }
            seen.push(page);
        }
        panic!("the page cycle never came back to the agents: {seen:?}");
    }

    #[test]
    fn the_page_key_walks_the_lists_first_and_the_command_pages_after_them() {
        // "Where is it" before "do this": the first question is the one this product exists for,
        // and it should not be behind the second.
        let profile = Profile::for_capabilities(&DeckModel::Plus.capabilities());
        assert_eq!(
            cycle(&profile, &ordinary_state()),
            vec![Page::Agents, Page::Spaces, Page::Panes, Page::Make]
        );
    }

    #[test]
    fn a_session_with_worktrees_gets_a_stop_for_them_and_a_session_without_never_sees_one() {
        // No new key, no configuration: the deck that has worktrees grows a place to see them and
        // the deck that does not is unchanged. A stop showing an empty grid would cost everybody
        // who does not use worktrees a press on the way back to their agents.
        let profile = Profile::for_capabilities(&DeckModel::Plus.capabilities());
        let with_trees = ordinary_state().with_worktrees(vec![worktree("fix-auth", None)]);
        assert_eq!(
            cycle(&profile, &with_trees),
            vec![
                Page::Agents,
                Page::Spaces,
                Page::Trees,
                Page::Panes,
                Page::Make
            ]
        );
        assert!(!cycle(&profile, &ordinary_state()).contains(&Page::Trees));
    }

    #[test]
    fn a_deck_showing_a_page_already_does_not_stop_at_it_again() {
        // The whole difference a big deck buys: pane control and the make page are on its keys, so
        // the cycle is the three lists and nothing else. Walking to a page you can already see
        // would be a press that changes nothing but the half of the deck you were reading.
        let profile = Profile::for_capabilities(&DeckModel::Xl.capabilities());
        let stops = cycle(&profile, &ordinary_state());
        assert_eq!(stops, vec![Page::Agents, Page::Spaces]);
        assert!(
            !stops.contains(&Page::Panes) && !stops.contains(&Page::Make),
            "a page already on the keys has nothing to show elsewhere"
        );
    }

    #[test]
    fn a_page_only_half_shown_stays_in_the_cycle_so_the_rest_is_still_reachable() {
        // The dangerous half of the rule above. An XL pins six make keys; a config with enough
        // presets makes the page longer than that, and the entries that did not fit have to be
        // reachable somehow or they are keys that exist and cannot be pressed.
        let mut config = Config::default();
        for name in ["dev", "review", "release", "scratch"] {
            config.layouts.insert(name.into(), LayoutPreset::default());
        }
        let profile = Profile::derive(&DeckModel::Xl.capabilities(), &config);
        let commands = Page::Make.commands(&config.presets());
        assert!(
            profile.pinned_slots(Page::Make) < commands.len(),
            "this test is only meaningful when the page overflows its keys"
        );
        assert!(cycle(&profile, &ordinary_state()).contains(&Page::Make));
    }

    #[test]
    fn leaving_a_page_whose_list_emptied_always_works_rather_than_trapping_the_key() {
        // Worktrees are refreshed underneath the deck, so the list can empty while somebody is
        // looking at it. Leaving has to be unconditional or the page key becomes a trap.
        let profile = Profile::for_capabilities(&DeckModel::Plus.capabilities());
        let state = ordinary_state();
        let acked = Acknowledged::default();
        let deck = ResolvedDeck::new(
            &profile,
            &state,
            Page::Trees,
            0,
            Selection::default(),
            &acked,
        );
        assert_eq!(deck.next_page(), Page::Agents);
    }

    #[test]
    fn the_agents_page_is_one_press_away_from_every_page_on_every_deck() {
        // The promise the whole product rests on. However far the control surface grows, the
        // agent that needs you is never more than one key away — so every deck that can leave the
        // agents page must carry the key that comes back, and that key must mean the same thing
        // from every page it can be pressed on.
        let state = ordinary_state();
        let acked = Acknowledged::default();
        for model in [
            DeckModel::Plus,
            DeckModel::Mini,
            DeckModel::Original,
            DeckModel::Xl,
            DeckModel::Neo,
            DeckModel::Pedal,
        ] {
            let profile = Profile::for_capabilities(&model.capabilities());
            let home = profile
                .keys
                .iter()
                .position(|key| *key == KeyBinding::Attention)
                .unwrap_or_else(|| panic!("{model:?} has no way back to the agents"));
            for page in Page::ALL {
                let deck =
                    ResolvedDeck::new(&profile, &state, page, 0, Selection::default(), &acked);
                assert!(
                    matches!(deck.key_action(home), SlotAction::Attention { .. }),
                    "{model:?} loses its way home on the {} page",
                    page.label()
                );
            }
        }
    }

    #[test]
    fn an_agent_that_wants_you_is_visible_from_every_page_and_not_only_from_the_agents() {
        // A control surface that could hide a blocked agent behind a page would be a control
        // surface that broke the one thing this deck is for.
        let profile = Profile::for_capabilities(&DeckModel::Plus.capabilities());
        let state = ordinary_state();
        let acked = Acknowledged::default();
        let home = profile
            .keys
            .iter()
            .position(|key| *key == KeyBinding::Attention)
            .expect("the plus has an attention key");
        for page in Page::ALL {
            let deck = ResolvedDeck::new(&profile, &state, page, 0, Selection::default(), &acked);
            match deck.tile(home) {
                Tile::Attention { count, away } => {
                    assert_eq!(count, 1, "the count must survive leaving the agents page");
                    assert_eq!(
                        away,
                        page != Page::Agents,
                        "the key has to say it is also the way back, and only when it is"
                    );
                }
                other => panic!("expected the attention key, got {other:?}"),
            }
        }
    }

    #[test]
    fn every_page_says_which_page_it_is_rather_than_only_where_the_next_press_goes() {
        // Counting presses to work out where you are is exactly the work this deck exists to save.
        let profile = Profile::for_capabilities(&DeckModel::Plus.capabilities());
        let state = ordinary_state();
        let acked = Acknowledged::default();
        let key = profile
            .keys
            .iter()
            .position(|key| *key == KeyBinding::PageCycle)
            .expect("the plus has a page key");
        for page in Page::ALL {
            let deck = ResolvedDeck::new(&profile, &state, page, 0, Selection::default(), &acked);
            match deck.tile(key) {
                Tile::Page { current, next, .. } => {
                    assert_eq!(current, page.label());
                    assert_ne!(next.as_deref(), Some(page.label()), "a key to nowhere");
                }
                other => panic!("expected the page key, got {other:?}"),
            }
        }
    }

    #[test]
    fn every_model_lays_out_without_a_key_that_addresses_nothing() {
        // The layout engine hands out ranks and page pins by arithmetic, and arithmetic against a
        // key count is where an off-by-one lands somebody a deck with a key that draws nothing and
        // does nothing. Every model, every page, every key.
        let state = ordinary_state().with_worktrees(vec![worktree("fix-auth", Some("w2"))]);
        let acked = Acknowledged::default();
        for model in [
            DeckModel::Plus,
            DeckModel::Mini,
            DeckModel::Original,
            DeckModel::Xl,
            DeckModel::Neo,
            DeckModel::Pedal,
            DeckModel::Unknown,
        ] {
            let caps = model.capabilities();
            let profile = Profile::for_capabilities(&caps);
            assert_eq!(profile.keys.len(), caps.key_count(), "{model:?}");
            assert_eq!(profile.dials.len(), caps.dials as usize, "{model:?}");
            for page in Page::ALL {
                let deck =
                    ResolvedDeck::new(&profile, &state, page, 0, Selection::default(), &acked);
                // A pinned key never pages, so its rank is a promise about a page's length that
                // the layout engine made when it had no state to check it against.
                for (index, key) in profile.keys.iter().enumerate() {
                    if let KeyBinding::Dynamic {
                        rank,
                        page: Some(pinned),
                    } = key
                    {
                        assert!(
                            *rank < deck.page_len(*pinned),
                            "{model:?} key {index} pins entry {rank} of a {}-entry page",
                            deck.page_len(*pinned)
                        );
                    }
                    // And resolving is its own assertion: an index past the end of a list has to
                    // come back as an empty key, never as a panic.
                    deck.tile(index);
                    deck.key_action(index);
                    deck.key_long_press_action(index);
                }
            }
        }
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

    /// A dial whose list is empty used to read `—`, which is indistinguishable from a dial that
    /// is broken, still loading, or bound to nothing. On real hardware that made the dials hard
    /// to trust: you could turn one and not know whether it had done nothing or was dead. Every
    /// scrub target must say what is empty — the attention dial already said "all clear", and the
    /// others now match it.
    #[test]
    fn an_empty_dial_says_what_is_empty_rather_than_drawing_a_dash() {
        let caps = DeckModel::Plus.capabilities();
        let profile = Profile::for_capabilities(&caps);
        let state = state_with(vec![]);
        let acked = Acknowledged::default();
        let deck = ResolvedDeck::new(
            &profile,
            &state,
            Page::Agents,
            0,
            Selection::default(),
            &acked,
        );
        for dial in 0..profile.dials.len() {
            let (title, value, _) = deck.dial_feedback(dial);
            if title.is_empty() {
                continue; // not a scrub dial
            }
            assert!(
                !value.contains('\u{2014}'),
                "dial `{title}` renders a bare dash for an empty list, which reads as broken"
            );
            assert!(
                !value.trim().is_empty(),
                "dial `{title}` renders nothing at all for an empty list"
            );
        }
    }
}
