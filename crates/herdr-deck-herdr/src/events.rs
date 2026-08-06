//! The one long-lived connection: `events.subscribe`.
//!
//! # How this is used
//!
//! Events are treated as a **wake-up signal, not as the source of truth**. The daemon holds a
//! subscription and re-reads `session.snapshot` whenever an event arrives (coalesced), plus on a
//! slow timer and after every reconnect. That costs one extra round trip per burst and buys
//! immunity to every dropped-event, out-of-order, and missed-field bug that a pure event-sourced
//! cache would be exposed to. Collie arrived at the same design against the same server.
//!
//! # Subscribing
//!
//! herdr has two event families with overlapping names. `pane.agent_status_changed` exists both
//! as a per-pane *filtered* subscription (which requires a `pane_id` we do not have up front)
//! and as an unfiltered *broadcast*. We want the broadcast form: `{"type": "..."}` with no
//! filter, so we see every pane without enumerating them first.

use serde::Deserialize;
use serde_json::json;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::net::UnixStream;

use crate::client::HerdrClient;
use crate::wire::AgentStatus;
use crate::{HerdrError, Result};

/// Broadcast event types worth waking up for.
///
/// Anything that can change what the deck renders belongs here. Unrecognised events are still
/// delivered as [`HerdrEvent::Other`] and still trigger a reconcile, so this list being
/// incomplete costs latency, never correctness.
pub const DEFAULT_SUBSCRIPTIONS: &[&str] = &[
    "pane.agent_status_changed",
    "pane.agent_detected",
    "pane.created",
    "pane.closed",
    "pane.exited",
    "pane.focused",
    "pane.updated",
    "pane.moved",
    "tab.created",
    "tab.closed",
    "tab.renamed",
    "tab.focused",
    "workspace.created",
    "workspace.closed",
    "workspace.renamed",
    "workspace.focused",
    "workspace.updated",
];

/// An event pushed by herdr.
#[derive(Debug, Clone, PartialEq)]
pub enum HerdrEvent {
    /// An agent changed state. The payload herdr sends with this is rich enough to render a
    /// tile immediately, before the reconciling snapshot lands.
    AgentStatusChanged(AgentStatusChanged),
    /// Any other subscribed event. We do not model these individually — their only job is to
    /// tell the daemon "something moved, go re-snapshot".
    Other { kind: String },
}

impl HerdrEvent {
    pub fn kind(&self) -> &str {
        match self {
            HerdrEvent::AgentStatusChanged(_) => "pane.agent_status_changed",
            HerdrEvent::Other { kind } => kind,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct AgentStatusChanged {
    pub pane_id: String,
    #[serde(default)]
    pub workspace_id: String,
    #[serde(default)]
    pub agent_status: AgentStatus,
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default)]
    pub display_agent: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
}

/// A live subscription. Dropping this closes the connection and unsubscribes.
pub struct EventStream {
    lines: Lines<BufReader<UnixStream>>,
}

impl EventStream {
    /// Open a subscription for [`DEFAULT_SUBSCRIPTIONS`].
    pub async fn subscribe(client: &HerdrClient) -> Result<Self> {
        Self::subscribe_to(client, DEFAULT_SUBSCRIPTIONS).await
    }

    /// Open a subscription for an explicit set of broadcast event types.
    pub async fn subscribe_to(client: &HerdrClient, kinds: &[&str]) -> Result<Self> {
        let stream = client.connect().await?;
        let subscriptions: Vec<_> = kinds.iter().map(|k| json!({ "type": k })).collect();
        let request = json!({
            "id": "hd_sub",
            "method": "events.subscribe",
            "params": { "subscriptions": subscriptions },
        });

        let mut reader = BufReader::new(stream);
        let mut line = serde_json::to_string(&request).expect("subscribe request serialises");
        line.push('\n');
        reader.get_mut().write_all(line.as_bytes()).await?;
        reader.get_mut().flush().await?;

        // herdr acknowledges with `subscription_started` before any events flow.
        let mut ack = String::new();
        if reader.read_line(&mut ack).await? == 0 {
            return Err(HerdrError::NoResponse {
                method: "events.subscribe".to_string(),
            });
        }
        let ack_value: serde_json::Value =
            serde_json::from_str(ack.trim()).map_err(|source| HerdrError::Decode {
                method: "events.subscribe".to_string(),
                source,
            })?;
        if let Some(error) = ack_value.get("error") {
            return Err(HerdrError::Rpc {
                method: "events.subscribe".to_string(),
                code: error
                    .get("code")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string(),
                message: error
                    .get("message")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string(),
            });
        }

        Ok(Self {
            lines: reader.lines(),
        })
    }

    /// Await the next event.
    ///
    /// Returns `Ok(None)` when herdr closes the connection — the caller's cue to re-snapshot
    /// and resubscribe.
    pub async fn next(&mut self) -> Result<Option<HerdrEvent>> {
        loop {
            let Some(line) = self.lines.next_line().await? else {
                return Ok(None);
            };
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
                tracing::debug!(%line, "skipping unparseable event line from herdr");
                continue;
            };
            if let Some(event) = parse_event(&value) {
                return Ok(Some(event));
            }
        }
    }
}

