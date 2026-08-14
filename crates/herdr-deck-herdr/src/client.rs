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
use crate::wire::{
    CreateSpec, LayoutPreset, PaneDirection, PaneFocusDirectionResult, PaneOutcome, PaneZoomResult,
    Request, Response, SessionSnapshot, SessionSnapshotResult, SplitDirection, WorktreeInfo,
    WorktreeListResult, ZoomMode,
};
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
            // One code is worth pulling out of the crowd: it is the only refusal that leaves
            // something on the user's screen, so it is the only one whose message has to send
            // them looking for it.
            if err.code == "confirmation_required" {
                return Err(HerdrError::ConfirmationRequired {
                    method: method.to_string(),
                });
            }
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
    ///
    /// herdr wraps this one: the result is `{"type": "session_snapshot", "snapshot": {...}}`, and
    /// the snapshot's own fields are a level down. Reading the result as the snapshot directly
    /// does not fail — every field of [`SessionSnapshot`] has a default, so it yields a perfectly
    /// valid empty one, and the deck shows no agents while `doctor` reports that herdr never sent
    /// a protocol version. That shipped. The wrapper below is deliberately strict so the same
    /// mistake is an error rather than an empty screen.
    pub async fn session_snapshot(&self) -> Result<SessionSnapshot> {
        let wrapper: SessionSnapshotResult = self.call("session.snapshot", json!({})).await?;
        Ok(wrapper.snapshot)
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

    /// Move the focus one pane in `direction`, within the tab herdr is already on.
    ///
    /// Sent **without** a pane id, which is what makes the key stateless: with one, herdr
    /// navigates to that pane first, so a key labelled "left" would also teleport whoever pressed
    /// it. Omitted, it means "from wherever I am", and herdr resolves that against its own current
    /// focus rather than against a snapshot the deck took some seconds ago.
    ///
    /// At the edge of a layout this returns `Unchanged`, not an error — herdr reports it as a
    /// success carrying a reason, and so do we.
    ///
    /// herdr also hands back the whole layout snapshot here. We drop it: the deck's view of herdr
    /// is rebuilt wholesale by the watcher, and letting a key press inject one tab's geometry into
    /// it would make what the deck believes depend on which key you last pressed.
    pub async fn pane_focus_direction(&self, direction: PaneDirection) -> Result<PaneOutcome> {
        let result: PaneFocusDirectionResult = self
            .call("pane.focus_direction", json!({ "direction": direction }))
            .await?;
        // herdr said yes; a body we could not read is not evidence that nothing happened.
        Ok(result.focus.map_or(PaneOutcome::Changed, |f| f.outcome()))
    }

    /// Zoom the focused pane to fill its tab, or restore it.
    ///
    /// Always an explicit end state, never a toggle — see [`ZoomMode`]. No pane id, for the same
    /// reason as [`Self::pane_focus_direction`]: with one, `pane.zoom` navigates first.
    pub async fn pane_zoom(&self, mode: ZoomMode) -> Result<PaneOutcome> {
        let result: PaneZoomResult = self.call("pane.zoom", json!({ "mode": mode })).await?;
        Ok(result.zoom.map_or(PaneOutcome::Changed, |z| z.outcome()))
    }

    /// Split the focused pane, putting a new shell to its right or below it.
    ///
    /// `focus: true` because a split you then have to navigate to is half a command, and the deck
    /// has no second key to finish it with. It cannot take the user anywhere unexpected: the new
    /// pane is in the tab they were already looking at.
    pub async fn pane_split(&self, direction: SplitDirection) -> Result<()> {
        self.call_raw(
            "pane.split",
            json!({ "direction": direction, "focus": true }),
        )
        .await
        .map(|_| ())
    }

    /// Close one pane, by id.
    ///
    /// Destructive, and more so than it looks: the last pane of a tab takes the tab with it, and
    /// the last tab takes the workspace. herdr answers a bare `ok` either way, so the caller
    /// cannot report which of the three happened — only that it was asked for.
    ///
    /// Unlike the commands above, this one takes an explicit id: herdr has no "the focused one"
    /// form, and inventing one by reading the focus first would put a round trip between the press
    /// and the close in which the focus could move.
    pub async fn pane_close(&self, pane_id: &str) -> Result<()> {
        self.call_raw("pane.close", json!({ "pane_id": pane_id }))
            .await
            .map(|_| ())
    }

    // ----- structure: worktrees, workspaces, tabs, layouts ---------------------------------

    /// Every git worktree of the repository the active workspace belongs to.
    ///
    /// A pure read, and the only one in this crate that is not `session.snapshot`. herdr resolves
    /// the repository from whichever workspace is active and shells out to `git worktree list`,
    /// which is why the deck asks for it on a slow timer rather than on every reconcile.
    ///
    /// Outside a git repository herdr answers `not_git_worktree`; that is an ordinary error here,
    /// and the caller is expected to read it as "there are none" rather than as a fault.
    pub async fn worktree_list(&self) -> Result<Vec<WorktreeInfo>> {
        let result: WorktreeListResult = self.call("worktree.list", json!({})).await?;
        Ok(result.worktrees)
    }

    /// Open a worktree as a workspace and go there.
    ///
    /// Identified by `path` rather than by branch, because a detached checkout has no branch and a
    /// key that worked for most worktrees and not for others would be worse than one that works
    /// for all of them.
    ///
    /// Idempotent: a checkout that is already open is not opened twice, it is simply focused. That
    /// makes this the one structural command safe to press repeatedly, which is exactly what a
    /// physical key gets.
    pub async fn worktree_open(&self, path: &str) -> Result<()> {
        self.call_raw("worktree.open", json!({ "path": path, "focus": true }))
            .await
            .map(|_| ())
    }

    /// Create a worktree and open it.
    ///
    /// Sent with no parameters at all: herdr generates the branch name, bases it on `HEAD` and
    /// puts the checkout in its configured worktree directory. That is the whole reason this is
    /// possible from a deck — there is nothing here anybody would have had to type.
    ///
    /// Slow. herdr defers this until git finishes, so the response can be seconds away on a large
    /// repository — one of only two methods in the whole API that does not answer at once.
    pub async fn worktree_create(&self) -> Result<()> {
        self.call_raw("worktree.create", json!({ "focus": true }))
            .await
            .map(|_| ())
    }

    /// Remove a worktree's checkout, closing the workspace it was open as.
    ///
    /// **Never forced, and this function takes no argument that could force it.** With `force`
    /// false, git refuses to remove a checkout holding uncommitted or untracked changes and herdr
    /// passes that refusal back — which is the only confirmation prompt available to a device with
    /// no screen to put one on, and it is a good one. Forcing would instead kill every agent in the
    /// workspace *before* git even runs, and then delete the uncommitted work anyway.
    ///
    /// Committed work always survives either way: herdr never deletes the branch.
    ///
    /// Slow, for the same reason as [`Self::worktree_create`].
    pub async fn worktree_remove(&self, workspace_id: &str) -> Result<()> {
        self.call_raw(
            "worktree.remove",
            json!({ "workspace_id": workspace_id, "force": false }),
        )
        .await
        .map(|_| ())
    }

    /// Create a workspace from a named preset and go to it.
    pub async fn workspace_create(&self, spec: &CreateSpec) -> Result<()> {
        self.call_raw("workspace.create", spec.to_params(true))
            .await
            .map(|_| ())
    }

    /// Create a tab in the active workspace from a named preset and go to it.
    ///
    /// No `workspace_id`: herdr puts it in whichever workspace is active, which is the one the
    /// user is looking at. Naming one from a config file would mean a key that opened a tab
    /// somewhere the user cannot see.
    pub async fn tab_create(&self, spec: &CreateSpec) -> Result<()> {
        self.call_raw("tab.create", spec.to_params(true))
            .await
            .map(|_| ())
    }

    /// Close one tab, by id.
    ///
    /// Destructive, and it cascades the same way [`Self::pane_close`] does: the last tab of a
    /// workspace takes the workspace with it, and herdr answers the same bare `ok` either way.
    pub async fn tab_close(&self, tab_id: &str) -> Result<()> {
        self.call_raw("tab.close", json!({ "tab_id": tab_id }))
            .await
            .map(|_| ())
    }

    /// Build a new tab from a named layout.
    ///
    /// Sent **without** `tab_id`, and this function offers no way to supply one. With a tab id
    /// `layout.apply` does not arrange that tab — it builds a replacement and then *closes* the
    /// named one, killing every process in it, with no confirmation and no warning in its name.
    /// Additive is the only form of this command that belongs on a key you can lean on.
    pub async fn layout_apply(&self, preset: &LayoutPreset) -> Result<()> {
        let mut params = json!({ "root": preset.root.to_params(), "focus": true });
        if let Some(label) = &preset.label {
            params["tab_label"] = json!(label);
        }
        self.call_raw("layout.apply", params).await.map(|_| ())
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
        // Shaped as herdr 0.8.0 actually answers — captured from a running herdr, not composed
        // from the docs. The snapshot is nested under `snapshot`; reading the result directly
        // yields an all-default snapshot instead of an error, which is how this shipped.
        mock.reply(
            "session.snapshot",
            json!({
                "type": "session_snapshot",
                "snapshot": {
                    "version": "0.8.0",
                    "protocol": 19,
                    "focused_workspace_id": "w1",
                    "workspaces": [{"workspace_id":"w1","label":"api","agent_status":"blocked"}],
                    "agents": [{
                        "terminal_id":"term_a","agent_status":"blocked","workspace_id":"w1",
                        "tab_id":"w1:t1","pane_id":"w1:p1","focused":false,"revision":1
                    }]
                }
            }),
        )
        .await;
        let client = HerdrClient::new(mock.socket_path());
        let snap = client.session_snapshot().await.unwrap();
        assert_eq!(snap.protocol, Some(19));
        assert_eq!(snap.agents[0].terminal_id, "term_a");
    }

    #[tokio::test]
    async fn a_snapshot_that_is_not_wrapped_is_an_error_and_not_an_empty_session() {
        // The failure that shipped: read one level too high, every field defaults, and the deck
        // shows a herdr with nothing running while `doctor` says herdr sent no protocol version.
        // Silence is the worst possible outcome here, so it must be loud.
        let mock = MockHerdr::start().await;
        mock.reply(
            "session.snapshot",
            json!({ "type": "session_snapshot", "protocol": 19, "agents": [] }),
        )
        .await;
        let client = HerdrClient::new(mock.socket_path());
        let err = client
            .session_snapshot()
            .await
            .expect_err("an unwrapped snapshot must not decode as an empty session");
        assert!(matches!(err, crate::HerdrError::Decode { .. }));
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

    // --- Pane control ------------------------------------------------------------------------

    #[tokio::test]
    async fn a_direction_key_names_only_a_direction_and_never_a_pane() {
        // With a pane id herdr navigates to that pane before moving, so a key labelled "left"
        // would first teleport its owner somewhere they did not ask to be.
        let mock = MockHerdr::start().await;
        mock.serve_panes().await;
        let client = HerdrClient::new(mock.socket_path());

        client
            .pane_focus_direction(crate::wire::PaneDirection::Left)
            .await
            .unwrap();

        let params = mock.observed_params("pane.focus_direction").await.unwrap();
        assert_eq!(params["direction"], "left");
        assert!(
            params.get("pane_id").is_none(),
            "naming a pane turns a direction key into a navigation key: {params}"
        );
    }

    #[tokio::test]
    async fn zoom_states_the_end_it_wants_and_never_asks_herdr_to_toggle() {
        // A toggle flips a boolean the deck cannot see, so the same press means different things
        // depending on state that may have moved since the deck last looked.
        let mock = MockHerdr::start().await;
        mock.serve_panes().await;
        let client = HerdrClient::new(mock.socket_path());

        client.pane_zoom(crate::wire::ZoomMode::On).await.unwrap();
        let params = mock.observed_params("pane.zoom").await.unwrap();
        assert_eq!(params["mode"], "on");
        assert!(params.get("pane_id").is_none());
        assert_ne!(params["mode"], "toggle");
    }

    #[tokio::test]
    async fn a_split_asks_for_the_new_pane_to_be_focused_because_nothing_else_will() {
        let mock = MockHerdr::start().await;
        mock.serve_panes().await;
        let client = HerdrClient::new(mock.socket_path());

        client
            .pane_split(crate::wire::SplitDirection::Down)
            .await
            .unwrap();
        let params = mock.observed_params("pane.split").await.unwrap();
        assert_eq!(params["direction"], "down");
        assert_eq!(params["focus"], true);
    }

    #[tokio::test]
    async fn a_confirmation_herdr_wants_is_told_apart_from_an_ordinary_refusal() {
        // This is the only refusal that leaves a dialog on the user's screen. It has to arrive as
        // something a key can word differently, or the user is told "that failed" while herdr sits
        // waiting for an answer they never learn it wants.
        let mock = MockHerdr::start().await;
        mock.reply_error(
            "pane.close",
            "confirmation_required",
            "closing this would close the worktree group",
        )
        .await;
        let client = HerdrClient::new(mock.socket_path());

        let err = client.pane_close("w1:p1").await.unwrap_err();
        assert!(
            matches!(err, HerdrError::ConfirmationRequired { .. }),
            "got {err:?}"
        );
        assert!(err.to_string().contains("its own window"), "got {err}");
    }

    // --- Structure ---------------------------------------------------------------------------

    #[tokio::test]
    async fn applying_a_layout_never_names_a_tab_because_naming_one_would_close_it() {
        // The single most important assertion in this file. `layout.apply` with a `tab_id` builds
        // the replacement and then closes the named tab, killing every process in it — with no
        // confirmation and nothing in the method's name to suggest it. Without a tab id the same
        // call is purely additive.
        let mock = MockHerdr::start().await;
        mock.serve_structure().await;
        let client = HerdrClient::new(mock.socket_path());

        let preset = crate::wire::LayoutPreset {
            label: Some("dev".into()),
            root: crate::wire::LayoutNode {
                split: Some(SplitDirection::Down),
                ratio: Some(70),
                first: Some(Box::default()),
                second: Some(Box::default()),
                ..Default::default()
            },
        };
        client.layout_apply(&preset).await.unwrap();

        let params = mock.observed_params("layout.apply").await.unwrap();
        assert!(
            params.get("tab_id").is_none(),
            "a preset must never target an existing tab: {params}"
        );
        assert!(
            params.get("workspace_id").is_none(),
            "and must land in the workspace the user is in: {params}"
        );
        assert_eq!(params["tab_label"], "dev");
        assert_eq!(params["root"]["ratio"], 0.7);
    }

    #[tokio::test]
    async fn opening_a_worktree_asks_for_it_to_be_focused_and_names_it_by_path() {
        let mock = MockHerdr::start().await;
        mock.serve_structure().await;
        let client = HerdrClient::new(mock.socket_path());

        client
            .worktree_open("/src/.worktrees/api/fix")
            .await
            .unwrap();

        let params = mock.observed_params("worktree.open").await.unwrap();
        assert_eq!(params["path"], "/src/.worktrees/api/fix");
        assert_eq!(params["focus"], true);
        assert!(
            params.get("branch").is_none(),
            "exactly one of path or branch may be sent: {params}"
        );
    }

    #[tokio::test]
    async fn creating_a_worktree_supplies_nothing_at_all_and_still_works() {
        // The reason a "new worktree" key is possible on hardware with no keyboard: herdr invents
        // the branch name, the base and the path itself.
        let mock = MockHerdr::start().await;
        mock.serve_structure().await;
        let client = HerdrClient::new(mock.socket_path());

        client.worktree_create().await.unwrap();

        let params = mock.observed_params("worktree.create").await.unwrap();
        assert_eq!(params, serde_json::json!({"focus": true}));
    }

    #[tokio::test]
    async fn removing_a_worktree_is_never_forced() {
        // With force, herdr kills every agent in the workspace *before* git runs and then deletes
        // uncommitted and untracked files. Without it, git refuses a dirty checkout and that
        // refusal is the confirmation prompt this hardware cannot otherwise show.
        let mock = MockHerdr::start().await;
        mock.serve_structure().await;
        let client = HerdrClient::new(mock.socket_path());

        client.worktree_remove("w3").await.unwrap();

        let params = mock.observed_params("worktree.remove").await.unwrap();
        assert_eq!(params["workspace_id"], "w3");
        assert_eq!(
            params["force"], false,
            "force must be sent, and sent false: {params}"
        );
    }

    #[tokio::test]
    async fn a_dirty_checkout_refusing_to_be_removed_reaches_the_caller_as_an_error() {
        // git's refusal is the whole safety mechanism, so swallowing it would leave the user
        // believing a worktree was removed when it is still there.
        let mock = MockHerdr::start().await;
        mock.reply_error(
            "worktree.remove",
            "worktree_remove_failed",
            "contains modified or untracked files, use --force to delete it",
        )
        .await;
        let client = HerdrClient::new(mock.socket_path());

        let err = client.worktree_remove("w3").await.unwrap_err();
        assert!(err.to_string().contains("untracked"), "got {err}");
    }

    #[tokio::test]
    async fn a_new_tab_lands_in_the_workspace_the_user_is_looking_at() {
        let mock = MockHerdr::start().await;
        mock.serve_structure().await;
        let client = HerdrClient::new(mock.socket_path());

        client
            .tab_create(&crate::wire::CreateSpec {
                label: Some("logs".into()),
                ..Default::default()
            })
            .await
            .unwrap();

        let params = mock.observed_params("tab.create").await.unwrap();
        assert_eq!(params["label"], "logs");
        assert_eq!(params["focus"], true);
        assert!(
            params.get("workspace_id").is_none(),
            "a workspace named in a config file is one the user cannot see: {params}"
        );
    }

    #[tokio::test]
    async fn listing_worktrees_outside_a_repository_is_an_ordinary_error() {
        let mock = MockHerdr::start().await;
        mock.with_nothing_under_git().await;
        let client = HerdrClient::new(mock.socket_path());

        let err = client.worktree_list().await.unwrap_err();
        assert!(matches!(err, HerdrError::Rpc { .. }), "got {err:?}");
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
