//! The daemon, end to end.
//!
//! Every layer below this has its own unit tests, but the seam where they meet — a real unix
//! socket, a real watcher polling a real (mock) herdr, a real session deciding what to paint —
//! only ever got exercised by hand. These tests drive that seam: a fake frontend speaking the
//! documented newline-delimited JSON protocol against `server::bind` + `server::serve`, with
//! `MockHerdr` standing in for herdr underneath.
//!
//! # Never sleep, always wait for something
//!
//! Nothing here waits a fixed duration for a result. Every wait is either "read until the
//! message we expect arrives" or "watch the state until it says what we need", each wrapped in
//! a timeout so a regression fails the suite instead of hanging it. Where a test needs to prove
//! that *nothing* was sent, it sends a `ping` and reads up to the `pong`: the daemon writes each
//! batch of messages in one go, so anything it decided to send earlier is already in the stream
//! ahead of that pong.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use herdr_deck_core::protocol::DaemonMessage;
use herdr_deck_core::render::{Tile, TileRenderer};
use herdr_deck_core::state::DeckState;
use herdr_deck_core::Config;
use herdr_deck_focus::FocusEngine;
use herdr_deck_herdr::mock::MockHerdr;
use herdr_deck_herdr::wire::{AgentInfo, AgentStatus, SessionSnapshot, WorkspaceInfo};
use herdr_deck_herdr::{HerdrClient, SocketPath};
use herdr_deckd::audit::AuditLog;
use herdr_deckd::server::{self, ServerContext};
use herdr_deckd::watcher::Watcher;
use serde_json::json;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::UnixStream;
use tokio::sync::watch;
use tokio::time::timeout;

/// Long enough that a loaded CI box does not fail spuriously, short enough that a wedged daemon
/// fails the test rather than hanging the suite.
const PATIENCE: Duration = Duration::from_secs(10);

/// A Stream Deck + as the macOS plugin reports it.
const PLUS_KEYS: usize = 8;
const PLUS_DIALS: usize = 4;
const KEY_PX: u32 = 120;

// ---------------------------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------------------------

/// The config every test runs with.
///
/// Window raising is off because these tests must not shell out to a window manager that may or
/// may not exist on the machine running them; the focus path is still fully exercised, and the
/// deck reports the partial success as an `alert`, which is exactly the behaviour we want to
/// pin down. The reconcile interval is short so a change in herdr shows up promptly — tests
/// still wait for the resulting message rather than for the interval.
fn test_config() -> Config {
    let mut config = Config {
        reconcile_interval_ms: 100,
        ..Config::default()
    };
    config.focus.raise_window = false;
    config
}

/// A running daemon: watcher, socket, and accept loop, exactly as `main` wires them.
struct Daemon {
    socket: PathBuf,
    state: watch::Receiver<Arc<DeckState>>,
    serve: tokio::task::JoinHandle<()>,
    audit: Arc<AuditLog>,
    audit_path: PathBuf,
    // Held so the socket's directory outlives the listener.
    _dir: tempfile::TempDir,
}

impl Daemon {
    async fn start(client: HerdrClient) -> Self {
        Self::start_with(client, test_config()).await
    }

    async fn start_with(client: HerdrClient, config: Config) -> Self {
        let renderer = Arc::new(TileRenderer::new(config.theme));
        let focus = Arc::new(FocusEngine::new(client.clone(), config.focus.clone()));
        let state = Watcher::new(client, &config).spawn();

        let dir = tempfile::tempdir().expect("tempdir");
        let socket = dir.path().join("herdr-deck.sock");
        let listener = server::bind(&socket).await.expect("bind frontend socket");

        let audit_path = dir.path().join("commands.jsonl");
        let audit = Arc::new(AuditLog::open(audit_path.clone()));

        let context = Arc::new(ServerContext {
            config,
            renderer,
            focus,
            state: state.clone(),
            audit: Arc::clone(&audit),
        });
        let serve = tokio::spawn(async move {
            let _ = server::serve(listener, context).await;
        });

        Self {
            socket,
            state,
            serve,
            audit,
            audit_path,
            _dir: dir,
        }
    }

    /// Every command this daemon recorded, once everything queued has reached the file.
    async fn audit_lines(&self) -> Vec<serde_json::Value> {
        self.audit.flush().await;
        std::fs::read_to_string(&self.audit_path)
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str(line).expect("every audit line is valid JSON"))
            .collect()
    }

    /// Block until the daemon's view of herdr satisfies `pred`.
    ///
    /// Tests connect their frontend only once this says the state is what they meant to test
    /// against, so the first paint is never a race against the bootstrap snapshot.
    async fn wait_for_state(&mut self, mut pred: impl FnMut(&DeckState) -> bool) {
        timeout(PATIENCE, async {
            loop {
                if pred(&self.state.borrow_and_update().clone()) {
                    return;
                }
                self.state.changed().await.expect("watcher stays alive");
            }
        })
        .await
        .expect("the daemon never reached the state this test needs");
    }

    async fn wait_until_online(&mut self) {
        self.wait_for_state(|state| !state.is_offline()).await;
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        self.serve.abort();
    }
}

/// A frontend, as dumb as the real ones: it writes JSON lines and reads JSON lines.
struct Frontend {
    writer: OwnedWriteHalf,
    lines: Lines<BufReader<OwnedReadHalf>>,
}

impl Frontend {
    async fn connect(daemon: &Daemon) -> Self {
        Self::connect_to(&daemon.socket).await
    }

