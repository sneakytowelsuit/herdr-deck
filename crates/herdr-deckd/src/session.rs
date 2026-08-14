//! One connected frontend.
//!
//! # Pure state machine, async executor
//!
//! [`Session`] is deliberately synchronous and side-effect free: it takes a frontend message,
//! the current state and the current time, and returns the bytes to send back plus at most one
//! [`PendingAction`] for the caller to perform. All the interesting behaviour — what a key
//! means, when to repaint, how the dials move, how long a press lasted — is therefore testable
//! without a socket, a deck, a herdr, or a sleep.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::time::{Duration, Instant};

use herdr_deck_core::capabilities::DeckCapabilities;
use herdr_deck_core::command::DeckCommand;
use herdr_deck_core::layout::{Mode, Profile, ResolvedDeck, ScrubTarget, Selection, SlotAction};
use herdr_deck_core::protocol::{DaemonMessage, DeviceReport, FrontendMessage, FRONTEND_PROTOCOL};
use herdr_deck_core::render::{Tile, TileRenderer};
use herdr_deck_core::state::{Acknowledged, DeckState};
use herdr_deck_core::Config;

use base64::Engine as _;

/// How long a key must be held to count as a long press.
///
/// Half a second is the usual touch-interface figure: comfortably longer than any tap, short
/// enough that holding for it does not feel like waiting. It lives here rather than in the
/// frontends because the frontends are not allowed to know what a gesture *means*.
const LONG_PRESS: Duration = Duration::from_millis(500);

/// Something the connection loop must do asynchronously.
///
/// One command and the control that asked for it — not one variant per command. Everything a key
/// can ask herdr for arrives here in the same shape, which is what keeps adding a command to a
/// vocabulary rather than to four match statements.
///
/// `key` is the key to report back on; a dial press has no face to flash, so it carries `None`
/// and the outcome goes to the log instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingAction {
    pub command: DeckCommand,
    pub key: Option<usize>,
}

/// What a frontend message produced.
#[derive(Debug, Default)]
pub struct Outcome {
    pub messages: Vec<DaemonMessage>,
    pub action: Option<PendingAction>,
}

impl Outcome {
    fn just(messages: Vec<DaemonMessage>) -> Self {
        Self {
            messages,
            action: None,
        }
    }
}

/// A key that is currently held, and what it meant when it went down.
///
/// Carrying the actions rather than re-deriving them at release is the whole point: state moves
/// underneath a held key, and the gesture must act on what the user was actually looking at.
#[derive(Debug, Clone)]
struct Press {
    at: Instant,
    short: SlotAction,
    long: SlotAction,
}

/// Per-connection state for one frontend.
pub struct Session {
    capabilities: DeckCapabilities,
    profile: Profile,
    renderer: Arc<TileRenderer>,
    mode: Mode,
    page: usize,
    selection: Selection,
    /// Hash of what we last sent for each key, so a reconcile that changes nothing sends
    /// nothing. Without this the deck would be repainted on every timer tick.
    sent_keys: Vec<Option<u64>>,
    sent_dials: Vec<Option<u64>>,
    /// When each key that has a second action went down, so the release can tell a tap from a
    /// hold. Only ever set for keys that *have* a second action — see [`Session::handle`].
    pressed: Vec<Option<Press>>,
    /// Agents this frontend has dismissed from its attention queue.
    ///
    /// Per connection, not per daemon: acknowledgement is a statement about what *this* person
    /// looking at *this* deck has seen. Two decks attached at once therefore each keep their own,
    /// which is the honest reading — dismissing an alert on the deck in front of you should not
    /// silence the one in the next room.
    acked: Acknowledged,
    greeted: bool,
}

impl Session {
    /// Build a session from a frontend's `hello`.
    pub fn new(device: &DeviceReport, config: &Config, renderer: Arc<TileRenderer>) -> Self {
        let capabilities = device.to_capabilities();
        let profile = match &config.layout {
            // A hand-written layout is used verbatim, but never allowed to be shorter than the
            // hardware — unbound trailing keys are explicitly empty rather than out of range.
            Some(override_layout) if !override_layout.keys.is_empty() => {
                let mut keys = override_layout.keys.clone();
                keys.resize(
                    capabilities.key_count(),
                    herdr_deck_core::layout::KeyBinding::Empty,
                );
                let mut dials = override_layout.dials.clone();
                dials.resize(
                    capabilities.dials as usize,
                    herdr_deck_core::layout::DialBinding::Unused,
                );
                Profile { keys, dials }
            }
            _ => Profile::for_capabilities(&capabilities),
        };
        let key_count = profile.keys.len();
        let dial_count = profile.dials.len();
        Self {
            capabilities,
            profile,
            renderer,
            mode: Mode::default(),
            page: 0,
            selection: Selection::default(),
            sent_keys: vec![None; key_count],
            sent_dials: vec![None; dial_count],
            pressed: vec![None; key_count],
            acked: Acknowledged::default(),
            greeted: false,
        }
    }

    pub fn capabilities(&self) -> &DeckCapabilities {
        &self.capabilities
    }

