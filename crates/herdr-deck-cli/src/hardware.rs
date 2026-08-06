//! Looking for the deck, and making it reachable.
//!
//! `install` and `doctor` both have to answer "is there a deck, and can we talk to it?", and on
//! Linux that is two questions wearing the same clothes: a deck with no udev rule enumerates as
//! nothing at all. Reporting "no deck found" as a flat fact would send someone hunting for a
//! loose cable when the real problem is a missing file under `/etc`, so every message here keeps
//! the two apart and says which to check first.
//!
//! On macOS there is nothing for us to enumerate — Elgato's application owns the device and the
//! plugin talks to it through the app. Saying that plainly is more useful than a detector that
//! always finds nothing.

use std::fmt::Write as _;

use herdr_deck_core::capabilities::{DeckCapabilities, DeckModel};

use crate::doctor::Level;

/// Where the udev rule belongs.
///
/// Spelled out rather than taken from `herdr-deck-hid`, which is a Linux-only dependency: these
/// messages have to compile on macOS too, where the path is only ever quoted in prose. A test
/// keeps this copy in step with the constant the driver actually uses.
const UDEV_RULE_PATH: &str = "/etc/udev/rules.d/70-herdr-deck.rules";

/// One deck, as enumeration reported it.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoundDeck {
    /// The name the HID layer gives this hardware.
    pub name: String,
    pub serial: String,
    /// Published geometry for the model, when the name matches one we have an entry for.
    pub capabilities: Option<DeckCapabilities>,
}

impl FoundDeck {
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub fn new(name: impl Into<String>, serial: impl Into<String>) -> Self {
        let name = name.into();
        let capabilities = model_for_hid_name(&name).map(DeckModel::capabilities);
        Self {
            name,
            serial: serial.into(),
            capabilities,
        }
    }

    /// The model, and what it can do.
    pub fn describe(&self) -> String {
        match &self.capabilities {
            Some(capabilities) => {
                format!("{} (serial {})", capabilities.describe(), self.serial)
            }
            // Unknown hardware still works — the daemon lays out from the geometry the device
            // reports — so say what was seen instead of guessing at a key count.
            None => format!(
                "{} (serial {}) — model not in this build's table; the daemon will lay it out \
                 from the geometry the device reports",
                self.name, self.serial
            ),
        }
    }

    fn model_name(&self) -> &str {
        match &self.capabilities {
            Some(capabilities) => &capabilities.model_name,
            None => &self.name,
        }
    }
}

/// Map a HID model name onto a known deck.
///
/// Only ever used to make the installer's output readable. The daemon deliberately does *not*
/// do this — it lays out from the geometry the device reports — so a name missing from this
/// table costs a nicer sentence and nothing else.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn model_for_hid_name(name: &str) -> Option<DeckModel> {
    match name.to_ascii_lowercase().as_str() {
        "original" | "originalv2" | "mk2" | "mk2scissor" | "mk2module" => Some(DeckModel::Original),
        "mini" | "minimk2" | "minidiscord" | "minimk2module" => Some(DeckModel::Mini),
        "xl" | "xlv2" | "xlv2module" => Some(DeckModel::Xl),
        "plus" => Some(DeckModel::Plus),
        "neo" => Some(DeckModel::Neo),
        "pedal" => Some(DeckModel::Pedal),
        _ => None,
    }
}

/// What looking for hardware turned up.
///
/// Every platform builds only a subset of these — Linux never reaches `ElgatoOwnsTheDevice`,
/// macOS never reaches anything else — but the enum stays whole so the prose below is one piece
/// of writing rather than two cfg'd copies that drift apart. Hence the blanket allow.
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Detection {
    /// At least one deck answered.
    Found(Vec<FoundDeck>),
    /// Enumeration worked and turned up nothing — which on Linux is ambiguous.
    NothingFound { udev_rule_installed: bool },
    /// Enumeration itself failed: HID would not open at all.
    Unavailable { reason: String },
    /// This platform does not enumerate. Elgato's app does.
    ElgatoOwnsTheDevice,
}

