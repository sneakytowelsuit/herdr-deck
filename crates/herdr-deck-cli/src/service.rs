//! Running `herdr-deckd` in the background.
//!
//! # Why a service manager and not a herdr startup hook
//!
//! herdr's `[[startup]]` hooks are one-shot initialisation commands, not supervised daemons —
//! they are expected to do their work and exit. So the herdr plugin action does not *become*
//! the daemon; it asks the platform's service manager to run it. That also means the deck keeps
//! working across a herdr restart, which is what you want from a status display.
//!
//! `launchd` on macOS, `systemd --user` on Linux.

use std::path::PathBuf;
use std::process::Command;

use clap::Subcommand;

const SERVICE_LABEL: &str = "dev.herdr.herdr-deck";

#[derive(Debug, Clone, Copy, Subcommand)]
pub enum Action {
    /// Write the service unit and enable it.
    Install,
    /// Remove the service unit.
    Uninstall,
    Start,
    Stop,
    Restart,
    /// Show whether the service is running.
    Status,
    /// Print the unit file that would be written, without touching anything.
    Show,
}

/// Which service manager to drive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Manager {
    Launchd,
    Systemd,
    /// Neither is available; the user runs `herdr-deckd` themselves.
    None,
}

impl Manager {
    pub fn detect() -> Self {
        if cfg!(target_os = "macos") {
            Manager::Launchd
        } else if cfg!(target_os = "linux") {
            Manager::Systemd
        } else {
            Manager::None
        }
    }
}

pub fn run(action: Action) -> anyhow::Result<()> {
    let manager = Manager::detect();
    let daemon = daemon_binary();

    match manager {
        Manager::None => {
            anyhow::bail!(
                "no supported service manager on this platform — run `herdr-deckd` directly"
            )
        }
        Manager::Launchd => launchd(action, &daemon),
        Manager::Systemd => systemd(action, &daemon),
    }
}

/// Where `herdr-deckd` lives.
///
/// Next to this binary first: that is how both arrive from the same release archive, and it
/// keeps a locally-built pair from accidentally using a system-installed daemon.
pub fn daemon_binary() -> PathBuf {
    if let Ok(explicit) = std::env::var("HERDR_DECKD_PATH") {
        if !explicit.is_empty() {
            return PathBuf::from(explicit);
        }
    }
    if let Ok(current) = std::env::current_exe() {
        if let Some(dir) = current.parent() {
            let sibling = dir.join("herdr-deckd");
            if sibling.is_file() {
                return sibling;
            }
        }
    }
    PathBuf::from("herdr-deckd")
}

// ----- launchd ---------------------------------------------------------------------------

fn launchd_plist_path() -> anyhow::Result<PathBuf> {
    let base = directories::BaseDirs::new()
        .ok_or_else(|| anyhow::anyhow!("could not locate the home directory"))?;
    Ok(base
        .home_dir()
        .join("Library")
        .join("LaunchAgents")
        .join(format!("{SERVICE_LABEL}.plist")))
}

/// The launchd agent definition.
///
/// `KeepAlive` restarts the daemon if it dies; `RunAtLoad` starts it at login. Logs go to the
/// user's Logs directory so `doctor` can point at a real file.
pub fn launchd_plist(daemon: &std::path::Path) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>{SERVICE_LABEL}</string>
  <key>ProgramArguments</key>
  <array>
    <string>{daemon}</string>
  </array>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <true/>
  <key>StandardOutPath</key>
  <string>{home}/Library/Logs/herdr-deck.log</string>
  <key>StandardErrorPath</key>
  <string>{home}/Library/Logs/herdr-deck.log</string>
</dict>
</plist>
"#,
        daemon = daemon.display(),
        home = directories::BaseDirs::new()
            .map(|b| b.home_dir().display().to_string())
            .unwrap_or_else(|| "/tmp".into()),
    )
}

fn launchd(action: Action, daemon: &std::path::Path) -> anyhow::Result<()> {
    let path = launchd_plist_path()?;
    match action {
        Action::Show => {
            print!("{}", launchd_plist(daemon));
            Ok(())
        }
        Action::Install => {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&path, launchd_plist(daemon))?;
            println!("wrote {}", path.display());
            launchctl(&["load", "-w", &path.display().to_string()])
        }
        Action::Uninstall => {
            let _ = launchctl(&["unload", "-w", &path.display().to_string()]);
            if path.exists() {
                std::fs::remove_file(&path)?;
                println!("removed {}", path.display());
            }
            Ok(())
        }
        Action::Start => launchctl(&["start", SERVICE_LABEL]),
        Action::Stop => launchctl(&["stop", SERVICE_LABEL]),
        Action::Restart => {
            let _ = launchctl(&["stop", SERVICE_LABEL]);
            launchctl(&["start", SERVICE_LABEL])
        }
        Action::Status => launchctl(&["list", SERVICE_LABEL]),
    }
}

fn launchctl(args: &[&str]) -> anyhow::Result<()> {
    run_tool("launchctl", args)
}

// ----- systemd ---------------------------------------------------------------------------

fn systemd_unit_path() -> anyhow::Result<PathBuf> {
    let base = directories::BaseDirs::new()
        .ok_or_else(|| anyhow::anyhow!("could not locate the config directory"))?;
    Ok(base
        .config_dir()
        .join("systemd")
        .join("user")
        .join("herdr-deck.service"))
}

