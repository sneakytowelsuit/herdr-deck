//! Locating herdr's API socket.
//!
//! Resolution order mirrors herdr's own, so that `herdr-deck` talks to the same server the
//! user's `herdr` CLI would talk to:
//!
//! 1. an explicit session name (equivalent to `herdr --session <name>`)
//! 2. `HERDR_SOCKET_PATH`
//! 3. `HERDR_SESSION=<name>`
//! 4. the default session socket
//!
//! Paths:
//! - default: `~/.config/herdr/herdr.sock`
//! - named:   `~/.config/herdr/sessions/<name>/herdr.sock`
//!
//! Note there is also a `herdr-client.sock` alongside these. That is herdr's *internal* client
//! protocol between the TUI and the server — deliberately not used here.

use std::path::{Path, PathBuf};

/// A resolved path to a herdr API socket, plus how we arrived at it (used by `doctor`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SocketPath {
    pub path: PathBuf,
    pub origin: SocketOrigin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SocketOrigin {
    /// An explicit `--session <name>`.
    ExplicitSession,
    /// The `HERDR_SOCKET_PATH` environment variable.
    EnvSocketPath,
    /// The `HERDR_SESSION` environment variable.
    EnvSession,
    /// The default session.
    Default,
}

impl SocketOrigin {
    pub fn describe(self) -> &'static str {
        match self {
            SocketOrigin::ExplicitSession => "explicit --session",
            SocketOrigin::EnvSocketPath => "HERDR_SOCKET_PATH",
            SocketOrigin::EnvSession => "HERDR_SESSION",
            SocketOrigin::Default => "default session",
        }
    }
}

impl SocketPath {
    /// Resolve the socket path using the standard precedence.
    ///
    /// `session` corresponds to an explicit `--session <name>` and wins over everything.
    pub fn resolve(session: Option<&str>) -> Self {
        Self::resolve_with(session, EnvVars::from_process(), &config_dir())
    }

    /// Resolution with injected environment and config dir, so the precedence rules are
    /// testable without mutating process-global state.
    pub fn resolve_with(session: Option<&str>, env: EnvVars, config_dir: &Path) -> Self {
        if let Some(name) = session {
            return Self {
                path: named_session_socket(config_dir, name),
                origin: SocketOrigin::ExplicitSession,
            };
        }
        if let Some(raw) = env.socket_path.filter(|s| !s.is_empty()) {
            return Self {
                path: PathBuf::from(raw),
                origin: SocketOrigin::EnvSocketPath,
            };
        }
        if let Some(name) = env.session.filter(|s| !s.is_empty()) {
            return Self {
                path: named_session_socket(config_dir, &name),
                origin: SocketOrigin::EnvSession,
            };
        }
        Self {
            path: config_dir.join("herdr.sock"),
            origin: SocketOrigin::Default,
        }
    }

    pub fn exists(&self) -> bool {
        self.path.exists()
    }

    pub fn display(&self) -> std::path::Display<'_> {
        self.path.display()
    }
}

/// The subset of the environment that affects socket resolution.
#[derive(Debug, Clone, Default)]
pub struct EnvVars {
    pub socket_path: Option<String>,
    pub session: Option<String>,
}

impl EnvVars {
    pub fn from_process() -> Self {
        Self {
            socket_path: std::env::var("HERDR_SOCKET_PATH").ok(),
            session: std::env::var("HERDR_SESSION").ok(),
        }
    }
}

fn named_session_socket(config_dir: &Path, name: &str) -> PathBuf {
    config_dir.join("sessions").join(name).join("herdr.sock")
}

/// herdr's config directory: `$XDG_CONFIG_HOME/herdr`, else `~/.config/herdr`.
///
/// herdr also honours `HERDR_CONFIG_PATH`, but that points at the config *file* rather than the
/// directory, so it is not used for socket resolution.
fn config_dir() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        if !xdg.is_empty() {
            return PathBuf::from(xdg).join("herdr");
        }
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".config").join("herdr")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> PathBuf {
        PathBuf::from("/home/u/.config/herdr")
    }

    #[test]
    fn explicit_session_wins_over_everything() {
        let env = EnvVars {
            socket_path: Some("/ignored.sock".into()),
            session: Some("ignored".into()),
        };
        let r = SocketPath::resolve_with(Some("work"), env, &cfg());
        assert_eq!(r.path, cfg().join("sessions/work/herdr.sock"));
        assert_eq!(r.origin, SocketOrigin::ExplicitSession);
    }

    #[test]
    fn socket_path_env_wins_over_session_env() {
        let env = EnvVars {
            socket_path: Some("/run/custom.sock".into()),
            session: Some("work".into()),
        };
        let r = SocketPath::resolve_with(None, env, &cfg());
        assert_eq!(r.path, PathBuf::from("/run/custom.sock"));
        assert_eq!(r.origin, SocketOrigin::EnvSocketPath);
    }

    #[test]
    fn session_env_selects_named_socket() {
        let env = EnvVars {
            socket_path: None,
            session: Some("work".into()),
        };
        let r = SocketPath::resolve_with(None, env, &cfg());
        assert_eq!(r.path, cfg().join("sessions/work/herdr.sock"));
        assert_eq!(r.origin, SocketOrigin::EnvSession);
    }

    #[test]
    fn falls_back_to_default_socket() {
        let r = SocketPath::resolve_with(None, EnvVars::default(), &cfg());
        assert_eq!(r.path, cfg().join("herdr.sock"));
        assert_eq!(r.origin, SocketOrigin::Default);
    }

    #[test]
    fn empty_env_vars_are_treated_as_unset() {
        let env = EnvVars {
            socket_path: Some(String::new()),
            session: Some(String::new()),
        };
        let r = SocketPath::resolve_with(None, env, &cfg());
        assert_eq!(r.origin, SocketOrigin::Default);
    }
}
