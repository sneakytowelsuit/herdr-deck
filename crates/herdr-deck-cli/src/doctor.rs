//! `herdr-deck doctor` — one command that explains why the deck is not doing what you expect.
//!
//! Every failure mode this project has is diagnosable from here: herdr not running, a protocol
//! the daemon does not know, the daemon not started, a window-raise backend that cannot work in
//! this session, a missing `wmctrl`. Each check reports what it found *and* what to do about it,
//! because a diagnostic that only says "failed" just moves the problem.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use herdr_deck_core::config::Config;
use herdr_deck_focus::{detect_or_override, Backend, FocusEnv};
use herdr_deck_herdr::HerdrClient;

/// How a single check turned out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Ok,
    /// Works, but not fully — the user should know.
    Warn,
    /// Broken; the deck will not do what they asked.
    Fail,
}

impl Level {
    fn marker(self) -> &'static str {
        match self {
            Level::Ok => "ok  ",
            Level::Warn => "warn",
            Level::Fail => "FAIL",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Check {
    pub name: String,
    pub level: Level,
    pub detail: String,
    /// What to do about it. Only set when there is something to do.
    pub remedy: Option<String>,
}

impl Check {
    fn ok(name: &str, detail: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            level: Level::Ok,
            detail: detail.into(),
            remedy: None,
        }
    }

    fn warn(name: &str, detail: impl Into<String>, remedy: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            level: Level::Warn,
            detail: detail.into(),
            remedy: Some(remedy.into()),
        }
    }

    fn fail(name: &str, detail: impl Into<String>, remedy: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            level: Level::Fail,
            detail: detail.into(),
            remedy: Some(remedy.into()),
        }
    }
}

/// The full report.
#[derive(Debug, Clone, Default)]
pub struct Report {
    pub checks: Vec<Check>,
}

impl Report {
    pub fn worst(&self) -> Level {
        if self.checks.iter().any(|c| c.level == Level::Fail) {
            Level::Fail
        } else if self.checks.iter().any(|c| c.level == Level::Warn) {
            Level::Warn
        } else {
            Level::Ok
        }
    }

    /// Non-zero when something is actually broken. A warning is not a failure — a working deck
    /// on GNOME Wayland should not make a scripted health check fail.
    pub fn exit_code(&self) -> i32 {
        match self.worst() {
            Level::Fail => 1,
            _ => 0,
        }
    }

    pub fn render(&self) -> String {
        let mut out = String::new();
        for check in &self.checks {
            let _ = writeln!(
                out,
                "[{}] {}: {}",
                check.level.marker(),
                check.name,
                check.detail
            );
            if let Some(remedy) = &check.remedy {
                for line in wrap(remedy, 76) {
                    let _ = writeln!(out, "         → {line}");
                }
            }
        }
        let summary = match self.worst() {
            Level::Ok => "Everything looks good.",
            Level::Warn => "Working, with limitations noted above.",
            Level::Fail => "Something is broken — see the FAIL lines above.",
        };
        let _ = writeln!(out, "\n{summary}");
        out
    }
}

/// Run every check.
pub async fn run(config_path: &Path, config: &Config, session: Option<&str>) -> Report {
    let mut report = Report::default();
    report.checks.push(check_config(config_path));

    let client = HerdrClient::from_env(session.or(config.herdr_session.as_deref()));
    report.checks.push(check_herdr_socket(&client));
    if let Some(check) = check_herdr_protocol(&client).await {
        report.checks.push(check);
    }
    report
        .checks
        .push(check_daemon_socket(&daemon_socket_path()).await);
    report.checks.extend(check_focus(config));
    report
}

fn check_config(path: &Path) -> Check {
    if !path.exists() {
        return Check::ok(
            "config",
            format!("no file at {} — using defaults", path.display()),
        );
    }
    match Config::load(path) {
        Ok(_) => Check::ok("config", format!("loaded {}", path.display())),
        Err(e) => Check::fail(
            "config",
            format!("{} is invalid: {e}", path.display()),
            "Fix or delete the file; herdr-deck works with no config at all.",
        ),
    }
}

fn check_herdr_socket(client: &HerdrClient) -> Check {
    let socket = client.socket();
    if socket.exists() {
        Check::ok(
            "herdr socket",
            format!("{} (via {})", socket.display(), socket.origin.describe()),
        )
    } else {
        Check::fail(
            "herdr socket",
            format!("nothing at {}", socket.display()),
            "Start herdr, or point herdr-deck at the right session with \
             `--session <name>` or HERDR_SOCKET_PATH.",
        )
    }
}