    /// Diagnostic accessor, also used by the tests below.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn profile(&self) -> &Profile {
        &self.profile
    }

    /// The `ready` handshake plus a first full paint.
    pub fn greet(&mut self, state: &DeckState) -> Vec<DaemonMessage> {
        self.greeted = true;
        let mut messages = vec![DaemonMessage::Ready {
            protocol: FRONTEND_PROTOCOL,
            keys: self.profile.keys.len(),
            dials: self.profile.dials.len(),
            device: self.capabilities.describe(),
        }];
        messages.extend(self.repaint(state));
        messages
    }

    /// Handle one frontend message. `now` is the clock the press timer runs on.
    pub fn handle(&mut self, message: FrontendMessage, state: &DeckState, now: Instant) -> Outcome {
        match message {
            FrontendMessage::Hello { .. } => {
                // A second hello means the frontend reconnected in place; repaint everything.
                self.force_full_repaint();
                Outcome::just(self.greet(state))
            }
            FrontendMessage::Ping => Outcome::just(vec![DaemonMessage::Pong]),
            FrontendMessage::Refresh => {
                self.force_full_repaint();
                Outcome::just(self.repaint(state))
            }

            // A key with only one action still acts on the press, because that is what makes the
            // deck feel immediate — and because a page key that did nothing until you let go
            // would simply read as broken. Only a key that has a *second* action has anything to
            // wait for, and for those the press cannot be interpreted until the release: focusing
            // is not something we could take back on discovering the user meant to hold.
            FrontendMessage::KeyDown { index } => {
                let (short, long) = self.key_actions(index, state);
                if long == SlotAction::None {
                    self.forget_press(index);
                    self.act(short, Some(index), state)
                } else {
                    // Both meanings are captured HERE, at the press, and replayed on release.
                    // The deck repaints while a key is held — a newly blocked agent reorders the
                    // attention list and takes rank 0 — so re-resolving at release would act on
                    // whichever agent moved under the finger, acknowledging one the user never
                    // looked at. The key belongs to whatever it showed when it was pressed.
                    self.remember_press(index, now, short, long);
                    Outcome::default()
                }
            }

            FrontendMessage::KeyUp { index } => {
                // No recorded press means this key acted on the way down and has nothing left to
                // do — or the frontend connected mid-press, which is the same thing to us.
                let Some(press) = self.forget_press(index) else {
                    return Outcome::default();
                };
                let held = now.saturating_duration_since(press.at);
                let action = if held >= LONG_PRESS {
                    press.long
                } else {
                    press.short
                };
                self.act(action, Some(index), state)
            }
            FrontendMessage::DialRotate { dial, ticks } => {
                let action = self.resolved(state).dial_rotate_action(dial, ticks);
                self.act(action, None, state)
            }
            FrontendMessage::DialDown { dial } => {
                let action = self.resolved(state).dial_press_action(dial);
                self.act(action, None, state)
            }
            FrontendMessage::DialUp { .. } => Outcome::default(),
            // Tapping the strip above a dial does what pressing that dial does.
            FrontendMessage::TouchTap { dial } => match dial {
                Some(dial) => {
                    let action = self.resolved(state).dial_press_action(dial);
                    self.act(action, None, state)
                }
                None => Outcome::default(),
            },
        }
    }

    fn resolved<'a>(&'a self, state: &'a DeckState) -> ResolvedDeck<'a> {
        ResolvedDeck::new(
            &self.profile,
            state,
            self.mode,
            self.page,
            self.selection,
            &self.acked,
        )
    }

    /// Both meanings of one key: what a tap does, and what a hold does.
    ///
    /// Resolved together and taken by value, so the borrow of state ends before anything acts.
    /// Callers must resolve at the PRESS and hold the result — see [`Press`].
    fn key_actions(&self, index: usize, state: &DeckState) -> (SlotAction, SlotAction) {
        let deck = self.resolved(state);
        (deck.key_action(index), deck.key_long_press_action(index))
    }

    fn remember_press(&mut self, index: usize, now: Instant, short: SlotAction, long: SlotAction) {
        if let Some(slot) = self.pressed.get_mut(index) {
            *slot = Some(Press {
                at: now,
                short,
                long,
            });
        }
    }

    /// Take the press timer for a key, leaving it clear.
    fn forget_press(&mut self, index: usize) -> Option<Press> {
        self.pressed.get_mut(index).and_then(Option::take)
    }

    fn act(&mut self, action: SlotAction, key: Option<usize>, state: &DeckState) -> Outcome {
        match action {
            SlotAction::None => Outcome::default(),

            // Already in herdr's own terms, so there is nothing to resolve: hand it on.
            SlotAction::Command(command) => Outcome {
                messages: vec![],
                action: Some(PendingAction { command, key }),
            },

            SlotAction::FocusAgent { terminal_id } => {
                // Resolve the stable terminal id to the pane id herdr's focus API wants. If the
                // agent vanished between paint and press, do nothing rather than focus whatever
                // now occupies that slot.
                match state.agent_by_terminal_id(&terminal_id) {
                    Some(agent) => Outcome {
                        messages: vec![],
                        action: Some(PendingAction {
                            command: DeckCommand::FocusPane {
                                pane_id: agent.pane_id.clone(),
                            },
                            key,
                        }),
                    },
                    None => Outcome::just(alert(key, "that agent is gone")),
                }
            }

            // The guard, or anything else that decided not to act, saying why. A dial has no face
            // to say it on, so that goes to the log rather than nowhere.
            SlotAction::Refuse { message } => {
                if key.is_none() {
                    tracing::info!(%message, "a control declined to act");
                }
                Outcome::just(alert(key, &message))
            }

            // The point of this action is what it does *not* do: no herdr call, no window raised,
            // no focus. The attention queue simply stops counting this agent, until herdr says
            // the agent's state moved.
            SlotAction::AcknowledgeAgent { terminal_id } => {
                match state.agent_by_terminal_id(&terminal_id) {
                    Some(agent) if self.acked.acknowledge(agent) => {
                        Outcome::just(self.repaint(state))
                    }
                    // herdr gave us nothing that could ever expire the acknowledgement, so we
                    // refuse it and say so. Silently muting an agent that might never come back
                    // is the one failure this deck must not have.
                    Some(_) => Outcome::just(alert(key, "herdr reported no state sequence")),
                    None => Outcome::just(alert(key, "that agent is gone")),
                }
            }

            SlotAction::ToggleMode => {
                self.mode = self.mode.toggled();
                self.page = 0;
                Outcome::just(self.repaint(state))
            }

            SlotAction::ChangePage { delta } => {
                let pages = self.resolved(state).page_count();
                let next = (self.page as i64 + delta as i64).rem_euclid(pages as i64) as usize;
                self.page = next;
                Outcome::just(self.repaint(state))
            }

            SlotAction::Scrub { target, delta } => {
                let len = self.list_len(target, state);
                self.selection.scrub(target, delta, len);
                Outcome::just(self.repaint(state))
            }
        }
    }

    fn list_len(&self, target: ScrubTarget, state: &DeckState) -> usize {
        self.resolved(state).scrub_len(target)
    }

    /// Repaint, sending only what changed.
    pub fn repaint(&mut self, state: &DeckState) -> Vec<DaemonMessage> {
        // An acknowledgement whose agent moved on can never match again, so drop it here rather
        // than letting a long-lived connection hoard one entry per agent it ever dismissed.
        self.acked.forget_stale(state);

        // Keep cursors valid: agents come and go underneath us constantly.
        let (agents, workspaces, tabs, attention) = self.resolved(state).list_lengths();
        self.selection.clamp(agents, workspaces, tabs, attention);
        let pages = self.resolved(state).page_count();
        if self.page >= pages {
            self.page = pages - 1;
        }

        let mut messages = Vec::new();
        if self.capabilities.has_display() {
            messages.extend(self.repaint_keys(state));
        }
        messages.extend(self.repaint_dials(state));
        messages
    }

    fn repaint_keys(&mut self, state: &DeckState) -> Vec<DaemonMessage> {
        let mut messages = Vec::new();
        let size = self.capabilities.key_image_px;
        // Resolve every tile up front: one pass over the deck rather than one per key, and the
        // borrow is over before anything below needs to record what was sent.
        let tiles: Vec<Tile> = {
            let deck = self.resolved(state);
            (0..self.profile.keys.len()).map(|i| deck.tile(i)).collect()
        };
        for (index, tile) in tiles.into_iter().enumerate() {
            let hash = hash_of(&tile);
            if self.sent_keys.get(index).copied().flatten() == Some(hash) {
                continue;
            }
            match self.renderer.render_key(&tile, size) {
                Ok(png) => {
                    if let Some(slot) = self.sent_keys.get_mut(index) {
                        *slot = Some(hash);
                    }
                    messages.push(DaemonMessage::SetKeyImage {
                        index,
                        png: base64::engine::general_purpose::STANDARD.encode(png),
                    });
                }
                Err(e) => {
                    // A tile that will not render must not take the deck down; leave the key as
                    // it was and log it.
                    tracing::warn!(index, error = %e, "could not render tile");
                }
            }
        }
        messages
    }

    fn repaint_dials(&mut self, state: &DeckState) -> Vec<DaemonMessage> {
        let mut messages = Vec::new();
        let strip = self.capabilities.touchstrip;
        let dial_count = self.capabilities.dials;
        let feedback: Vec<_> = {
            let deck = self.resolved(state);
            (0..self.profile.dials.len())
                .map(|dial| deck.dial_feedback(dial))
                .collect()
        };
        for (dial, (title, value, status)) in feedback.into_iter().enumerate() {
            let hash = hash_of(&(&title, &value, status));
            if self.sent_dials.get(dial).copied().flatten() == Some(hash) {
                continue;
            }
            if let Some(slot) = self.sent_dials.get_mut(dial) {
                *slot = Some(hash);
            }
            // Only render a strip image when there is a strip to draw it on. The macOS plugin
            // uses Elgato's own layout system and needs only the text.
            let png = strip.and_then(|strip| {
                self.renderer
                    .render_strip(
                        &title,
                        &value,
                        status,
                        strip.segment_width(dial_count),
                        strip.height,
                    )
                    .ok()
                    .map(|bytes| base64::engine::general_purpose::STANDARD.encode(bytes))
            });
            messages.push(DaemonMessage::SetDialFeedback {
                dial,
                title,
                value,
                png,
            });
        }
        messages
    }

    /// Forget what we have sent, so the next repaint redraws everything.
    fn force_full_repaint(&mut self) {
        self.sent_keys.iter_mut().for_each(|slot| *slot = None);
        self.sent_dials.iter_mut().for_each(|slot| *slot = None);
    }

    /// Which tile a key is currently showing. Test and diagnostic helper.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn tile_at(&self, index: usize, state: &DeckState) -> Tile {
        self.resolved(state).tile(index)
    }
}

