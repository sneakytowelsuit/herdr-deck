//! An in-process fake herdr server.
//!
//! This exists so the entire wire layer — transport, one-shot semantics, error mapping, event
//! reconciliation — is testable on a machine with no herdr installed and no Stream Deck
//! attached. It replicates the two behaviours that actually bite:
//!
//! 1. RPC connections are **closed after one response**.
//! 2. `events.subscribe` connections **stay open** and stream further lines.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use serde_json::json;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::Mutex;

use crate::socket::{SocketOrigin, SocketPath};

#[derive(Debug, Clone)]
enum Behaviour {
    Result(serde_json::Value),
    Error {
        code: String,
        message: String,
    },
    /// Close the connection without answering.
    HangUp,
}

#[derive(Debug, Default)]
struct State {
    behaviours: HashMap<String, Behaviour>,
    /// Lines pushed after a successful `events.subscribe`.
    events: Vec<serde_json::Value>,
    observed: Vec<Observed>,
    connections: usize,
}

#[derive(Debug, Clone)]
struct Observed {
    id: String,
    method: String,
    params: serde_json::Value,
}

/// A fake herdr listening on a unix socket in a temporary directory.
///
/// Dropping it (or calling [`MockHerdr::shutdown`]) closes any held-open subscription, which is
/// how tests exercise the "herdr went away" path. Without that, an event-stream test would block
/// forever waiting on a connection nothing will ever close.
pub struct MockHerdr {
    path: PathBuf,
    // Held so the directory outlives the socket.
    _dir: tempfile::TempDir,
    state: Arc<Mutex<State>>,
    shutdown: tokio::sync::broadcast::Sender<()>,
}

impl MockHerdr {
    pub async fn start() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("herdr.sock");
        let listener = UnixListener::bind(&path).expect("bind mock herdr socket");
        let state = Arc::new(Mutex::new(State::default()));
        let (shutdown, _) = tokio::sync::broadcast::channel(4);

        let accept_state = Arc::clone(&state);
        let accept_shutdown = shutdown.clone();
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    return;
                };
                let conn_state = Arc::clone(&accept_state);
                let conn_shutdown = accept_shutdown.subscribe();
                tokio::spawn(async move {
                    let _ = handle(stream, conn_state, conn_shutdown).await;
                });
            }
        });

        Self {
            path,
            _dir: dir,
            state,
            shutdown,
        }
    }

    /// Close any open subscription connections, as herdr would on shutdown.
    pub fn shutdown(&self) {
        let _ = self.shutdown.send(());
    }

    pub fn socket_path(&self) -> SocketPath {
        SocketPath {
            path: self.path.clone(),
            origin: SocketOrigin::Default,
        }
    }

    /// Answer `method` with `result`.
    pub async fn reply(&self, method: &str, result: serde_json::Value) {
        self.state
            .lock()
            .await
            .behaviours
            .insert(method.to_string(), Behaviour::Result(result));
    }

    /// Answer `method` with an error.
    pub async fn reply_error(&self, method: &str, code: &str, message: &str) {
        self.state.lock().await.behaviours.insert(
            method.to_string(),
            Behaviour::Error {
                code: code.to_string(),
                message: message.to_string(),
            },
        );
    }

    /// Close the connection on `method` without answering.
    pub async fn hang_up_on(&self, method: &str) {
        self.state
            .lock()
            .await
            .behaviours
            .insert(method.to_string(), Behaviour::HangUp);
    }

    /// Queue lines to push after a successful `events.subscribe`.
    pub async fn queue_events(&self, events: Vec<serde_json::Value>) {
        self.state.lock().await.events = events;
    }

    pub async fn observed_methods(&self) -> Vec<String> {
        self.state
            .lock()
            .await
            .observed
            .iter()
            .map(|o| o.method.clone())
            .collect()
    }

    pub async fn observed_ids(&self) -> Vec<String> {
        self.state
            .lock()
            .await
            .observed
            .iter()
            .map(|o| o.id.clone())
            .collect()
    }

    pub async fn observed_params(&self, method: &str) -> Option<serde_json::Value> {
        self.state
            .lock()
            .await
            .observed
            .iter()
            .find(|o| o.method == method)
            .map(|o| o.params.clone())
    }

    pub async fn call_count(&self, method: &str) -> usize {
        self.state
            .lock()
            .await
            .observed
            .iter()
            .filter(|o| o.method == method)
            .count()
    }

    /// Serve `snapshot` and accept focus calls — what any consumer test needs to stand up a
    /// working herdr.
    ///
    /// This exists so callers never have to spell a herdr method name. Everything above this
    /// crate is supposed to hold no protocol knowledge, and a test that hand-writes
    /// `"agent.focus"` quietly breaks that rule in the place people are least likely to look.
    pub async fn serve_session(&self, snapshot: &crate::wire::SessionSnapshot) {
        // Wrapped exactly as herdr wraps it. This mock previously answered with the bare
        // snapshot, which is the same mistake the client made, so the two agreed with each other
        // and disagreed with herdr — every test passed against a client that read an empty
        // snapshot from a real session. A mock is only worth what it copies faithfully.
        let value = json!({
            "type": "session_snapshot",
            "snapshot": serde_json::to_value(snapshot).expect("snapshot serialises"),
        });
        self.reply("session.snapshot", value).await;
        self.reply("agent.focus", json!({ "type": "ok" })).await;
        self.reply("workspace.focus", json!({ "type": "ok" })).await;
        self.reply("tab.focus", json!({ "type": "ok" })).await;
        self.reply("client.window_title.set", json!({ "type": "ok" }))
            .await;
        self.with_attached_client().await;
    }

    /// Answer as a herdr with a terminal attached to it.
    ///
    /// Named for the fact rather than the method because that is the only thing callers above
    /// this crate are allowed to care about: whether there is a client whose window could be
    /// raised. herdr answers that question through its toast delivery, which is this crate's
    /// business and nobody else's.
    pub async fn with_attached_client(&self) {
        self.reply(
            "notification.show",
            json!({"type": "notification_show", "shown": true, "reason": "shown"}),
        )
        .await;
    }

    /// Answer as a headless herdr: running, holding state, with no terminal attached.
    pub async fn without_attached_client(&self) {
        self.reply(
            "notification.show",
            json!({
                "type": "notification_show",
                "shown": false,
                "reason": "no_foreground_client"
            }),
        )
        .await;
    }

    /// The target of each `agent.focus` call, in the order they arrived.
    pub async fn agent_focus_targets(&self) -> Vec<String> {
        self.observed_string_params("agent.focus", "target").await
    }

    /// The workspace id of each `workspace.focus` call, in order.
    pub async fn workspace_focus_ids(&self) -> Vec<String> {
        self.observed_string_params("workspace.focus", "workspace_id")
            .await
    }

    /// The tab id of each `tab.focus` call, in order.
    pub async fn tab_focus_ids(&self) -> Vec<String> {
        self.observed_string_params("tab.focus", "tab_id").await
    }

    /// How many times state was read.
    pub async fn snapshot_count(&self) -> usize {
        self.call_count("session.snapshot").await
    }

    async fn observed_string_params(&self, method: &str, field: &str) -> Vec<String> {
        self.state
            .lock()
            .await
            .observed
            .iter()
            .filter(|o| o.method == method)
            .filter_map(|o| o.params.get(field).and_then(|v| v.as_str()))
            .map(str::to_string)
            .collect()
    }

    pub async fn connection_count(&self) -> usize {
        self.state.lock().await.connections
    }
}