    async fn connect_to(path: &Path) -> Self {
        let stream = timeout(PATIENCE, UnixStream::connect(path))
            .await
            .expect("connecting to the daemon socket timed out")
            .expect("the daemon should be accepting connections");
        let (read, writer) = stream.into_split();
        Self {
            writer,
            lines: BufReader::new(read).lines(),
        }
    }

    async fn send(&mut self, message: serde_json::Value) {
        let line = format!("{message}\n");
        self.writer
            .write_all(line.as_bytes())
            .await
            .expect("writing to the daemon");
        self.writer.flush().await.expect("flushing to the daemon");
    }

    /// Press and release a key quickly enough that the daemon reads it as an ordinary press.
    async fn tap(&mut self, index: usize) {
        self.send(json!({"type": "key_down", "index": index})).await;
        self.send(json!({"type": "key_up", "index": index})).await;
    }

    /// Press a key, hold it past the long-press threshold, and let go.
    ///
    /// The one wait in this file that is not waiting for a result. The *duration* is the input
    /// here — a gesture is defined by how long it lasted — so there is nothing to observe
    /// instead. It is generous over the daemon's 500ms so a loaded box does not read it as a tap.
    async fn hold(&mut self, index: usize) {
        self.send(json!({"type": "key_down", "index": index})).await;
        tokio::time::sleep(Duration::from_millis(700)).await;
        self.send(json!({"type": "key_up", "index": index})).await;
    }

    /// The next line, or `None` if the daemon closed the connection.
    async fn next_line(&mut self) -> Option<String> {
        timeout(PATIENCE, self.lines.next_line())
            .await
            .expect("the daemon sent nothing before the deadline")
            .expect("reading from the daemon")
    }

    async fn read_one(&mut self) -> DaemonMessage {
        let line = self
            .next_line()
            .await
            .expect("the daemon closed the connection unexpectedly");
        serde_json::from_str(&line)
            .unwrap_or_else(|e| panic!("undecodable daemon line {line}: {e}"))
    }

    /// Read exactly `count` messages, failing rather than hanging if they do not arrive.
    async fn read(&mut self, count: usize) -> Vec<DaemonMessage> {
        let mut messages = Vec::with_capacity(count);
        for _ in 0..count {
            messages.push(self.read_one().await);
        }
        messages
    }

    /// Read until `pred` matches, returning everything up to but excluding the match.
    async fn read_until(
        &mut self,
        mut pred: impl FnMut(&DaemonMessage) -> bool,
    ) -> (Vec<DaemonMessage>, DaemonMessage) {
        let mut before = Vec::new();
        loop {
            let message = self.read_one().await;
            if pred(&message) {
                return (before, message);
            }
            before.push(message);
        }
    }

    /// Everything the daemon had already decided to send, up to a fresh `pong`.
    ///
    /// This is how a test asserts silence without sleeping: the reply to a ping cannot overtake
    /// messages the daemon wrote before it.
    async fn drain_to_pong(&mut self) -> Vec<DaemonMessage> {
        self.send(json!({"type": "ping"})).await;
        let (before, _pong) = self.read_until(|m| matches!(m, DaemonMessage::Pong)).await;
        before
    }

    /// Assert the daemon hung up, rather than merely going quiet.
    async fn expect_disconnected(&mut self) {
        assert!(
            self.next_line().await.is_none(),
            "the daemon should have closed this connection"
        );
    }
}

// ---------------------------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------------------------

fn hello(dials: u8) -> serde_json::Value {
    let touchstrip = (dials > 0).then(|| json!({"width": 800, "height": 100}));
    json!({
        "type": "hello",
        "frontend": "test-frontend",
        "protocol": 1,
        "device": {
            "model": "plus",
            "model_name": "Stream Deck +",
            "columns": 4,
            "rows": 2,
            "key_image_px": KEY_PX,
            "dials": dials,
            "touchstrip": touchstrip,
        }
    })
}

// Fixtures are built from herdr-deck-herdr's own wire types rather than hand-written JSON.
// Everything above that crate is supposed to hold no herdr protocol knowledge, and spelling
// field names here would put a second, unversioned copy of the protocol in a test file — the
// place it would rot most quietly.
fn agent(terminal_id: &str, pane_id: &str, name: &str, status: AgentStatus, seq: u64) -> AgentInfo {
    AgentInfo {
        terminal_id: terminal_id.to_string(),
        agent_status: status,
        workspace_id: "w1".to_string(),
        tab_id: "w1:t1".to_string(),
        pane_id: pane_id.to_string(),
        agent: Some("claude".to_string()),
        name: Some(name.to_string()),
        state_change_seq: Some(seq),
        revision: 1,
        ..Default::default()
    }
}

fn snapshot(agents: Vec<AgentInfo>) -> SessionSnapshot {
    SessionSnapshot {
        protocol: Some(herdr_deck_herdr::EXPECTED_PROTOCOL),
        agents,
        workspaces: vec![WorkspaceInfo {
            workspace_id: "w1".to_string(),
            label: Some("api".to_string()),
            agent_status: AgentStatus::Blocked,
            focused: true,
            ..Default::default()
        }],
        ..Default::default()
    }
}

/// A blocked agent first, an idle one behind it. The blocked one owns key 0.
fn two_agents(second_status: AgentStatus) -> SessionSnapshot {
    snapshot(vec![
        agent(
            "term_a",
            "w1:p9",
            "refactor-auth",
            AgentStatus::Blocked,
            100,
        ),
        agent("term_b", "w1:p2", "flaky-tests", second_status, 50),
    ])
}