impl Detection {
    /// One line, for `doctor`.
    pub fn summary(&self) -> String {
        match self {
            Detection::Found(decks) if decks.len() == 1 => decks[0].describe(),
            Detection::Found(decks) => format!(
                "{} decks attached: {}",
                decks.len(),
                decks
                    .iter()
                    .map(|deck| format!("{} (serial {})", deck.model_name(), deck.serial))
                    .collect::<Vec<_>>()
                    .join("; ")
            ),
            // The two halves of the most common failure. Neither is stated as fact, because
            // from here they are genuinely indistinguishable.
            Detection::NothingFound {
                udev_rule_installed: false,
            } => "no deck found — but the udev rule is missing, which alone would hide one"
                .to_string(),
            Detection::NothingFound {
                udev_rule_installed: true,
            } => "no deck found (the udev rule is installed, so probably nothing is plugged in)"
                .to_string(),
            Detection::Unavailable { reason } => {
                format!("could not look for hardware: {reason}")
            }
            Detection::ElgatoOwnsTheDevice => {
                "not enumerated here — on macOS the Stream Deck app owns the device".to_string()
            }
        }
    }

    /// What to do about it, when there is anything to do.
    pub fn remedy(&self) -> Option<String> {
        match self {
            Detection::Found(_) => None,
            Detection::NothingFound {
                udev_rule_installed: false,
            } => Some(format!(
                "Install the rule with `sudo herdr-deck install --write-udev`, then unplug and \
                 replug the deck. If you have no deck attached yet, nothing is wrong — the rule \
                 is still worth installing before you plug one in. The rule goes to \
                 {UDEV_RULE_PATH}."
            )),
            Detection::NothingFound {
                udev_rule_installed: true,
            } => Some(
                "Plug a deck in. If one is already attached, unplug and replug it — a rule only \
                 takes effect on a device that appears after it was loaded — and check `lsusb` \
                 for vendor 0fd9 to see whether the kernel sees the device at all."
                    .to_string(),
            ),
            Detection::Unavailable { .. } => Some(format!(
                "The HID layer would not open. Check that {UDEV_RULE_PATH} exists (`sudo \
                 herdr-deck install --write-udev` writes it) and that this machine has hidraw \
                 available — a container without /dev access cannot reach USB at all."
            )),
            // Nothing is wrong on macOS, but "we cannot see your deck" invites the question of
            // what does, so answer it here rather than making them find the docs.
            Detection::ElgatoOwnsTheDevice => Some(
                "Nothing to do here. Install the Stream Deck plugin and drag `herdr Agent` onto \
                 the keys you want — see the macOS section of the install guide."
                    .to_string(),
            ),
        }
    }

    /// How bad this is for `doctor`.
    ///
    /// An absent deck is a warning, not a failure: plenty of people run `doctor` before the
    /// hardware arrives, or over SSH. HID refusing to open is a failure — the Linux frontend
    /// cannot work at all in that state.
    pub fn level(&self) -> Level {
        match self {
            Detection::Found(_) | Detection::ElgatoOwnsTheDevice => Level::Ok,
            Detection::NothingFound { .. } => Level::Warn,
            Detection::Unavailable { .. } => Level::Fail,
        }
    }

    /// The block `install` prints, which has room for more than one line.
    pub fn report(&self) -> String {
        let mut out = String::new();
        match self {
            Detection::Found(decks) => {
                let _ = writeln!(
                    out,
                    "Found {} deck{}:",
                    decks.len(),
                    if decks.len() == 1 { "" } else { "s" }
                );
                for deck in decks {
                    let _ = writeln!(out, "  {}", deck.describe());
                }
            }
            other => {
                let _ = writeln!(out, "{}", other.summary());
            }
        }
        if let Some(remedy) = self.remedy() {
            let _ = writeln!(out, "\n{remedy}");
        }
        out
    }
}

