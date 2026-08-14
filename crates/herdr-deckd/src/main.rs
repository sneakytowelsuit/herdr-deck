//! The `herdr-deckd` binary: argument parsing, wiring, and the dry-run renderer.
//!
//! Everything with behaviour worth testing lives in the library next door (see `lib.rs`).

use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;
use herdr_deck_core::capabilities::{DeckCapabilities, DeckModel};
use herdr_deck_core::layout::{Mode, Profile, ResolvedDeck, Selection};
use herdr_deck_core::render::TileRenderer;
use herdr_deck_core::state::{Acknowledged, DeckState};
use herdr_deck_core::Config;
use herdr_deck_focus::FocusEngine;
use herdr_deck_herdr::HerdrClient;

use herdr_deckd::audit::AuditLog;
use herdr_deckd::server::{self, ServerContext};
use herdr_deckd::watcher::Watcher;

#[derive(Debug, Parser)]
#[command(
    name = "herdr-deckd",
    about = "Watches herdr and drives a Stream Deck",
    version
)]
struct Args {
    /// Config file. Defaults to ~/.config/herdr-deck/config.toml.
    #[arg(long, value_name = "PATH")]
    config: Option<PathBuf>,

    /// Frontend socket path. Defaults to $XDG_RUNTIME_DIR/herdr-deck.sock.
    #[arg(long, value_name = "PATH")]
    socket: Option<PathBuf>,

    /// herdr session name, equivalent to `herdr --session <name>`.
    #[arg(long, value_name = "NAME")]
    session: Option<String>,

    /// Log filter, e.g. `info`, `herdr_deckd=debug`.
    #[arg(long, default_value = "info", env = "HERDR_DECK_LOG")]
    log: String,

    /// Render the default layout to PNGs in DIR and exit, without touching herdr or hardware.
    ///
    /// This is how CI and the docs get pictures of the deck, and how you can check a theme
    /// change without owning every model.
    #[arg(long, value_name = "DIR")]
    dry_run: Option<PathBuf>,

    /// Hardware to assume for `--dry-run`.
    #[arg(long, default_value = "plus", value_name = "MODEL")]
    dry_run_model: String,

    /// Talk to the first attached deck directly, bypassing the daemon and herdr entirely.
    ///
    /// For hardware bring-up: paints a numbered test pattern on every key (and touchstrip
    /// segment, if any) so key numbering and dial mapping can be checked by eye, then prints
    /// every input event as controls are pressed. Exits and resets the deck on Ctrl-C. Linux
    /// only — on macOS the Elgato app owns the device, so there is nothing for this to open; use
    /// the Stream Deck plugin instead. See docs/help/hardware-bringup.md.
    #[arg(long)]
    selftest: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    init_tracing(&args.log);

    if args.selftest {
        return run_selftest().await;
    }

    let config_path = args
        .config
        .clone()
        .or_else(Config::default_path)
        .unwrap_or_else(|| PathBuf::from("herdr-deck.toml"));
    let config = Config::load(&config_path)?;

    if let Some(dir) = &args.dry_run {
        return dry_run(&config, dir, &args.dry_run_model);
    }

    let session_name = args
        .session
        .clone()
        .or_else(|| config.herdr_session.clone());
    let client = HerdrClient::from_env(session_name.as_deref());
    tracing::info!(
        socket = %client.socket().display(),
        via = client.socket().origin.describe(),
        "using herdr socket"
    );

    let mut focus = FocusEngine::new(client.clone(), config.focus.clone());
    tracing::info!(backend = focus.backend().name(), "window raise backend");
    if let Some(reason) = focus.backend().unsupported_reason() {
        tracing::warn!("{reason}");
    }
    // Stamp a marker into herdr's window title so the raiser can find that exact window rather
    // than merely the right application. Best effort: if herdr has no attached client yet, we
    // simply fall back to app-level raising.
    let marker = format!("herdr-deck:{}", std::process::id());
    if focus.install_title_marker(&marker).await {
        tracing::debug!(%marker, "installed herdr window title marker");
    }

