//! A record of what the deck asked herdr to do.
//!
//! # Why a physical control surface needs one
//!
//! A key is pressed by a hand, often the wrong hand, sometimes by an elbow. Most of what the deck
//! can do takes nothing away, but it can now close a pane — and the last pane of a tab takes the
//! tab, and the last tab takes the workspace. "Did I do that, or did herdr?" is a question the
//! user is entitled to an answer to. One line per command, in the order they were issued.
//!
//! # What is deliberately not in here
//!
//! Reads and repaints. The watcher re-reads herdr on a timer whether anyone touches the deck or
//! not, so logging that would bury the handful of lines that matter under thousands that never
//! do. Only a [`DeckCommand`] reaches this module, and a command is by definition something the
//! deck asked herdr to *change*.
//!
//! Nothing the user or their agents wrote, either. A command names its target by id — `w1:p2`,
//! `w2` — by a word from a closed set, such as `left`, or by the name of a config preset, which is
//! a word the user chose for a *key*. Never by label, title or working directory: an audit trail
//! that quoted those would be a file that had to be guarded like a secret instead of read like a
//! log. A command whose only identifier is a path or a branch name records no target at all rather
//! than bending that rule, and its own name still says what was done.
//!
//! # Never in the way
//!
//! Writing happens on its own task behind a bounded channel. A slow or full disk therefore costs
//! audit lines, not key latency: [`AuditLog::record`] never waits and never fails a command. It
//! says so in the log when it drops one, because a silently incomplete audit trail is worse than
//! an obviously incomplete one.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use herdr_deck_core::command::DeckCommand;
use herdr_deck_focus::FocusVerdict;
use tokio::io::AsyncWriteExt as _;
use tokio::sync::{mpsc, oneshot};

/// How large the log may grow before it is rotated.
///
/// One generation is kept, so the trail occupies at most twice this on disk. A deck issues
/// commands at the speed of a human pressing keys; a quarter of a megabyte is months of them, and
/// a bounded log nobody has to think about is worth more than a complete one that quietly fills a
/// home directory.
const MAX_BYTES: u64 = 256 * 1024;

/// How many lines may be waiting to be written before further ones are dropped.
///
/// Generous enough to absorb a burst of presses against a stalled disk, small enough that a disk
/// that never comes back cannot grow the daemon's memory without limit.
const QUEUE: usize = 256;

enum Message {
    Line(String),
    /// Wait until everything queued ahead of this has reached the file.
    Flush(oneshot::Sender<()>),
}

/// An append-only record of the commands this daemon issued.
pub struct AuditLog {
    /// `None` when there is nowhere to write, which is a working deck with no audit trail rather
    /// than an error — the daemon's job is to reach agents, not to keep records.
    tx: Option<mpsc::Sender<Message>>,
}

impl AuditLog {
    /// Where the log lives: the daemon's own state directory.
    ///
    /// Not `$XDG_RUNTIME_DIR`, where the frontend socket lives — that is cleared on every reboot,
    /// and a record of what was closed last week is exactly the thing worth surviving one.
    pub fn default_path() -> Option<PathBuf> {
        directories::BaseDirs::new().map(|dirs| {
            dirs.data_local_dir()
                .join("herdr-deck")
                .join("commands.jsonl")
        })
    }

    /// Open the log at `path`, writing on a task of its own.
    pub fn open(path: PathBuf) -> Self {
        Self::with_limit(path, MAX_BYTES)
    }

    pub fn with_limit(path: PathBuf, max_bytes: u64) -> Self {
        let (tx, rx) = mpsc::channel(QUEUE);
        tokio::spawn(write_lines(path, max_bytes, rx));
        Self { tx: Some(tx) }
    }

    /// An audit log that records nothing, for when there is no state directory to write to.
    pub fn disabled() -> Self {
        Self { tx: None }
    }

    /// Record one command that was issued to herdr, and what came of it.
    ///
    /// Returns immediately, always. Nothing about a key press waits on a disk.
    pub fn record(&self, command: &DeckCommand, verdict: FocusVerdict) {
        let Some(tx) = &self.tx else { return };
        let line = serde_json::json!({
            "at": utc_timestamp(SystemTime::now()),
            "command": command.name(),
            "target": command.target(),
            "outcome": verdict.as_str(),
        });
        if tx.try_send(Message::Line(line.to_string())).is_err() {
            // Degrading loudly: an audit trail with an unmarked hole in it is worse than one that
            // admits where the hole is.
            tracing::warn!(
                command = command.name(),
                "audit log is not keeping up; this command was not recorded"
            );
        }
    }

    /// Wait until everything recorded so far has reached the file.
    pub async fn flush(&self) {
        let Some(tx) = &self.tx else { return };
        let (ack, done) = oneshot::channel();
        if tx.send(Message::Flush(ack)).await.is_ok() {
            let _ = done.await;
        }
    }
}