/// A mock herdr that answers a snapshot and accepts focus calls.
async fn mock_herdr(snapshot: SessionSnapshot) -> MockHerdr {
    let mock = MockHerdr::start().await;
    mock.serve_session(&snapshot).await;
    mock
}

fn key_images(messages: &[DaemonMessage]) -> Vec<(usize, String)> {
    messages
        .iter()
        .filter_map(|m| match m {
            DaemonMessage::SetKeyImage { index, png } => Some((*index, png.clone())),
            _ => None,
        })
        .collect()
}

fn dial_indices(messages: &[DaemonMessage]) -> Vec<usize> {
    messages
        .iter()
        .filter_map(|m| match m {
            DaemonMessage::SetDialFeedback { dial, .. } => Some(*dial),
            _ => None,
        })
        .collect()
}

// ---------------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn a_hello_is_answered_with_ready_and_one_image_for_every_key_and_dial() {
    let mock = mock_herdr(two_agents(AgentStatus::Idle)).await;
    let mut daemon = Daemon::start(HerdrClient::new(mock.socket_path())).await;
    daemon.wait_until_online().await;

    let mut frontend = Frontend::connect(&daemon).await;
    frontend.send(hello(PLUS_DIALS as u8)).await;

    let greeting = frontend.read(1 + PLUS_KEYS + PLUS_DIALS).await;
    match &greeting[0] {
        DaemonMessage::Ready {
            protocol,
            keys,
            dials,
            device,
        } => {
            assert_eq!(*protocol, herdr_deck_core::protocol::FRONTEND_PROTOCOL);
            assert_eq!(*keys, PLUS_KEYS);
            assert_eq!(*dials, PLUS_DIALS);
            assert!(device.contains("Stream Deck +"), "got device {device}");
        }
        other => panic!("the first message must be ready, got {other:?}"),
    }

    // A first paint has to cover the whole deck: the frontend starts with blank keys and the
    // daemon only ever sends what it believes has changed.
    let painted: Vec<usize> = key_images(&greeting[1..])
        .into_iter()
        .map(|(i, _)| i)
        .collect();
    assert_eq!(painted, (0..PLUS_KEYS).collect::<Vec<_>>());
    assert_eq!(
        dial_indices(&greeting[1..]),
        (0..PLUS_DIALS).collect::<Vec<_>>()
    );

    assert!(
        frontend.drain_to_pong().await.is_empty(),
        "a settled deck should be repainted with nothing at all"
    );
}

#[tokio::test]
async fn a_frontend_speaking_a_different_protocol_is_told_so_and_disconnected() {
    // The macOS plugin installs separately from the daemon, so version skew is real. Failing
    // loudly here is the difference between "reinstall the plugin" and "the deck is broken".
    let mock = mock_herdr(two_agents(AgentStatus::Idle)).await;
    let mut daemon = Daemon::start(HerdrClient::new(mock.socket_path())).await;
    daemon.wait_until_online().await;

    let mut frontend = Frontend::connect(&daemon).await;
    let mut greeting = hello(PLUS_DIALS as u8);
    greeting["protocol"] = json!(999);
    frontend.send(greeting).await;

    match frontend.read_one().await {
        DaemonMessage::ProtocolMismatch { expected, got } => {
            assert_eq!(expected, herdr_deck_core::protocol::FRONTEND_PROTOCOL);
            assert_eq!(got, 999);
        }
        other => panic!("expected protocol_mismatch, got {other:?}"),
    }
    frontend.expect_disconnected().await;
}

#[tokio::test]
async fn a_frontend_that_does_not_say_hello_first_is_rejected() {
    // Without a hello there is no hardware description, so there is no layout and nothing
    // sensible to paint. Dropping the connection is better than serving a guess.
    let mock = mock_herdr(two_agents(AgentStatus::Idle)).await;
    let mut daemon = Daemon::start(HerdrClient::new(mock.socket_path())).await;
    daemon.wait_until_online().await;

    let mut frontend = Frontend::connect(&daemon).await;
    frontend.send(json!({"type": "key_down", "index": 0})).await;
    frontend.expect_disconnected().await;

    // The daemon must still be serving everyone else.
    let mut good = Frontend::connect(&daemon).await;
    good.send(hello(PLUS_DIALS as u8)).await;
    assert!(matches!(good.read_one().await, DaemonMessage::Ready { .. }));
}

#[tokio::test]
async fn a_change_in_herdr_repaints_the_deck_unasked_and_resends_only_the_keys_that_changed() {
    let mock = mock_herdr(two_agents(AgentStatus::Idle)).await;
    let mut daemon = Daemon::start(HerdrClient::new(mock.socket_path())).await;
    daemon.wait_until_online().await;

    let mut frontend = Frontend::connect(&daemon).await;
    frontend.send(hello(PLUS_DIALS as u8)).await;
    let greeting = frontend.read(1 + PLUS_KEYS + PLUS_DIALS).await;
    let before = key_images(&greeting);

    // The second agent starts working. It keeps its place in the attention order (blocked still
    // sorts first) and it is not an attention state, so exactly one tile can have changed.
    mock.serve_session(&two_agents(AgentStatus::Working)).await;

    // Nobody asked for this: the frontend is not polling, the daemon pushes.
    let repaint = frontend.read(1).await;
    let after = key_images(&repaint);
    assert_eq!(after.len(), 1, "expected one key image, got {repaint:?}");
    assert_eq!(after[0].0, 1, "key 1 is the agent whose status changed");
    assert_ne!(
        after[0].1, before[1].1,
        "the repainted key should carry a different image"
    );

    assert!(
        frontend.drain_to_pong().await.is_empty(),
        "keys that did not change must not be resent — the deck is a serial device and \
         redundant traffic shows up as lag"
    );
}

