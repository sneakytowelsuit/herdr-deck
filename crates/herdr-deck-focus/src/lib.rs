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

use herdr_deck_core::command::DeckCommand;
use herdr_deck_core::config::FocusConfig;
use herdr_deck_herdr::HerdrClient;

/// How long a window-raise command gets before we give up on it.
///
/// A wedged `hyprctl` must never stall the deck's event loop.
const RAISE_TIMEOUT: Duration = Duration::from_secs(3);

/// What the deck says when it asks herdr whether anyone is attached.
///
/// A canned string, because a deck has no keyboard and because this ends up in front of the user
/// when a client *is* attached — in which case it is a true and useful thing to have said.
const NO_CLIENT_PROBE: &str = "herdr-deck: could not raise the window";

/// What happened when we tried to raise a window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RaiseOutcome {
    /// A command succeeded.
    Raised { via: String },
    /// Every command ran but none matched a window.
    NotFound,
    /// There is no window because nothing is attached to herdr.
    ///
    /// Not a failure. herdr owns the focus, not its clients: it moved, it persists, and the next
    /// terminal to attach opens on that pane. Reporting this as an error would teach people to
    /// ignore the deck's alerts, which is the one thing this hardware cannot afford.
    NoClient,
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
            RaiseOutcome::NoClient => {
                "herdr focused it; nothing is attached, so it waits for the next one".to_string()
            }
            RaiseOutcome::Unsupported { reason } => reason.clone(),
            RaiseOutcome::ToolMissing { program } => {
                format!("herdr focused, but `{program}` is not installed to raise the window")
            }
            RaiseOutcome::Disabled => "herdr focused (window raising is disabled)".to_string(),
        }
    }
}

/// How much of what the user asked for actually happened.
///
/// Three outcomes rather than two, because "not everything happened" and "something went wrong"
/// are different things and a key that conflates them is a key whose alerts stop meaning
/// anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusVerdict {
    /// herdr focused it and the window came forward.
    Complete,
    /// herdr focused it, and there was nothing further that could be done.
    ///
    /// Nothing is attached to raise, so herdr's focus is the whole of the journey available: it
    /// persists, and the next client to attach opens there. Everything that *could* happen did,
    /// which is why this must not read as a failure.
    Settled,
    /// herdr focused it, but the window did not come forward and could have.
    Partial,
    /// herdr did not focus it, so nothing happened at all.
    Failed,
}

impl FocusVerdict {
    /// Should the key flash an alert?
    pub fn is_error(self) -> bool {
        matches!(self, FocusVerdict::Partial | FocusVerdict::Failed)
    }

