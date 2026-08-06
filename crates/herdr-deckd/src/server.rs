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
use herdr_deck_focus::FocusEngine;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::watch;

use crate::session::{PendingAction, Session};

/// Everything a connection needs.
pub struct ServerContext {
    pub config: Config,
    pub renderer: Arc<TileRenderer>,
    pub focus: Arc<FocusEngine>,
    pub state: watch::Receiver<Arc<DeckState>>,
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

                let outcome = session.handle(message, &state);
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

/// Carry out a focus request and turn the result into deck feedback.
async fn perform(context: &ServerContext, action: PendingAction) -> Vec<DaemonMessage> {
    let (report, key) = match action {
        PendingAction::FocusAgent { pane_id, key } => {
            (context.focus.focus_agent(&pane_id).await, key)
        }
        PendingAction::FocusWorkspace { workspace_id, key } => {
            (context.focus.focus_workspace(&workspace_id).await, key)
        }
    };

    if !report.fully_succeeded() {
        // A partial success — herdr focused but the window did not come forward — is worth
        // saying out loud, because otherwise the user presses a key and nothing visibly happens.
        tracing::info!(outcome = %report.describe(), "focus did not fully succeed");
    }

    let Some(index) = key else { return vec![] };
    if report.fully_succeeded() {
        vec![DaemonMessage::Ok { index }]
    } else {
        vec![DaemonMessage::Alert {
            index,
            message: report.describe(),
        }]
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