#[tokio::test]
async fn pressing_a_blocked_agents_key_focuses_that_agents_pane_and_not_its_terminal_id() {
    // herdr's `agent.focus` does not accept a terminal_id, and terminal ids are what the layout
    // binds to. Getting this resolution wrong focuses nothing at all.
    let mock = mock_herdr(two_agents(AgentStatus::Idle)).await;
    let mut daemon = Daemon::start(HerdrClient::new(mock.socket_path())).await;
    daemon.wait_until_online().await;

    let mut frontend = Frontend::connect(&daemon).await;
    frontend.send(hello(PLUS_DIALS as u8)).await;
    frontend.read(1 + PLUS_KEYS + PLUS_DIALS).await;

    // Key 0 shows the top of the attention order, which is the blocked agent.
    frontend.tap(0).await;

    // Window raising is disabled in the test config, so this is a partial success — and the
    // deck says so rather than pretending everything happened.
    match frontend.read_one().await {
        DaemonMessage::Alert { index, message } => {
            assert_eq!(index, 0);
            assert!(
                message.contains("herdr focused"),
                "the alert should say what did work: {message}"
            );
        }
        other => panic!("expected feedback on the pressed key, got {other:?}"),
    }

    assert_eq!(
        mock.agent_focus_targets().await,
        vec!["w1:p9"],
        "focus takes the pane id; the terminal id the key is bound to would not resolve"
    );
}

#[tokio::test]
async fn a_command_that_reached_herdr_leaves_a_line_in_the_audit_log() {
    // The trail has to be written where the command actually leaves the daemon, or it will
    // eventually describe presses that never became anything.
    let mock = mock_herdr(two_agents(AgentStatus::Idle)).await;
    let mut daemon = Daemon::start(HerdrClient::new(mock.socket_path())).await;
    daemon.wait_until_online().await;

    let mut frontend = Frontend::connect(&daemon).await;
    frontend.send(hello(PLUS_DIALS as u8)).await;
    frontend.read(1 + PLUS_KEYS + PLUS_DIALS).await;

    frontend.tap(0).await;
    frontend.drain_to_pong().await;

    let recorded = daemon.audit_lines().await;
    assert_eq!(recorded.len(), 1, "one command, one line: {recorded:?}");
    assert_eq!(recorded[0]["command"], "focus_pane");
    assert_eq!(
        recorded[0]["target"], "w1:p9",
        "the log names the pane herdr was actually asked about"
    );
    // Window raising is off in the test config, so herdr focused and nothing was raised.
    assert_eq!(recorded[0]["outcome"], "partial");
    assert!(
        recorded[0]["at"]
            .as_str()
            .is_some_and(|at| at.ends_with('Z')),
        "got {}",
        recorded[0]["at"]
    );
}

#[tokio::test]
async fn an_action_that_never_reaches_herdr_leaves_no_trace_in_the_audit_log() {
    // Acknowledging, paging and switching mode rearrange the deck's own view of herdr and change
    // nothing in it. Recording those would bury the handful of lines that matter — and would put
    // a line on disk for something that, from herdr's side, never happened.
    let mock = mock_herdr(two_agents(AgentStatus::Done)).await;
    let mut daemon = Daemon::start(HerdrClient::new(mock.socket_path())).await;
    daemon.wait_until_online().await;

    let mut frontend = Frontend::connect(&daemon).await;
    frontend.send(hello(PLUS_DIALS as u8)).await;
    frontend.read(1 + PLUS_KEYS + PLUS_DIALS).await;

    // Key 1 is the `done` agent: holding it dismisses without focusing.
    frontend.hold(1).await;
    // ...and scrubbing a dial only moves a cursor.
    frontend
        .send(json!({"type": "dial_rotate", "dial": 0, "ticks": 1}))
        .await;
    frontend.drain_to_pong().await;

    assert!(
        mock.agent_focus_targets().await.is_empty(),
        "precondition: nothing should have reached herdr"
    );
    assert!(
        daemon.audit_lines().await.is_empty(),
        "the log is for commands, not for the deck rearranging itself"
    );
}

#[tokio::test]
async fn a_dial_press_is_audited_even_though_there_is_no_key_to_report_it_on() {
    // A dial carries no key index, so its outcome never reaches the deck's face. That is exactly
    // the press the log is most worth having: the only other record of it is a window that may or
    // may not have moved.
    let mock = mock_herdr(two_agents(AgentStatus::Idle)).await;
    let mut daemon = Daemon::start(HerdrClient::new(mock.socket_path())).await;
    daemon.wait_until_online().await;

    let mut frontend = Frontend::connect(&daemon).await;
    frontend.send(hello(PLUS_DIALS as u8)).await;
    frontend.read(1 + PLUS_KEYS + PLUS_DIALS).await;

    // Dial 0 scrubs agents and its press focuses whichever one the cursor is on.
    frontend.send(json!({"type": "dial_down", "dial": 0})).await;
    frontend.drain_to_pong().await;

    let recorded = daemon.audit_lines().await;
    assert_eq!(recorded.len(), 1, "got {recorded:?}");
    assert_eq!(recorded[0]["command"], "focus_pane");
    assert_eq!(recorded[0]["target"], "w1:p9");
}