async fn check_herdr_protocol(client: &HerdrClient) -> Option<Check> {
    if !client.socket().exists() {
        return None;
    }
    match client.session_snapshot().await {
        Ok(snapshot) => {
            let expected = herdr_deck_herdr::EXPECTED_PROTOCOL;
            let agents = snapshot.agents.len();
            match snapshot.protocol {
                Some(protocol) if protocol == expected => Some(Check::ok(
                    "herdr protocol",
                    format!("protocol {protocol}, {agents} agent(s) visible"),
                )),
                Some(protocol) => Some(Check::warn(
                    "herdr protocol",
                    format!("herdr speaks protocol {protocol}, herdr-deck expects {expected}"),
                    "Usually harmless — the client tolerates added fields. If tiles look wrong, \
                     update herdr-deck.",
                )),
                None => Some(Check::warn(
                    "herdr protocol",
                    "herdr did not report a protocol version",
                    "Check `herdr --version`; very old builds may not be compatible.",
                )),
            }
        }
        Err(e) => Some(Check::fail(
            "herdr protocol",
            format!("could not read session.snapshot: {e}"),
            "Confirm herdr is healthy with `herdr status`.",
        )),
    }
}

async fn check_daemon_socket(path: &Path) -> Check {
    if !path.exists() {
        return Check::warn(
            "herdr-deckd",
            format!("not running (no socket at {})", path.display()),
            "Start it with `herdr-deck service start`, or run `herdr-deckd` in a terminal to \
             watch its logs.",
        );
    }
    match tokio::net::UnixStream::connect(path).await {
        Ok(_) => Check::ok("herdr-deckd", format!("running, socket {}", path.display())),
        Err(e) => Check::fail(
            "herdr-deckd",
            format!("stale socket at {} ({e})", path.display()),
            "Remove the file and restart the daemon.",
        ),
    }
}

fn check_focus(config: &Config) -> Vec<Check> {
    let env = FocusEnv::from_process();
    let backend = detect_or_override(&env, config.focus.backend.as_deref());
    let mut checks = Vec::new();

    if !config.focus.raise_window {
        checks.push(Check::warn(
            "window raising",
            "disabled in config",
            "Set `focus.raise_window = true` if you want the terminal brought forward.",
        ));
        return checks;
    }

    match backend.unsupported_reason() {
        Some(reason) => checks.push(Check::warn("window raising", "no usable backend", reason)),
        None => {
            checks.push(Check::ok(
                "window raising",
                format!("backend `{}`", backend.name()),
            ));
            checks.push(check_backend_tools(backend));
        }
    }

    let app_id = if matches!(backend, Backend::MacOs) {
        &config.focus.macos_bundle_id
    } else {
        &config.focus.linux_app_id
    };
    checks.push(Check::ok("terminal", format!("targeting `{app_id}`")));
    checks
}

/// Are the backend's helper programs actually installed?
fn check_backend_tools(backend: Backend) -> Check {
    let required = backend.required_tools();
    let missing: Vec<_> = required
        .iter()
        .filter(|tool| which(tool).is_none())
        .copied()
        .collect();

    if missing.is_empty() {
        Check::ok("window tools", format!("found {}", required.join(", ")))
    } else if missing.len() == required.len() {
        Check::warn(
            "window tools",
            format!("none of {} are installed", required.join(", ")),
            format!(
                "Install one of: {}. herdr focus still works without them; only the window \
                 raise will not.",
                required.join(", ")
            ),
        )
    } else {
        // Backends list alternatives, so a partial set is fine.
        Check::ok(
            "window tools",
            format!(
                "found some of {} (missing: {})",
                required.join(", "),
                missing.join(", ")
            ),
        )
    }
}

/// Minimal `which`, to avoid a dependency for one lookup.
fn which(program: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(program))
        .find(|candidate| candidate.is_file())
}

