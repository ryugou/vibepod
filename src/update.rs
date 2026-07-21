//! Keeping the container's Claude Code binary current.
//!
//! The Docker image bakes in whatever Claude Code version `install.sh`
//! returned at `vibepod init` time. Nothing afterwards refreshes it: `run`
//! only checks that the image exists, and the container's `docker exec`
//! just launches `claude`. A user who ran `vibepod init` months ago keeps
//! getting that months-old build forever.
//!
//! This module closes that gap by running `claude update` inside the
//! container before handing control to Claude. The container's Claude Code
//! is installed by the native installer, which ships a working
//! self-update — verified against the real image:
//!
//! ```text
//! $ claude --version           -> 2.1.100 (Claude Code)
//! $ claude update
//!   Current version: 2.1.100
//!   Checking for updates to latest version...
//!   Successfully updated from 2.1.100 to version 2.1.216
//! $ claude --version           -> 2.1.216 (Claude Code)
//! ```
//!
//! Two properties matter operationally:
//!
//! * **Throttled.** A network round-trip on every `vibepod run` would make
//!   startup feel broken. See [`DEFAULT_UPDATE_INTERVAL_SECS`].
//! * **Never fatal.** A flaky network or a rate limit must not be able to
//!   stop `vibepod run` from working. Failures warn and continue on the
//!   version already installed.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::ui::sanitize::sanitize_single_line;

/// How long a successful update check stays valid.
///
/// 24 hours. Claude Code ships new builds on roughly a daily cadence, so a
/// shorter window spends network round-trips on runs that would almost
/// always find nothing new, while a longer one lets a container drift
/// noticeably behind. This only bounds *automatic* checks — `--update`
/// forces one immediately, and a freshly created container always checks
/// regardless of this interval.
pub const DEFAULT_UPDATE_INTERVAL_SECS: u64 = 24 * 60 * 60;

/// Upper bound on a single `claude update` run.
///
/// The update downloads a binary, so it is legitimately slower than a
/// metadata ping, but it must not be able to hang `vibepod run`
/// indefinitely on a stalled connection. Three minutes is generous for the
/// download and still short enough that a hung check looks like a hiccup
/// rather than a freeze.
const UPDATE_TIMEOUT: Duration = Duration::from_secs(180);

/// Entries untouched for this long are dropped from the state file.
///
/// Container names are derived from the project path, so the file would
/// otherwise accumulate one permanent entry per project ever run, including
/// long-deleted ones. A container unused for 30 days will always be past
/// its check interval anyway, so forgetting it costs nothing.
const STATE_PRUNE_AFTER_SECS: u64 = 30 * 24 * 60 * 60;

/// What the user asked for on the command line.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum UpdatePolicy {
    /// Throttled automatic check (default).
    #[default]
    Auto,
    /// `--update`: check now, ignoring the interval.
    Force,
    /// `--no-update`: do not check at all.
    Disabled,
}