#[tokio::test]
async fn holding_an_agents_key_clears_it_from_the_attention_queue_without_touching_herdr() {
    // The whole reason the gesture exists: clearing a finished agent must not focus it, and so
    // must not drag its terminal window in front of whatever you were doing. A `done` agent that
    // herdr had been asked to focus would also be marked seen, so "herdr was never called" is the
    // property to pin — not merely "no window moved".
    let mock = mock_herdr(two_agents(AgentStatus::Done)).await;
    let mut daemon = Daemon::start(HerdrClient::new(mock.socket_path())).await;
    daemon.wait_until_online().await;

    let mut frontend = Frontend::connect(&daemon).await;
    frontend.send(hello(PLUS_DIALS as u8)).await;
    let greeting = frontend.read(1 + PLUS_KEYS + PLUS_DIALS).await;
    let before = key_images(&greeting);

    // Key 1 is the `done` agent, behind the blocked one.
    frontend.hold(1).await;

    let repaint = frontend.drain_to_pong().await;
    let after = key_images(&repaint);
    assert!(
        after
            .iter()
            .any(|(index, png)| *index == 1 && Some(png) != before.get(1).map(|(_, p)| p)),
        "the dismissed agent's tile must go calm, got {repaint:?}"
    );
    assert!(
        !repaint
            .iter()
            .any(|m| matches!(m, DaemonMessage::Ok { .. } | DaemonMessage::Alert { .. })),
        "a long press asks for no action, so there is no action to report on, got {repaint:?}"
    );
    assert!(
        mock.agent_focus_targets().await.is_empty(),
        "herdr must not be called at all"
    );
}

#[tokio::test]
async fn a_re_hello_reporting_new_hardware_rebuilds_the_layout_and_greets_again() {
    // The macOS plugin discovers its dials as their actions appear, so the first hello can
    // honestly say "no dials" and a later one "four".
    let mock = mock_herdr(two_agents(AgentStatus::Idle)).await;
    let mut daemon = Daemon::start(HerdrClient::new(mock.socket_path())).await;
    daemon.wait_until_online().await;

    let mut frontend = Frontend::connect(&daemon).await;
    frontend.send(hello(0)).await;
    let greeting = frontend.read(1 + PLUS_KEYS).await;
    match &greeting[0] {
        DaemonMessage::Ready { dials, .. } => assert_eq!(*dials, 0),
        other => panic!("expected ready, got {other:?}"),
    }

    frontend.send(hello(PLUS_DIALS as u8)).await;
    let (_before, ready) = frontend
        .read_until(|m| matches!(m, DaemonMessage::Ready { .. }))
        .await;
    match ready {
        DaemonMessage::Ready { keys, dials, .. } => {
            assert_eq!(dials, PLUS_DIALS, "the dials the frontend grew must appear");
            assert_eq!(keys, PLUS_KEYS);
        }
        other => unreachable!("read_until returned {other:?}"),
    }

    // A rebuilt layout means every control is now something else; all of them get repainted.
    let repaint = frontend.read(PLUS_KEYS + PLUS_DIALS).await;
    assert_eq!(key_images(&repaint).len(), PLUS_KEYS);
    assert_eq!(dial_indices(&repaint).len(), PLUS_DIALS);
}

#[tokio::test]
async fn when_herdr_is_unreachable_every_key_says_so_and_pressing_one_does_nothing() {
    // A half-live deck is worse than an obviously dead one: the whole deck states the problem,
    // and no key pretends it can still act.
    let mut daemon = Daemon::start(HerdrClient::new(SocketPath {
        path: PathBuf::from("/nonexistent/herdr.sock"),
        origin: herdr_deck_herdr::socket::SocketOrigin::Default,
    }))
    .await;
    daemon
        .wait_for_state(|state| state.offline_reason.as_deref() == Some("herdr not running"))
        .await;

    let mut frontend = Frontend::connect(&daemon).await;
    frontend.send(hello(PLUS_DIALS as u8)).await;
    let greeting = frontend.read(1 + PLUS_KEYS + PLUS_DIALS).await;

    let offline = TileRenderer::new(test_config().theme)
        .render_key(
            &Tile::Offline {
                message: "herdr not running".to_string(),
            },
            KEY_PX,
        )
        .expect("the offline tile renders");
    let offline = base64::engine::general_purpose::STANDARD.encode(offline);
    for (index, png) in key_images(&greeting) {
        assert_eq!(png, offline, "key {index} should show the offline tile");
    }

    frontend.send(json!({"type": "key_down", "index": 0})).await;
    assert!(
        frontend.drain_to_pong().await.is_empty(),
        "an offline deck must not act on a press — and must stay connected to say so"
    );
}

// ---------------------------------------------------------------------------------------------
// Pane control
// ---------------------------------------------------------------------------------------------
//
// These are the keys someone presses without looking, dozens of times a session, while sitting at
// the terminal they are pressing them about. What matters end to end is not that the right method
// went down the socket — that is pinned closer to the wire — but what the *key* does afterwards:
// which presses are worth an alert, which are not, and what the trail says the next morning.

/// A deck laid out for pane work: an arrow, and a key that closes the focused pane.
fn pane_config() -> Config {
    let mut config = test_config();
    config.layout = Some(herdr_deck_core::config::LayoutOverride {
        keys: vec![
            herdr_deck_core::layout::KeyBinding::Command {
                command: herdr_deck_core::command::DeckCommand::MovePaneFocus {
                    direction: herdr_deck_herdr::wire::PaneDirection::Left,
                },
            },
            herdr_deck_core::layout::KeyBinding::ClosePane,
        ],
        dials: vec![],
    });
    config
}

