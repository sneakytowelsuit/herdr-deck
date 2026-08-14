//! `herdr-deck` — the command-line companion to the daemon.
//!
//! Diagnostics, a look at what the deck currently shows, and the installer that adapts to
//! whatever hardware is plugged in.

mod doctor;
mod hardware;
mod icons;
mod service;

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use herdr_deck_core::capabilities::DeckModel;
use herdr_deck_core::layout::{KeyBinding, Profile};
use herdr_deck_core::Config;
use herdr_deck_herdr::HerdrClient;

#[derive(Debug, Parser)]
#[command(name = "herdr-deck", about = "Stream Deck control for herdr", version)]
struct Args {
    /// Config file. Defaults to ~/.config/herdr-deck/config.toml.
    #[arg(long, global = true, value_name = "PATH")]
    config: Option<PathBuf>,

    /// herdr session name, equivalent to `herdr --session <name>`.
    #[arg(long, global = true, value_name = "NAME")]
    session: Option<String>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Check everything and explain anything that is wrong.
    Doctor,
    /// Show what herdr currently reports, in attention order.
    Status {
        /// Print JSON instead of a table.
        #[arg(long)]
        json: bool,
    },
    /// Show the layout that would be used for a given deck.
    Layout {
        /// Hardware model: plus, original, mini, xl, neo, pedal.
        #[arg(long, default_value = "plus")]
        model: String,
    },
    /// Write a starter config, and check the deck can actually be reached.
    Install {
        /// Overwrite an existing config.
        #[arg(long)]
        force: bool,
        /// Install the udev rule to /etc and reload udev. Linux only; needs root.
        #[arg(long)]
        write_udev: bool,
    },
    /// Regenerate the Stream Deck plugin's icon set from the theme.
    ///
    /// A build-time tool: the icons are rendered from SVG by the same renderer that draws the
    /// deck, so they stay in step with the palette.
    Icons {
        /// Output directory, normally plugin/com.herdrdeck.sdPlugin/imgs.
        #[arg(long, value_name = "DIR")]
        out: PathBuf,
    },
    /// Manage the background service.
    Service {
        #[command(subcommand)]
        action: service::Action,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let config_path = args
        .config
        .clone()
        .or_else(Config::default_path)
        .unwrap_or_else(|| PathBuf::from("herdr-deck.toml"));
    // Doctor has to run even when the config is broken — diagnosing that is its job.
    let config = Config::load(&config_path).unwrap_or_default();

    match args.command {
        Command::Doctor => {
            let report = doctor::run(&config_path, &config, args.session.as_deref()).await;
            print!("{}", report.render());
            std::process::exit(report.exit_code());
        }
        Command::Status { json } => status(&config, args.session.as_deref(), json).await,
        Command::Layout { model } => layout(&model),
        Command::Install { force, write_udev } => {
            install(&config_path, force, write_udev).map(|_| ())
        }
        Command::Icons { out } => {
            let written = icons::generate(&out)?;
            println!("wrote {} icons to {}", written.len(), out.display());
            Ok(())
        }
        Command::Service { action } => service::run(action),
    }
}

async fn status(config: &Config, session: Option<&str>, json: bool) -> anyhow::Result<()> {
    let client = HerdrClient::from_env(session.or(config.herdr_session.as_deref()));
    let snapshot = client.session_snapshot().await?;
    let state = herdr_deck_core::state::DeckState::from_snapshot(snapshot);

    if json {
        let agents: Vec<_> = state
            .attention_order()
            .iter()
            .map(|a| {
                serde_json::json!({
                    "terminal_id": a.terminal_id,
                    "pane_id": a.pane_id,
                    "workspace_id": a.workspace_id,
                    "status": a.agent_status.as_str(),
                    "label": a.label(),
                    "focused": a.focused,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&agents)?);
        return Ok(());
    }

    let counts = state.status_counts();
    println!(
        "{} agent(s): {} blocked, {} done, {} working, {} idle, {} unknown\n",
        counts.total(),
        counts.blocked,
        counts.done,
        counts.working,
        counts.idle,
        counts.unknown
    );
    // Attention order, so the top line is always what to look at first.
    for agent in state.attention_order() {
        println!(
            "  {:<8} {:<24} {:<10} {}",
            agent.agent_status.as_str(),
            agent.label(),
            agent.workspace_id,
            agent.pane_id
        );
    }
    if counts.needing_attention() == 0 {
        println!("\nNothing is waiting on you.");
    }
    Ok(())
}

fn layout(model: &str) -> anyhow::Result<()> {
    let model = parse_model(model)?;
    let capabilities = model.capabilities();
    let profile = Profile::for_capabilities(&capabilities);

    println!("{}\n", capabilities.describe());
    for (index, binding) in profile.keys.iter().enumerate() {
        println!("  key {index:>2}  {}", describe_key(binding));
    }
    for (dial, binding) in profile.dials.iter().enumerate() {
        let text = match binding {
            herdr_deck_core::layout::DialBinding::Scrub { target } => {
                format!("scrub {} (press to focus)", target.label())
            }
            herdr_deck_core::layout::DialBinding::Unused => "unused".to_string(),
        };
        println!("  dial {dial}   {text}");
    }
    Ok(())
}

fn describe_key(binding: &KeyBinding) -> String {
    match binding {
        KeyBinding::Dynamic { rank } => format!("agent #{rank} in attention order"),
        KeyBinding::PinnedAgent { terminal_id } => format!("pinned agent {terminal_id}"),
        KeyBinding::PinnedWorkspace { workspace_id } => format!("pinned workspace {workspace_id}"),
        // Naming the gesture matters more than naming the command: a key that only works when
        // held is the one thing about a layout you cannot discover by looking at the deck.
        KeyBinding::Command { command } if command.is_destructive() => {
            format!("{} {} — hold to confirm", command.label(), command.target())
        }
        KeyBinding::Command { command } => format!("{} {}", command.label(), command.target()),
        KeyBinding::NextAttention => "jump to the agent that needs you most".to_string(),
        KeyBinding::ModeToggle => "toggle agents / workspaces".to_string(),
        KeyBinding::PagePrev => "previous page".to_string(),
        KeyBinding::PageNext => "next page".to_string(),
        KeyBinding::Scrub { target, delta } => format!(
            "{} {}",
            if *delta < 0 { "previous" } else { "next" },
            target.label()
        ),
        KeyBinding::Empty => "—".to_string(),
    }
}

/// What `install` actually did.
///
/// Returned rather than kept internal purely so the tests can assert on it: the interesting
/// property of `install` is *which steps ran*, and asserting on side effects alone gives you a
/// test that passes just as happily when a step is skipped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct InstallOutcome {
    config_written: bool,
    udev_attempted: bool,
    hardware_checked: bool,
}

fn install(config_path: &PathBuf, force: bool, write_udev: bool) -> anyhow::Result<InstallOutcome> {
    let config_written = write_starter_config(config_path, force)?;

    // The rule goes in before we look: installing it is exactly what un-hides a deck that
    // enumeration could not see, so detecting first would report a state we are about to fix.
    if write_udev {
        let outcome = hardware::install_udev_rule()?;
        println!("\n{}", outcome.message());
        if outcome.needs_manual_commands() {
            report_udev_rule();
        }
    }

    // Detection runs whether or not the config was written: an existing config is no reason to
    // skip the one part of `install` that can tell you the deck is unreachable.
    println!();
    print!("{}", hardware::detect().report());

    if !write_udev {
        report_udev_rule();
    }
    Ok(InstallOutcome {
        config_written,
        udev_attempted: write_udev,
        hardware_checked: true,
    })
}

/// Returns whether a config was actually written.
fn write_starter_config(config_path: &PathBuf, force: bool) -> anyhow::Result<bool> {
    if config_path.exists() && !force {
        println!(
            "{} already exists; leaving it alone (pass --force to overwrite).",
            config_path.display()
        );
        return Ok(false);
    }
    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let config = Config::default();
    let text = toml::to_string_pretty(&config)?;
    std::fs::write(config_path, &text)?;
    println!("wrote {}", config_path.display());
    println!(
        "\nherdr-deck adapts to whatever deck is plugged in, so there is nothing to configure \
         for your hardware. Run `herdr-deck doctor` to check the rest."
    );
    Ok(true)
}

/// On Linux, reaching the deck without root needs a udev rule.
///
/// Printed rather than written unless `--write-udev` is passed: the file lives under /etc, and
/// an install that quietly writes there because it happened to be run under sudo would be a
/// surprise. Showing the exact two commands is faster than sending someone to the docs.
#[cfg(target_os = "linux")]
fn report_udev_rule() {
    use std::path::Path;

    if Path::new(herdr_deck_hid::UDEV_RULE_PATH).exists() {
        println!(
            "\nudev rule already installed at {}.",
            herdr_deck_hid::UDEV_RULE_PATH
        );
        return;
    }
    println!(
        "\nOne more step: the deck needs a udev rule to be reachable without root.\n\n\
         sudo tee {path} >/dev/null <<'EOF'\n{rule}EOF\n\
         sudo udevadm control --reload-rules && sudo udevadm trigger\n\n\
         Then unplug and replug the deck.",
        path = herdr_deck_hid::UDEV_RULE_PATH,
        rule = herdr_deck_hid::UDEV_RULE,
    );
}

/// macOS drives the deck through Elgato's application, so there is no device permission to set.
#[cfg(not(target_os = "linux"))]
fn report_udev_rule() {}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_key_binding_has_a_human_description() {
        // `herdr-deck layout` is how people learn what their deck does; a binding rendered as
        // debug output would be a poor answer.
        let bindings = [
            KeyBinding::Dynamic { rank: 0 },
            KeyBinding::PinnedAgent {
                terminal_id: "t".into(),
            },
            KeyBinding::PinnedWorkspace {
                workspace_id: "w".into(),
            },
            KeyBinding::NextAttention,
            KeyBinding::ModeToggle,
            KeyBinding::PagePrev,
            KeyBinding::PageNext,
            KeyBinding::Scrub {
                target: herdr_deck_core::layout::ScrubTarget::Agents,
                delta: 1,
            },
            KeyBinding::Empty,
        ];
        for binding in bindings {
            let text = describe_key(&binding);
            assert!(!text.is_empty());
            assert!(!text.contains('{'), "{text} looks like debug output");
        }
    }

    #[test]
    fn layout_renders_for_every_supported_model() {
        for model in ["plus", "original", "mini", "xl", "neo", "pedal"] {
            layout(model).unwrap_or_else(|e| panic!("layout failed for {model}: {e}"));
        }
    }

    #[test]
    fn install_writes_a_config_that_loads_back() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        write_starter_config(&path, false).unwrap();
        assert!(path.exists());
        let loaded = Config::load(&path).unwrap();
        assert_eq!(loaded, Config::default());
    }