/// Deck feedback for something that did not happen, when we know which key asked for it.
///
/// A dial press has no key to flash, and an alert nobody can see is worse than none at all —
/// the caller logs those instead.
fn alert(key: Option<usize>, message: &str) -> Vec<DaemonMessage> {
    key.map_or(vec![], |index| {
        vec![DaemonMessage::Alert {
            index,
            message: message.to_string(),
        }]
    })
}

fn hash_of<T: Hash>(value: &T) -> u64 {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use herdr_deck_core::capabilities::DeckModel;
    use herdr_deck_core::theme::Theme;
    use herdr_deck_herdr::wire::{AgentInfo, AgentStatus, SessionSnapshot, TabInfo, WorkspaceInfo};

    fn renderer() -> Arc<TileRenderer> {
        Arc::new(TileRenderer::new(Theme::Dark))
    }

    fn plus_device() -> DeviceReport {
        DeviceReport {
            model: Some(DeckModel::Plus),
            model_name: Some("Stream Deck +".into()),
            columns: 4,
            rows: 2,
            key_image_px: 120,
            dials: 4,
            touchstrip: Some(herdr_deck_core::TouchStrip {
                width: 800,
                height: 100,
            }),
        }
    }

    fn agent(id: &str, status: AgentStatus, seq: u64) -> AgentInfo {
        AgentInfo {
            terminal_id: id.into(),
            agent_status: status,
            workspace_id: "w1".into(),
            tab_id: "w1:t1".into(),
            pane_id: format!("w1:pane-{id}"),
            state_change_seq: Some(seq),
            ..Default::default()
        }
    }

    fn state(agents: Vec<AgentInfo>) -> DeckState {
        DeckState::from_snapshot(SessionSnapshot {
            agents,
            ..Default::default()
        })
    }

    fn session() -> Session {
        Session::new(&plus_device(), &Config::default(), renderer())
    }

    /// The command a key press should have produced to take the user to `pane_id`.
    fn focus_pane(pane_id: &str, key: Option<usize>) -> PendingAction {
        PendingAction {
            command: DeckCommand::FocusPane {
                pane_id: pane_id.to_string(),
            },
            key,
        }
    }

    fn key_indices(messages: &[DaemonMessage]) -> Vec<usize> {
        messages
            .iter()
            .filter_map(|m| match m {
                DaemonMessage::SetKeyImage { index, .. } => Some(*index),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_repaint_under_a_held_key_cannot_move_the_acknowledgement_to_another_agent() {
        // The deck repaints while a key is held, and a newly blocked agent takes rank 0 — so the
        // key under the finger changes owner mid-hold. Re-resolving the gesture at release would
        // dismiss an agent the user never looked at, which is this product's worst failure: an
        // agent that needs you but whose key looks calm.
        let mut session = session();
        let before = state(vec![agent("alice", AgentStatus::Blocked, 1)]);
        session.greet(&before);

        let down = Instant::now();
        session.handle(FrontendMessage::KeyDown { index: 0 }, &before, down);

        // Mid-hold, `bob` blocks and displaces `alice` from key 0.
        let during = state(vec![
            agent("alice", AgentStatus::Blocked, 1),
            agent("bob", AgentStatus::Blocked, 9),
        ]);
        session.repaint(&during);
        assert_eq!(
            during.attention_order()[0].terminal_id,
            "bob",
            "precondition: the newcomer must really have taken key 0"
        );

        session.handle(
            FrontendMessage::KeyUp { index: 0 },
            &during,
            down + LONG_PRESS,
        );

        // Asserted by name, not by key index: dismissing an agent reorders the list, so a
        // positional check here would pass for the wrong reason when the wrong agent is taken.
        assert_eq!(
            acknowledged_labels(&session, &during),
            vec!["alice"],
            "the hold must dismiss the agent that was on the key when it went down, not \
             whoever moved under the finger while it was held"
        );
    }

    #[test]
    fn a_short_press_also_acts_on_the_agent_that_was_there_when_it_went_down() {
        // Same hazard, cheaper gesture: a tap that focuses whoever moved under the finger would
        // take the user to a pane they did not choose.
        let mut session = session();
        let before = state(vec![agent("alice", AgentStatus::Blocked, 1)]);
        session.greet(&before);

        let down = Instant::now();
        session.handle(FrontendMessage::KeyDown { index: 0 }, &before, down);
        let during = state(vec![
            agent("alice", AgentStatus::Blocked, 1),
            agent("bob", AgentStatus::Blocked, 9),
        ]);
        session.repaint(&during);

        let released = session.handle(
            FrontendMessage::KeyUp { index: 0 },
            &during,
            down + Duration::from_millis(50),
        );
        assert_eq!(released.action, Some(focus_pane("w1:pane-alice", Some(0))));
    }

    #[test]
    fn an_agent_that_vanishes_mid_hold_says_so_rather_than_doing_nothing_quietly() {
        // The press is real, so the release owes the user an answer. Silence here would look
        // exactly like a dead key.
        let mut session = session();
        let before = state(vec![agent("alice", AgentStatus::Blocked, 1)]);
        session.greet(&before);

        let down = Instant::now();
        session.handle(FrontendMessage::KeyDown { index: 0 }, &before, down);
        let gone = state(vec![]);
        let released = session.handle(
            FrontendMessage::KeyUp { index: 0 },
            &gone,
            down + Duration::from_millis(50),
        );
        assert!(released.action.is_none());
        assert!(
            released
                .messages
                .iter()
                .any(|m| matches!(m, DaemonMessage::Alert { index: 0, .. })),
            "expected an alert on the pressed key, got {:?}",
            released.messages
        );
    }

    #[test]
    fn holding_the_key_of_a_calm_agent_arms_no_dismissal_for_later() {
        // Acknowledging something that is not asking for attention would store a mute that fires
        // the moment it blocks — a dismissal the user never made for a state they never saw.
        let mut session = session();
        let calm = state(vec![agent("busy", AgentStatus::Working, 1)]);
        session.greet(&calm);

        // A calm key has no hold meaning, so it acts on the way down like any plain key — that
        // is not what this test is about. What matters is that nothing was stored.
        press_for(&mut session, 0, LONG_PRESS, &calm);

        // Same agent, now blocked at the same seq: it must still be asking for attention.
        let blocked = state(vec![agent("busy", AgentStatus::Blocked, 1)]);
        session.repaint(&blocked);
        assert_eq!(
            attention_count(&session, &blocked),
            1,
            "a calm-key hold must not have muted this agent in advance"
        );
    }

    #[test]
    fn acknowledgements_survive_herdr_briefly_going_away() {
        // An offline state carries no agents, so pruning against it would silently empty the
        // dismissal set and refill a queue the user had just cleared. The wait clock is built to
        // survive the same blip; these two must agree.
        let mut session = session();
        let live = state(vec![agent("alice", AgentStatus::Done, 1)]);
        session.greet(&live);
        press_for(&mut session, 0, LONG_PRESS, &live);

        session.repaint(&DeckState::offline("herdr not running"));
        session.repaint(&live);

        assert!(
            tile_is_acknowledged(&session, 0, &live),
            "the dismissal must survive herdr blinking; got {:?}",
            session.tile_at(0, &live)
        );
    }

    /// Send one message at a moment the test does not care about.
    fn send(session: &mut Session, message: FrontendMessage, state: &DeckState) -> Outcome {
        session.handle(message, state, Instant::now())
    }

    /// Press key `index` and let go `held` later, returning whatever the gesture produced.
    ///
    /// A key acts either on the press or on the release and never both, so folding the two
    /// outcomes into one loses nothing and lets each test below read like the gesture it is about.
    fn press_for(
        session: &mut Session,
        index: usize,
        held: Duration,
        state: &DeckState,
    ) -> Outcome {
        let down = Instant::now();
        let pressed = session.handle(FrontendMessage::KeyDown { index }, state, down);
        let released = session.handle(FrontendMessage::KeyUp { index }, state, down + held);
        assert!(
            pressed.action.is_none() || released.action.is_none(),
            "one press must not produce two actions"
        );
        if released.action.is_some() || !released.messages.is_empty() {
            released
        } else {
            pressed
        }
    }

    fn tap(session: &mut Session, index: usize, state: &DeckState) -> Outcome {
        press_for(session, index, Duration::from_millis(40), state)
    }

    fn hold(session: &mut Session, index: usize, state: &DeckState) -> Outcome {
        press_for(session, index, LONG_PRESS, state)
    }

    /// What the deck's attention key is currently counting.
    fn attention_count(session: &Session, state: &DeckState) -> usize {
        let index = session
            .profile()
            .keys
            .iter()
            .position(|k| *k == herdr_deck_core::layout::KeyBinding::NextAttention)
            .expect("this profile has an attention key");
        match session.tile_at(index, state) {
            Tile::Attention { count } => count,
            other => panic!("expected the attention tile, got {other:?}"),
        }
    }

    /// The labels of every agent currently drawn as dismissed, in key order.
    ///
    /// Dismissing an agent reorders the attention list, so any assertion about *which* agent was
    /// dismissed has to be made by name — a positional one moves with the thing it is measuring.
    fn acknowledged_labels(session: &Session, state: &DeckState) -> Vec<String> {
        (0..session.profile().keys.len())
            .filter_map(|index| match session.tile_at(index, state) {
                Tile::Agent {
                    label,
                    acknowledged: true,
                    ..
                } => Some(label),
                _ => None,
            })
            .collect()
    }

    /// Whether key `index` is drawing an agent that has been dismissed.
    fn tile_is_acknowledged(session: &Session, index: usize, state: &DeckState) -> bool {
        match session.tile_at(index, state) {
            Tile::Agent { acknowledged, .. } => acknowledged,
            other => panic!("key {index} is not showing an agent: {other:?}"),
        }
    }

    #[test]
    fn greeting_announces_the_hardware_and_paints_every_key() {
        let mut session = session();
        let state = state(vec![agent("a", AgentStatus::Blocked, 1)]);
        let messages = session.greet(&state);

        match &messages[0] {
            DaemonMessage::Ready {
                protocol,
                keys,
                dials,
                ..
            } => {
                assert_eq!(*protocol, FRONTEND_PROTOCOL);
                assert_eq!(*keys, 8);
                assert_eq!(*dials, 4);
            }
            other => panic!("expected ready first, got {other:?}"),
        }
        assert_eq!(
            key_indices(&messages).len(),
            8,
            "first paint covers every key"
        );
    }

    #[test]
    fn a_repaint_that_changes_nothing_sends_nothing() {
        // This is what stops the reconcile timer repainting the deck twice a second forever.
        let mut session = session();
        let state = state(vec![agent("a", AgentStatus::Working, 1)]);
        session.greet(&state);
        let second = session.repaint(&state);
        assert!(
            second.is_empty(),
            "unchanged state should produce no traffic, got {second:?}"
        );
    }

    #[test]
    fn only_the_keys_that_changed_are_resent() {
        let mut session = session();
        let before = state(vec![
            agent("a", AgentStatus::Working, 1),
            agent("b", AgentStatus::Working, 2),
        ]);
        session.greet(&before);

        // `b` becomes blocked, which also reorders it above `a`. Two agent tiles change, plus
        // the attention counter. Nothing else should move.
        let after = state(vec![
            agent("a", AgentStatus::Working, 1),
            agent("b", AgentStatus::Blocked, 3),
        ]);
        let messages = session.repaint(&after);
        let changed = key_indices(&messages);
        assert!(!changed.is_empty());
        assert!(
            changed.len() < 8,
            "a single status change should not repaint the whole deck, changed: {changed:?}"
        );
    }

    // --- Wait escalation, and the repaint budget it is allowed ---------------------------------
    //
    // Bucketing a wait rather than counting seconds exists entirely for these two tests. A live
    // counter would change every tile's hash on every tick, the diff above would match nothing,
    // and the reconcile timer would repaint the whole deck twice a second forever — which is the
    // exact failure the hashing was added to prevent.

    /// A state whose blocked agent has been waiting `waited`, from one long-lived clock.
    ///
    /// Mirrors the watcher: one clock, a fresh `DeckState` per reconcile.
    struct Reconciler {
        clock: herdr_deck_core::state::WaitClock,
        start: Instant,
    }

    impl Reconciler {
        fn new() -> Self {
            Self {
                clock: herdr_deck_core::state::WaitClock::default(),
                start: Instant::now(),
            }
        }

        fn at(&mut self, waited: Duration, agents: Vec<AgentInfo>) -> DeckState {
            let mut state = state(agents);
            self.clock.stamp(&mut state, self.start + waited);
            state
        }
    }

    #[test]
    fn a_reconcile_that_only_advances_the_clock_within_a_bucket_still_sends_nothing() {
        // The steady state: an agent has been blocked for a while, nothing about it changes, and
        // the deck must stay silent between bucket crossings.
        let mut session = session();
        let mut reconciler = Reconciler::new();
        let blocked = vec![agent("stuck", AgentStatus::Blocked, 1)];

        session.greet(&reconciler.at(Duration::ZERO, blocked.clone()));
        for tick in 1..20 {
            let state = reconciler.at(Duration::from_secs(tick * 2), blocked.clone());
            let messages = session.repaint(&state);
            assert!(
                messages.is_empty(),
                "reconcile at {}s repainted {:?}",
                tick * 2,
                key_indices(&messages)
            );
        }
    }

    #[test]
    fn crossing_a_wait_bucket_repaints_the_waiting_agent_and_nothing_else() {
        // The cost side of the same bargain: an escalation is worth one repaint of one key, and
        // must not drag the rest of the deck along with it.
        let mut session = session();
        let mut reconciler = Reconciler::new();
        let agents = vec![
            agent("stuck", AgentStatus::Blocked, 1),
            agent("busy", AgentStatus::Working, 2),
        ];
        session.greet(&reconciler.at(Duration::ZERO, agents.clone()));

        let crossed = reconciler.at(Duration::from_secs(61), agents.clone());
        assert_eq!(
            key_indices(&session.repaint(&crossed)),
            vec![0],
            "only the key showing the waiting agent should be redrawn"
        );
        assert!(
            session
                .repaint(&reconciler.at(Duration::from_secs(63), agents.clone()))
                .is_empty(),
            "and it settles again immediately"
        );

        let overdue = reconciler.at(Duration::from_secs(301), agents);
        assert_eq!(key_indices(&session.repaint(&overdue)), vec![0]);
    }

    #[test]
    fn an_agent_waiting_the_whole_time_costs_exactly_two_extra_repaints() {
        // Stated as a budget because that is how the feature was justified: two buckets to cross,
        // so two repaints, however long the incident runs.
        let mut session = session();
        let mut reconciler = Reconciler::new();
        let blocked = vec![agent("stuck", AgentStatus::Blocked, 1)];
        session.greet(&reconciler.at(Duration::ZERO, blocked.clone()));

        // Ten minutes of reconciles, two seconds apart.
        let repaints: usize = (1..300)
            .map(|tick| {
                let state = reconciler.at(Duration::from_secs(tick * 2), blocked.clone());
                key_indices(&session.repaint(&state)).len()
            })
            .sum();
        assert_eq!(repaints, 2, "one crossing into 1m+, one into 5m+, no more");
    }

    #[test]
    fn pressing_a_key_resolves_the_stable_terminal_id_to_a_pane_id() {
        // herdr's agent.focus takes a pane id; the deck binds to terminal ids. Getting this
        // mapping wrong would focus the wrong pane after any workspace reorganisation.
        let mut session = session();
        let state = state(vec![agent("term_x", AgentStatus::Blocked, 1)]);
        session.greet(&state);

        let outcome = tap(&mut session, 0, &state);
        assert_eq!(outcome.action, Some(focus_pane("w1:pane-term_x", Some(0))));
    }

    // --- Long press ------------------------------------------------------------------------
    //
    // Holding an agent key dismisses it from the attention queue. The tests that matter most
    // here are the ones about the acknowledgement *ending*: a dismissal that outlived the state
    // it was about would hide an agent that needs you, and a deck that hides those has no reason
    // to exist.

    #[test]
    fn holding_an_agent_key_acknowledges_it_without_ever_asking_for_a_focus() {
        // The entire point: clearing a finished agent must not drag the terminal window in front
        // of whatever you were doing.
        let mut session = session();
        let state = state(vec![agent("finished", AgentStatus::Done, 1)]);
        session.greet(&state);
        assert_eq!(attention_count(&session, &state), 1);

        let outcome = hold(&mut session, 0, &state);
        assert!(
            outcome.action.is_none(),
            "a long press must produce no action for the daemon to perform, got {:?}",
            outcome.action
        );
        assert!(
            !outcome.messages.is_empty(),
            "the deck has to repaint, or the gesture looks like it did nothing"
        );
        assert_eq!(attention_count(&session, &state), 0);
        assert!(tile_is_acknowledged(&session, 0, &state));
    }

    #[test]
    fn an_acknowledged_agent_keeps_its_key_and_a_short_press_still_takes_you_there() {
        let mut session = session();
        let state = state(vec![agent("finished", AgentStatus::Done, 1)]);
        session.greet(&state);
        hold(&mut session, 0, &state);

        assert!(matches!(session.tile_at(0, &state), Tile::Agent { .. }));
        assert_eq!(
            tap(&mut session, 0, &state).action,
            Some(focus_pane("w1:pane-finished", Some(0)))
        );
    }

    #[test]
    fn an_agent_that_blocks_again_after_being_acknowledged_comes_straight_back() {
        // The invariant the whole feature stands on. If an acknowledgement ever survives the
        // state it dismissed, the deck goes quiet about an agent that is waiting on you — a
        // false negative, and the worst thing this product can do.
        let mut session = session();
        let blocked = state(vec![agent("stuck", AgentStatus::Blocked, 1)]);
        session.greet(&blocked);
        hold(&mut session, 0, &blocked);
        assert_eq!(attention_count(&session, &blocked), 0);

        // You answered it elsewhere and it got on with the work.
        let working = state(vec![agent("stuck", AgentStatus::Working, 2)]);
        session.repaint(&working);
        assert_eq!(attention_count(&session, &working), 0);

        // Now it needs you again. herdr bumps the sequence, so the acknowledgement no longer
        // describes anything and the agent is back at the top of the queue.
        let again = state(vec![agent("stuck", AgentStatus::Blocked, 3)]);
        let messages = session.repaint(&again);
        assert_eq!(
            attention_count(&session, &again),
            1,
            "a re-blocked agent must count again"
        );
        assert!(
            !tile_is_acknowledged(&session, 0, &again),
            "and must be drawn as the alarm it is"
        );
        assert!(
            !messages.is_empty(),
            "the deck has to be repainted to say so"
        );
    }

    #[test]
    fn an_acknowledgement_survives_an_unrelated_agent_changing_state() {
        // Acknowledgement is per agent. Expiring the lot on any state change anywhere would make
        // the gesture useless on a busy machine, where something is always moving.
        let mut session = session();
        let before = state(vec![
            agent("dismissed", AgentStatus::Done, 1),
            agent("other", AgentStatus::Working, 2),
        ]);
        session.greet(&before);
        // `done` outranks `working`, so key 0 is the agent this test dismisses.
        hold(&mut session, 0, &before);
        assert_eq!(attention_count(&session, &before), 0);

        let after = state(vec![
            agent("dismissed", AgentStatus::Done, 1),
            agent("other", AgentStatus::Blocked, 9),
        ]);
        session.repaint(&after);
        assert_eq!(
            attention_count(&session, &after),
            1,
            "only the newly blocked agent should be counted"
        );
        // The blocked agent now owns key 0 and the dismissed one has dropped behind it.
        assert!(tile_is_acknowledged(&session, 1, &after));
    }

    #[test]
    fn a_press_of_exactly_the_threshold_is_a_hold_and_a_millisecond_less_is_a_tap() {
        // The boundary is worth pinning in both directions: a threshold that only fires above it
        // makes the gesture feel unreliable, and one that fires below it steals ordinary presses.
        let state = state(vec![agent("finished", AgentStatus::Done, 1)]);

        let mut just_short = session();
        just_short.greet(&state);
        let outcome = press_for(
            &mut just_short,
            0,
            LONG_PRESS - Duration::from_millis(1),
            &state,
        );
        assert!(
            outcome.action.is_some(),
            "a hair under the threshold is still a press, and a press focuses"
        );
        assert_eq!(attention_count(&just_short, &state), 1);

        let mut exactly = session();
        exactly.greet(&state);
        let outcome = press_for(&mut exactly, 0, LONG_PRESS, &state);
        assert!(outcome.action.is_none());
        assert_eq!(attention_count(&exactly, &state), 0);
    }

    #[test]
    fn a_key_with_no_second_action_still_acts_the_instant_it_goes_down() {
        // Nothing about the mode toggle is ambiguous, so nothing about it should wait. A key that
        // did nothing until you let go would read as broken hardware.
        let mut session = session();
        let mut s = state(vec![agent("a", AgentStatus::Idle, 1)]);
        s.workspaces = vec![WorkspaceInfo {
            workspace_id: "w1".into(),
            label: Some("api".into()),
            ..Default::default()
        }];
        session.greet(&s);
        let toggle = session
            .profile()
            .keys
            .iter()
            .position(|k| *k == herdr_deck_core::layout::KeyBinding::ModeToggle)
            .unwrap();

        let outcome = session.handle(
            FrontendMessage::KeyDown { index: toggle },
            &s,
            Instant::now(),
        );
        assert!(!outcome.messages.is_empty(), "it must act on the press");
        assert!(matches!(session.tile_at(0, &s), Tile::Workspace { .. }));
    }

    #[test]
    fn holding_a_key_with_no_second_action_does_not_act_a_second_time_on_release() {
        let mut session = session();
        let mut s = state(vec![agent("a", AgentStatus::Idle, 1)]);
        s.workspaces = vec![WorkspaceInfo {
            workspace_id: "w1".into(),
            label: Some("api".into()),
            ..Default::default()
        }];
        session.greet(&s);
        let toggle = session
            .profile()
            .keys
            .iter()
            .position(|k| *k == herdr_deck_core::layout::KeyBinding::ModeToggle)
            .unwrap();

        let now = Instant::now();
        session.handle(FrontendMessage::KeyDown { index: toggle }, &s, now);
        let released = session.handle(
            FrontendMessage::KeyUp { index: toggle },
            &s,
            now + LONG_PRESS * 4,
        );
        assert!(released.messages.is_empty() && released.action.is_none());
        assert!(
            matches!(session.tile_at(0, &s), Tile::Workspace { .. }),
            "the mode must have toggled exactly once"
        );
    }

    #[test]
    fn a_release_with_no_press_behind_it_is_ignored() {
        // A frontend that connects while a key is already held, or one that repeats a key_up,
        // must not act on a press this session never saw.
        let mut session = session();
        let s = state(vec![agent("finished", AgentStatus::Done, 1)]);
        session.greet(&s);

        let outcome = send(&mut session, FrontendMessage::KeyUp { index: 0 }, &s);
        assert!(outcome.action.is_none());
        assert!(outcome.messages.is_empty());
        assert_eq!(attention_count(&session, &s), 1);
    }

    #[test]
    fn an_agent_herdr_gave_no_state_sequence_for_is_refused_out_loud_rather_than_muted_forever() {
        // Without a sequence nothing could ever expire the acknowledgement, so the only safe
        // answer is to decline it — and to say so, rather than leave the user believing an agent
        // was dismissed when it was not.
        let mut session = session();
        let mut sequenceless = agent("mystery", AgentStatus::Blocked, 0);
        sequenceless.state_change_seq = None;
        let s = state(vec![sequenceless]);
        session.greet(&s);

        let outcome = hold(&mut session, 0, &s);
        assert!(outcome.action.is_none());
        match outcome.messages.as_slice() {
            [DaemonMessage::Alert { index, message }] => {
                assert_eq!(*index, 0);
                assert!(message.contains("state sequence"), "got {message}");
            }
            other => panic!("expected one alert on the pressed key, got {other:?}"),
        }
        assert_eq!(
            attention_count(&session, &s),
            1,
            "the agent must still be asking for you"
        );
    }

    #[test]
    fn pressing_a_key_whose_agent_vanished_alerts_instead_of_focusing_the_wrong_thing() {
        let mut session = session();
        let painted = state(vec![agent("gone", AgentStatus::Blocked, 1)]);
        session.greet(&painted);

        // The agent disappears between paint and press.
        let now = state(vec![]);
        let outcome = send(&mut session, FrontendMessage::KeyDown { index: 0 }, &now);
        assert!(outcome.action.is_none());
        // With no agent, the slot is empty and simply does nothing.
        assert!(outcome.messages.is_empty());
    }

    #[test]
    fn toggling_mode_switches_to_workspaces_and_repaints() {
        let mut session = session();
        let mut s = state(vec![agent("a", AgentStatus::Idle, 1)]);
        s.workspaces = vec![WorkspaceInfo {
            workspace_id: "w1".into(),
            label: Some("api".into()),
            ..Default::default()
        }];
        session.greet(&s);

        let toggle = session
            .profile()
            .keys
            .iter()
            .position(|k| *k == herdr_deck_core::layout::KeyBinding::ModeToggle)
            .unwrap();
        let outcome = send(&mut session, FrontendMessage::KeyDown { index: toggle }, &s);
        assert!(!outcome.messages.is_empty(), "mode change must repaint");
        assert!(matches!(session.tile_at(0, &s), Tile::Workspace { .. }));
    }

    #[test]
    fn rotating_a_dial_moves_its_cursor_and_updates_the_touchstrip() {
        let mut session = session();
        let s = state(vec![
            agent("first", AgentStatus::Blocked, 9),
            agent("second", AgentStatus::Blocked, 5),
        ]);
        session.greet(&s);

        let outcome = send(
            &mut session,
            FrontendMessage::DialRotate { dial: 0, ticks: 1 },
            &s,
        );
        let feedback: Vec<_> = outcome
            .messages
            .iter()
            .filter_map(|m| match m {
                DaemonMessage::SetDialFeedback { dial, value, .. } => Some((*dial, value.clone())),
                _ => None,
            })
            .collect();
        assert!(
            feedback.iter().any(|(d, v)| *d == 0 && v == "second"),
            "dial 0 should now be on the second agent, got {feedback:?}"
        );
    }

    #[test]
    fn pressing_a_dial_focuses_its_current_selection() {
        let mut session = session();
        let s = state(vec![
            agent("first", AgentStatus::Blocked, 9),
            agent("second", AgentStatus::Blocked, 5),
        ]);
        session.greet(&s);
        send(
            &mut session,
            FrontendMessage::DialRotate { dial: 0, ticks: 1 },
            &s,
        );

        let outcome = send(&mut session, FrontendMessage::DialDown { dial: 0 }, &s);
        assert_eq!(outcome.action, Some(focus_pane("w1:pane-second", None)));
    }

    #[test]
    fn pressing_the_tab_dial_asks_for_a_tab_focus_rather_than_its_workspace() {
        // The tab dial exists to reach a tab that is *not* the workspace's current one, so
        // handing the daemon a workspace focus here would quietly undo the whole gesture.
        let mut session = session();
        let mut s = state(vec![agent("term_x", AgentStatus::Blocked, 1)]);
        s.tabs = vec![
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
        session.greet(&s);
        send(
            &mut session,
            FrontendMessage::DialRotate { dial: 2, ticks: 1 },
            &s,
        );

        let outcome = send(&mut session, FrontendMessage::DialDown { dial: 2 }, &s);
        assert_eq!(
            outcome.action,
            Some(PendingAction {
                command: DeckCommand::FocusTab {
                    tab_id: "w1:t2".into()
                },
                key: None
            })
        );
    }

    #[test]
    fn tapping_the_touchstrip_does_the_same_as_pressing_that_dial() {
        let mut session = session();
        let s = state(vec![agent("only", AgentStatus::Blocked, 1)]);
        session.greet(&s);
        let outcome = send(
            &mut session,
            FrontendMessage::TouchTap { dial: Some(0) },
            &s,
        );
        assert_eq!(outcome.action, Some(focus_pane("w1:pane-only", None)));
    }

    #[test]
    fn cursors_are_clamped_when_the_agent_they_pointed_at_disappears() {
        let mut session = session();
        let many = state(vec![
            agent("a", AgentStatus::Blocked, 3),
            agent("b", AgentStatus::Blocked, 2),
            agent("c", AgentStatus::Blocked, 1),
        ]);
        session.greet(&many);
        send(
            &mut session,
            FrontendMessage::DialRotate { dial: 0, ticks: 2 },
            &many,
        );

        let fewer = state(vec![agent("a", AgentStatus::Blocked, 3)]);
        session.repaint(&fewer);
        let outcome = send(&mut session, FrontendMessage::DialDown { dial: 0 }, &fewer);
        assert_eq!(
            outcome.action,
            Some(focus_pane("w1:pane-a", None)),
            "a stale cursor must not select nothing"
        );
    }

    #[test]
    fn an_explicit_refresh_repaints_everything_even_if_nothing_changed() {
        // The Stream Deck app asks for this after waking from sleep.
        let mut session = session();
        let s = state(vec![agent("a", AgentStatus::Working, 1)]);
        session.greet(&s);
        assert!(session.repaint(&s).is_empty());

        let outcome = send(&mut session, FrontendMessage::Refresh, &s);
        assert_eq!(key_indices(&outcome.messages).len(), 8);
    }

    #[test]
    fn ping_is_answered_with_pong() {
        let mut session = session();
        let s = state(vec![]);
        let outcome = send(&mut session, FrontendMessage::Ping, &s);
        assert_eq!(outcome.messages, vec![DaemonMessage::Pong]);
    }

    #[test]
    fn an_offline_state_paints_every_key_with_the_reason_and_ignores_presses() {
        let mut session = session();
        let offline = DeckState::offline("herdr not running");
        session.greet(&offline);
        for index in 0..8 {
            assert!(matches!(
                session.tile_at(index, &offline),
                Tile::Offline { .. }
            ));
        }
        let outcome = send(
            &mut session,
            FrontendMessage::KeyDown { index: 0 },
            &offline,
        );
        assert!(outcome.action.is_none());
    }

    #[test]
    fn a_hardware_change_produces_a_different_layout_without_any_config() {
        let xl = DeviceReport {
            model: Some(DeckModel::Xl),
            model_name: Some("Stream Deck XL".into()),
            columns: 8,
            rows: 4,
            key_image_px: 96,
            dials: 0,
            touchstrip: None,
        };
        let session = Session::new(&xl, &Config::default(), renderer());
        assert_eq!(session.profile().keys.len(), 32);
        assert!(session.profile().dials.is_empty());
        assert!(
            session.profile().has_paging(),
            "a deck with no dials needs paging keys"
        );
    }

    #[test]
    fn a_hand_written_layout_is_padded_to_the_hardware_rather_than_leaving_keys_out_of_range() {
        let mut config = Config::default();
        config.layout = Some(herdr_deck_core::config::LayoutOverride {
            keys: vec![herdr_deck_core::layout::KeyBinding::NextAttention],
            dials: vec![],
        });
        let session = Session::new(&plus_device(), &config, renderer());
        assert_eq!(session.profile().keys.len(), 8);
        assert_eq!(session.profile().dials.len(), 4);
        assert_eq!(
            session.profile().keys[7],
            herdr_deck_core::layout::KeyBinding::Empty
        );
    }

    #[test]
    fn a_pedal_gets_no_key_images_because_it_has_no_screens() {
        let pedal = DeviceReport {
            model: Some(DeckModel::Pedal),
            model_name: Some("Stream Deck Pedal".into()),
            columns: 3,
            rows: 1,
            key_image_px: 0,
            dials: 0,
            touchstrip: None,
        };
        let mut session = Session::new(&pedal, &Config::default(), renderer());
        let s = state(vec![agent("a", AgentStatus::Blocked, 1)]);
        let messages = session.greet(&s);
        assert!(
            key_indices(&messages).is_empty(),
            "nothing should be drawn to a device with no displays"
        );
    }

    #[test]
    fn a_key_bound_straight_to_a_command_issues_it_and_needs_no_list_behind_it() {
        // The path every command that is not a focus will arrive by: no agent, no cursor, no
        // page — the key holds the command and the daemon hands it on unchanged.
        let mut config = Config::default();
        config.layout = Some(herdr_deck_core::config::LayoutOverride {
            keys: vec![herdr_deck_core::layout::KeyBinding::Command {
                command: DeckCommand::FocusTab {
                    tab_id: "w1:t2".into(),
                },
            }],
            dials: vec![],
        });
        let mut session = Session::new(&plus_device(), &config, renderer());
        let s = state(vec![]);
        session.greet(&s);

        assert_eq!(
            tap(&mut session, 0, &s).action,
            Some(PendingAction {
                command: DeckCommand::FocusTab {
                    tab_id: "w1:t2".into()
                },
                key: Some(0)
            })
        );
    }

    #[test]
    fn pressing_a_pedal_still_focuses_the_top_attention_agent() {
        let pedal = DeviceReport {
            model: Some(DeckModel::Pedal),
            model_name: None,
            columns: 3,
            rows: 1,
            key_image_px: 0,
            dials: 0,
            touchstrip: None,
        };
        let mut session = Session::new(&pedal, &Config::default(), renderer());
        let s = state(vec![agent("stuck", AgentStatus::Blocked, 1)]);
        let outcome = send(&mut session, FrontendMessage::KeyDown { index: 0 }, &s);
        assert_eq!(outcome.action, Some(focus_pane("w1:pane-stuck", Some(0))));
    }
}