/// A herdr with one idle agent and a pane the user is sitting in.
fn snapshot_with_a_focused_pane() -> SessionSnapshot {
    let mut snapshot = snapshot(vec![agent(
        "term_a",
        "w1:p9",
        "refactor-auth",
        AgentStatus::Idle,
        100,
    )]);
    snapshot.focused_pane_id = Some("w1:p9".to_string());
    snapshot
}

/// Press key `index` and read back what the daemon put on its face.
async fn feedback_from(frontend: &mut Frontend) -> DaemonMessage {
    let (_before, feedback) = frontend
        .read_until(|m| matches!(m, DaemonMessage::Ok { .. } | DaemonMessage::Alert { .. }))
        .await;
    feedback
}

#[tokio::test]
async fn a_direction_key_reads_as_success_even_though_no_window_was_raised() {
    // Window raising is off in these tests, which is exactly the situation that makes an agent
    // focus alert. A pane move must not: it never wanted the window, so nothing was left undone
    // and the key has nothing to complain about.
    let mock = mock_herdr(snapshot_with_a_focused_pane()).await;
    let mut daemon = Daemon::start_with(HerdrClient::new(mock.socket_path()), pane_config()).await;
    daemon.wait_until_online().await;

    let mut frontend = Frontend::connect(&daemon).await;
    frontend.send(hello(0)).await;
    frontend.read(1 + PLUS_KEYS).await;

    frontend.tap(0).await;
    assert_eq!(
        feedback_from(&mut frontend).await,
        DaemonMessage::Ok { index: 0 }
    );

    let recorded = daemon.audit_lines().await;
    assert_eq!(recorded.len(), 1, "one press, one line: {recorded:?}");
    assert_eq!(recorded[0]["command"], "move_pane_focus");
    assert_eq!(
        recorded[0]["target"], "left",
        "a command with no id still has to record which one it was"
    );
    assert_eq!(recorded[0]["outcome"], "settled");
}

#[tokio::test]
async fn running_a_thumb_off_the_edge_of_a_layout_never_flashes_an_alert() {
    // The press this whole distinction exists for. herdr answers "there is no pane that way" as a
    // success carrying a reason; if the deck turned that into an alert, every user would meet one
    // within a minute of finding these keys and would stop reading alerts by the end of the day.
    let mock = mock_herdr(snapshot_with_a_focused_pane()).await;
    mock.with_no_pane_that_way().await;
    let mut daemon = Daemon::start_with(HerdrClient::new(mock.socket_path()), pane_config()).await;
    daemon.wait_until_online().await;

    let mut frontend = Frontend::connect(&daemon).await;
    frontend.send(hello(0)).await;
    frontend.read(1 + PLUS_KEYS).await;

    frontend.tap(0).await;
    assert_eq!(
        feedback_from(&mut frontend).await,
        DaemonMessage::Ok { index: 0 }
    );

    // The key stays quiet; the log does not. "I pressed left four times and only moved twice" is
    // a question somebody is entitled to an answer to.
    let recorded = daemon.audit_lines().await;
    assert_eq!(recorded[0]["outcome"], "unchanged");
}

#[tokio::test]
async fn closing_a_pane_takes_a_hold_and_a_tap_only_says_so() {
    // Both halves of the guard, through a real socket: the tap reaches nothing and explains
    // itself, the hold reaches herdr and names the pane the user was actually in.
    let mock = mock_herdr(snapshot_with_a_focused_pane()).await;
    let mut daemon = Daemon::start_with(HerdrClient::new(mock.socket_path()), pane_config()).await;
    daemon.wait_until_online().await;

    let mut frontend = Frontend::connect(&daemon).await;
    frontend.send(hello(0)).await;
    frontend.read(1 + PLUS_KEYS).await;

    frontend.tap(1).await;
    match feedback_from(&mut frontend).await {
        DaemonMessage::Alert { index, message } => {
            assert_eq!(index, 1);
            assert!(message.contains("hold"), "got {message}");
        }
        other => panic!("a tap must refuse out loud, got {other:?}"),
    }
    assert!(
        daemon.audit_lines().await.is_empty(),
        "a refused tap never reached herdr, so it must leave no trail"
    );

    frontend.hold(1).await;
    assert_eq!(
        feedback_from(&mut frontend).await,
        DaemonMessage::Ok { index: 1 }
    );

    let recorded = daemon.audit_lines().await;
    assert_eq!(
        recorded.len(),
        1,
        "only the hold is a command: {recorded:?}"
    );
    assert_eq!(recorded[0]["command"], "close_pane");
    assert_eq!(recorded[0]["target"], "w1:p9");
}

// ---------------------------------------------------------------------------------------------
// Structure: layouts, worktrees, workspaces and tabs
// ---------------------------------------------------------------------------------------------