/// Look for attached hardware.
#[cfg(target_os = "linux")]
pub fn detect() -> Detection {
    use std::path::Path;

    match herdr_deck_hid::list_decks() {
        Ok(decks) if decks.is_empty() => Detection::NothingFound {
            udev_rule_installed: Path::new(herdr_deck_hid::UDEV_RULE_PATH).exists(),
        },
        Ok(decks) => Detection::Found(
            decks
                .into_iter()
                .map(|(name, serial)| FoundDeck::new(name, serial))
                .collect(),
        ),
        Err(e) => Detection::Unavailable {
            reason: format!("{e:#}"),
        },
    }
}

/// macOS reaches the deck through Elgato's application, which does its own enumeration; there is
/// no device for us to open and nothing we could learn by trying.
#[cfg(not(target_os = "linux"))]
pub fn detect() -> Detection {
    Detection::ElgatoOwnsTheDevice
}

/// What `install --write-udev` managed to do.
///
/// `NotApplicable` is the macOS answer and the rest are the Linux ones; as with [`Detection`],
/// the enum is whole on both so the wording stays in one place.
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UdevOutcome {
    /// The rule is in place and udev has been reloaded.
    Installed { replaced_existing: bool },
    /// The file was already exactly this rule, so nothing was touched.
    AlreadyCurrent,
    /// Not root, so we did not try.
    NotRoot,
    /// Written, but `udevadm` did not run — the rule will apply on the next boot.
    ReloadFailed { error: String },
    /// There is no udev on this platform.
    NotApplicable,
}

impl UdevOutcome {
    pub fn message(&self) -> String {
        match self {
            UdevOutcome::Installed {
                replaced_existing: false,
            } => format!(
                "Installed the udev rule at {UDEV_RULE_PATH} and reloaded udev.\nUnplug and \
                 replug the deck so it picks up the new permissions."
            ),
            UdevOutcome::Installed {
                replaced_existing: true,
            } => format!(
                "Replaced the udev rule at {UDEV_RULE_PATH} and reloaded udev.\nUnplug and \
                 replug the deck so it picks up the new permissions."
            ),
            UdevOutcome::AlreadyCurrent => {
                format!("The udev rule at {UDEV_RULE_PATH} is already up to date.")
            }
            // Refusing before touching anything, and naming the command to re-run, beats an
            // "permission denied" thrown from the middle of an install.
            UdevOutcome::NotRoot => format!(
                "Cannot write {UDEV_RULE_PATH} without root, so nothing was changed.\nRe-run as \
                 root:\n\n  sudo herdr-deck install --write-udev\n\nor run the commands below by \
                 hand."
            ),
            UdevOutcome::ReloadFailed { error } => format!(
                "Wrote {UDEV_RULE_PATH}, but could not reload udev: {error}\nRun `sudo udevadm \
                 control --reload-rules && sudo udevadm trigger` yourself, or reboot."
            ),
            UdevOutcome::NotApplicable => {
                "--write-udev does nothing on this platform: there is no udev, and macOS reaches \
                 the deck through Elgato's Stream Deck app rather than through a device node."
                    .to_string()
            }
        }
    }

    /// Should the manual commands be printed as well?
    ///
    /// Only when we could not do the job — then the two commands are the fastest way out.
    pub fn needs_manual_commands(&self) -> bool {
        matches!(self, UdevOutcome::NotRoot)
    }
}

/// Install the udev rule and make udev notice it.
///
/// Returns an outcome rather than an error for the cases that are the user's to fix; a real
/// `Err` here means the filesystem said no for a reason we cannot explain any better.
#[cfg(target_os = "linux")]
pub fn install_udev_rule() -> anyhow::Result<UdevOutcome> {
    use std::path::Path;

    use anyhow::Context as _;

    let path = Path::new(herdr_deck_hid::UDEV_RULE_PATH);
    // Check content before privileges: someone who already has the rule should be told so,
    // not told to go and find sudo.
    if std::fs::read_to_string(path).is_ok_and(|current| current == herdr_deck_hid::UDEV_RULE) {
        return Ok(UdevOutcome::AlreadyCurrent);
    }
    if effective_uid() != Some(0) {
        return Ok(UdevOutcome::NotRoot);
    }

    let replaced_existing = path.exists();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("could not create {}", parent.display()))?;
    }
    std::fs::write(path, herdr_deck_hid::UDEV_RULE)
        .with_context(|| format!("could not write {}", path.display()))?;

    if let Err(e) = reload_udev() {
        return Ok(UdevOutcome::ReloadFailed {
            error: format!("{e:#}"),
        });
    }
    Ok(UdevOutcome::Installed { replaced_existing })
}

