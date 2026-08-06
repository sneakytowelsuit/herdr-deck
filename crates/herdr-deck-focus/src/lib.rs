//! Focusing an agent from the deck.
//!
//! # Two steps, because herdr only does one
//!
//! Pressing a key that says "this agent needs you" has to do two separate things:
//!
//! 1. **Switch herdr's active pane** — `agent.focus` over the socket. This also marks the tab
//!    seen, which flips a `done` agent to `idle`. herdr does this part.
//! 2. **Raise the terminal window** — bring the OS window hosting herdr to the front, switching
//!    desktop/workspace if needed. **herdr has no API for this**; the only window activation in
//!    herdr is `terminal-notifier -activate` when a user clicks a macOS notification. So we do
//!    it ourselves, per platform, in [`backend`].
//!
//! Step 1 works everywhere. Step 2 does not — notably GNOME on Wayland blocks programmatic
//! activation. When step 2 is unavailable the focus still half-succeeds, and we say so loudly
//! rather than silently doing less than the user asked for.

pub mod backend;
pub mod detect;

use std::time::Duration;

pub use backend::{Backend, CommandSpec, WindowTarget};
pub use detect::{detect, detect_or_override, FocusEnv, TargetOs};

use herdr_deck_core::config::FocusConfig;
use herdr_deck_herdr::HerdrClient;

/// How long a window-raise command gets before we give up on it.
///
/// A wedged `hyprctl` must never stall the deck's event loop.
const RAISE_TIMEOUT: Duration = Duration::from_secs(3);

/// What happened when we tried to raise a window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RaiseOutcome {
    /// A command succeeded.
    Raised { via: String },
    /// Every command ran but none matched a window.
    NotFound,
    /// This environment has no usable mechanism.
    Unsupported { reason: String },
    /// The backend's tools are missing.
    ToolMissing { program: String },
    /// Window raising is switched off in config.
    Disabled,
}

impl RaiseOutcome {
    pub fn succeeded(&self) -> bool {
        matches!(self, RaiseOutcome::Raised { .. })
    }

    /// A short line for the deck and for `doctor`.
    pub fn describe(&self) -> String {
        match self {
            RaiseOutcome::Raised { via } => format!("window raised via {via}"),
            RaiseOutcome::NotFound => {
                "herdr focused, but no matching terminal window was found".to_string()
            }
            RaiseOutcome::Unsupported { reason } => reason.clone(),
            RaiseOutcome::ToolMissing { program } => {
                format!("herdr focused, but `{program}` is not installed to raise the window")
            }
            RaiseOutcome::Disabled => "herdr focused (window raising is disabled)".to_string(),
        }
    }
}

/// The result of a focus request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FocusReport {
    /// Did herdr switch its active pane?
    pub herdr_focused: bool,
    pub raise: RaiseOutcome,
    /// Set when step 1 failed.
    pub error: Option<String>,
}

impl FocusReport {
    /// Did everything the user asked for actually happen?
    ///
    /// Deliberately strict: focusing herdr without raising the window is a *partial* success,
    /// and the deck flashes an alert for it so the user is never left wondering why nothing
    /// appeared to happen.
    pub fn fully_succeeded(&self) -> bool {
        self.herdr_focused && self.raise.succeeded()
    }

    pub fn describe(&self) -> String {
        match &self.error {
            Some(err) => format!("could not focus in herdr: {err}"),
            None => self.raise.describe(),
        }
    }
}