/// A config with one named layout and one named tab, and keys for both — plus the two destructive
/// structural keys, which a derived layout would never hand out.
fn structure_config() -> Config {
    use herdr_deck_core::layout::KeyBinding;
    use herdr_deck_herdr::wire::{CreateSpec, LayoutNode, LayoutPreset, SplitDirection};

    let mut config = test_config();
    config.layouts.insert(
        "dev".to_string(),
        LayoutPreset {
            label: Some("dev".to_string()),
            root: LayoutNode {
                split: Some(SplitDirection::Down),
                ratio: Some(70),
                first: Some(Box::default()),
                second: Some(Box::new(LayoutNode {
                    command: Some(vec!["cargo".to_string(), "watch".to_string()]),
                    ..Default::default()
                })),
                ..Default::default()
            },
        },
    );
    config.tabs.insert(
        "logs".to_string(),
        CreateSpec {
            label: Some("logs".to_string()),
            ..Default::default()
        },
    );
    config.layout = Some(herdr_deck_core::config::LayoutOverride {
        keys: vec![
            KeyBinding::Layout {
                preset: "dev".to_string(),
            },
            KeyBinding::NewTab {
                preset: Some("logs".to_string()),
            },
            KeyBinding::NewWorktree,
            KeyBinding::CloseTab,
            KeyBinding::RemoveWorktree,
            KeyBinding::Dynamic {
                rank: 0,
                page: None,
            },
        ],
        dials: vec![],
    });
    config
}

/// A herdr sitting in a linked worktree, with a sibling checkout that is not open.
fn snapshot_in_a_worktree() -> SessionSnapshot {
    let mut snapshot = snapshot(vec![agent(
        "term_a",
        "w1:p1",
        "refactor-auth",
        AgentStatus::Idle,
        100,
    )]);
    snapshot.focused_workspace_id = Some("w1".to_string());
    snapshot.focused_tab_id = Some("w1:t1".to_string());
    snapshot
}

fn worktrees() -> Vec<herdr_deck_herdr::wire::WorktreeInfo> {
    use herdr_deck_herdr::wire::WorktreeInfo;
    vec![
        WorktreeInfo {
            path: "/src/api".to_string(),
            branch: Some("main".to_string()),
            is_linked_worktree: false,
            open_workspace_id: Some("w2".to_string()),
            ..Default::default()
        },
        WorktreeInfo {
            path: "/src/.worktrees/api/fix-auth".to_string(),
            branch: Some("fix-auth".to_string()),
            is_linked_worktree: true,
            open_workspace_id: Some("w1".to_string()),
            ..Default::default()
        },
        WorktreeInfo {
            path: "/src/.worktrees/api/spike".to_string(),
            branch: Some("spike".to_string()),
            is_linked_worktree: true,
            ..Default::default()
        },
    ]
}

#[tokio::test]
async fn a_layout_key_builds_a_new_tab_and_never_touches_the_one_you_are_in() {
    // The end-to-end form of the constraint the whole layout preset feature is shaped around.
    // `layout.apply` with a tab id closes the tab it names and kills every process in it; this
    // asserts that pressing the key on a real deck, through a real socket, sends no tab id.
    let mock = mock_herdr(snapshot_in_a_worktree()).await;
    let mut daemon =
        Daemon::start_with(HerdrClient::new(mock.socket_path()), structure_config()).await;
    daemon.wait_until_online().await;

    let mut frontend = Frontend::connect(&daemon).await;
    frontend.send(hello(0)).await;
    frontend.read(1 + PLUS_KEYS).await;

    frontend.tap(0).await;
    assert_eq!(
        feedback_from(&mut frontend).await,
        DaemonMessage::Ok { index: 0 }
    );

    let params = mock
        .observed_params("layout.apply")
        .await
        .expect("the key should have applied a layout");
    assert!(
        params.get("tab_id").is_none(),
        "a preset key must never target an existing tab: {params}"
    );

    let recorded = daemon.audit_lines().await;
    assert_eq!(recorded[0]["command"], "apply_layout");
    assert_eq!(
        recorded[0]["target"], "dev",
        "the trail has to say which layout, or several keys become one line"
    );
}

#[tokio::test]
async fn a_preset_key_carries_the_settings_a_deck_could_never_have_typed() {
    // The whole point of presets: a deck has no keyboard, so a label or a working directory only
    // reaches herdr by being written down once and given a name.
    let mock = mock_herdr(snapshot_in_a_worktree()).await;
    let mut daemon =
        Daemon::start_with(HerdrClient::new(mock.socket_path()), structure_config()).await;
    daemon.wait_until_online().await;

    let mut frontend = Frontend::connect(&daemon).await;
    frontend.send(hello(0)).await;
    frontend.read(1 + PLUS_KEYS).await;

    frontend.tap(1).await;
    assert_eq!(
        feedback_from(&mut frontend).await,
        DaemonMessage::Ok { index: 1 }
    );

    let params = mock.observed_params("tab.create").await.expect("a new tab");
    assert_eq!(params["label"], "logs");
    assert_eq!(daemon.audit_lines().await[0]["target"], "logs");
}

#[tokio::test]
async fn a_new_worktree_key_reports_back_without_freezing_the_rest_of_the_deck() {
    // `worktree.create` is one of only two herdr methods that does not answer at once — it waits
    // on git, which on a large repository is seconds. The daemon runs it alongside its event loop,
    // so the key still reports what happened and everything else keeps working meanwhile.
    let mock = mock_herdr(snapshot_in_a_worktree()).await;
    let mut daemon =
        Daemon::start_with(HerdrClient::new(mock.socket_path()), structure_config()).await;
    daemon.wait_until_online().await;

    let mut frontend = Frontend::connect(&daemon).await;
    frontend.send(hello(0)).await;
    frontend.read(1 + PLUS_KEYS).await;

    frontend.tap(2).await;
    assert_eq!(
        feedback_from(&mut frontend).await,
        DaemonMessage::Ok { index: 2 }
    );

    let params = mock
        .observed_params("worktree.create")
        .await
        .expect("the key should have made a worktree");
    assert_eq!(
        params.get("branch"),
        None,
        "herdr generates the branch name; that is what makes this key possible: {params}"
    );

    let recorded = daemon.audit_lines().await;
    assert_eq!(recorded[0]["command"], "create_worktree");
    assert!(
        recorded[0]["target"].is_null(),
        "a branch name is the user's, not ours: {recorded:?}"
    );
}

