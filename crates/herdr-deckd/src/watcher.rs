//! Keeping a live view of herdr.
//!
//! # Snapshot is truth, events are a doorbell
//!
//! The obvious design is to apply events to a local cache. We deliberately do not: events are
//! only used to decide *when* to re-read `session.snapshot`, never *what* the state is. That
//! costs one extra round trip per burst of activity and buys immunity to every dropped-event,
//! out-of-order, and missing-field bug an event-sourced cache would be exposed to. A status
//! display that is 200ms late is fine; one that is subtly wrong is not.
//!
//! A slow timer reconciles regardless, so a dropped event costs latency rather than a frozen
//! deck.

use std::sync::Arc;
use std::time::{Duration, Instant};

use herdr_deck_core::config::Config;
use herdr_deck_core::state::{DeckState, WaitClock};
use herdr_deck_herdr::{EventStream, HerdrClient, HerdrError};
use tokio::sync::watch;

/// How long to wait before retrying after herdr goes away, and the ceiling for that backoff.
const RETRY_MIN: Duration = Duration::from_millis(500);
const RETRY_MAX: Duration = Duration::from_secs(10);

/// Coalescing window for event bursts.
///
/// A single user action in herdr can emit several events. Without this we would snapshot once
/// per event; with it, a burst costs one snapshot.
const COALESCE: Duration = Duration::from_millis(60);

/// Watches herdr and publishes state.
pub struct Watcher {
    client: HerdrClient,
    reconcile_interval: Duration,
}

impl Watcher {
    pub fn new(client: HerdrClient, config: &Config) -> Self {
        Self {
            client,
            reconcile_interval: config.reconcile_interval(),
        }
    }

    /// Start watching. The returned receiver always holds the latest known state.
    pub fn spawn(self) -> watch::Receiver<Arc<DeckState>> {
        let (tx, rx) = watch::channel(Arc::new(DeckState::offline("connecting to herdr…")));
        tokio::spawn(async move { self.run(tx).await });
        rx
    }

    async fn run(self, tx: watch::Sender<Arc<DeckState>>) {
        let mut backoff = RETRY_MIN;
        // Outside the connection loop on purpose: herdr restarting, or a socket blip, is not the
        // agents calming down. An agent that was blocked before the blip and is still blocked
        // after it — same `state_change_seq` — has been waiting the whole time, and resetting
        // its clock here would quietly turn a ten-minute wait back into a fresh one.
        let mut clock = WaitClock::default();
        loop {
            match self.connected_loop(&tx, &mut clock).await {
                Ok(()) => {
                    // The subscription ended cleanly — herdr restarted or shut down. Retry
                    // promptly rather than backing off; this is the common case after a herdr
                    // upgrade and the user expects the deck to come back quickly.
                    backoff = RETRY_MIN;
                }
                Err(e) => {
                    tracing::debug!(error = %e, "herdr connection lost");
                    let _ = tx.send(Arc::new(DeckState::offline(offline_message(&e))));
                }
            }
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(RETRY_MAX);
        }
    }

    /// One connected lifetime: bootstrap, then follow events until the stream ends.
    async fn connected_loop(
        &self,
        tx: &watch::Sender<Arc<DeckState>>,
        clock: &mut WaitClock,
    ) -> Result<(), HerdrError> {
        // Subscribe *before* the first snapshot so no change can slip through the gap between
        // reading state and starting to listen.
        let mut events = EventStream::subscribe(&self.client).await?;
        self.reconcile(tx, clock).await?;

        loop {
            let next = tokio::time::timeout(self.reconcile_interval, events.next()).await;
            match next {
                // An event arrived: let the burst settle, drain it, then reconcile once.
                Ok(Ok(Some(_))) => {
                    tokio::time::sleep(COALESCE).await;
                    while let Ok(Ok(Some(_))) =
                        tokio::time::timeout(Duration::from_millis(1), events.next()).await
                    {
                    }
                    self.reconcile(tx, clock).await?;
                }
                // herdr closed the subscription.
                Ok(Ok(None)) => return Ok(()),
                Ok(Err(e)) => return Err(e),
                // Nothing happened for a while; reconcile anyway as the safety net.
                Err(_) => self.reconcile(tx, clock).await?,
            }
        }
    }

    async fn reconcile(
        &self,
        tx: &watch::Sender<Arc<DeckState>>,
        clock: &mut WaitClock,
    ) -> Result<(), HerdrError> {
        let snapshot = self.client.session_snapshot().await?;
        warn_on_protocol_mismatch(snapshot.protocol);
        let mut state = DeckState::from_snapshot(snapshot);
        // Stamped here rather than in `from_snapshot` because this is the only layer that has
        // both the fresh state and a clock older than it.
        clock.stamp(&mut state, Instant::now());
        let _ = tx.send(Arc::new(state));
        Ok(())
    }
}