    let renderer = Arc::new(TileRenderer::new(config.theme));
    let state = Watcher::new(client, &config).spawn();

    let socket_path = args
        .socket
        .clone()
        .unwrap_or_else(server::default_socket_path);
    let listener = server::bind(&socket_path).await?;
    tracing::info!(socket = %socket_path.display(), "listening for frontends");

    // On Linux there is no Stream Deck application to plug into, so the daemon drives the
    // hardware itself — as an ordinary frontend client of its own socket, so there is exactly
    // one protocol and one set of semantics across both platforms.
    #[cfg(target_os = "linux")]
    {
        let hid_socket = socket_path.clone();
        tokio::spawn(async move {
            if let Err(e) = herdr_deck_hid::run(hid_socket).await {
                tracing::warn!(error = %e, "HID frontend stopped");
            }
        });
    }

    // A record of what the deck asked herdr to do. Not having somewhere to write it is a working
    // deck with no trail, never a reason to refuse to start.
    let audit = match AuditLog::default_path() {
        Some(path) => {
            tracing::info!(path = %path.display(), "recording issued commands");
            AuditLog::open(path)
        }
        None => {
            tracing::warn!("no state directory: commands will not be recorded");
            AuditLog::disabled()
        }
    };

    let context = Arc::new(ServerContext {
        config,
        renderer,
        focus: Arc::new(focus),
        state,
        audit: Arc::new(audit),
    });

    // Remove the socket on the way out so the next start does not have to reclaim it.
    let cleanup_path = socket_path.clone();
    let result = tokio::select! {
        result = server::serve(listener, context) => result,
        _ = shutdown_signal() => {
            tracing::info!("shutting down");
            Ok(())
        }
    };
    let _ = std::fs::remove_file(&cleanup_path);
    result
}

/// Run the hardware self-test, on the platform where that is even possible.
///
/// On macOS the Stream Deck app has the device open exclusively; there is no HID path here at
/// all, only the plugin. Saying so on the terminal — rather than failing with a confusing "no
/// device found" — is the whole point of degrading loudly.
#[cfg(target_os = "linux")]
async fn run_selftest() -> anyhow::Result<()> {
    herdr_deck_hid::selftest::run().await
}

#[cfg(not(target_os = "linux"))]
async fn run_selftest() -> anyhow::Result<()> {
    println!(
        "--selftest is Linux-only: on macOS, Elgato's Stream Deck app owns the device and there \
         is no way for herdr-deckd to open it directly. Install the herdr-deck Stream Deck \
         plugin instead — see docs/help/hardware-bringup.md."
    );
    Ok(())
}

fn init_tracing(filter: &str) {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_new(filter).unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .init();
}

async fn shutdown_signal() {
    use tokio::signal::unix::{signal, SignalKind};
    let mut term = match signal(SignalKind::terminate()) {
        Ok(s) => s,
        Err(_) => return std::future::pending().await,
    };
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = term.recv() => {}
    }
}