#[tokio::test]
async fn giving_a_worktree_back_to_git_takes_a_hold_and_is_never_forced() {
    // Two things at once, because they are the same decision. The hold is what makes a destructive
    // key safe to have; `force: false` is what makes this particular one safe to press, because a
    // dirty checkout then makes git refuse and that refusal is the confirmation dialog this
    // hardware has nowhere to show.
    let mock = mock_herdr(snapshot_in_a_worktree()).await;
    mock.with_worktrees(&worktrees()).await;
    let mut daemon =
        Daemon::start_with(HerdrClient::new(mock.socket_path()), structure_config()).await;
    daemon
        .wait_for_state(|state| !state.worktrees.is_empty())
        .await;

    let mut frontend = Frontend::connect(&daemon).await;
    frontend.send(hello(0)).await;
    frontend.read(1 + PLUS_KEYS).await;

    frontend.tap(4).await;
    match feedback_from(&mut frontend).await {
        DaemonMessage::Alert { message, .. } => assert!(message.contains("hold"), "got {message}"),
        other => panic!("a tap must refuse out loud, got {other:?}"),
    }
    assert!(daemon.audit_lines().await.is_empty());

    frontend.hold(4).await;
    assert_eq!(
        feedback_from(&mut frontend).await,
        DaemonMessage::Ok { index: 4 }
    );

    let params = mock
        .observed_params("worktree.remove")
        .await
        .expect("the hold should have removed a worktree");
    assert_eq!(params["force"], false, "got {params}");
    assert_eq!(
        params["workspace_id"], "w1",
        "it must name the workspace the user is actually in: {params}"
    );

    let recorded = daemon.audit_lines().await;
    assert_eq!(recorded[0]["command"], "remove_worktree");
    assert_eq!(recorded[0]["target"], "w1");
}

#[tokio::test]
async fn a_worktree_key_pointed_at_a_repository_rather_than_a_worktree_refuses_both_gestures() {
    // herdr will not remove the source checkout, and the deck already holds the listing that says
    // which one that is. Letting the key look usable and then fail would make somebody press it to
    // find out.
    let mut snapshot = snapshot_in_a_worktree();
    snapshot.focused_workspace_id = Some("w2".to_string()); // the source repo
    let mock = mock_herdr(snapshot).await;
    mock.with_worktrees(&worktrees()).await;
    let mut daemon =
        Daemon::start_with(HerdrClient::new(mock.socket_path()), structure_config()).await;
    daemon
        .wait_for_state(|state| !state.worktrees.is_empty())
        .await;

    let mut frontend = Frontend::connect(&daemon).await;
    frontend.send(hello(0)).await;
    frontend.read(1 + PLUS_KEYS).await;

    frontend.tap(4).await;
    assert!(matches!(
        feedback_from(&mut frontend).await,
        DaemonMessage::Alert { .. }
    ));
    frontend.hold(4).await;
    assert!(matches!(
        feedback_from(&mut frontend).await,
        DaemonMessage::Alert { .. }
    ));
    assert!(
        daemon.audit_lines().await.is_empty(),
        "neither gesture may reach herdr"
    );
}

#[tokio::test]
async fn a_deck_with_worktrees_can_reach_them_from_the_mode_key_and_open_one() {
    // No extra key and no configuration: the mode key grows a third stop for the sessions that
    // have worktrees, and opening one is the same press as focusing an agent.
    let mock = mock_herdr(snapshot_in_a_worktree()).await;
    mock.with_worktrees(&worktrees()).await;
    let mut daemon = Daemon::start(HerdrClient::new(mock.socket_path())).await;
    daemon
        .wait_for_state(|state| !state.worktrees.is_empty())
        .await;

    let mut frontend = Frontend::connect(&daemon).await;
    frontend.send(hello(0)).await;
    frontend.read(1 + PLUS_KEYS).await;

    // Agents -> spaces -> trees. This frontend reports no dials, so it gets paging keys and the
    // page key lands last, at key 7.
    frontend.tap(7).await;
    frontend.drain_to_pong().await;
    frontend.tap(7).await;
    frontend.drain_to_pong().await;

    frontend.tap(0).await;
    // Window raising is switched off in these tests, and a worktree key is a *journey* — so it
    // reports the same half-done outcome an agent key does here, for the same reason and in the
    // same words. That it alerts at all is the proof it asked for the window.
    match feedback_from(&mut frontend).await {
        DaemonMessage::Alert { index, message } => {
            assert_eq!(index, 0);
            assert!(message.contains("raising is disabled"), "got {message}");
        }
        other => panic!("opening a worktree must go looking for the window, got {other:?}"),
    }

    assert_eq!(
        mock.worktree_open_paths().await,
        vec!["/src/api"],
        "the first worktree key should have opened the first checkout"
    );
    let recorded = daemon.audit_lines().await;
    assert_eq!(recorded[0]["command"], "open_worktree");
    assert!(
        recorded[0]["target"].is_null(),
        "a checkout path is the user's, not ours: {recorded:?}"
    );
}