/// A short, user-facing reason for the offline tile.
///
/// This ends up on the physical deck, so it has to fit and has to be actionable.
pub fn offline_message(error: &HerdrError) -> String {
    match error {
        HerdrError::SocketMissing { .. } => "herdr not running".to_string(),
        HerdrError::Connect { .. } => "cannot reach herdr".to_string(),
        HerdrError::Rpc { code, .. } => format!("herdr error: {code}"),
        _ => "herdr unavailable".to_string(),
    }
}

fn warn_on_protocol_mismatch(protocol: Option<u32>) {
    use std::sync::atomic::{AtomicBool, Ordering};
    static WARNED: AtomicBool = AtomicBool::new(false);

    let Some(protocol) = protocol else { return };
    if protocol == herdr_deck_herdr::EXPECTED_PROTOCOL {
        return;
    }
    // Warn once, then carry on. The wire types are tolerant of additions, and refusing to run
    // would be a worse outcome than rendering a field we do not understand.
    if !WARNED.swap(true, Ordering::Relaxed) {
        tracing::warn!(
            herdr_protocol = protocol,
            expected = herdr_deck_herdr::EXPECTED_PROTOCOL,
            "herdr speaks a different socket protocol than herdr-deck was built against; \
             continuing, but some fields may be missing. Run `herdr-deck doctor` for details."
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use herdr_deck_herdr::mock::MockHerdr;
    use herdr_deck_herdr::wire::AgentStatus;
    use serde_json::json;

    /// Shaped the way herdr answers: the snapshot is nested under `snapshot`. Building the bare
    /// form here agreed with a client that read one level too high, so these tests passed while
    /// a real herdr produced an empty deck.
    fn snapshot_with(status: &str) -> serde_json::Value {
        json!({
            "type": "session_snapshot",
            "snapshot": {
                "version": "0.8.0",
                "protocol": 19,
                "agents": [{
                    "terminal_id": "term_a", "agent_status": status, "workspace_id": "w1",
                    "tab_id": "w1:t1", "pane_id": "w1:p1", "focused": false, "revision": 1
                }]
            }
        })
    }

    async fn wait_for<F>(rx: &mut watch::Receiver<Arc<DeckState>>, mut pred: F) -> Arc<DeckState>
    where
        F: FnMut(&DeckState) -> bool,
    {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                {
                    let current = rx.borrow_and_update().clone();
                    if pred(&current) {
                        return current;
                    }
                }
                rx.changed().await.expect("watch channel stays open");
            }
        })
        .await
        .expect("expected state did not arrive in time")
    }

    #[tokio::test]
    async fn publishes_state_from_the_bootstrap_snapshot() {
        let mock = MockHerdr::start().await;
        mock.reply("session.snapshot", snapshot_with("blocked"))
            .await;
        let config = Config::default();
        let watcher = Watcher::new(HerdrClient::new(mock.socket_path()), &config);
        let mut rx = watcher.spawn();

        let state = wait_for(&mut rx, |s| !s.is_offline()).await;
        assert_eq!(state.agents.len(), 1);
        assert_eq!(state.agents[0].agent_status, AgentStatus::Blocked);
    }

    #[tokio::test]
    async fn subscribes_before_snapshotting_so_no_change_slips_through_the_gap() {
        let mock = MockHerdr::start().await;
        mock.reply("session.snapshot", snapshot_with("idle")).await;
        let config = Config::default();
        let watcher = Watcher::new(HerdrClient::new(mock.socket_path()), &config);
        let mut rx = watcher.spawn();
        wait_for(&mut rx, |s| !s.is_offline()).await;

        let methods = mock.observed_methods().await;
        let sub = methods.iter().position(|m| m == "events.subscribe");
        let snap = methods.iter().position(|m| m == "session.snapshot");
        assert!(
            sub < snap,
            "must subscribe before the first snapshot, got {methods:?}"
        );
    }

    #[tokio::test]
    async fn an_event_triggers_a_fresh_snapshot() {
        let mock = MockHerdr::start().await;
        mock.reply("session.snapshot", snapshot_with("working"))
            .await;
        mock.queue_events(vec![json!({
            "type": "pane.agent_status_changed",
            "pane_id": "w1:p1", "workspace_id": "w1", "agent_status": "blocked"
        })])
        .await;
        let config = Config::default();
        let watcher = Watcher::new(HerdrClient::new(mock.socket_path()), &config);
        let mut rx = watcher.spawn();

        wait_for(&mut rx, |s| !s.is_offline()).await;
        // Two snapshots: the bootstrap, then one caused by the event.
        tokio::time::timeout(Duration::from_secs(5), async {
            while mock.call_count("session.snapshot").await < 2 {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("an event should have triggered a reconcile");
    }

    #[tokio::test]
    async fn the_state_reflects_what_the_snapshot_says_not_what_the_event_said() {
        // The event claims `blocked`, the snapshot says `idle`. The snapshot must win — it is
        // the source of truth, and this is the whole point of the design.
        let mock = MockHerdr::start().await;
        mock.reply("session.snapshot", snapshot_with("idle")).await;
        mock.queue_events(vec![json!({
            "type": "pane.agent_status_changed",
            "pane_id": "w1:p1", "agent_status": "blocked"
        })])
        .await;
        let config = Config::default();
        let watcher = Watcher::new(HerdrClient::new(mock.socket_path()), &config);
        let mut rx = watcher.spawn();

        let state = wait_for(&mut rx, |s| !s.is_offline()).await;
        tokio::time::sleep(Duration::from_millis(200)).await;
        let latest = rx.borrow_and_update().clone();
        assert_eq!(latest.agents[0].agent_status, AgentStatus::Idle);
        assert_eq!(state.agents[0].agent_status, AgentStatus::Idle);
    }

    #[tokio::test]
    async fn the_published_state_carries_how_long_a_blocked_agent_has_been_waiting() {
        // Nothing else stamps a wait. If this wiring is ever dropped the buckets are simply
        // absent everywhere, every tile silently loses its marker, and no other test notices —
        // they all stamp their own clock by hand.
        use herdr_deck_core::state::WaitBucket;
        use herdr_deck_herdr::wire::{AgentInfo, SessionSnapshot};

        let mock = MockHerdr::start().await;
        mock.serve_session(&SessionSnapshot {
            protocol: Some(herdr_deck_herdr::EXPECTED_PROTOCOL),
            agents: vec![AgentInfo {
                terminal_id: "term_a".into(),
                agent_status: AgentStatus::Blocked,
                workspace_id: "w1".into(),
                pane_id: "w1:p1".into(),
                state_change_seq: Some(7),
                ..Default::default()
            }],
            ..Default::default()
        })
        .await;
        let config = Config::default();
        let watcher = Watcher::new(HerdrClient::new(mock.socket_path()), &config);
        let mut rx = watcher.spawn();

        let state = wait_for(&mut rx, |s| !s.is_offline()).await;
        assert_eq!(
            state.wait_bucket(&state.agents[0]),
            Some(WaitBucket::Fresh),
            "an agent that just blocked has been waiting no time at all, but it is being timed"
        );
    }

    #[tokio::test]
    async fn reports_offline_when_herdr_is_not_running() {
        let socket = herdr_deck_herdr::SocketPath {
            path: "/nonexistent/herdr.sock".into(),
            origin: herdr_deck_herdr::socket::SocketOrigin::Default,
        };
        let config = Config::default();
        let watcher = Watcher::new(HerdrClient::new(socket), &config);
        let mut rx = watcher.spawn();

        let state = wait_for(&mut rx, |s| {
            s.offline_reason.as_deref() == Some("herdr not running")
        })
        .await;
        assert!(state.is_offline());
        assert!(state.agents.is_empty());
    }

    #[tokio::test]
    async fn recovers_when_herdr_comes_back() {
        // Start with herdr absent, then bring it up on the same path.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("herdr.sock");
        let socket = herdr_deck_herdr::SocketPath {
            path: path.clone(),
            origin: herdr_deck_herdr::socket::SocketOrigin::Default,
        };
        let config = Config::default();
        let watcher = Watcher::new(HerdrClient::new(socket), &config);
        let mut rx = watcher.spawn();
        wait_for(&mut rx, |s| s.is_offline()).await;

        // Now stand up a real server at that path.
        let listener = tokio::net::UnixListener::bind(&path).unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    return;
                };
                tokio::spawn(async move {
                    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
                    let mut reader = BufReader::new(stream);
                    let mut line = String::new();
                    if reader.read_line(&mut line).await.unwrap_or(0) == 0 {
                        return;
                    }
                    let req: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
                    let id = req["id"].as_str().unwrap_or("x").to_string();
                    let method = req["method"].as_str().unwrap_or("").to_string();
                    let response = if method == "events.subscribe" {
                        json!({"id": id, "result": {"type": "subscription_started"}})
                    } else {
                        json!({"id": id, "result": snapshot_with("done")})
                    };
                    let mut out = serde_json::to_string(&response).unwrap();
                    out.push('\n');
                    let _ = reader.get_mut().write_all(out.as_bytes()).await;
                    let _ = reader.get_mut().flush().await;
                    if method == "events.subscribe" {
                        let mut sink = String::new();
                        let _ = reader.read_line(&mut sink).await;
                    }
                });
            }
        });

        let state = wait_for(&mut rx, |s| !s.is_offline()).await;
        assert_eq!(state.agents[0].agent_status, AgentStatus::Done);
    }

    #[test]
    fn offline_messages_are_short_enough_for_a_key_and_say_what_is_wrong() {
        let cases = [
            HerdrError::SocketMissing { path: "/x".into() },
            HerdrError::Rpc {
                method: "m".into(),
                code: "denied".into(),
                message: "no".into(),
            },
        ];
        for error in cases {
            let msg = offline_message(&error);
            assert!(!msg.is_empty());
            assert!(msg.len() <= 40, "{msg:?} is too long for a tile");
        }
    }
}