/// Render every tile of a model's default layout to `dir`.
///
/// Uses a representative fake state rather than talking to herdr, so it works anywhere — CI, a
/// machine with no deck, a machine with no herdr.
fn dry_run(config: &Config, dir: &std::path::Path, model_name: &str) -> anyhow::Result<()> {
    let model = parse_model(model_name)?;
    let capabilities: DeckCapabilities = model.capabilities();
    // The layout this config would actually produce, so a dry run renders this machine's deck
    // rather than the one it would have had with an empty config file.
    let profile = Profile::for_config(&capabilities, config);
    let renderer = TileRenderer::new(config.theme);
    let state = sample_state();

    std::fs::create_dir_all(dir)?;
    // A dry run has no frontend and therefore nothing acknowledged: it shows the deck as it
    // looks when everything that wants you is still asking.
    let nothing_acknowledged = Acknowledged::default();
    let deck = ResolvedDeck::new(
        &profile,
        &state,
        Mode::Agents,
        0,
        Selection::default(),
        &nothing_acknowledged,
    );

    if capabilities.has_display() {
        for index in 0..profile.keys.len() {
            let tile = deck.tile(index);
            let png = renderer.render_key(&tile, capabilities.key_image_px)?;
            std::fs::write(dir.join(format!("key-{index:02}.png")), png)?;
        }
    }

    if let Some(strip) = capabilities.touchstrip {
        for dial in 0..profile.dials.len() {
            let (title, value, status) = deck.dial_feedback(dial);
            let png = renderer.render_strip(
                &title,
                &value,
                status,
                strip.segment_width(capabilities.dials),
                strip.height,
            )?;
            std::fs::write(dir.join(format!("dial-{dial}.png")), png)?;
        }
    }

    println!("rendered {} to {}", capabilities.describe(), dir.display());
    Ok(())
}

fn parse_model(name: &str) -> anyhow::Result<DeckModel> {
    match name.to_ascii_lowercase().as_str() {
        "plus" | "+" => Ok(DeckModel::Plus),
        "original" | "mk2" | "mk.2" => Ok(DeckModel::Original),
        "mini" => Ok(DeckModel::Mini),
        "xl" => Ok(DeckModel::Xl),
        "neo" => Ok(DeckModel::Neo),
        "pedal" => Ok(DeckModel::Pedal),
        other => {
            anyhow::bail!("unknown model `{other}` (try: plus, original, mini, xl, neo, pedal)")
        }
    }
}

/// A state that exercises every status, so a dry run shows the whole palette.
///
/// The two waiting agents are given deliberately different ages, because a dry run that showed
/// only one rung of the wait escalation would be no use for checking it — and checking a theme
/// change without owning every model is the entire point of `--dry-run`.
fn sample_state() -> DeckState {
    use herdr_deck_core::state::WaitClock;
    use herdr_deck_herdr::wire::AgentStatus;
    use std::time::{Duration, Instant};

    let start = Instant::now();
    let mut clock = WaitClock::default();

    // The blocked agent starts asking first, alone...
    let mut blocked_only = sample_snapshot();
    blocked_only
        .agents
        .retain(|a| a.agent_status == AgentStatus::Blocked);
    clock.stamp(&mut DeckState::from_snapshot(blocked_only), start);

    // ...and six minutes later the rest of the session appears around it, so the blocked agent
    // is long overdue and the freshly finished one has only just raised its hand.
    let mut state = DeckState::from_snapshot(sample_snapshot());
    clock.stamp(&mut state, start + Duration::from_secs(360));
    state
}

