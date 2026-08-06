//! One-shot RPC against the herdr socket.
//!
//! herdr closes the connection after a single response, so there is no connection to pool and
//! no multiplexing to do: each call is connect → write one line → read one line → drop. That is
//! not a workaround, it is the protocol. The only long-lived connection in this crate is the
//! event subscription in [`crate::events`].

use std::sync::atomic::{AtomicU64, Ordering};

use serde::de::DeserializeOwned;
use serde_json::json;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

use crate::socket::SocketPath;
use crate::wire::{Request, Response, SessionSnapshot};
use crate::{HerdrError, Result};

/// A client for herdr's API socket.
///
/// Cheap to clone conceptually — it holds no connection, only the socket path — but each call
/// dials a fresh connection.
#[derive(Debug, Clone)]
pub struct HerdrClient {
    socket: SocketPath,
    next_id: std::sync::Arc<AtomicU64>,
}

impl HerdrClient {
    pub fn new(socket: SocketPath) -> Self {
        Self {
            socket,
            next_id: std::sync::Arc::new(AtomicU64::new(1)),
        }
    }

    /// Resolve the socket from the environment (see [`SocketPath::resolve`]).
    pub fn from_env(session: Option<&str>) -> Self {
        Self::new(SocketPath::resolve(session))
    }

    pub fn socket(&self) -> &SocketPath {
        &self.socket
    }

    fn next_id(&self) -> String {
        format!("hd_{}", self.next_id.fetch_add(1, Ordering::Relaxed))
    }

    /// Open a connection to the socket, distinguishing "herdr isn't running" from other errors
    /// so the caller can render a useful message rather than a raw ENOENT.
    pub(crate) async fn connect(&self) -> Result<UnixStream> {
        if !self.socket.exists() {
            return Err(HerdrError::SocketMissing {
                path: self.socket.path.display().to_string(),
            });
        }
        UnixStream::connect(&self.socket.path)
            .await
            .map_err(|source| HerdrError::Connect {
                path: self.socket.path.display().to_string(),
                source,
            })
    }

