//! The frontend socket server.
//!
//! Accepts frontend connections on a unix socket and drives one [`Session`] per connection.
//! Multiple frontends may connect at once — that is how a Linux box with the in-process HID
//! frontend and a debugging CLI can both be attached, and it costs nothing to support.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context as _;
use herdr_deck_core::protocol::{DaemonMessage, FrontendMessage, FRONTEND_PROTOCOL};
use herdr_deck_core::render::TileRenderer;
use herdr_deck_core::state::DeckState;
use herdr_deck_core::Config;
use herdr_deck_focus::{FocusEngine, FocusReport};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::watch;

use crate::audit::AuditLog;
use crate::session::{PendingAction, Session};

/// Everything a connection needs.
pub struct ServerContext {
    pub config: Config,
    pub renderer: Arc<TileRenderer>,
    pub focus: Arc<FocusEngine>,
    pub state: watch::Receiver<Arc<DeckState>>,
    pub audit: Arc<AuditLog>,
}

/// Where the frontend socket lives.
///
/// `$XDG_RUNTIME_DIR` is the right home for a per-user, per-boot socket. Falling back to the
/// state directory keeps it working on macOS, which has no `XDG_RUNTIME_DIR`.
pub fn default_socket_path() -> PathBuf {
    if let Ok(runtime) = std::env::var("XDG_RUNTIME_DIR") {
        if !runtime.is_empty() {
            return PathBuf::from(runtime).join("herdr-deck.sock");
        }
    }
    directories::BaseDirs::new()
        .map(|dirs| {
            dirs.data_local_dir()
                .join("herdr-deck")
                .join("herdr-deck.sock")
        })
        .unwrap_or_else(|| PathBuf::from("/tmp/herdr-deck.sock"))
}

/// Bind the frontend socket, replacing any stale one left by a crash.
pub async fn bind(path: &Path) -> anyhow::Result<UnixListener> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("could not create {}", parent.display()))?;
    }
    // A leftover socket file from a killed daemon would make bind fail with EADDRINUSE. Only
    // remove it once we know nothing is listening, so two daemons cannot fight over it.
    if path.exists() {
        match UnixStream::connect(path).await {
            Ok(_) => anyhow::bail!(
                "another herdr-deckd is already listening on {}",
                path.display()
            ),
            Err(_) => {
                std::fs::remove_file(path)
                    .with_context(|| format!("could not remove stale socket {}", path.display()))?;
            }
        }
    }

    let listener =
        UnixListener::bind(path).with_context(|| format!("could not bind {}", path.display()))?;
    restrict_permissions(path)?;
    Ok(listener)
}

/// Owner-only. The socket can focus panes and raise windows; nobody else on the box needs it.
fn restrict_permissions(path: &Path) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .with_context(|| format!("could not restrict permissions on {}", path.display()))?;
    }
    Ok(())
}

/// Serve until cancelled.
pub async fn serve(listener: UnixListener, context: Arc<ServerContext>) -> anyhow::Result<()> {
    loop {
        let (stream, _) = listener.accept().await?;
        let context = Arc::clone(&context);
        tokio::spawn(async move {
            if let Err(e) = handle_connection(stream, context).await {
                tracing::debug!(error = %e, "frontend connection ended");
            }
        });
    }
}