/// There is no udev off Linux, and nothing to install.
#[cfg(not(target_os = "linux"))]
pub fn install_udev_rule() -> anyhow::Result<UdevOutcome> {
    Ok(UdevOutcome::NotApplicable)
}

/// Reload, then trigger.
///
/// The trigger matters: reloading alone applies the rule to devices that appear *afterwards*,
/// so a deck that was already plugged in stays unreachable and the install looks like it did
/// nothing.
#[cfg(target_os = "linux")]
fn reload_udev() -> anyhow::Result<()> {
    run_tool("udevadm", &["control", "--reload-rules"])?;
    run_tool("udevadm", &["trigger"])
}

#[cfg(target_os = "linux")]
fn run_tool(program: &str, args: &[&str]) -> anyhow::Result<()> {
    let status = std::process::Command::new(program)
        .args(args)
        .status()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                anyhow::anyhow!("`{program}` is not installed")
            } else {
                anyhow::anyhow!("could not run `{program}`: {e}")
            }
        })?;
    if !status.success() {
        anyhow::bail!("`{program} {}` failed", args.join(" "));
    }
    Ok(())
}

/// Our effective uid, read from `/proc`.
///
/// Rather than `libc::geteuid`, which would put a C dependency and an `unsafe` block in the CLI
/// for the sake of one integer. This path is Linux-only, where `/proc/self/status` is always
/// there.
#[cfg(target_os = "linux")]
fn effective_uid() -> Option<u32> {
    parse_effective_uid(&std::fs::read_to_string("/proc/self/status").ok()?)
}