/// Runs commands. Swapped out in tests so backend behaviour is testable with no desktop.
pub trait CommandRunner: Send + Sync {
    /// Run a command; [`RunResult::Success`] means it worked.
    fn run(
        &self,
        spec: &CommandSpec,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = RunResult> + Send + '_>>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunResult {
    Success,
    /// Ran but did not match anything.
    Failed,
    /// The program is not installed.
    NotInstalled,
}

/// The real runner: spawns processes, never through a shell.
#[derive(Debug, Default, Clone, Copy)]
pub struct ProcessRunner;

impl CommandRunner for ProcessRunner {
    fn run(
        &self,
        spec: &CommandSpec,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = RunResult> + Send + '_>> {
        let spec = spec.clone();
        Box::pin(async move {
            let mut command = tokio::process::Command::new(&spec.program);
            command.args(&spec.args);
            command.stdin(std::process::Stdio::null());
            command.stdout(std::process::Stdio::null());
            command.stderr(std::process::Stdio::null());

            match tokio::time::timeout(RAISE_TIMEOUT, command.status()).await {
                Ok(Ok(status)) if status.success() => RunResult::Success,
                Ok(Ok(_)) => RunResult::Failed,
                Ok(Err(e)) if e.kind() == std::io::ErrorKind::NotFound => RunResult::NotInstalled,
                Ok(Err(e)) => {
                    tracing::warn!(program = %spec.program, error = %e, "window raise command failed");
                    RunResult::Failed
                }
                Err(_) => {
                    tracing::warn!(program = %spec.program, "window raise command timed out");
                    RunResult::Failed
                }
            }
        })
    }
}

/// Ties herdr focus and window raising together.
pub struct FocusEngine<R: CommandRunner = ProcessRunner> {
    client: HerdrClient,
    backend: Backend,
    config: FocusConfig,
    runner: R,
    /// The marker currently stamped into herdr's window title, if any.
    title_marker: Option<String>,
}

impl FocusEngine<ProcessRunner> {
    pub fn new(client: HerdrClient, config: FocusConfig) -> Self {
        let backend = detect_or_override(&FocusEnv::from_process(), config.backend.as_deref());
        Self::with_runner(client, config, backend, ProcessRunner)
    }
}

impl<R: CommandRunner> FocusEngine<R> {
    pub fn with_runner(
        client: HerdrClient,
        config: FocusConfig,
        backend: Backend,
        runner: R,
    ) -> Self {
        Self {
            client,
            backend,
            config,
            runner,
            title_marker: None,
        }
    }

    pub fn backend(&self) -> Backend {
        self.backend
    }

    pub fn title_marker(&self) -> Option<&str> {
        self.title_marker.as_deref()
    }

    /// Stamp a unique marker into herdr's terminal window title so we can find that exact
    /// window later.
    ///
    /// Only worth doing on backends that can match by title — on macOS we can only ever
    /// activate the application, so we skip it and leave the user's title alone.
    pub async fn install_title_marker(&mut self, marker: &str) -> bool {
        if !self.config.use_title_marker || !self.backend.supports_title_matching() {
            return false;
        }
        match self.client.set_window_title(marker).await {
            Ok(()) => {
                self.title_marker = Some(marker.to_string());
                true
            }
            Err(e) => {
                tracing::debug!(error = %e, "could not set herdr window title marker");
                false
            }
        }
    }

    fn window_target(&self) -> WindowTarget {
        WindowTarget {
            app_id: if matches!(self.backend, Backend::MacOs) {
                self.config.macos_bundle_id.clone()
            } else {
                self.config.linux_app_id.clone()
            },
            title_marker: self.title_marker.clone(),
        }
    }

    /// Focus an agent: switch herdr's active pane, then raise the terminal window.
    ///
    /// `target` is an agent name or a **pane id** — herdr's `agent.focus` does not accept a
    /// `terminal_id`, so callers holding a stable terminal id must resolve it against current
    /// state first.
    pub async fn focus_agent(&self, target: &str) -> FocusReport {
        match self.client.agent_focus(target).await {
            Ok(()) => FocusReport {
                herdr_focused: true,
                raise: self.raise_window().await,
                error: None,
            },
            Err(e) => FocusReport {
                herdr_focused: false,
                raise: RaiseOutcome::Disabled,
                error: Some(e.to_string()),
            },
        }
    }