async fn handle_connection(stream: UnixStream, context: Arc<ServerContext>) -> anyhow::Result<()> {
    let (read_half, mut write_half) = stream.into_split();
    let mut lines = BufReader::new(read_half).lines();
    let mut state_rx = context.state.clone();

    // The first message must be a hello: we cannot lay out a deck we know nothing about.
    let Some(first) = lines.next_line().await? else {
        return Ok(());
    };
    let hello: FrontendMessage = serde_json::from_str(first.trim())
        .with_context(|| format!("first frontend message was not valid JSON: {first}"))?;
    let FrontendMessage::Hello {
        frontend,
        device,
        protocol,
    } = hello
    else {
        anyhow::bail!("frontend must send `hello` first");
    };

    if let Some(protocol) = protocol {
        if protocol != FRONTEND_PROTOCOL {
            // Version skew between a separately-installed plugin and the daemon is a real
            // possibility, so say so explicitly instead of failing in a confusing way later.
            let message = DaemonMessage::ProtocolMismatch {
                expected: FRONTEND_PROTOCOL,
                got: protocol,
            };
            write_half.write_all(message.to_line().as_bytes()).await?;
            anyhow::bail!("frontend speaks protocol {protocol}, we speak {FRONTEND_PROTOCOL}");
        }
    }

    let mut session = Session::new(&device, &context.config, Arc::clone(&context.renderer));
    tracing::info!(
        frontend = %frontend,
        device = %session.capabilities().describe(),
        "frontend connected"
    );

    {
        let state = state_rx.borrow_and_update().clone();
        let greeting = session.greet(&state);
        write_all(&mut write_half, &greeting).await?;
    }

    loop {
        tokio::select! {
            line = lines.next_line() => {
                let Some(line) = line? else { break };
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                let message: FrontendMessage = match serde_json::from_str(line) {
                    Ok(m) => m,
                    Err(e) => {
                        // One bad line should not drop a working frontend.
                        tracing::warn!(error = %e, %line, "ignoring unparseable frontend message");
                        continue;
                    }
                };

                let state = state_rx.borrow().clone();

                // A frontend may re-announce mid-connection: the macOS plugin discovers dials
                // as their actions appear, so the first hello can say "no dials" and a later
                // one "four". Rebuild the layout rather than keeping a stale one.
                if let FrontendMessage::Hello { device, .. } = &message {
                    let rebuilt = Session::new(device, &context.config, Arc::clone(&context.renderer));
                    if rebuilt.capabilities() != session.capabilities() {
                        tracing::info!(
                            device = %rebuilt.capabilities().describe(),
                            "frontend re-announced different hardware; rebuilding layout"
                        );
                        session = rebuilt;
                        let greeting = session.greet(&state);
                        write_all(&mut write_half, &greeting).await?;
                        continue;
                    }
                }

                // The session times key presses, so it needs a clock; taking it here keeps the
                // session itself a pure function of its inputs.
                let outcome = session.handle(message, &state, std::time::Instant::now());
                write_all(&mut write_half, &outcome.messages).await?;

                if let Some(action) = outcome.action {
                    let feedback = perform(&context, action).await;
                    write_all(&mut write_half, &feedback).await?;
                }
            }

            changed = state_rx.changed() => {
                if changed.is_err() {
                    break;
                }
                let state = state_rx.borrow_and_update().clone();
                let messages = session.repaint(&state);
                write_all(&mut write_half, &messages).await?;
            }
        }
    }

    tracing::info!(frontend = %frontend, "frontend disconnected");
    Ok(())
}

/// Carry out one command and turn the result into deck feedback.
///
/// The only place a command leaves this daemon, which is why it is also the only place that
/// records one. An audit line written anywhere else would eventually describe something that was
/// never actually issued.
async fn perform(context: &ServerContext, action: PendingAction) -> Vec<DaemonMessage> {
    let PendingAction { command, key } = action;
    let report = context.focus.perform(&command).await;
    let verdict = report.verdict();
    context.audit.record(&command, verdict);

    if verdict.is_error() {
        // Only a real shortfall is worth a line. A command that found nothing to do, or that never
        // wanted a window, did everything it was asked to — logging those as near-misses would
        // bury the ones that are.
        tracing::info!(
            outcome = %report.describe(),
            verdict = verdict.as_str(),
            "a command did not fully succeed"
        );
    }

    match key {
        Some(index) => vec![key_feedback(index, &report)],
        None => vec![],
    }
}

/// What the key that asked for a command should show.
///
/// A verdict is not an error merely because something was left undone. With no client attached
/// there was no window to raise, so nothing was left undone at all — and at the edge of a layout
/// there was nothing to move to, which is herdr answering the question rather than declining it.
/// A key that alerted about either would teach the user to ignore the alerts that mean something.
fn key_feedback(index: usize, report: &FocusReport) -> DaemonMessage {
    if report.verdict().is_error() {
        DaemonMessage::Alert {
            index,
            message: report.describe(),
        }
    } else {
        DaemonMessage::Ok { index }
    }
}