    #[test]
    fn install_does_not_clobber_an_existing_config_without_force() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "# mine\nreconcile_interval_ms = 4000\n").unwrap();
        write_starter_config(&path, false).unwrap();
        assert!(std::fs::read_to_string(&path).unwrap().contains("# mine"));

        write_starter_config(&path, true).unwrap();
        assert!(!std::fs::read_to_string(&path).unwrap().contains("# mine"));
    }

    #[test]
    fn install_still_checks_the_hardware_when_the_config_is_left_alone() {
        // An existing config used to end the command early, which meant the person most likely
        // to be debugging a dead deck — someone re-running `install` — was the one person who
        // never got told whether their deck could be seen. Asserting only that the file was
        // left alone would pass under that old behaviour too, so assert the step ran.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "# mine\n").unwrap();

        let outcome = install(&path, false, false).unwrap();
        assert!(
            outcome.hardware_checked,
            "re-running install must still report whether the deck is visible"
        );
        assert!(
            !outcome.config_written,
            "an existing config must be left alone"
        );
        assert!(std::fs::read_to_string(&path).unwrap().contains("# mine"));
    }

    #[test]
    fn install_writes_the_config_when_there_is_none_and_still_checks_hardware() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let outcome = install(&path, false, false).unwrap();
        assert!(outcome.config_written);
        assert!(outcome.hardware_checked);
        assert!(!outcome.udev_attempted, "writing to /etc must stay opt-in");
    }
}