    pub async fn focus_workspace(&self, workspace_id: &str) -> FocusReport {
        match self.client.workspace_focus(workspace_id).await {
            Ok(()) => FocusReport {
                herdr_focused: true,
                raise: self.raise_window().await,
                error: None,
            },
            Err(e) => FocusReport {
                herdr_focused: false,
                raise: RaiseOutcome::Disabled,
                error: Some(e.to_string()),
            },
        }
    }

    /// Try each of the backend's commands until one works.
    pub async fn raise_window(&self) -> RaiseOutcome {
        if !self.config.raise_window {
            return RaiseOutcome::Disabled;
        }
        if let Some(reason) = self.backend.unsupported_reason() {
            return RaiseOutcome::Unsupported {
                reason: reason.to_string(),
            };
        }

        let target = self.window_target();
        let commands = self.backend.raise_commands(&target);
        if commands.is_empty() {
            return RaiseOutcome::Unsupported {
                reason: format!(
                    "no window-raise commands for backend {}",
                    self.backend.name()
                ),
            };
        }

        let mut missing: Option<String> = None;
        for spec in &commands {
            match self.runner.run(spec).await {
                RunResult::Success => {
                    return RaiseOutcome::Raised {
                        via: spec.program.clone(),
                    }
                }
                // Remember the first missing tool, but keep trying the alternatives — a machine
                // with xdotool but no wmctrl should still work.
                RunResult::NotInstalled => {
                    missing.get_or_insert_with(|| spec.program.clone());
                }
                RunResult::Failed => {}
            }
        }

        match missing {
            Some(program) => RaiseOutcome::ToolMissing { program },
            None => RaiseOutcome::NotFound,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use herdr_deck_herdr::mock::MockHerdr;
    use serde_json::json;
    use std::sync::{Arc, Mutex};

    /// Records what was run and answers from a script.
    #[derive(Clone, Default)]
    struct FakeRunner {
        ran: Arc<Mutex<Vec<CommandSpec>>>,
        /// Program name -> result. Anything unlisted fails.
        results: Arc<Mutex<std::collections::HashMap<String, RunResult>>>,
    }

    impl FakeRunner {
        fn with(results: &[(&str, RunResult)]) -> Self {
            let map = results
                .iter()
                .map(|(k, v)| (k.to_string(), *v))
                .collect::<std::collections::HashMap<_, _>>();
            Self {
                ran: Arc::new(Mutex::new(Vec::new())),
                results: Arc::new(Mutex::new(map)),
            }
        }

        fn ran(&self) -> Vec<CommandSpec> {
            self.ran.lock().unwrap().clone()
        }
    }

    impl CommandRunner for FakeRunner {
        fn run(
            &self,
            spec: &CommandSpec,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = RunResult> + Send + '_>> {
            let spec = spec.clone();
            let ran = Arc::clone(&self.ran);
            let results = Arc::clone(&self.results);
            Box::pin(async move {
                ran.lock().unwrap().push(spec.clone());
                results
                    .lock()
                    .unwrap()
                    .get(&spec.program)
                    .copied()
                    .unwrap_or(RunResult::Failed)
            })
        }
    }

    fn config() -> FocusConfig {
        FocusConfig::default()
    }

    async fn engine_with(
        mock: &MockHerdr,
        backend: Backend,
        runner: FakeRunner,
    ) -> FocusEngine<FakeRunner> {
        FocusEngine::with_runner(
            HerdrClient::new(mock.socket_path()),
            config(),
            backend,
            runner,
        )
    }

    #[tokio::test]
    async fn focusing_calls_herdr_then_raises_the_window() {
        let mock = MockHerdr::start().await;
        mock.reply("agent.focus", json!({"type": "ok"})).await;
        let runner = FakeRunner::with(&[("hyprctl", RunResult::Success)]);
        let engine = engine_with(&mock, Backend::Hyprland, runner.clone()).await;

        let report = engine.focus_agent("w1:p1").await;
        assert!(report.herdr_focused);
        assert!(report.fully_succeeded());
        assert_eq!(mock.call_count("agent.focus").await, 1);
        assert_eq!(runner.ran().len(), 1);
    }

    #[tokio::test]
    async fn a_failed_herdr_focus_does_not_go_on_to_raise_a_window() {
        // Raising a window for a focus that did not happen would show the user the wrong pane.
        let mock = MockHerdr::start().await;
        mock.reply_error("agent.focus", "not_found", "pane not found")
            .await;
        let runner = FakeRunner::with(&[("hyprctl", RunResult::Success)]);
        let engine = engine_with(&mock, Backend::Hyprland, runner.clone()).await;

        let report = engine.focus_agent("w9:p9").await;
        assert!(!report.herdr_focused);
        assert!(!report.fully_succeeded());
        assert!(report.error.as_deref().unwrap().contains("pane not found"));
        assert!(
            runner.ran().is_empty(),
            "must not raise a window on failure"
        );
    }

    #[tokio::test]
    async fn backend_commands_are_tried_in_order_until_one_succeeds() {
        let mock = MockHerdr::start().await;
        mock.reply("agent.focus", json!({"type": "ok"})).await;
        // wmctrl fails to match; xdotool succeeds.
        let runner = FakeRunner::with(&[
            ("wmctrl", RunResult::Failed),
            ("xdotool", RunResult::Success),
        ]);
        let engine = engine_with(&mock, Backend::X11, runner.clone()).await;

        let outcome = engine.raise_window().await;
        assert_eq!(
            outcome,
            RaiseOutcome::Raised {
                via: "xdotool".into()
            }
        );
        assert_eq!(runner.ran().len(), 2, "should have fallen through wmctrl");
    }

    #[tokio::test]
    async fn a_missing_tool_is_reported_but_alternatives_are_still_tried() {
        let mock = MockHerdr::start().await;
        let runner = FakeRunner::with(&[
            ("wmctrl", RunResult::NotInstalled),
            ("xdotool", RunResult::Success),
        ]);
        let engine = engine_with(&mock, Backend::X11, runner).await;
        assert_eq!(
            engine.raise_window().await,
            RaiseOutcome::Raised {
                via: "xdotool".into()
            }
        );
    }

    #[tokio::test]
    async fn when_every_tool_is_missing_the_message_names_one_to_install() {
        let mock = MockHerdr::start().await;
        let runner = FakeRunner::with(&[
            ("wmctrl", RunResult::NotInstalled),
            ("xdotool", RunResult::NotInstalled),
        ]);
        let engine = engine_with(&mock, Backend::X11, runner).await;
        let outcome = engine.raise_window().await;
        assert!(matches!(outcome, RaiseOutcome::ToolMissing { .. }));
        assert!(outcome.describe().contains("wmctrl"));
    }

    #[tokio::test]
    async fn no_matching_window_is_distinct_from_a_missing_tool() {
        let mock = MockHerdr::start().await;
        let runner = FakeRunner::with(&[
            ("wmctrl", RunResult::Failed),
            ("xdotool", RunResult::Failed),
        ]);
        let engine = engine_with(&mock, Backend::X11, runner).await;
        assert_eq!(engine.raise_window().await, RaiseOutcome::NotFound);
    }

    #[tokio::test]
    async fn gnome_wayland_still_focuses_herdr_and_explains_why_the_window_did_not_move() {
        // The degradation path the user will actually hit. herdr focus must still happen.
        let mock = MockHerdr::start().await;
        mock.reply("agent.focus", json!({"type": "ok"})).await;
        let runner = FakeRunner::default();
        let engine = engine_with(&mock, Backend::Unsupported, runner.clone()).await;

        let report = engine.focus_agent("w1:p1").await;
        assert!(report.herdr_focused, "herdr focus must still work");
        assert!(
            !report.fully_succeeded(),
            "and we must not claim full success"
        );
        assert!(matches!(report.raise, RaiseOutcome::Unsupported { .. }));
        assert!(report.describe().contains("GNOME"));
        assert!(runner.ran().is_empty());
    }

    #[tokio::test]
    async fn disabling_window_raising_is_respected() {
        let mock = MockHerdr::start().await;
        mock.reply("agent.focus", json!({"type": "ok"})).await;
        let mut cfg = config();
        cfg.raise_window = false;
        let runner = FakeRunner::with(&[("hyprctl", RunResult::Success)]);
        let engine = FocusEngine::with_runner(
            HerdrClient::new(mock.socket_path()),
            cfg,
            Backend::Hyprland,
            runner.clone(),
        );

        let report = engine.focus_agent("w1:p1").await;
        assert!(report.herdr_focused);
        assert_eq!(report.raise, RaiseOutcome::Disabled);
        assert!(runner.ran().is_empty());
    }

    #[tokio::test]
    async fn the_title_marker_is_installed_and_then_used_to_target_the_window() {
        let mock = MockHerdr::start().await;
        mock.reply("client.window_title.set", json!({"type": "ok"}))
            .await;
        let runner = FakeRunner::with(&[("hyprctl", RunResult::Success)]);
        let mut engine = engine_with(&mock, Backend::Hyprland, runner.clone()).await;

        assert!(engine.install_title_marker("herdr-deck:abc123").await);
        engine.raise_window().await;

        let first = &runner.ran()[0];
        assert!(
            first.args.iter().any(|a| a.contains("herdr-deck:abc123")),
            "the marker should be used to find the exact window: {first:?}"
        );
    }

    #[tokio::test]
    async fn macos_skips_the_title_marker_because_it_cannot_target_one_window() {
        // Setting it would rewrite the user's terminal title for no benefit.
        let mock = MockHerdr::start().await;
        mock.reply("client.window_title.set", json!({"type": "ok"}))
            .await;
        let mut engine = engine_with(&mock, Backend::MacOs, FakeRunner::default()).await;

        assert!(!engine.install_title_marker("herdr-deck:abc").await);
        assert_eq!(engine.title_marker(), None);
        assert_eq!(mock.call_count("client.window_title.set").await, 0);
    }

    #[tokio::test]
    async fn a_herdr_failure_while_setting_the_marker_is_not_fatal() {
        let mock = MockHerdr::start().await;
        mock.reply_error(
            "client.window_title.set",
            "unsupported",
            "no client attached",
        )
        .await;
        let mut engine = engine_with(&mock, Backend::Sway, FakeRunner::default()).await;
        assert!(!engine.install_title_marker("herdr-deck:abc").await);
        assert_eq!(engine.title_marker(), None);
    }

    #[tokio::test]
    async fn macos_raises_by_bundle_id_from_config() {
        let mock = MockHerdr::start().await;
        let runner = FakeRunner::with(&[("open", RunResult::Success)]);
        let engine = engine_with(&mock, Backend::MacOs, runner.clone()).await;
        engine.raise_window().await;
        assert_eq!(
            runner.ran()[0].args,
            vec!["-b", "com.mitchellh.ghostty"],
            "should use the configured Ghostty bundle id"
        );
    }

    #[tokio::test]
    async fn focusing_a_workspace_follows_the_same_two_step_path() {
        let mock = MockHerdr::start().await;
        mock.reply("workspace.focus", json!({"type": "ok"})).await;
        let runner = FakeRunner::with(&[("hyprctl", RunResult::Success)]);
        let engine = engine_with(&mock, Backend::Hyprland, runner.clone()).await;

        let report = engine.focus_workspace("w2").await;
        assert!(report.fully_succeeded());
        let params = mock.observed_params("workspace.focus").await.unwrap();
        assert_eq!(params["workspace_id"], "w2");
    }
}