/// Where the daemon's frontend socket lives. Mirrors `herdr-deckd`.
pub fn daemon_socket_path() -> PathBuf {
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

fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        if !current.is_empty() && current.len() + 1 + word.len() > width {
            lines.push(std::mem::take(&mut current));
        }
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(word);
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_report_with_only_warnings_exits_zero() {
        // GNOME Wayland users have a working deck; a health check must not call it broken.
        let report = Report {
            checks: vec![
                Check::ok("a", "fine"),
                Check::warn("b", "limited", "do this"),
            ],
        };
        assert_eq!(report.worst(), Level::Warn);
        assert_eq!(report.exit_code(), 0);
    }

    #[test]
    fn a_report_with_a_failure_exits_non_zero() {
        let report = Report {
            checks: vec![Check::ok("a", "fine"), Check::fail("b", "broken", "fix it")],
        };
        assert_eq!(report.exit_code(), 1);
    }

    #[test]
    fn an_all_clear_report_exits_zero() {
        let report = Report {
            checks: vec![Check::ok("a", "fine")],
        };
        assert_eq!(report.worst(), Level::Ok);
        assert_eq!(report.exit_code(), 0);
    }

    #[test]
    fn every_problem_comes_with_something_to_do_about_it() {
        // A diagnostic that only says "failed" just moves the problem.
        let report = Report {
            checks: vec![
                Check::warn("b", "limited", "do this"),
                Check::fail("c", "broken", "fix it"),
            ],
        };
        for check in &report.checks {
            assert!(check.remedy.is_some(), "{} has no remedy", check.name);
        }
    }

    #[test]
    fn a_missing_config_is_fine_because_config_is_optional() {
        let check = check_config(Path::new("/nonexistent/herdr-deck/config.toml"));
        assert_eq!(check.level, Level::Ok);
        assert!(check.detail.contains("defaults"));
    }

    #[test]
    fn a_malformed_config_fails_loudly_and_says_how_to_recover() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[focus]\nraise_windows = true\n").unwrap();
        let check = check_config(&path);
        assert_eq!(check.level, Level::Fail);
        assert!(check.remedy.unwrap().contains("delete"));
    }

    #[test]
    fn a_missing_herdr_socket_says_herdr_is_not_running() {
        let client = HerdrClient::new(herdr_deck_herdr::SocketPath {
            path: "/nonexistent/herdr.sock".into(),
            origin: herdr_deck_herdr::socket::SocketOrigin::Default,
        });
        let check = check_herdr_socket(&client);
        assert_eq!(check.level, Level::Fail);
        assert!(check.remedy.unwrap().contains("Start herdr"));
    }

    #[tokio::test]
    async fn a_live_herdr_reports_its_protocol_and_agent_count() {
        use herdr_deck_herdr::mock::MockHerdr;
        use serde_json::json;

        let mock = MockHerdr::start().await;
        mock.reply(
            "session.snapshot",
            json!({"protocol": 19, "agents": [
                {"terminal_id":"t","agent_status":"idle","workspace_id":"w1",
                 "tab_id":"w1:t1","pane_id":"w1:p1","focused":false,"revision":1}
            ]}),
        )
        .await;
        let client = HerdrClient::new(mock.socket_path());
        let check = check_herdr_protocol(&client).await.unwrap();
        assert_eq!(check.level, Level::Ok);
        assert!(check.detail.contains("1 agent"));
    }

    #[tokio::test]
    async fn a_protocol_mismatch_warns_rather_than_failing() {
        use herdr_deck_herdr::mock::MockHerdr;
        use serde_json::json;

        let mock = MockHerdr::start().await;
        mock.reply("session.snapshot", json!({"protocol": 99, "agents": []}))
            .await;
        let client = HerdrClient::new(mock.socket_path());
        let check = check_herdr_protocol(&client).await.unwrap();
        assert_eq!(
            check.level,
            Level::Warn,
            "additive changes are usually harmless"
        );
    }

    #[tokio::test]
    async fn a_daemon_that_is_not_running_warns_and_says_how_to_start_it() {
        let check = check_daemon_socket(Path::new("/nonexistent/herdr-deck.sock")).await;
        assert_eq!(check.level, Level::Warn);
        assert!(check.remedy.unwrap().contains("service start"));
    }

    #[tokio::test]
    async fn a_stale_daemon_socket_is_a_failure_because_nothing_will_connect() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("herdr-deck.sock");
        std::fs::write(&path, b"not a socket").unwrap();
        let check = check_daemon_socket(&path).await;
        assert_eq!(check.level, Level::Fail);
    }

    #[test]
    fn disabling_window_raising_is_reported_but_not_treated_as_broken() {
        let mut config = Config::default();
        config.focus.raise_window = false;
        let checks = check_focus(&config);
        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].level, Level::Warn);
    }

    #[test]
    fn the_rendered_report_shows_every_check_and_a_summary() {
        let report = Report {
            checks: vec![
                Check::ok("herdr socket", "/run/herdr.sock"),
                Check::fail("herdr-deckd", "not running", "Start it."),
            ],
        };
        let text = report.render();
        assert!(text.contains("herdr socket"));
        assert!(text.contains("FAIL"));
        assert!(text.contains("→ Start it."));
        assert!(text.contains("Something is broken"));
    }

    #[test]
    fn long_remedies_wrap_instead_of_running_off_the_terminal() {
        let long = "word ".repeat(40);
        let lines = wrap(&long, 40);
        assert!(lines.len() > 1);
        for line in lines {
            assert!(line.len() <= 40, "{line:?} is too long");
        }
    }
}