    /// Issue a single RPC and decode its `result` into `T`.
    pub async fn call<T: DeserializeOwned>(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<T> {
        let value = self.call_raw(method, params).await?;
        serde_json::from_value(value).map_err(|source| HerdrError::Decode {
            method: method.to_string(),
            source,
        })
    }

    /// Issue a single RPC and return the raw `result` value.
    pub async fn call_raw(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value> {
        let stream = self.connect().await?;
        let id = self.next_id();
        let request = Request {
            id: &id,
            method,
            params,
        };
        let mut line = serde_json::to_string(&request).expect("request serialises");
        line.push('\n');

        let mut reader = BufReader::new(stream);
        reader.get_mut().write_all(line.as_bytes()).await?;
        reader.get_mut().flush().await?;

        let mut buf = String::new();
        let read = reader.read_line(&mut buf).await?;
        if read == 0 {
            return Err(HerdrError::NoResponse {
                method: method.to_string(),
            });
        }

        let response: Response =
            serde_json::from_str(buf.trim()).map_err(|source| HerdrError::Decode {
                method: method.to_string(),
                source,
            })?;

        if let Some(err) = response.error {
            return Err(HerdrError::Rpc {
                method: method.to_string(),
                code: err.code,
                message: err.message,
            });
        }
        Ok(response.result.unwrap_or(serde_json::Value::Null))
    }

    // ----- the methods herdr-deck actually uses -------------------------------------------

    /// Liveness check. Returns `Ok(())` when herdr answers.
    pub async fn ping(&self) -> Result<()> {
        self.call_raw("ping", json!({})).await.map(|_| ())
    }

    /// The bootstrap snapshot: every workspace, tab, pane and agent, plus what is focused.
    pub async fn session_snapshot(&self) -> Result<SessionSnapshot> {
        self.call("session.snapshot", json!({})).await
    }

    /// Focus an agent by name or pane id.
    ///
    /// This also **marks the tab seen**, which flips a `done` agent to `idle` — exactly the
    /// behaviour you want from pressing a key that says "I'm looking at this now".
    ///
    /// Note this changes herdr's *internal* focus only. Raising the OS terminal window is a
    /// separate step herdr does not provide; see the `herdr-deck-focus` crate.
    pub async fn agent_focus(&self, target: &str) -> Result<()> {
        self.call_raw("agent.focus", json!({ "target": target }))
            .await
            .map(|_| ())
    }

    pub async fn workspace_focus(&self, workspace_id: &str) -> Result<()> {
        self.call_raw("workspace.focus", json!({ "workspace_id": workspace_id }))
            .await
            .map(|_| ())
    }

    pub async fn tab_focus(&self, tab_id: &str) -> Result<()> {
        self.call_raw("tab.focus", json!({ "tab_id": tab_id }))
            .await
            .map(|_| ())
    }

    /// Stamp a marker into the attached client's terminal window title.
    ///
    /// This is how the focus engine finds *which* OS window is hosting herdr: set a unique
    /// title, then ask the window manager for the window with that title. Far more reliable
    /// than guessing from the terminal's process tree.
    pub async fn set_window_title(&self, title: &str) -> Result<()> {
        self.call_raw("client.window_title.set", json!({ "title": title }))
            .await
            .map(|_| ())
    }

    pub async fn clear_window_title(&self) -> Result<()> {
        self.call_raw("client.window_title.clear", json!({}))
            .await
            .map(|_| ())
    }

    /// Ask herdr to show a toast. Useful for confirming a deck action inside the TUI.
    ///
    /// Returns the `reason` herdr reports (`shown`, `disabled`, `rate_limited`,
    /// `no_foreground_client`, `busy`). `no_foreground_client` doubles as a liveness probe for
    /// "is a TUI actually attached right now" — which tells the focus engine whether raising a
    /// window can possibly help.
    pub async fn notification_show(&self, title: &str, body: Option<&str>) -> Result<String> {
        let mut params = json!({ "title": title });
        if let Some(body) = body {
            params["body"] = json!(body);
        }
        let value = self.call_raw("notification.show", params).await?;
        Ok(value
            .get("reason")
            .and_then(|r| r.as_str())
            .unwrap_or("unknown")
            .to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mock::MockHerdr;

    #[tokio::test]
    async fn ping_round_trips() {
        let mock = MockHerdr::start().await;
        mock.reply("ping", json!({"type": "pong"})).await;
        let client = HerdrClient::new(mock.socket_path());
        client.ping().await.unwrap();
        assert_eq!(mock.observed_methods().await, vec!["ping"]);
    }

    #[tokio::test]
    async fn each_call_opens_a_fresh_connection() {
        // herdr's RPC is one-shot: proving we reconnect per call guards against someone
        // "optimising" this into a pooled connection that would hang on the second request.
        let mock = MockHerdr::start().await;
        mock.reply("ping", json!({"type": "pong"})).await;
        let client = HerdrClient::new(mock.socket_path());
        client.ping().await.unwrap();
        client.ping().await.unwrap();
        client.ping().await.unwrap();
        assert_eq!(mock.connection_count().await, 3);
    }

    #[tokio::test]
    async fn request_ids_are_unique_per_call() {
        let mock = MockHerdr::start().await;
        mock.reply("ping", json!({"type": "pong"})).await;
        let client = HerdrClient::new(mock.socket_path());
        client.ping().await.unwrap();
        client.ping().await.unwrap();
        let ids = mock.observed_ids().await;
        assert_eq!(ids.len(), 2);
        assert_ne!(ids[0], ids[1]);
    }

    #[tokio::test]
    async fn snapshot_decodes_into_typed_state() {
        let mock = MockHerdr::start().await;
        mock.reply(
            "session.snapshot",
            json!({
                "type": "session_snapshot",
                "protocol": 19,
                "workspaces": [{"workspace_id":"w1","label":"api","agent_status":"blocked"}],
                "agents": [{
                    "terminal_id":"term_a","agent_status":"blocked","workspace_id":"w1",
                    "tab_id":"w1:t1","pane_id":"w1:p1","focused":false,"revision":1
                }]
            }),
        )
        .await;
        let client = HerdrClient::new(mock.socket_path());
        let snap = client.session_snapshot().await.unwrap();
        assert_eq!(snap.protocol, Some(19));
        assert_eq!(snap.agents[0].terminal_id, "term_a");
    }

    #[tokio::test]
    async fn an_rpc_error_is_surfaced_with_code_and_message() {
        let mock = MockHerdr::start().await;
        mock.reply_error("agent.focus", "not_found", "pane not found")
            .await;
        let client = HerdrClient::new(mock.socket_path());
        let err = client.agent_focus("w9:p9").await.unwrap_err();
        match err {
            HerdrError::Rpc { code, message, .. } => {
                assert_eq!(code, "not_found");
                assert_eq!(message, "pane not found");
            }
            other => panic!("expected an Rpc error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_missing_socket_says_herdr_is_not_running() {
        let client = HerdrClient::new(SocketPath {
            path: "/nonexistent/herdr.sock".into(),
            origin: crate::socket::SocketOrigin::Default,
        });
        let err = client.ping().await.unwrap_err();
        assert!(matches!(err, HerdrError::SocketMissing { .. }));
        assert!(err.to_string().contains("is herdr running?"));
    }

    #[tokio::test]
    async fn a_connection_closed_without_a_reply_is_reported_clearly() {
        let mock = MockHerdr::start().await;
        mock.hang_up_on("ping").await;
        let client = HerdrClient::new(mock.socket_path());
        let err = client.ping().await.unwrap_err();
        assert!(matches!(err, HerdrError::NoResponse { .. }));
    }

    #[tokio::test]
    async fn focus_sends_the_target_herdr_expects() {
        let mock = MockHerdr::start().await;
        mock.reply("agent.focus", json!({"type": "ok"})).await;
        let client = HerdrClient::new(mock.socket_path());
        client.agent_focus("w1:p1").await.unwrap();
        let params = mock.observed_params("agent.focus").await.unwrap();
        assert_eq!(params["target"], "w1:p1");
    }

    #[tokio::test]
    async fn notification_reason_is_returned_for_liveness_probing() {
        let mock = MockHerdr::start().await;
        mock.reply(
            "notification.show",
            json!({"type":"notification_show","shown":false,"reason":"no_foreground_client"}),
        )
        .await;
        let client = HerdrClient::new(mock.socket_path());
        let reason = client.notification_show("hi", None).await.unwrap();
        assert_eq!(reason, "no_foreground_client");
    }
}
