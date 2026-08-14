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
//!
//! # Not every command is a journey
//!
//! This crate performs the deck's whole write surface, and only part of it is a focus. Moving one
//! pane left, zooming, splitting and closing all rearrange the terminal the user is already
//! sitting at, so they do step 1 and stop there. See [`DeckCommand::raises_the_window`], which is
//! where that line is drawn, and [`in_place`], which is where it is honoured.

pub mod backend;
pub mod detect;

use std::time::Duration;

pub use backend::{Backend, CommandSpec, WindowTarget};
pub use detect::{detect, detect_or_override, FocusEnv, TargetOs};

use herdr_deck_core::command::DeckCommand;
use herdr_deck_core::config::FocusConfig;
use herdr_deck_herdr::wire::{NothingToDo, PaneOutcome};
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
    /// The command never wanted a window.
    ///
    /// Splitting, zooming, closing and stepping between panes rearrange the terminal the user is
    /// already sitting at. Fetching that terminal would spend a round trip nobody asked for, and
    /// on a desktop that cannot raise windows it would turn every one of those presses into an
    /// alert about a failure that never happened.
    NotNeeded,
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
            RaiseOutcome::NotNeeded => "herdr did it where you already were".to_string(),
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
/// More outcomes than the two a key can show, because "not everything happened", "there was
/// nothing to happen" and "something went wrong" are three different things and a key that
/// conflated them is a key whose alerts stop meaning anything. Only [`FocusVerdict::is_error`]
/// reaches the deck; the rest of the distinction is for the log.
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
    /// herdr understood, and there was nothing to change.
    ///
    /// The edge of a layout, a tab with one pane in it, a zoom that was already on. herdr reports
    /// all three as successes carrying a reason, and so does the deck: a key that flashed an alert
    /// every time a thumb ran off the end of a row would teach its owner to stop reading alerts,
    /// and the alerts are the only thing this hardware has to say something is wrong.
    ///
    /// Kept apart from [`FocusVerdict::Complete`] for the log's sake, not the key's — "I pressed
    /// left four times and only moved twice" is a question the trail should be able to answer.
    Unchanged,
    /// herdr focused it, but the window did not come forward and could have.
    Partial,
    /// herdr did not do it, so nothing happened at all.
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
            FocusVerdict::Unchanged => "unchanged",
            FocusVerdict::Partial => "partial",
            FocusVerdict::Failed => "failed",
        }
    }
}

/// The result of one command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FocusReport {
    /// Did herdr carry the command out?
    pub herdr_acted: bool,
    pub raise: RaiseOutcome,
    /// Set when herdr refused.
    pub error: Option<String>,
    /// Set when herdr understood and there was nothing to change.
    ///
    /// Distinct from `error`, and the distinction is load-bearing: this is herdr answering the
    /// question rather than declining it, so it must never reach the key as an alert.
    pub unchanged: Option<NothingToDo>,
}

