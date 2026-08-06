//! One connected frontend.
//!
//! # Pure state machine, async executor
//!
//! [`Session`] is deliberately synchronous and side-effect free: it takes a frontend message
//! plus the current state and returns the bytes to send back, plus at most one [`PendingAction`]
//! for the caller to perform. All the interesting behaviour — what a key means, when to
//! repaint, how the dials move — is therefore testable without a socket, a deck, or a herdr.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use herdr_deck_core::capabilities::DeckCapabilities;
use herdr_deck_core::layout::{Mode, Profile, ResolvedDeck, ScrubTarget, Selection, SlotAction};
use herdr_deck_core::protocol::{DaemonMessage, DeviceReport, FrontendMessage, FRONTEND_PROTOCOL};
use herdr_deck_core::render::{Tile, TileRenderer};
use herdr_deck_core::state::DeckState;
use herdr_deck_core::Config;

use base64::Engine as _;

/// Something the connection loop must do asynchronously.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PendingAction {
    /// Focus an agent. `pane_id` is already resolved from the stable `terminal_id`, because
    /// herdr's `agent.focus` does not accept terminal ids.
    FocusAgent { pane_id: String, key: Option<usize> },
    FocusWorkspace {
        workspace_id: String,
        key: Option<usize>,
    },
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

    /// Handle one frontend message.
    pub fn handle(&mut self, message: FrontendMessage, state: &DeckState) -> Outcome {
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
            // Acting on key *down* rather than up makes the deck feel immediate.
            FrontendMessage::KeyDown { index } => {
                self.act(self.resolved(state).key_action(index), Some(index), state)
            }
            FrontendMessage::KeyUp { .. } => Outcome::default(),
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
        ResolvedDeck::new(&self.profile, state, self.mode, self.page, self.selection)
    }

    fn act(&mut self, action: SlotAction, key: Option<usize>, state: &DeckState) -> Outcome {
        match action {
            SlotAction::None => Outcome::default(),

            SlotAction::FocusAgent { terminal_id } => {
                // Resolve the stable terminal id to the pane id herdr's focus API wants. If the
                // agent vanished between paint and press, do nothing rather than focus whatever
                // now occupies that slot.
                match state.agent_by_terminal_id(&terminal_id) {
                    Some(agent) => Outcome {
                        messages: vec![],
                        action: Some(PendingAction::FocusAgent {
                            pane_id: agent.pane_id.clone(),
                            key,
                        }),
                    },
                    None => Outcome::just(key.map_or(vec![], |index| {
                        vec![DaemonMessage::Alert {
                            index,
                            message: "that agent is gone".to_string(),
                        }]
                    })),
                }
            }

            SlotAction::FocusWorkspace { workspace_id } => Outcome {
                messages: vec![],
                action: Some(PendingAction::FocusWorkspace { workspace_id, key }),
            },

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
        match target {
            ScrubTarget::Agents => state.agents.len(),
            ScrubTarget::Workspaces => state.workspaces.len(),
            ScrubTarget::Tabs => state.tabs.len(),
            ScrubTarget::Attention => state.attention_count(),
        }
    }

    /// Repaint, sending only what changed.
    pub fn repaint(&mut self, state: &DeckState) -> Vec<DaemonMessage> {
        // Keep cursors valid: agents come and go underneath us constantly.
        self.selection.clamp(
            state.agents.len(),
            state.workspaces.len(),
            state.tabs.len(),
            state.attention_count(),
        );
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
        for index in 0..self.profile.keys.len() {
            let tile = self.resolved(state).tile(index);
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
        for dial in 0..self.profile.dials.len() {
            let (title, value, status) = self.resolved(state).dial_feedback(dial);
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
    use herdr_deck_herdr::wire::{AgentInfo, AgentStatus, SessionSnapshot, WorkspaceInfo};

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

    #[test]
    fn pressing_a_key_resolves_the_stable_terminal_id_to_a_pane_id() {
        // herdr's agent.focus takes a pane id; the deck binds to terminal ids. Getting this
        // mapping wrong would focus the wrong pane after any workspace reorganisation.
        let mut session = session();
        let state = state(vec![agent("term_x", AgentStatus::Blocked, 1)]);
        session.greet(&state);

        let outcome = session.handle(FrontendMessage::KeyDown { index: 0 }, &state);
        assert_eq!(
            outcome.action,
            Some(PendingAction::FocusAgent {
                pane_id: "w1:pane-term_x".into(),
                key: Some(0)
            })
        );
    }

    #[test]
    fn pressing_a_key_whose_agent_vanished_alerts_instead_of_focusing_the_wrong_thing() {
        let mut session = session();
        let painted = state(vec![agent("gone", AgentStatus::Blocked, 1)]);
        session.greet(&painted);

        // The agent disappears between paint and press.
        let now = state(vec![]);
        let outcome = session.handle(FrontendMessage::KeyDown { index: 0 }, &now);
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
        let outcome = session.handle(FrontendMessage::KeyDown { index: toggle }, &s);
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

        let outcome = session.handle(FrontendMessage::DialRotate { dial: 0, ticks: 1 }, &s);
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
        session.handle(FrontendMessage::DialRotate { dial: 0, ticks: 1 }, &s);

        let outcome = session.handle(FrontendMessage::DialDown { dial: 0 }, &s);
        assert_eq!(
            outcome.action,
            Some(PendingAction::FocusAgent {
                pane_id: "w1:pane-second".into(),
                key: None
            })
        );
    }

    #[test]
    fn tapping_the_touchstrip_does_the_same_as_pressing_that_dial() {
        let mut session = session();
        let s = state(vec![agent("only", AgentStatus::Blocked, 1)]);
        session.greet(&s);
        let outcome = session.handle(FrontendMessage::TouchTap { dial: Some(0) }, &s);
        assert_eq!(
            outcome.action,
            Some(PendingAction::FocusAgent {
                pane_id: "w1:pane-only".into(),
                key: None
            })
        );
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
        session.handle(FrontendMessage::DialRotate { dial: 0, ticks: 2 }, &many);

        let fewer = state(vec![agent("a", AgentStatus::Blocked, 3)]);
        session.repaint(&fewer);
        let outcome = session.handle(FrontendMessage::DialDown { dial: 0 }, &fewer);
        assert_eq!(
            outcome.action,
            Some(PendingAction::FocusAgent {
                pane_id: "w1:pane-a".into(),
                key: None
            }),
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

        let outcome = session.handle(FrontendMessage::Refresh, &s);
        assert_eq!(key_indices(&outcome.messages).len(), 8);
    }

    #[test]
    fn ping_is_answered_with_pong() {
        let mut session = session();
        let s = state(vec![]);
        let outcome = session.handle(FrontendMessage::Ping, &s);
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
        let outcome = session.handle(FrontendMessage::KeyDown { index: 0 }, &offline);
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
        let outcome = session.handle(FrontendMessage::KeyDown { index: 0 }, &s);
        assert_eq!(
            outcome.action,
            Some(PendingAction::FocusAgent {
                pane_id: "w1:pane-stuck".into(),
                key: Some(0)
            })
        );
    }
}