/// Why a check was skipped. Carried so the caller can explain itself
/// instead of silently doing nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SkipReason {
    /// `--no-update` was passed.
    Disabled,
    /// `--no-network`: the container has no route to the update server.
    NetworkDisabled,
    /// Checked recently; `next_due_in_secs` until the interval elapses.
    RecentlyChecked { next_due_in_secs: u64 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpdateDecision {
    Run,
    Skip(SkipReason),
}

/// Decide whether to run `claude update`, given the policy, the container's
/// last successful check, and the current time.
///
/// `now` and `last_checked` are parameters rather than being read from the
/// clock inside so the throttling rules are directly testable.
///
/// Precedence, highest first:
///
/// 1. `--no-update` wins over everything — an explicit opt-out.
/// 2. `--no-network` skips even under `--update`: the container cannot
///    reach the update server, so attempting it only wastes the timeout.
/// 3. `--update` ignores the interval.
/// 4. A freshly created container always checks. Its Claude Code comes
///    straight from the image and is as old as the last `vibepod init`.
///    This case must bypass `last_checked`, because container names are
///    derived from the project path: recreating a container reuses the
///    name, and the previous container's timestamp would otherwise
///    suppress the check on what is really a brand-new install.
/// 5. Never checked before → check.
/// 6. A timestamp in the future means the clock moved backwards (or the
///    state file was tampered with). Treat it as unusable and check now,
///    rather than trusting it and potentially skipping checks for years.
/// 7. Otherwise, check once the interval has elapsed.
pub fn decide_update(
    policy: UpdatePolicy,
    network_disabled: bool,
    container_created: bool,
    last_checked: Option<u64>,
    now: u64,
    interval_secs: u64,
) -> UpdateDecision {
    if policy == UpdatePolicy::Disabled {
        return UpdateDecision::Skip(SkipReason::Disabled);
    }
    if network_disabled {
        return UpdateDecision::Skip(SkipReason::NetworkDisabled);
    }
    if policy == UpdatePolicy::Force || container_created {
        return UpdateDecision::Run;
    }
    let Some(last) = last_checked else {
        return UpdateDecision::Run;
    };
    if last > now {
        return UpdateDecision::Run;
    }
    let elapsed = now - last;
    if elapsed >= interval_secs {
        UpdateDecision::Run
    } else {
        UpdateDecision::Skip(SkipReason::RecentlyChecked {
            next_due_in_secs: interval_secs - elapsed,
        })
    }
}

/// Timestamps of the last successful update check, keyed by container name.
///
/// Keyed per container rather than globally because each container carries
/// its own Claude Code install in its writable layer: updating container A
/// says nothing about how stale container B is.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct UpdateState {
    #[serde(default)]
    pub containers: HashMap<String, u64>,
}

pub fn state_path(config_dir: &Path) -> PathBuf {
    config_dir.join("update-check.json")
}

/// Load the state file. A missing file is normal (first run).
///
/// A corrupt file is **not** fatal: the worst consequence of losing these
/// timestamps is one extra update check. Refusing to run `vibepod` over an
/// unparseable cache file would be a far worse trade, so we warn and start
/// from empty.
pub fn load_state(config_dir: &Path) -> UpdateState {
    let path = state_path(config_dir);
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return UpdateState::default();
    };
    match serde_json::from_str(&raw) {
        Ok(state) => state,
        Err(e) => {
            eprintln!(
                "  Warning: ignoring unreadable update-check state at {} ({}). \
                 Treating it as empty; an extra update check may run.",
                path.display(),
                sanitize_single_line(&format!("{e}"), 200)
            );
            UpdateState::default()
        }
    }
}

/// Record a successful check for `container` at `now`, pruning stale
/// entries, and persist.
pub fn record_check(config_dir: &Path, container: &str, now: u64) -> Result<()> {
    let mut state = load_state(config_dir);
    state.containers.insert(container.to_string(), now);
    state
        .containers
        .retain(|_, ts| now.saturating_sub(*ts) < STATE_PRUNE_AFTER_SECS);

    let path = state_path(config_dir);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create {}", parent.display()))?;
    }
    let serialized =
        serde_json::to_string_pretty(&state).context("Failed to serialize update-check state")?;
    std::fs::write(&path, serialized)
        .with_context(|| format!("Failed to write {}", path.display()))?;
    Ok(())
}