fn sample_snapshot() -> herdr_deck_herdr::wire::SessionSnapshot {
    use herdr_deck_herdr::wire::{AgentInfo, AgentStatus, SessionSnapshot, WorkspaceInfo};

    let agent = |name: &str, kind: &str, workspace: &str, status, seq| AgentInfo {
        terminal_id: format!("term_{name}"),
        agent_status: status,
        workspace_id: workspace.to_string(),
        tab_id: format!("{workspace}:t1"),
        pane_id: format!("{workspace}:p1"),
        name: Some(name.to_string()),
        agent: Some(kind.to_string()),
        state_change_seq: Some(seq),
        ..Default::default()
    };

    SessionSnapshot {
        protocol: Some(herdr_deck_herdr::EXPECTED_PROTOCOL),
        agents: vec![
            agent("refactor-auth", "claude", "api", AgentStatus::Blocked, 90),
            agent("migrate-db", "codex", "api", AgentStatus::Done, 80),
            agent("write-docs", "claude", "web", AgentStatus::Working, 70),
            agent("flaky-tests", "cursor", "web", AgentStatus::Idle, 60),
            agent("scratch", "claude", "infra", AgentStatus::Unknown, 50),
        ],
        workspaces: vec![
            WorkspaceInfo {
                workspace_id: "api".into(),
                label: Some("api".into()),
                agent_status: AgentStatus::Blocked,
                focused: true,
                ..Default::default()
            },
            WorkspaceInfo {
                workspace_id: "web".into(),
                label: Some("web".into()),
                agent_status: AgentStatus::Working,
                ..Default::default()
            },
        ],
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_names_are_accepted_in_the_forms_people_actually_type() {
        assert_eq!(parse_model("plus").unwrap(), DeckModel::Plus);
        assert_eq!(parse_model("PLUS").unwrap(), DeckModel::Plus);
        assert_eq!(parse_model("+").unwrap(), DeckModel::Plus);
        assert_eq!(parse_model("xl").unwrap(), DeckModel::Xl);
        assert_eq!(parse_model("mk.2").unwrap(), DeckModel::Original);
    }

    #[test]
    fn an_unknown_model_lists_the_valid_ones() {
        let err = parse_model("streamdeck9000").unwrap_err().to_string();
        assert!(err.contains("plus"), "error should list options: {err}");
    }

    #[test]
    fn the_sample_state_covers_every_status_so_a_dry_run_shows_the_whole_palette() {
        use herdr_deck_herdr::wire::AgentStatus;
        let state = sample_state();
        let counts = state.status_counts();
        assert_eq!(counts.blocked, 1);
        assert_eq!(counts.done, 1);
        assert_eq!(counts.working, 1);
        assert_eq!(counts.idle, 1);
        assert_eq!(counts.unknown, 1);
        assert_eq!(
            state.top_attention().unwrap().agent_status,
            AgentStatus::Blocked
        );
    }

    #[test]
    fn the_sample_state_shows_both_ends_of_the_wait_escalation() {
        // One rung would let a dry run pass while the escalation was broken at the other end,
        // which defeats the point of being able to check a theme change without hardware.
        use herdr_deck_core::state::WaitBucket;
        use herdr_deck_herdr::wire::AgentStatus;
        let state = sample_state();
        let bucket = |status| {
            let agent = state
                .agents
                .iter()
                .find(|a| a.agent_status == status)
                .expect("the sample covers every status");
            state.wait_bucket(agent)
        };
        assert_eq!(bucket(AgentStatus::Blocked), Some(WaitBucket::Overdue));
        assert_eq!(bucket(AgentStatus::Done), Some(WaitBucket::Fresh));
        assert_eq!(
            bucket(AgentStatus::Working),
            None,
            "nothing calm should carry a wait"
        );
    }

    #[test]
    fn a_dry_run_writes_one_image_per_key_and_dial() {
        let dir = tempfile::tempdir().unwrap();
        dry_run(&Config::default(), dir.path(), "plus").unwrap();
        for index in 0..8 {
            let path = dir.path().join(format!("key-{index:02}.png"));
            assert!(path.exists(), "missing {}", path.display());
        }
        for dial in 0..4 {
            assert!(dir.path().join(format!("dial-{dial}.png")).exists());
        }
    }

    #[test]
    fn a_dry_run_for_a_screenless_device_writes_nothing_rather_than_failing() {
        let dir = tempfile::tempdir().unwrap();
        dry_run(&Config::default(), dir.path(), "pedal").unwrap();
        assert!(std::fs::read_dir(dir.path()).unwrap().next().is_none());
    }

    #[test]
    fn a_dry_run_works_for_every_model_we_claim_to_support() {
        for model in ["plus", "original", "mini", "xl", "neo", "pedal"] {
            let dir = tempfile::tempdir().unwrap();
            dry_run(&Config::default(), dir.path(), model)
                .unwrap_or_else(|e| panic!("dry run failed for {model}: {e}"));
        }
    }
}