/// Pull an event out of a pushed line.
///
/// herdr's exact envelope is not pinned down by the docs, so we accept the shapes it plausibly
/// uses — a bare event object, or one wrapped in `event`/`result`. Anything without a `type` is
/// ignored rather than treated as an error.
fn parse_event(value: &serde_json::Value) -> Option<HerdrEvent> {
    let body = value
        .get("event")
        .or_else(|| value.get("result"))
        .unwrap_or(value);

    let kind = body.get("type").and_then(|v| v.as_str())?;

    // The subscribe acknowledgement can arrive here on a reconnect; it is not an event.
    if kind == "subscription_started" {
        return None;
    }

    if kind == "pane.agent_status_changed" {
        if let Ok(payload) = serde_json::from_value::<AgentStatusChanged>(body.clone()) {
            return Some(HerdrEvent::AgentStatusChanged(payload));
        }
        tracing::debug!("agent status event missing expected fields, treating as a generic wake");
    }

    Some(HerdrEvent::Other {
        kind: kind.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mock::MockHerdr;

    #[tokio::test]
    async fn subscribes_to_the_unfiltered_broadcast_form() {
        // A `pane_id` filter here would silently limit us to one pane. Guard against it.
        let mock = MockHerdr::start().await;
        mock.queue_events(vec![]).await;
        let client = HerdrClient::new(mock.socket_path());
        let _stream = EventStream::subscribe(&client).await.unwrap();

        let params = mock.observed_params("events.subscribe").await.unwrap();
        let subs = params["subscriptions"].as_array().unwrap();
        assert!(!subs.is_empty());
        for sub in subs {
            assert!(sub.get("type").is_some(), "each subscription names a type");
            assert!(
                sub.get("pane_id").is_none(),
                "must not filter by pane_id, or we only see one pane"
            );
        }
        let types: Vec<_> = subs.iter().map(|s| s["type"].as_str().unwrap()).collect();
        assert!(types.contains(&"pane.agent_status_changed"));
    }

    #[tokio::test]
    async fn yields_a_typed_agent_status_event() {
        let mock = MockHerdr::start().await;
        mock.queue_events(vec![json!({
            "type": "pane.agent_status_changed",
            "pane_id": "w1:p1",
            "workspace_id": "w1",
            "agent_status": "blocked",
            "agent": "claude",
            "display_agent": "Claude: auth",
            "title": "Refactor auth middleware"
        })])
        .await;
        let client = HerdrClient::new(mock.socket_path());
        let mut stream = EventStream::subscribe(&client).await.unwrap();

        let event = stream.next().await.unwrap().unwrap();
        match event {
            HerdrEvent::AgentStatusChanged(payload) => {
                assert_eq!(payload.pane_id, "w1:p1");
                assert_eq!(payload.agent_status, AgentStatus::Blocked);
                assert_eq!(payload.display_agent.as_deref(), Some("Claude: auth"));
            }
            other => panic!("expected an agent status change, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn unmodelled_events_still_arrive_as_a_wake_signal() {
        let mock = MockHerdr::start().await;
        mock.queue_events(vec![json!({"type": "layout.updated", "tab_id": "w1:t1"})])
            .await;
        let client = HerdrClient::new(mock.socket_path());
        let mut stream = EventStream::subscribe(&client).await.unwrap();
        let event = stream.next().await.unwrap().unwrap();
        assert_eq!(event.kind(), "layout.updated");
    }

    #[tokio::test]
    async fn accepts_events_wrapped_in_an_envelope() {
        let mock = MockHerdr::start().await;
        mock.queue_events(vec![json!({
            "id": "hd_sub",
            "event": {"type": "pane.agent_status_changed", "pane_id": "w2:p3", "agent_status": "done"}
        })])
        .await;
        let client = HerdrClient::new(mock.socket_path());
        let mut stream = EventStream::subscribe(&client).await.unwrap();
        match stream.next().await.unwrap().unwrap() {
            HerdrEvent::AgentStatusChanged(p) => {
                assert_eq!(p.pane_id, "w2:p3");
                assert_eq!(p.agent_status, AgentStatus::Done);
            }
            other => panic!("expected an agent status change, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn garbage_lines_are_skipped_rather_than_killing_the_stream() {
        let mock = MockHerdr::start().await;
        mock.queue_events(vec![
            json!({"no_type_field": true}),
            json!({"type": "pane.focused", "pane_id": "w1:p1"}),
        ])
        .await;
        let client = HerdrClient::new(mock.socket_path());
        let mut stream = EventStream::subscribe(&client).await.unwrap();
        let event = stream.next().await.unwrap().unwrap();
        assert_eq!(event.kind(), "pane.focused");
    }

    #[tokio::test]
    async fn herdr_going_away_reports_end_of_stream_rather_than_blocking() {
        // This is the reconnect trigger: if it ever blocks instead of returning `None`, the
        // daemon would sit with a dead subscription and a frozen deck.
        let mock = MockHerdr::start().await;
        mock.queue_events(vec![json!({"type": "pane.focused", "pane_id": "w1:p1"})])
            .await;
        let client = HerdrClient::new(mock.socket_path());
        let mut stream = EventStream::subscribe(&client).await.unwrap();
        assert!(stream.next().await.unwrap().is_some());

        mock.shutdown();
        let ended = tokio::time::timeout(std::time::Duration::from_secs(5), stream.next())
            .await
            .expect("stream must not hang when herdr goes away");
        assert!(matches!(ended, Ok(None)));
    }
}