/// The writer task: owns the file, so nothing else ever has to hold a lock to log.
async fn write_lines(path: PathBuf, max_bytes: u64, mut rx: mpsc::Receiver<Message>) {
    let mut open: Option<(tokio::fs::File, u64)> = None;

    while let Some(message) = rx.recv().await {
        match message {
            Message::Line(mut line) => {
                line.push('\n');
                // Opened on the first command rather than at startup, so a daemon that is only
                // ever looked at leaves no file behind at all.
                if open.is_none() {
                    open = append_to(&path).await;
                }
                // Rotate before the line rather than after, so the cap is a ceiling and not a
                // figure the file is allowed to sit just above.
                if let Some((_, size)) = &open {
                    if *size > 0 && *size + line.len() as u64 > max_bytes {
                        open = rotate(&path).await;
                    }
                }
                let Some((file, size)) = &mut open else {
                    continue;
                };
                match file.write_all(line.as_bytes()).await {
                    Ok(()) => *size += line.len() as u64,
                    Err(e) => {
                        tracing::warn!(path = %path.display(), error = %e, "could not write audit line");
                        open = None;
                    }
                }
            }
            Message::Flush(ack) => {
                if let Some((file, _)) = &mut open {
                    let _ = file.flush().await;
                }
                let _ = ack.send(());
            }
        }
    }
}

/// Open the log for appending, creating its directory, and report how long it already is.
async fn append_to(path: &Path) -> Option<(tokio::fs::File, u64)> {
    if let Some(parent) = path.parent() {
        if let Err(e) = tokio::fs::create_dir_all(parent).await {
            tracing::warn!(path = %parent.display(), error = %e, "no audit log: could not create directory");
            return None;
        }
    }
    match tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .await
    {
        Ok(file) => {
            let size = file.metadata().await.map(|m| m.len()).unwrap_or(0);
            Some((file, size))
        }
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e, "no audit log: could not open file");
            None
        }
    }
}

/// Move the log aside and start a fresh one, keeping exactly one generation.
async fn rotate(path: &Path) -> Option<(tokio::fs::File, u64)> {
    let previous = path.with_extension("jsonl.1");
    if let Err(e) = tokio::fs::rename(path, &previous).await {
        tracing::warn!(path = %path.display(), error = %e, "could not rotate the audit log");
        return None;
    }
    append_to(path).await
}

/// `SystemTime` as `YYYY-MM-DDTHH:MM:SSZ`.
///
/// Hand-rolled because the daemon has no date library and would gain one for this alone. An audit
/// line wants a timestamp a human can line up against `journalctl` output, and that is the whole
/// requirement.
fn utc_timestamp(now: SystemTime) -> String {
    let seconds = now
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_secs() as i64)
        .unwrap_or(0);
    let (year, month, day) = civil_from_days(seconds.div_euclid(86_400));
    let time = seconds.rem_euclid(86_400);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        time / 3600,
        (time % 3600) / 60,
        time % 60
    )
}