/// Pull the effective uid out of `/proc/self/status`.
///
/// The line reads `Uid:\treal\teffective\tsaved\tfilesystem`. The *effective* one is what the
/// kernel checks when we write to `/etc`, and it is the one that differs under `sudo`.
#[cfg(target_os = "linux")]
fn parse_effective_uid(status: &str) -> Option<u32> {
    status
        .lines()
        .find_map(|line| line.strip_prefix("Uid:"))
        .and_then(|rest| rest.split_whitespace().nth(1))
        .and_then(|uid| uid.parse().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_recognised_deck_is_reported_with_its_key_and_dial_count() {
        // "It found my deck" is the whole point of detecting; a bare model name would leave the
        // user still wondering whether herdr-deck understood the hardware.
        let deck = FoundDeck::new("Plus", "ABC123");
        let text = deck.describe();
        assert!(text.contains("Stream Deck +"), "{text}");
        assert!(text.contains("4x2 keys"), "{text}");
        assert!(text.contains("4 dials"), "{text}");
        assert!(text.contains("ABC123"), "{text}");
    }

    #[test]
    fn every_hid_name_the_driver_can_report_for_known_hardware_maps_to_a_model() {
        // These are the `Kind` debug names the HID layer hands us. A typo here silently
        // downgrades a perfectly ordinary deck to "unrecognised".
        for (name, expected) in [
            ("Original", DeckModel::Original),
            ("OriginalV2", DeckModel::Original),
            ("Mk2", DeckModel::Original),
            ("Mini", DeckModel::Mini),
            ("MiniMk2", DeckModel::Mini),
            ("Xl", DeckModel::Xl),
            ("XlV2", DeckModel::Xl),
            ("Plus", DeckModel::Plus),
            ("Neo", DeckModel::Neo),
            ("Pedal", DeckModel::Pedal),
        ] {
            assert_eq!(model_for_hid_name(name), Some(expected), "{name}");
        }
    }

    #[test]
    fn hardware_released_after_this_table_is_named_rather_than_guessed_at() {
        // Guessing a key count for unknown hardware would put a wrong number in front of the
        // user; the daemon lays out from reported geometry regardless.
        let deck = FoundDeck::new("PlusXl", "ZZ9");
        assert!(deck.capabilities.is_none());
        let text = deck.describe();
        assert!(text.contains("PlusXl"), "{text}");
        assert!(text.contains("geometry the device reports"), "{text}");
    }

    #[test]
    fn a_missing_deck_with_no_udev_rule_says_the_rule_could_be_the_reason() {
        // The single most common failure: the deck is plugged in and invisible. Reporting "no
        // deck" as fact sends the user looking at the cable instead of at /etc.
        let detection = Detection::NothingFound {
            udev_rule_installed: false,
        };
        let summary = detection.summary();
        assert!(summary.contains("no deck found"), "{summary}");
        assert!(summary.contains("udev rule is missing"), "{summary}");
        let remedy = detection.remedy().unwrap();
        assert!(remedy.contains("--write-udev"), "{remedy}");
        assert!(remedy.contains(UDEV_RULE_PATH), "{remedy}");
    }

    #[test]
    fn a_missing_deck_with_the_rule_installed_points_at_the_cable_instead() {
        let detection = Detection::NothingFound {
            udev_rule_installed: true,
        };
        let summary = detection.summary();
        assert!(
            summary.contains("probably nothing is plugged in"),
            "{summary}"
        );
        let remedy = detection.remedy().unwrap();
        assert!(remedy.contains("replug"), "{remedy}");
        assert!(
            !remedy.contains("--write-udev"),
            "the rule is already there: {remedy}"
        );
    }

    #[test]
    fn a_found_deck_is_not_a_problem_and_carries_no_remedy() {
        let detection = Detection::Found(vec![FoundDeck::new("Plus", "A1")]);
        assert_eq!(detection.level(), Level::Ok);
        assert_eq!(detection.remedy(), None);
    }

    #[test]
    fn hid_refusing_to_open_is_a_failure_because_the_frontend_cannot_work_at_all() {
        let detection = Detection::Unavailable {
            reason: "could not open HID".to_string(),
        };
        assert_eq!(detection.level(), Level::Fail);
        assert!(detection.summary().contains("could not open HID"));
        assert!(detection.remedy().is_some());
    }

    #[test]
    fn an_absent_deck_is_a_warning_rather_than_a_failure() {
        // People run `doctor` over SSH, or before the hardware arrives. Failing there would
        // make a scripted health check call a fine install broken.
        assert_eq!(
            Detection::NothingFound {
                udev_rule_installed: true
            }
            .level(),
            Level::Warn
        );
    }

    #[test]
    fn macos_says_who_owns_the_device_instead_of_pretending_to_look() {
        let detection = Detection::ElgatoOwnsTheDevice;
        assert_eq!(detection.level(), Level::Ok);
        let summary = detection.summary();
        assert!(summary.contains("Stream Deck app"), "{summary}");
        let remedy = detection.remedy().unwrap();
        assert!(remedy.contains("plugin"), "{remedy}");
    }

    #[test]
    fn every_detection_that_is_not_ok_says_what_to_do_about_it() {
        // Same promise `doctor` makes everywhere else: a diagnostic that only says "failed"
        // just moves the problem.
        for detection in [
            Detection::NothingFound {
                udev_rule_installed: false,
            },
            Detection::NothingFound {
                udev_rule_installed: true,
            },
            Detection::Unavailable {
                reason: "boom".to_string(),
            },
        ] {
            assert!(
                detection.remedy().is_some(),
                "{detection:?} has nothing to do about it"
            );
        }
    }

    #[test]
    fn the_install_report_lists_every_attached_deck() {
        let detection = Detection::Found(vec![
            FoundDeck::new("Plus", "A1"),
            FoundDeck::new("Xl", "B2"),
        ]);
        let report = detection.report();
        assert!(report.contains("Found 2 decks:"), "{report}");
        assert!(report.contains("Stream Deck +"), "{report}");
        assert!(report.contains("Stream Deck XL"), "{report}");
    }

    #[test]
    fn the_install_report_repeats_the_remedy_so_it_is_not_only_in_doctor() {
        let report = Detection::NothingFound {
            udev_rule_installed: false,
        }
        .report();
        assert!(report.contains("--write-udev"), "{report}");
    }

    #[test]
    fn a_single_deck_summary_fits_on_one_doctor_line() {
        // `doctor` renders the detail inline after the check name; a newline would break the
        // layout of the whole report.
        for detection in [
            Detection::Found(vec![FoundDeck::new("Plus", "A1")]),
            Detection::Found(vec![
                FoundDeck::new("Plus", "A1"),
                FoundDeck::new("Mini", "B2"),
            ]),
            Detection::NothingFound {
                udev_rule_installed: false,
            },
            Detection::Unavailable {
                reason: "boom".to_string(),
            },
            Detection::ElgatoOwnsTheDevice,
        ] {
            let summary = detection.summary();
            assert!(!summary.contains('\n'), "{summary:?} spans lines");
        }
    }

    #[test]
    fn refusing_without_root_names_the_command_to_re_run() {
        // The one case where failing politely matters: a permission error thrown from the
        // middle of an install tells the user nothing about how to finish it.
        let message = UdevOutcome::NotRoot.message();
        assert!(message.contains("without root"), "{message}");
        assert!(
            message.contains("sudo herdr-deck install --write-udev"),
            "{message}"
        );
        assert!(UdevOutcome::NotRoot.needs_manual_commands());
    }

    #[test]
    fn a_successful_write_tells_the_user_to_replug() {
        // udev applies the rule on the next device appearance; without the replug the install
        // looks like it did nothing.
        let message = UdevOutcome::Installed {
            replaced_existing: false,
        }
        .message();
        assert!(message.contains(UDEV_RULE_PATH), "{message}");
        assert!(message.contains("replug"), "{message}");
        assert!(!UdevOutcome::Installed {
            replaced_existing: false
        }
        .needs_manual_commands());
    }

    #[test]
    fn an_already_current_rule_is_reported_rather_than_rewritten() {
        let message = UdevOutcome::AlreadyCurrent.message();
        assert!(message.contains("already up to date"), "{message}");
    }

    #[test]
    fn a_failed_reload_admits_the_rule_is_written_but_not_live() {
        // Silently doing less than asked is the one thing to avoid: the file exists, so a
        // later run would say "already up to date" and hide the missing reload.
        let message = UdevOutcome::ReloadFailed {
            error: "udevadm not installed".to_string(),
        }
        .message();
        assert!(message.contains("udevadm not installed"), "{message}");
        assert!(message.contains("--reload-rules"), "{message}");
    }

    #[test]
    fn write_udev_off_linux_says_so_instead_of_quietly_doing_nothing() {
        let message = UdevOutcome::NotApplicable.message();
        assert!(message.contains("no udev"), "{message}");
        assert!(message.contains("Stream Deck app"), "{message}");
    }

    #[test]
    fn detection_compiles_and_answers_on_whatever_platform_this_is() {
        // The cfg split has two arms and only one is ever compiled here; this at least proves
        // the arm for this platform produces a usable answer rather than panicking.
        let detection = detect();
        assert!(!detection.summary().is_empty());
        if cfg!(target_os = "linux") {
            assert_ne!(detection, Detection::ElgatoOwnsTheDevice);
        } else {
            assert_eq!(detection, Detection::ElgatoOwnsTheDevice);
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn the_udev_path_in_these_messages_is_the_one_the_driver_actually_uses() {
        // The constant is duplicated so the messages compile on macOS; this keeps the copy
        // honest.
        assert_eq!(UDEV_RULE_PATH, herdr_deck_hid::UDEV_RULE_PATH);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn the_effective_uid_is_read_rather_than_the_real_one_because_sudo_only_changes_that() {
        let status = "Name:\therdr-deck\nUid:\t1000\t0\t0\t1000\nGid:\t1000\t1000\t1000\t1000\n";
        assert_eq!(parse_effective_uid(status), Some(0));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn an_unreadable_status_file_means_we_assume_we_are_not_root() {
        // Assuming root and then failing mid-write is the worse of the two guesses.
        assert_eq!(parse_effective_uid("Name:\tsomething\n"), None);
        assert_ne!(parse_effective_uid(""), Some(0));
    }
}