/// Current wall-clock time as a Unix timestamp.
///
/// A clock before the epoch is not something we can act on, and it must not
/// break `vibepod run`, so it degrades to 0 — which `decide_update` reads
/// as "no usable timestamp" and simply checks.
pub fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Run `claude update` inside `container`.
///
/// Returns the trimmed stdout on success. Uses a login shell so the
/// installer's `PATH` (`~/.local/bin`) is present, matching how `claude`
/// itself is launched.
async fn exec_claude_update(container: &str) -> Result<String> {
    let child = tokio::process::Command::new("docker")
        .args(["exec", container, "bash", "--login", "-c", "claude update"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .context("Failed to spawn `docker exec` for the Claude Code update")?;

    let output = match tokio::time::timeout(UPDATE_TIMEOUT, child.wait_with_output()).await {
        Ok(result) => result.context("Failed to run `claude update` in the container")?,
        Err(_) => {
            // The child owns a docker exec session; leaving it running
            // would hold the container busy past our own lifetime.
            anyhow::bail!(
                "`claude update` did not finish within {}s (likely a stalled network connection)",
                UPDATE_TIMEOUT.as_secs()
            );
        }
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let detail = if stderr.trim().is_empty() {
            stdout
        } else {
            stderr
        };
        anyhow::bail!(
            "`claude update` exited with {}: {}",
            output
                .status
                .code()
                .map(|c| c.to_string())
                .unwrap_or_else(|| "signal".to_string()),
            sanitize_single_line(detail.trim(), 500)
        );
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Update Claude Code in `container` if the policy and throttle allow it.
///
/// Never returns an error: this runs on the critical path of `vibepod run`,
/// and a failed update must not stop the user from working. Every outcome
/// is reported, and every failure states both the cause and the operator's
/// next move, so a silently stale container is not a possible outcome.
pub async fn maybe_update_claude(
    config_dir: &Path,
    container: &str,
    policy: UpdatePolicy,
    network_disabled: bool,
    container_created: bool,
) {
    let now = now_unix();
    let last_checked = load_state(config_dir).containers.get(container).copied();

    let decision = decide_update(
        policy,
        network_disabled,
        container_created,
        last_checked,
        now,
        DEFAULT_UPDATE_INTERVAL_SECS,
    );

    match decision {
        UpdateDecision::Skip(SkipReason::Disabled) => return,
        UpdateDecision::Skip(SkipReason::NetworkDisabled) => {
            // Worth saying out loud: with --no-network the container keeps
            // whatever version it has, and --update cannot change that.
            println!("  ◇  Skipping Claude Code update check (--no-network).");
            return;
        }
        UpdateDecision::Skip(SkipReason::RecentlyChecked { next_due_in_secs }) => {
            let hours = next_due_in_secs / 3600;
            println!(
                "  ◇  Claude Code update check skipped (checked recently; \
                 next check in ~{}h, or run with --update).",
                hours
            );
            return;
        }
        UpdateDecision::Run => {}
    }

    println!("  ◇  Checking for Claude Code updates in the container...");

    match exec_claude_update(container).await {
        Ok(stdout) => {
            // `claude update` prints either "up to date" or "Successfully
            // updated from X to Y"; echo its last line so the run log
            // records which version actually ran.
            if let Some(last_line) = stdout.lines().rfind(|l| !l.trim().is_empty()) {
                println!("  │  {}", sanitize_single_line(last_line.trim(), 200));
            }
            if let Err(e) = record_check(config_dir, container, now) {
                // Non-fatal: the update itself succeeded. Losing the
                // timestamp only costs an extra check next run.
                eprintln!(
                    "  Warning: Claude Code was updated, but the check timestamp \
                     could not be saved: {}\n    \
                     The update check will run again on the next `vibepod run`.",
                    sanitize_single_line(&format!("{e}"), 300)
                );
            }
        }
        Err(e) => {
            // Deliberately not fatal, and deliberately loud. The run
            // continues on the already-installed version; the operator is
            // told the cause, the manual command, and the opt-out.
            eprintln!(
                "  Warning: Claude Code update check failed in container '{}': {}\n    \
                 Continuing with the version already installed in the container.\n    \
                 To update manually: docker exec -it {} bash --login -c 'claude update'\n    \
                 To skip this check:  vibepod run --no-update",
                container,
                sanitize_single_line(&format!("{e}"), 500),
                container
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOUR: u64 = 3600;
    const DAY: u64 = 24 * HOUR;
    /// An arbitrary but realistic "now" so the arithmetic in these tests
    /// reads as wall-clock time rather than as offsets from zero.
    const NOW: u64 = 1_700_000_000;

    #[test]
    fn no_update_flag_skips_even_for_a_brand_new_container() {
        let decision = decide_update(UpdatePolicy::Disabled, false, true, None, NOW, DAY);
        assert_eq!(decision, UpdateDecision::Skip(SkipReason::Disabled));
    }

    #[test]
    fn no_network_skips_even_when_update_is_forced() {
        // --update cannot succeed without a route to the update server,
        // so it must not burn the timeout trying.
        let decision = decide_update(UpdatePolicy::Force, true, false, Some(NOW), NOW, DAY);
        assert_eq!(decision, UpdateDecision::Skip(SkipReason::NetworkDisabled));
    }

    #[test]
    fn update_flag_overrides_a_check_that_just_happened() {
        let decision = decide_update(UpdatePolicy::Force, false, false, Some(NOW), NOW, DAY);
        assert_eq!(decision, UpdateDecision::Run);
    }

    #[test]
    fn newly_created_container_checks_despite_a_recent_timestamp() {
        // Container names are derived from the project path, so a
        // recreated container inherits the old one's timestamp while
        // actually carrying a fresh install from the image.
        let decision = decide_update(UpdatePolicy::Auto, false, true, Some(NOW), NOW, DAY);
        assert_eq!(decision, UpdateDecision::Run);
    }

    #[test]
    fn first_ever_check_runs() {
        let decision = decide_update(UpdatePolicy::Auto, false, false, None, NOW, DAY);
        assert_eq!(decision, UpdateDecision::Run);
    }

    #[test]
    fn check_is_skipped_within_the_interval() {
        let six_hours_ago = NOW - 6 * HOUR;
        let decision = decide_update(
            UpdatePolicy::Auto,
            false,
            false,
            Some(six_hours_ago),
            NOW,
            DAY,
        );
        assert_eq!(
            decision,
            UpdateDecision::Skip(SkipReason::RecentlyChecked {
                next_due_in_secs: 18 * HOUR,
            })
        );
    }

    #[test]
    fn check_runs_once_the_interval_has_exactly_elapsed() {
        // Boundary: elapsed == interval is due, not skipped.
        let decision = decide_update(UpdatePolicy::Auto, false, false, Some(NOW - DAY), NOW, DAY);
        assert_eq!(decision, UpdateDecision::Run);
    }

    #[test]
    fn check_is_skipped_one_second_before_the_interval() {
        let decision = decide_update(
            UpdatePolicy::Auto,
            false,
            false,
            Some(NOW - DAY + 1),
            NOW,
            DAY,
        );
        assert_eq!(
            decision,
            UpdateDecision::Skip(SkipReason::RecentlyChecked {
                next_due_in_secs: 1
            })
        );
    }

    #[test]
    fn future_timestamp_from_clock_skew_forces_a_check() {
        // A backwards clock jump must not be able to suppress updates
        // until the clock catches up.
        let decision = decide_update(
            UpdatePolicy::Auto,
            false,
            false,
            Some(NOW + 365 * DAY),
            NOW,
            DAY,
        );
        assert_eq!(decision, UpdateDecision::Run);
    }

    #[test]
    fn state_roundtrips_through_the_config_dir() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(load_state(tmp.path()).containers.is_empty());

        record_check(tmp.path(), "vibepod-demo-abc123", NOW).unwrap();

        let state = load_state(tmp.path());
        assert_eq!(state.containers.get("vibepod-demo-abc123"), Some(&NOW));
    }

    #[test]
    fn recording_a_check_prunes_long_unused_containers() {
        let tmp = tempfile::tempdir().unwrap();
        let ancient = NOW - STATE_PRUNE_AFTER_SECS - 1;
        record_check(tmp.path(), "vibepod-stale-000000", ancient).unwrap();

        record_check(tmp.path(), "vibepod-fresh-111111", NOW).unwrap();

        let state = load_state(tmp.path());
        assert!(
            !state.containers.contains_key("vibepod-stale-000000"),
            "entry older than the prune window should be dropped"
        );
        assert!(state.containers.contains_key("vibepod-fresh-111111"));
    }

    #[test]
    fn corrupt_state_file_degrades_to_empty_instead_of_failing() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(state_path(tmp.path()), "{ not json").unwrap();

        assert!(load_state(tmp.path()).containers.is_empty());
    }
}