impl Drop for MockHerdr {
    fn drop(&mut self) {
        self.shutdown();
    }
}

async fn handle(
    stream: UnixStream,
    state: Arc<Mutex<State>>,
    mut shutdown: tokio::sync::broadcast::Receiver<()>,
) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    if reader.read_line(&mut line).await? == 0 {
        return Ok(());
    }

    let request: serde_json::Value = match serde_json::from_str(line.trim()) {
        Ok(v) => v,
        Err(_) => return Ok(()),
    };
    let id = request
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let method = request
        .get("method")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let params = request.get("params").cloned().unwrap_or(json!({}));

    let (behaviour, events) = {
        let mut guard = state.lock().await;
        guard.connections += 1;
        guard.observed.push(Observed {
            id: id.clone(),
            method: method.clone(),
            params,
        });
        (guard.behaviours.get(&method).cloned(), guard.events.clone())
    };

    // `events.subscribe` is one of only two methods that keep the connection open.
    if method == "events.subscribe" {
        write_line(
            reader.get_mut(),
            &json!({"id": id, "result": {"type": "subscription_started"}}),
        )
        .await?;
        for event in events {
            write_line(reader.get_mut(), &event).await?;
        }
        // Hold the connection open like herdr does, until the client drops it or the mock is
        // shut down (which stands in for herdr exiting).
        let mut sink = String::new();
        tokio::select! {
            _ = reader.read_line(&mut sink) => {}
            _ = shutdown.recv() => {}
        }
        return Ok(());
    }

    let response = match behaviour {
        Some(Behaviour::Result(result)) => json!({"id": id, "result": result}),
        Some(Behaviour::Error { code, message }) => {
            json!({"id": id, "error": {"code": code, "message": message}})
        }
        // Unconfigured methods behave like a hang-up, which surfaces as a clear test failure.
        Some(Behaviour::HangUp) | None => return Ok(()),
    };
    write_line(reader.get_mut(), &response).await?;
    // One-shot: herdr drops the connection here, and so do we.
    Ok(())
}

async fn write_line(stream: &mut UnixStream, value: &serde_json::Value) -> std::io::Result<()> {
    let mut line = serde_json::to_string(value).expect("mock response serialises");
    line.push('\n');
    stream.write_all(line.as_bytes()).await?;
    stream.flush().await
}