/// Days since 1970-01-01 to a calendar date (Howard Hinnant's `civil_from_days`).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    // Shift the epoch to 0000-03-01 so leap days land at the end of the cycle and every other
    // month has a fixed length, which is what makes the arithmetic below branch-free.
    let shifted = days + 719_468;
    let era = (if shifted >= 0 {
        shifted
    } else {
        shifted - 146_096
    }) / 146_097;
    let day_of_era = shifted - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * month_index + 2) / 5 + 1) as u32;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    } as u32;
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn focus(pane: &str) -> DeckCommand {
        DeckCommand::FocusPane {
            pane_id: pane.to_string(),
        }
    }

    async fn read(path: &Path) -> String {
        tokio::fs::read_to_string(path).await.unwrap_or_default()
    }

    fn lines(text: &str) -> Vec<serde_json::Value> {
        text.lines()
            .map(|line| serde_json::from_str(line).expect("every audit line is valid JSON"))
            .collect()
    }

    #[tokio::test]
    async fn every_command_leaves_a_line_naming_whatever_it_is_allowed_to_name() {
        // Four direction keys share a name in the log and differ only by target, so a target left
        // blank there would turn every arrow press into an indistinguishable line — and the trail
        // exists precisely to answer "which key did I hit". The exceptions are the commands whose
        // only identifier is a path or a branch out of the user's own repository: those record
        // `null` deliberately rather than copying it to disk, and the command's own name is then
        // the whole of the record.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("commands.jsonl");
        let log = AuditLog::open(path.clone());

        for command in DeckCommand::every() {
            log.record(&command, FocusVerdict::Complete);
        }
        log.flush().await;

        let recorded = lines(&read(&path).await);
        assert_eq!(recorded.len(), DeckCommand::every().len());
        let anonymous: Vec<&str> = recorded
            .iter()
            .filter(|line| line["target"].is_null())
            .map(|line| line["command"].as_str().unwrap_or_default())
            .collect();
        assert_eq!(anonymous, vec!["open_worktree", "create_worktree"]);
        for line in &recorded {
            assert!(
                line["target"].is_null() || line["target"].as_str().is_some_and(|t| !t.is_empty()),
                "{line} recorded an empty target, which says less than recording none"
            );
        }
    }

    #[tokio::test]
    async fn a_press_that_found_nothing_to_do_is_told_apart_from_one_that_did_something() {
        // Both are successes and both look identical on the key. The log is the only place the
        // difference survives, and "I pressed left four times and only moved twice" is a question
        // somebody is entitled to an answer to.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("commands.jsonl");
        let log = AuditLog::open(path.clone());

        let left = DeckCommand::MovePaneFocus {
            direction: herdr_deck_herdr::wire::PaneDirection::Left,
        };
        log.record(&left, FocusVerdict::Settled);
        log.record(&left, FocusVerdict::Unchanged);
        log.flush().await;

        let recorded = lines(&read(&path).await);
        assert_eq!(recorded[0]["outcome"], "settled");
        assert_eq!(recorded[1]["outcome"], "unchanged");
    }

    #[tokio::test]
    async fn a_command_that_reached_herdr_is_recorded_with_what_came_of_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("commands.jsonl");
        let log = AuditLog::open(path.clone());

        log.record(&focus("w1:p2"), FocusVerdict::Complete);
        log.flush().await;

        let recorded = lines(&read(&path).await);
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0]["command"], "focus_pane");
        assert_eq!(recorded[0]["target"], "w1:p2");
        assert_eq!(recorded[0]["outcome"], "complete");
        assert!(
            recorded[0]["at"].as_str().unwrap().ends_with('Z'),
            "the timestamp has to say which clock it is on: {}",
            recorded[0]["at"]
        );
    }

    #[tokio::test]
    async fn a_failed_command_is_recorded_too_because_that_is_the_interesting_one() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("commands.jsonl");
        let log = AuditLog::open(path.clone());

        log.record(&focus("w9:p9"), FocusVerdict::Failed);
        log.flush().await;

        assert_eq!(lines(&read(&path).await)[0]["outcome"], "failed");
    }

    #[tokio::test]
    async fn the_log_is_capped_and_keeps_exactly_one_generation_behind_it() {
        // The property that matters is that a deck left running for a year cannot fill a disk.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("commands.jsonl");
        let cap = 512;
        let log = AuditLog::with_limit(path.clone(), cap);

        for i in 0..500 {
            log.record(&focus(&format!("w1:p{i}")), FocusVerdict::Complete);
            // Flushed each time so the rotation is exercised rather than raced past.
            log.flush().await;
        }

        let current = tokio::fs::metadata(&path).await.unwrap().len();
        assert!(current <= cap, "the live log grew to {current} bytes");

        let previous = path.with_extension("jsonl.1");
        let kept = tokio::fs::metadata(&previous).await.unwrap().len();
        assert!(kept <= cap, "the rotated log grew to {kept} bytes");
        assert!(
            !path.with_extension("jsonl.2").exists(),
            "one generation, not a pile of them"
        );
    }

    #[tokio::test]
    async fn nothing_is_written_at_all_until_a_command_is_actually_issued() {
        // A daemon that is only ever watched should leave no trail: the file appearing at all is
        // itself information.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("commands.jsonl");
        let log = AuditLog::open(path.clone());

        log.flush().await;
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn a_disabled_log_accepts_commands_without_complaining() {
        // No state directory must never be a reason a key press fails.
        let log = AuditLog::disabled();
        log.record(&focus("w1:p1"), FocusVerdict::Complete);
        log.flush().await;
    }

    #[tokio::test]
    async fn an_unwritable_location_costs_the_trail_and_not_the_daemon() {
        let dir = tempfile::tempdir().unwrap();
        // A file where the log's directory should be: creating the directory cannot succeed.
        let blocker = dir.path().join("blocked");
        tokio::fs::write(&blocker, b"not a directory")
            .await
            .unwrap();
        let log = AuditLog::open(blocker.join("commands.jsonl"));

        log.record(&focus("w1:p1"), FocusVerdict::Complete);
        log.flush().await;
    }

    #[test]
    fn timestamps_are_utc_and_readable_without_a_tool() {
        assert_eq!(utc_timestamp(UNIX_EPOCH), "1970-01-01T00:00:00Z");
        assert_eq!(
            utc_timestamp(UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000)),
            "2023-11-14T22:13:20Z"
        );
        // A leap day, because the month arithmetic is the part worth doubting.
        assert_eq!(
            utc_timestamp(UNIX_EPOCH + std::time::Duration::from_secs(1_709_164_800)),
            "2024-02-29T00:00:00Z"
        );
    }
}