impl FocusReport {
    /// How much of what the user asked for actually happened.
    pub fn verdict(&self) -> FocusVerdict {
        if !self.herdr_acted || self.error.is_some() {
            return FocusVerdict::Failed;
        }
        if self.unchanged.is_some() {
            return FocusVerdict::Unchanged;
        }
        match self.raise {
            RaiseOutcome::Raised { .. } => FocusVerdict::Complete,
            RaiseOutcome::NoClient | RaiseOutcome::NotNeeded => FocusVerdict::Settled,
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
        match (&self.error, self.unchanged) {
            (Some(err), _) => format!("herdr refused: {err}"),
            (None, Some(why)) => why.describe().to_string(),
            (None, None) => self.raise.describe(),
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

            // Everything below rearranges the terminal the user is already at, so none of it
            // raises a window — see [`DeckCommand::raises_the_window`], and the test that holds
            // the two halves of that decision together.
            DeckCommand::MovePaneFocus { direction } => {
                in_place(self.client.pane_focus_direction(*direction).await)
            }
            DeckCommand::ZoomPane { zoom } => in_place(self.client.pane_zoom(*zoom).await),
            DeckCommand::SplitPane { direction } => in_place(
                self.client
                    .pane_split(*direction)
                    .await
                    .map(|()| PaneOutcome::Changed),
            ),
            DeckCommand::ClosePane { pane_id } => in_place(
                self.client
                    .pane_close(pane_id)
                    .await
                    .map(|()| PaneOutcome::Changed),
            ),

            // Opening a worktree is the one structural command that is a journey, so it is the one
            // that also fetches the window — see [`DeckCommand::raises_the_window`].
            DeckCommand::OpenWorktree { path } => {
                self.report(self.client.worktree_open(path).await).await
            }

            // Everything below makes or unmakes part of herdr's structure around the terminal the
            // user is already at.
            DeckCommand::ApplyLayout { layout, .. } => {
                in_place(done(self.client.layout_apply(layout).await))
            }
            DeckCommand::CreateWorkspace { spec, .. } => {
                in_place(done(self.client.workspace_create(spec).await))
            }
            DeckCommand::CreateTab { spec, .. } => {
                in_place(done(self.client.tab_create(spec).await))
            }
            DeckCommand::CreateWorktree => in_place(done(self.client.worktree_create().await)),
            DeckCommand::RemoveWorktree { workspace_id } => {
                in_place(done(self.client.worktree_remove(workspace_id).await))
            }
            DeckCommand::CloseTab { tab_id } => in_place(done(self.client.tab_close(tab_id).await)),
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
                herdr_acted: true,
                raise: self.raise_or_explain().await,
                error: None,
                unchanged: None,
            },
            // Raising a window for a focus that did not happen would show the user the wrong pane.
            Err(e) => FocusReport {
                herdr_acted: false,
                raise: RaiseOutcome::Disabled,
                error: Some(e.to_string()),
                unchanged: None,
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

/// Turn "herdr did it, or did not, or found nothing to do" into a report, with no window involved.
///
/// The three outcomes stay apart all the way to the key. `Changed` is a plain success; `Unchanged`
/// is herdr answering the question rather than declining it, and must not alert; only an error is
/// a failure. Collapsing the middle one into either of the others is the mistake that would make
/// this deck's alerts worthless — a thumb run along a row of direction keys ends at an edge every
/// single time.
/// herdr answered a bare `ok`, so there was nothing for it to report but success.
///
/// Most of herdr's structural commands are like this: they make a thing, and the only two answers
/// are "made it" and an error. Naming the conversion rather than writing the same `map` at every
/// call site is what keeps the arms above readable as a list of one-line commands.
fn done(result: herdr_deck_herdr::Result<()>) -> herdr_deck_herdr::Result<PaneOutcome> {
    result.map(|()| PaneOutcome::Changed)
}

fn in_place(result: herdr_deck_herdr::Result<PaneOutcome>) -> FocusReport {
    match result {
        Ok(PaneOutcome::Changed) => FocusReport {
            herdr_acted: true,
            raise: RaiseOutcome::NotNeeded,
            error: None,
            unchanged: None,
        },
        Ok(PaneOutcome::Unchanged(why)) => FocusReport {
            herdr_acted: true,
            raise: RaiseOutcome::NotNeeded,
            error: None,
            unchanged: Some(why),
        },
        Err(e) => FocusReport {
            herdr_acted: false,
            raise: RaiseOutcome::NotNeeded,
            error: Some(e.to_string()),
            unchanged: None,
        },
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
        assert!(report.herdr_acted);
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
        assert!(!report.herdr_acted);
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
        assert!(report.herdr_acted, "herdr focus must still work");
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
        assert!(report.herdr_acted);
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
        assert!(report.herdr_acted);
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

        for command in DeckCommand::every() {
            let report = engine.perform(&command).await;
            assert!(
                report.herdr_acted,
                "{} did not reach herdr: {report:?}",
                command.name()
            );
        }
    }

    // --- Pane control ------------------------------------------------------------------------

    #[tokio::test]
    async fn each_direction_key_sends_its_own_direction_and_nothing_else() {
        // Four commands that differ only in one enum value: exactly where a copy-paste mistake
        // hides, and exactly the mistake nobody would report as a bug — they would assume they had
        // misremembered their own layout.
        let mock = MockHerdr::start().await;
        mock.serve_session(&SessionSnapshot::default()).await;
        let engine = engine_with(&mock, Backend::Hyprland, FakeRunner::default()).await;

        for direction in herdr_deck_herdr::wire::PaneDirection::ALL {
            engine
                .perform(&DeckCommand::MovePaneFocus { direction })
                .await;
        }

        assert_eq!(
            mock.pane_move_directions().await,
            vec!["left", "right", "up", "down"]
        );
    }

    #[tokio::test]
    async fn stepping_between_panes_never_fetches_the_window() {
        // These are pressed while sitting at the terminal doing the thing. Raising the window on
        // every press would spend a round trip nobody asked for — and, on a desktop that cannot
        // raise windows at all, would turn a working key into one that alerts every time.
        let mock = MockHerdr::start().await;
        mock.serve_session(&SessionSnapshot::default()).await;
        let runner = FakeRunner::with(&[("hyprctl", RunResult::Success)]);
        let engine = engine_with(&mock, Backend::Hyprland, runner.clone()).await;

        let report = engine
            .perform(&DeckCommand::MovePaneFocus {
                direction: herdr_deck_herdr::wire::PaneDirection::Left,
            })
            .await;

        assert!(
            runner.ran().is_empty(),
            "no window should have been touched"
        );
        assert_eq!(report.raise, RaiseOutcome::NotNeeded);
        assert!(
            !report.verdict().is_error(),
            "and not raising must not read as a shortfall"
        );
    }

    #[tokio::test]
    async fn reaching_the_edge_of_a_layout_is_not_reported_as_a_failure() {
        // herdr answers this as a success carrying a reason, and so must the deck. A thumb run
        // along a row of direction keys ends at an edge every time; a key that alerted for it
        // would teach its owner to stop reading alerts within a day.
        let mock = MockHerdr::start().await;
        mock.serve_session(&SessionSnapshot::default()).await;
        mock.with_no_pane_that_way().await;
        let engine = engine_with(&mock, Backend::Hyprland, FakeRunner::default()).await;

        let report = engine
            .perform(&DeckCommand::MovePaneFocus {
                direction: herdr_deck_herdr::wire::PaneDirection::Up,
            })
            .await;

        assert!(report.herdr_acted, "herdr answered; it did not refuse");
        assert!(report.error.is_none());
        assert_eq!(report.verdict(), FocusVerdict::Unchanged);
        assert!(!report.verdict().is_error());
        assert!(
            report.describe().contains("no pane that way"),
            "the log still deserves to know why nothing moved: {}",
            report.describe()
        );
    }

    #[tokio::test]
    async fn a_zoom_that_was_already_as_asked_is_a_success_and_not_a_wasted_press() {
        // The whole reason for stating the end state instead of toggling: press it twice and the
        // second press is a no-op that says so, rather than an unzoom nobody wanted.
        let mock = MockHerdr::start().await;
        mock.serve_session(&SessionSnapshot::default()).await;
        mock.with_zoom_already_as_asked().await;
        let engine = engine_with(&mock, Backend::Hyprland, FakeRunner::default()).await;

        let report = engine
            .perform(&DeckCommand::ZoomPane {
                zoom: herdr_deck_herdr::wire::ZoomMode::On,
            })
            .await;
        assert_eq!(report.verdict(), FocusVerdict::Unchanged);
        assert!(!report.verdict().is_error());
        assert_eq!(mock.pane_zoom_modes().await, vec!["on"]);
    }

    #[tokio::test]
    async fn splitting_right_and_splitting_down_arrive_at_herdr_as_different_things() {
        let mock = MockHerdr::start().await;
        mock.serve_session(&SessionSnapshot::default()).await;
        let engine = engine_with(&mock, Backend::Hyprland, FakeRunner::default()).await;

        for direction in herdr_deck_herdr::wire::SplitDirection::ALL {
            engine.perform(&DeckCommand::SplitPane { direction }).await;
        }
        assert_eq!(mock.pane_split_directions().await, vec!["right", "down"]);
    }

    #[tokio::test]
    async fn closing_a_pane_names_exactly_the_pane_it_was_given() {
        let mock = MockHerdr::start().await;
        mock.serve_session(&SessionSnapshot::default()).await;
        let engine = engine_with(&mock, Backend::Hyprland, FakeRunner::default()).await;

        let report = engine
            .perform(&DeckCommand::ClosePane {
                pane_id: "w2:p4".into(),
            })
            .await;
        assert!(report.herdr_acted);
        assert_eq!(mock.pane_close_ids().await, vec!["w2:p4".to_string()]);
    }

    #[tokio::test]
    async fn a_close_herdr_wants_confirmed_is_a_failure_that_says_where_the_question_went() {
        // herdr has opened a modal the deck can neither answer nor dismiss. Reporting a bare
        // failure would leave the user with a dialog they did not open and nothing connecting it
        // to the key they pressed.
        let mock = MockHerdr::start().await;
        mock.serve_session(&SessionSnapshot::default()).await;
        mock.with_a_close_herdr_wants_confirmed().await;
        let engine = engine_with(&mock, Backend::Hyprland, FakeRunner::default()).await;

        let report = engine
            .perform(&DeckCommand::ClosePane {
                pane_id: "w1:p1".into(),
            })
            .await;
        assert!(!report.herdr_acted);
        assert_eq!(report.verdict(), FocusVerdict::Failed);
        assert!(
            report.describe().contains("its own window"),
            "got {}",
            report.describe()
        );
    }

    #[tokio::test]
    async fn a_pane_command_reaches_herdr_as_exactly_one_call_and_drags_no_focus_along_with_it() {
        // The same discipline as the focus path: herdr resolves "wherever I am" itself, and a
        // helpful workspace or pane focus sent first would both cost a round trip and hand herdr
        // a target the deck read some seconds ago.
        let mock = MockHerdr::start().await;
        mock.serve_session(&SessionSnapshot::default()).await;
        let engine = engine_with(&mock, Backend::Hyprland, FakeRunner::default()).await;

        engine
            .perform(&DeckCommand::ZoomPane {
                zoom: herdr_deck_herdr::wire::ZoomMode::Off,
            })
            .await;

        assert_eq!(mock.pane_zoom_modes().await.len(), 1);
        assert!(mock.agent_focus_targets().await.is_empty());
        assert!(mock.workspace_focus_ids().await.is_empty());
        assert!(mock.tab_focus_ids().await.is_empty());
    }

    // --- Structure ---------------------------------------------------------------------------

    #[tokio::test]
    async fn opening_a_worktree_focuses_and_raises_exactly_as_focusing_an_agent_does() {
        // This is the promise the worktree list is for: it is the same "get me there" motion, and
        // half of getting there is the window. A worktree key that switched herdr and left the
        // user staring at a browser would be the half-done outcome the whole crate exists to
        // report loudly.
        let mock = MockHerdr::start().await;
        mock.serve_session(&SessionSnapshot::default()).await;
        let runner = FakeRunner::with(&[("hyprctl", RunResult::Success)]);
        let engine = engine_with(&mock, Backend::Hyprland, runner.clone()).await;

        let report = engine
            .perform(&DeckCommand::OpenWorktree {
                path: "/src/.worktrees/api/fix".into(),
            })
            .await;

        assert!(report.fully_succeeded(), "{report:?}");
        assert_eq!(
            mock.worktree_open_paths().await,
            vec!["/src/.worktrees/api/fix"]
        );
        assert_eq!(runner.ran().len(), 1, "the window has to come forward too");
    }

    #[tokio::test]
    async fn making_something_never_fetches_the_window() {
        // You are already at the terminal — you just asked it for a new tab. Raising would spend a
        // round trip nobody asked for and, on a desktop that cannot raise windows at all, would
        // turn every one of these into an alert about a failure that never happened.
        let mock = MockHerdr::start().await;
        mock.serve_session(&SessionSnapshot::default()).await;
        let runner = FakeRunner::with(&[("hyprctl", RunResult::Success)]);
        let engine = engine_with(&mock, Backend::Hyprland, runner.clone()).await;

        for command in DeckCommand::every()
            .into_iter()
            .filter(|c| !c.raises_the_window())
        {
            let report = engine.perform(&command).await;
            assert_eq!(
                report.raise,
                RaiseOutcome::NotNeeded,
                "{} went looking for a window",
                command.name()
            );
        }
        assert!(runner.ran().is_empty());
    }

    #[tokio::test]
    async fn applying_a_named_layout_makes_a_tab_and_never_touches_an_existing_one() {
        // The single most dangerous thing in this step. With a tab id, `layout.apply` builds the
        // replacement and then closes the tab it was given, killing every process in it — with no
        // confirmation and nothing in its name to warn you. The client cannot express that; this
        // is the test standing behind that decision from above.
        let mock = MockHerdr::start().await;
        mock.serve_session(&SessionSnapshot::default()).await;
        let engine = engine_with(&mock, Backend::Hyprland, FakeRunner::default()).await;

        engine
            .perform(&DeckCommand::ApplyLayout {
                preset: "dev".into(),
                layout: herdr_deck_herdr::wire::LayoutPreset {
                    label: Some("dev".into()),
                    root: herdr_deck_herdr::wire::LayoutNode::default(),
                },
            })
            .await;

        let params = mock.observed_params("layout.apply").await.unwrap();
        assert!(
            params.get("tab_id").is_none(),
            "a preset key must never target an existing tab: {params}"
        );
        assert_eq!(mock.tab_close_ids().await, Vec::<String>::new());
    }

    #[tokio::test]
    async fn removing_a_worktree_is_never_forced_however_it_is_asked_for() {
        // Forcing kills the workspace's agents *before* git runs, and then deletes uncommitted and
        // untracked files. Unforced, git refuses a dirty checkout — and that refusal is the only
        // confirmation prompt a device with no screen can offer.
        let mock = MockHerdr::start().await;
        mock.serve_session(&SessionSnapshot::default()).await;
        let engine = engine_with(&mock, Backend::Hyprland, FakeRunner::default()).await;

        engine
            .perform(&DeckCommand::RemoveWorktree {
                workspace_id: "w3".into(),
            })
            .await;

        assert_eq!(mock.worktree_remove_ids().await, vec!["w3"]);
        let params = mock.observed_params("worktree.remove").await.unwrap();
        assert_eq!(params["force"], false, "got {params}");
    }

    #[tokio::test]
    async fn git_refusing_to_remove_a_dirty_worktree_lands_on_the_key_and_is_not_swallowed() {
        // The refusal *is* the safety mechanism. Reporting it as a success would leave somebody
        // believing their worktree was gone when it is still there — and, worse, teach them that
        // this key does not really work so they should force it somehow.
        let mock = MockHerdr::start().await;
        mock.serve_session(&SessionSnapshot::default()).await;
        mock.reply_error(
            "worktree.remove",
            "worktree_remove_failed",
            "contains modified or untracked files, use --force to delete it",
        )
        .await;
        let engine = engine_with(&mock, Backend::Hyprland, FakeRunner::default()).await;

        let report = engine
            .perform(&DeckCommand::RemoveWorktree {
                workspace_id: "w3".into(),
            })
            .await;

        assert!(report.verdict().is_error(), "{report:?}");
        assert!(report.describe().contains("untracked"), "{report:?}");
    }

    #[tokio::test]
    async fn a_preset_reaches_herdr_as_the_working_directory_and_label_it_names() {
        // Presets are the only way any of this reaches herdr from a deck, so a preset that arrived
        // stripped of its settings would look exactly like a key that works and quietly does the
        // wrong thing.
        let mock = MockHerdr::start().await;
        mock.serve_session(&SessionSnapshot::default()).await;
        let engine = engine_with(&mock, Backend::Hyprland, FakeRunner::default()).await;

        engine
            .perform(&DeckCommand::CreateWorkspace {
                preset: Some("notes".into()),
                spec: herdr_deck_herdr::wire::CreateSpec {
                    label: Some("notes".into()),
                    cwd: Some("/home/dev/notes".into()),
                    ..Default::default()
                },
            })
            .await;

        let params = mock.observed_params("workspace.create").await.unwrap();
        assert_eq!(params["label"], "notes");
        assert_eq!(params["cwd"], "/home/dev/notes");
    }

    #[tokio::test]
    async fn a_structural_command_reaches_herdr_as_one_call_and_drags_no_focus_with_it() {
        // Same discipline as everywhere else: herdr resolves "where I am" itself, and a helpful
        // workspace focus sent first would cost a round trip and hand herdr a target the deck read
        // seconds ago.
        let mock = MockHerdr::start().await;
        mock.serve_session(&SessionSnapshot::default()).await;
        let engine = engine_with(&mock, Backend::Hyprland, FakeRunner::default()).await;

        engine.perform(&DeckCommand::CreateWorktree).await;

        assert_eq!(mock.call_count("worktree.create").await, 1);
        assert!(mock.agent_focus_targets().await.is_empty());
        assert!(mock.workspace_focus_ids().await.is_empty());
        assert!(mock.tab_focus_ids().await.is_empty());
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
        assert!(report.herdr_acted);
        assert!(report.fully_succeeded());
        assert_eq!(mock.tab_focus_ids().await, vec!["w1:t2".to_string()]);
        assert_eq!(runner.ran().len(), 1, "the window must still be raised");
    }
}