/// The systemd user unit.
///
/// Deliberately **not** ordered after herdr: the daemon handles herdr being absent by showing an
/// offline deck and reconnecting, so tying its lifecycle to herdr's would only add failure modes.
pub fn systemd_unit(daemon: &std::path::Path) -> String {
    format!(
        r#"[Unit]
Description=herdr-deck daemon (Stream Deck control for herdr)
Documentation=https://sneakytowelsuit.github.io/herdr-deck/
After=graphical-session.target
PartOf=graphical-session.target

[Service]
Type=simple
ExecStart={daemon}
Restart=always
RestartSec=2
# The daemon needs the session's environment to detect the compositor and raise windows.
PassEnvironment=WAYLAND_DISPLAY DISPLAY XDG_SESSION_TYPE XDG_CURRENT_DESKTOP HYPRLAND_INSTANCE_SIGNATURE SWAYSOCK

[Install]
WantedBy=graphical-session.target
"#,
        daemon = daemon.display()
    )
}

fn systemd(action: Action, daemon: &std::path::Path) -> anyhow::Result<()> {
    let path = systemd_unit_path()?;
    match action {
        Action::Show => {
            print!("{}", systemd_unit(daemon));
            Ok(())
        }
        Action::Install => {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&path, systemd_unit(daemon))?;
            println!("wrote {}", path.display());
            systemctl(&["daemon-reload"])?;
            systemctl(&["enable", "--now", "herdr-deck.service"])
        }
        Action::Uninstall => {
            let _ = systemctl(&["disable", "--now", "herdr-deck.service"]);
            if path.exists() {
                std::fs::remove_file(&path)?;
                println!("removed {}", path.display());
            }
            systemctl(&["daemon-reload"])
        }
        Action::Start => systemctl(&["start", "herdr-deck.service"]),
        Action::Stop => systemctl(&["stop", "herdr-deck.service"]),
        Action::Restart => systemctl(&["restart", "herdr-deck.service"]),
        Action::Status => systemctl(&["status", "--no-pager", "herdr-deck.service"]),
    }
}

fn systemctl(args: &[&str]) -> anyhow::Result<()> {
    let mut full = vec!["--user"];
    full.extend_from_slice(args);
    run_tool("systemctl", &full)
}

fn run_tool(program: &str, args: &[&str]) -> anyhow::Result<()> {
    let status = Command::new(program).args(args).status().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            anyhow::anyhow!("`{program}` is not installed — run `herdr-deckd` directly instead")
        } else {
            anyhow::anyhow!("could not run `{program}`: {e}")
        }
    })?;
    if !status.success() {
        anyhow::bail!("`{program} {}` failed", args.join(" "));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn the_launchd_plist_names_the_daemon_and_restarts_it() {
        let plist = launchd_plist(Path::new("/usr/local/bin/herdr-deckd"));
        assert!(plist.contains("/usr/local/bin/herdr-deckd"));
        assert!(plist.contains(SERVICE_LABEL));
        assert!(plist.contains("<key>KeepAlive</key>"));
        assert!(plist.contains("<key>RunAtLoad</key>"));
    }

    #[test]
    fn the_launchd_plist_is_well_formed_xml() {
        let plist = launchd_plist(Path::new("/usr/local/bin/herdr-deckd"));
        assert!(plist.starts_with("<?xml"));
        assert_eq!(plist.matches("<dict>").count(), plist.matches("</dict>").count());
        assert_eq!(plist.matches("<array>").count(), plist.matches("</array>").count());
        assert!(plist.trim_end().ends_with("</plist>"));
    }

    #[test]
    fn the_systemd_unit_restarts_and_passes_through_the_session_environment() {
        // Without these variables the daemon cannot detect the compositor, and window raising
        // silently degrades to "unsupported" for every user running under systemd.
        let unit = systemd_unit(Path::new("/usr/local/bin/herdr-deckd"));
        assert!(unit.contains("ExecStart=/usr/local/bin/herdr-deckd"));
        assert!(unit.contains("Restart=always"));
        for var in [
            "WAYLAND_DISPLAY",
            "DISPLAY",
            "XDG_SESSION_TYPE",
            "XDG_CURRENT_DESKTOP",
            "HYPRLAND_INSTANCE_SIGNATURE",
            "SWAYSOCK",
        ] {
            assert!(unit.contains(var), "unit should pass through {var}");
        }
    }

    #[test]
    fn the_systemd_unit_does_not_depend_on_herdr() {
        // The daemon shows an offline deck and reconnects on its own; coupling the two would
        // only add ways for the deck to be dead when herdr is fine.
        let unit = systemd_unit(Path::new("/x"));
        assert!(!unit.to_lowercase().contains("requires=herdr"));
        assert!(!unit.to_lowercase().contains("after=herdr"));
    }

    #[test]
    fn the_daemon_path_can_be_overridden_for_unusual_installs() {
        std::env::set_var("HERDR_DECKD_PATH", "/opt/herdr/herdr-deckd");
        assert_eq!(daemon_binary(), PathBuf::from("/opt/herdr/herdr-deckd"));
        std::env::remove_var("HERDR_DECKD_PATH");
    }

    #[test]
    fn a_missing_service_manager_says_how_to_run_the_daemon_anyway() {
        let err = run_tool("herdr-deck-nonexistent-tool", &[]).unwrap_err();
        assert!(err.to_string().contains("herdr-deckd` directly"));
    }
}