async fn write_all(
    writer: &mut tokio::net::unix::OwnedWriteHalf,
    messages: &[DaemonMessage],
) -> anyhow::Result<()> {
    if messages.is_empty() {
        return Ok(());
    }
    let mut buffer = String::new();
    for message in messages {
        buffer.push_str(&message.to_line());
    }
    writer.write_all(buffer.as_bytes()).await?;
    writer.flush().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use herdr_deck_focus::RaiseOutcome;

    fn report(raise: RaiseOutcome, error: Option<&str>) -> FocusReport {
        FocusReport {
            herdr_acted: error.is_none(),
            raise,
            error: error.map(str::to_string),
            unchanged: None,
        }
    }

    /// What herdr answers when a direction key is pressed at the edge of a layout.
    fn nothing_to_do() -> FocusReport {
        FocusReport {
            herdr_acted: true,
            raise: RaiseOutcome::NotNeeded,
            error: None,
            unchanged: Some(herdr_deck_herdr::wire::NothingToDo::NoNeighbour),
        }
    }

    #[test]
    fn a_focus_with_nothing_attached_to_raise_reads_as_success_on_the_key() {
        // herdr focused it and it persists to the next attach. There was no window, so nothing
        // failed — and a deck that cries wolf here is a deck whose alerts stop being read.
        assert_eq!(
            key_feedback(2, &report(RaiseOutcome::NoClient, None)),
            DaemonMessage::Ok { index: 2 }
        );
    }

    #[test]
    fn pressing_a_direction_at_the_edge_of_a_layout_is_not_a_failure() {
        // A thumb run along a row of direction keys ends at an edge every single time. If that
        // flashed an alert, the alerts would stop being read within a day — and they are the only
        // thing this hardware has to tell anyone something is actually wrong.
        assert_eq!(
            key_feedback(4, &nothing_to_do()),
            DaemonMessage::Ok { index: 4 }
        );
    }

    #[test]
    fn a_command_that_never_wanted_the_window_is_not_reported_as_half_done() {
        // Splitting a pane asks nothing of the window manager, so a machine that cannot raise
        // windows at all must still see a split as a plain success.
        assert_eq!(
            key_feedback(
                5,
                &FocusReport {
                    herdr_acted: true,
                    raise: RaiseOutcome::NotNeeded,
                    error: None,
                    unchanged: None,
                }
            ),
            DaemonMessage::Ok { index: 5 }
        );
    }

    #[test]
    fn a_close_herdr_wants_confirmed_sends_the_user_looking_for_the_dialog() {
        // herdr has put a modal on screen that the deck cannot answer or dismiss. Saying only
        // "that failed" would leave the user with a dialog they did not open and no idea why.
        let refused = report(
            RaiseOutcome::NotNeeded,
            Some(
                &herdr_deck_herdr::HerdrError::ConfirmationRequired {
                    method: "pane.close".into(),
                }
                .to_string(),
            ),
        );
        match key_feedback(6, &refused) {
            DaemonMessage::Alert { message, .. } => {
                assert!(message.contains("its own window"), "got {message}")
            }
            other => panic!("expected an alert, got {other:?}"),
        }
    }

    #[test]
    fn a_window_that_should_have_come_forward_and_did_not_still_alerts() {
        for raise in [
            RaiseOutcome::NotFound,
            RaiseOutcome::Disabled,
            RaiseOutcome::ToolMissing {
                program: "wmctrl".into(),
            },
            RaiseOutcome::Unsupported {
                reason: "GNOME on Wayland".into(),
            },
        ] {
            assert!(
                matches!(
                    key_feedback(0, &report(raise.clone(), None)),
                    DaemonMessage::Alert { .. }
                ),
                "{raise:?} left the user looking at the wrong window and must say so"
            );
        }
    }

    #[test]
    fn a_focus_herdr_refused_alerts_and_says_what_herdr_said() {
        match key_feedback(1, &report(RaiseOutcome::Disabled, Some("pane not found"))) {
            DaemonMessage::Alert { index, message } => {
                assert_eq!(index, 1);
                assert!(message.contains("pane not found"), "got {message}");
            }
            other => panic!("expected an alert, got {other:?}"),
        }
    }

    #[test]
    fn a_focus_that_fully_worked_reads_as_success() {
        assert_eq!(
            key_feedback(
                3,
                &report(
                    RaiseOutcome::Raised {
                        via: "hyprctl".into()
                    },
                    None
                )
            ),
            DaemonMessage::Ok { index: 3 }
        );
    }

    #[tokio::test]
    async fn binding_twice_is_refused_rather_than_stealing_the_socket() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("herdr-deck.sock");
        let _first = bind(&path).await.expect("first bind succeeds");
        let second = bind(&path).await;
        assert!(second.is_err(), "a second daemon must not steal the socket");
        assert!(second
            .unwrap_err()
            .to_string()
            .contains("already listening"));
    }

    #[tokio::test]
    async fn a_stale_socket_left_by_a_crash_is_cleaned_up() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("herdr-deck.sock");
        {
            let _listener = bind(&path).await.unwrap();
        } // dropped: nothing is listening, but the file remains
        assert!(path.exists());
        assert!(bind(&path).await.is_ok(), "should reclaim a dead socket");
    }

    #[tokio::test]
    async fn the_socket_is_owner_only() {
        // It can focus panes and raise windows; no other user needs reach.
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("herdr-deck.sock");
        let _listener = bind(&path).await.unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "socket should be 0600, got {mode:o}");
    }

    #[test]
    fn the_socket_path_prefers_the_runtime_directory() {
        // Set and restore rather than assuming the ambient environment.
        let original = std::env::var("XDG_RUNTIME_DIR").ok();
        std::env::set_var("XDG_RUNTIME_DIR", "/run/user/1000");
        assert_eq!(
            default_socket_path(),
            PathBuf::from("/run/user/1000/herdr-deck.sock")
        );
        match original {
            Some(value) => std::env::set_var("XDG_RUNTIME_DIR", value),
            None => std::env::remove_var("XDG_RUNTIME_DIR"),
        }
    }
}