    /// A stable word for the audit log.
    pub fn as_str(self) -> &'static str {
        match self {
            FocusVerdict::Complete => "complete",
            FocusVerdict::Settled => "settled",
            FocusVerdict::Partial => "partial",
            FocusVerdict::Failed => "failed",
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
    /// How much of the two-step focus happened.
    pub fn verdict(&self) -> FocusVerdict {
        if !self.herdr_focused || self.error.is_some() {
            return FocusVerdict::Failed;
        }
        match self.raise {
            RaiseOutcome::Raised { .. } => FocusVerdict::Complete,
            RaiseOutcome::NoClient => FocusVerdict::Settled,
            _ => FocusVerdict::Partial,
        }
    }

    /// Did everything the user asked for actually happen?
    ///
    /// Deliberately strict: focusing herdr without raising the window is a *partial* success,
    /// and the deck flashes an alert for it so the user is never left wondering why nothing
    /// appeared to happen. Use [`FocusReport::verdict`] to tell that apart from the case where
    /// there was no window to raise in the first place.
    pub fn fully_succeeded(&self) -> bool {
        self.verdict() == FocusVerdict::Complete
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
            command.stderr(std::process::Stdio::null());
            // Only capture stdout when the exit code alone cannot be trusted; otherwise let it
            // go to /dev/null so a chatty tool cannot fill a pipe nobody drains.
            if spec.requires_output {
                command.stdout(std::process::Stdio::piped());
            } else {
                command.stdout(std::process::Stdio::null());
            }

            let finished = if spec.requires_output {
                tokio::time::timeout(RAISE_TIMEOUT, command.output())
                    .await
                    .map(|result| {
                        result.map(|out| {
                            (out.status, !out.stdout.iter().all(u8::is_ascii_whitespace))
                        })
                    })
            } else {
                tokio::time::timeout(RAISE_TIMEOUT, command.status())
                    .await
                    .map(|result| result.map(|status| (status, true)))
            };

            match finished {
                Ok(Ok((status, produced_output))) if status.success() => {
                    if produced_output {
                        RunResult::Success
                    } else {
                        // Exited zero but matched nothing. Reporting this as success is how a
                        // user ends up being told the window was raised while nothing moved.
                        tracing::debug!(
                            program = %spec.program,
                            "command exited zero but matched nothing; treating as failure"
                        );
                        RunResult::Failed
                    }
                }
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

    /// Perform one command.
    ///
    /// The single place a [`DeckCommand`] becomes something that happens. Adding a command means
    /// an arm here and a call in `herdr-deck-herdr`; nothing between the key and this function
    /// has to learn what the new command is.
    pub async fn perform(&self, command: &DeckCommand) -> FocusReport {
        match command {
            DeckCommand::FocusPane { pane_id } => self.focus_agent(pane_id).await,
            DeckCommand::FocusWorkspace { workspace_id } => {
                self.focus_workspace(workspace_id).await
            }
            DeckCommand::FocusTab { tab_id } => self.focus_tab(tab_id).await,
        }
    }

    /// Focus an agent: switch herdr's active pane, then raise the terminal window.
    ///
    /// `target` is an agent name or a **pane id** — herdr's `agent.focus` does not accept a
    /// `terminal_id`, so callers holding a stable terminal id must resolve it against current
    /// state first.
    ///
    /// One call, and only one: herdr walks workspace → tab → pane itself and follows zoom while
    /// it does, so there is deliberately no workspace or tab focus around this.
    pub async fn focus_agent(&self, target: &str) -> FocusReport {
        self.report(self.client.agent_focus(target).await).await
    }

    pub async fn focus_workspace(&self, workspace_id: &str) -> FocusReport {
        self.report(self.client.workspace_focus(workspace_id).await)
            .await
    }

    pub async fn focus_tab(&self, tab_id: &str) -> FocusReport {
        self.report(self.client.tab_focus(tab_id).await).await
    }

    /// Turn "herdr said yes or no" into the two-step report, raising the window when it said yes.
    async fn report(&self, focused: herdr_deck_herdr::Result<()>) -> FocusReport {
        match focused {
            Ok(()) => FocusReport {
                herdr_focused: true,
                raise: self.raise_or_explain().await,
                error: None,
            },
            // Raising a window for a focus that did not happen would show the user the wrong pane.
            Err(e) => FocusReport {
                herdr_focused: false,
                raise: RaiseOutcome::Disabled,
                error: Some(e.to_string()),
            },
        }
    }

    /// Raise the window, and work out whether "no window" means there was never one to find.
    ///
    /// Only asked when the raise matched nothing, because that is the only outcome the answer
    /// changes — and because the question costs a round trip and, when a client *is* attached,
    /// a toast the user did not ask for. A headless herdr answers `no_foreground_client`, which
    /// turns a reported failure into the honest "focused, and it will be there when you attach".
    /// Anything else, including the probe itself failing, leaves the original verdict alone.
    async fn raise_or_explain(&self) -> RaiseOutcome {
        let outcome = self.raise_window().await;
        if outcome != RaiseOutcome::NotFound {
            return outcome;
        }
        match self.client.notification_show(NO_CLIENT_PROBE, None).await {
            Ok(reason) if reason == "no_foreground_client" => RaiseOutcome::NoClient,
            Ok(_) => outcome,
            Err(e) => {
                tracing::debug!(error = %e, "could not ask herdr whether a client is attached");
                outcome
            }
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
    use herdr_deck_herdr::wire::SessionSnapshot;
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
    async fn a_command_that_exits_zero_without_matching_anything_is_not_success() {
        // Exercised against real processes, because this is precisely the behaviour that
        // cannot be verified by inspecting a CommandSpec: `true` succeeds silently, which is
        // indistinguishable from "kdotool matched nothing" unless we look at stdout.
        let runner = ProcessRunner;

        let silent = CommandSpec::new_requiring_output("true", &[]);
        assert_eq!(
            runner.run(&silent).await,
            RunResult::Failed,
            "a zero exit with no output must not be reported as a raised window"
        );

        let speaks = CommandSpec::new_requiring_output("echo", &["{aabbcc-window-id}"]);
        assert_eq!(runner.run(&speaks).await, RunResult::Success);

        // Whitespace is not output: a trailing newline alone means nothing matched.
        let blank = CommandSpec::new_requiring_output("echo", &[""]);
        assert_eq!(runner.run(&blank).await, RunResult::Failed);
    }

    #[tokio::test]
    async fn a_command_we_trust_on_exit_code_alone_is_unaffected_by_its_output() {
        let runner = ProcessRunner;
        assert_eq!(
            runner.run(&CommandSpec::new("true", &[])).await,
            RunResult::Success
        );
        assert_eq!(
            runner.run(&CommandSpec::new("false", &[])).await,
            RunResult::Failed
        );
    }

    #[tokio::test]
    async fn a_missing_program_is_still_distinguished_from_a_failed_one() {
        let runner = ProcessRunner;
        assert_eq!(
            runner
                .run(&CommandSpec::new_requiring_output(
                    "herdr-deck-no-such-tool",
                    &[]
                ))
                .await,
            RunResult::NotInstalled
        );
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

    // --- Focusing a herdr nobody is looking at ------------------------------------------------
    //
    // herdr owns the focus, not its clients: a headless server still moves it, still persists it,
    // and the next terminal to attach opens on that pane. So the raise finding no window is only
    // a failure when there was a window to find.

    #[tokio::test]
    async fn focusing_with_nothing_attached_to_herdr_is_a_settled_outcome_and_not_a_failure() {
        let mock = MockHerdr::start().await;
        mock.serve_session(&SessionSnapshot::default()).await;
        mock.without_attached_client().await;
        // Every raise command runs and matches nothing, because there is no window to match.
        let runner = FakeRunner::with(&[("hyprctl", RunResult::Failed)]);
        let engine = engine_with(&mock, Backend::Hyprland, runner.clone()).await;

        let report = engine.focus_agent("w1:p1").await;
        assert!(report.herdr_focused);
        assert_eq!(report.raise, RaiseOutcome::NoClient);
        assert_eq!(
            report.verdict(),
            FocusVerdict::Settled,
            "there was nothing more that could have been done, so nothing was left undone"
        );
        assert!(
            !report.verdict().is_error(),
            "a key that alerted here would be crying wolf, and people stop reading alerts"
        );
    }

    #[tokio::test]
    async fn a_window_that_is_merely_hidden_is_still_reported_as_a_partial_focus() {
        // The distinction the whole outcome rests on: a client *is* attached, so there was a
        // window that should have come forward and did not. That is a real failure to report.
        let mock = MockHerdr::start().await;
        mock.serve_session(&SessionSnapshot::default()).await;
        let runner = FakeRunner::with(&[("hyprctl", RunResult::Failed)]);
        let engine = engine_with(&mock, Backend::Hyprland, runner).await;

        let report = engine.focus_agent("w1:p1").await;
        assert_eq!(report.raise, RaiseOutcome::NotFound);
        assert_eq!(report.verdict(), FocusVerdict::Partial);
        assert!(report.verdict().is_error());
    }

    #[tokio::test]
    async fn herdr_is_only_asked_who_is_attached_when_the_window_could_not_be_found() {
        // The question costs a round trip and, when a client is attached, a toast. Asking it on
        // every successful focus would put one in front of the user for nothing.
        let mock = MockHerdr::start().await;
        mock.serve_session(&SessionSnapshot::default()).await;
        let runner = FakeRunner::with(&[("hyprctl", RunResult::Success)]);
        let engine = engine_with(&mock, Backend::Hyprland, runner).await;

        assert!(engine.focus_agent("w1:p1").await.fully_succeeded());
        assert_eq!(mock.call_count("notification.show").await, 0);
    }

    #[tokio::test]
    async fn a_focus_reaches_herdr_as_exactly_one_call_and_never_chains_a_workspace_or_tab_around_it(
    ) {
        // herdr walks workspace → tab → pane itself from `agent.focus`, marks the tab seen and
        // follows zoom. Helpfully "preparing the way" with a workspace focus first would make
        // three round trips of a journey herdr does atomically, and fight its own logic.
        let mock = MockHerdr::start().await;
        mock.serve_session(&SessionSnapshot::default()).await;
        let runner = FakeRunner::with(&[("hyprctl", RunResult::Success)]);
        let engine = engine_with(&mock, Backend::Hyprland, runner).await;

        engine
            .perform(&DeckCommand::FocusPane {
                pane_id: "w1:p1".into(),
            })
            .await;

        assert_eq!(mock.agent_focus_targets().await, vec!["w1:p1".to_string()]);
        assert!(mock.workspace_focus_ids().await.is_empty());
        assert!(mock.tab_focus_ids().await.is_empty());
    }

    #[tokio::test]
    async fn every_command_the_deck_knows_reaches_herdr() {
        // One arm per command, in one place. This is the test that fails when a command is added
        // to the vocabulary and never wired to anything.
        let mock = MockHerdr::start().await;
        mock.serve_session(&SessionSnapshot::default()).await;
        let runner = FakeRunner::with(&[("hyprctl", RunResult::Success)]);
        let engine = engine_with(&mock, Backend::Hyprland, runner).await;

        for command in [
            DeckCommand::FocusPane {
                pane_id: "w1:p1".into(),
            },
            DeckCommand::FocusWorkspace {
                workspace_id: "w2".into(),
            },
            DeckCommand::FocusTab {
                tab_id: "w1:t2".into(),
            },
        ] {
            let report = engine.perform(&command).await;
            assert!(
                report.herdr_focused,
                "{} did not reach herdr: {report:?}",
                command.name()
            );
        }
    }

    #[tokio::test]
    async fn focusing_a_tab_follows_the_same_two_step_path() {
        // A tab focus that skipped the raise would switch herdr underneath a window the user
        // still cannot see, which is exactly the half-done outcome we report loudly elsewhere.
        let mock = MockHerdr::start().await;
        mock.serve_session(&SessionSnapshot::default()).await;
        let runner = FakeRunner::with(&[("hyprctl", RunResult::Success)]);
        let engine = engine_with(&mock, Backend::Hyprland, runner.clone()).await;

        let report = engine.focus_tab("w1:t2").await;
        assert!(report.herdr_focused);
        assert!(report.fully_succeeded());
        assert_eq!(mock.tab_focus_ids().await, vec!["w1:t2".to_string()]);
        assert_eq!(runner.ran().len(), 1, "the window must still be raised");
    }
}
