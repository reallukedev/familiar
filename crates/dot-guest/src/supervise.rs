//! `dot-guest supervise`: what launchd runs. It starts `current/dot-guest run` as a
//! child, restarts it when it exits, and rolls a new version back when it fails to come
//! up.
//!
//! launchd runs this stable launcher rather than the updated binary so macOS privacy
//! permissions survive updates: they belong to the responsible process, which for a
//! child is the launcher (see the README).
use crate::layout::{Layout, Trial};
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;
use tokio::sync::watch;
use tokio::time::Instant;

/// Set to `1` in the child's environment.
pub const SUPERVISED_ENV: &str = "DOT_GUEST_SUPERVISED";
/// `run` exits with this to ask for an immediate restart (after installing an update).
pub const EXIT_RESTART: u8 = 75;
/// Starts a trial version gets before it is rolled back.
pub const MAX_ATTEMPTS: u32 = 3;
/// A child that ran this long resets the restart backoff.
const STABLE_AFTER: Duration = Duration::from_secs(60);
/// How long a stopping child gets before it is killed.
const STOP_GRACE: Duration = Duration::from_secs(10);
const BACKOFF: [Duration; 5] = [
    Duration::from_secs(1),
    Duration::from_secs(2),
    Duration::from_secs(5),
    Duration::from_secs(10),
    Duration::from_secs(30),
];

/// Is the current process running under the supervisor?
pub fn is_supervised() -> bool {
    std::env::var_os(SUPERVISED_ENV).is_some_and(|v| v == "1")
}

/// Which binary a start used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launched {
    /// The binary.
    pub program: PathBuf,
    /// Its version.
    pub version: String,
    /// It came from `current` (otherwise it is the launcher's own binary).
    pub from_current: bool,
}

/// What to do after the child exits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Start it again after the usual backoff.
    Restart,
    /// A trial start failed; record this many failed attempts and try again.
    Retry {
        /// Failed attempts so far.
        attempts: u32,
    },
    /// The trial version has failed too often: roll back.
    RollBack,
}

/// Decide what an exit means. Only a start of the version on trial, from `current`,
/// counts against it; the old version exiting to make way for the new one doesn't.
pub fn after_exit(trial: Option<&Trial>, launched: &Launched) -> Verdict {
    match trial {
        Some(trial) if launched.from_current && trial.version == launched.version => {
            let attempts = trial.attempts.saturating_add(1);
            if attempts >= MAX_ATTEMPTS {
                Verdict::RollBack
            } else {
                Verdict::Retry { attempts }
            }
        }
        _ => Verdict::Restart,
    }
}

/// Delay before the next start.
pub fn backoff(failures: usize) -> Duration {
    BACKOFF[failures.min(BACKOFF.len() - 1)]
}

/// How one start ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The child exited (or never started); the verdict has been applied.
    Exited {
        /// Exit code, `None` for a signal or a failed start.
        code: Option<i32>,
        /// How long it ran.
        ran_for: Duration,
        /// What was decided.
        verdict: Verdict,
    },
    /// The supervisor was asked to stop; the child has been stopped.
    Stopped,
}

/// Starts and restarts the agent.
#[derive(Debug)]
pub struct Supervisor {
    /// The self-update folders.
    pub layout: Layout,
    /// The launcher's own binary, used when there is no usable `current`.
    pub own_binary: PathBuf,
    /// The launcher's own version.
    pub own_version: String,
    /// Arguments for the child, starting with `run`.
    pub child_args: Vec<OsString>,
}

impl Supervisor {
    /// What to start: `current` unless it's missing or marked bad, else the launcher.
    pub fn choose(&self) -> Launched {
        if let Some(version) = self.layout.current_version() {
            if !self.layout.is_bad(&version, None) {
                return Launched {
                    program: self.layout.version_binary(&version),
                    version,
                    from_current: true,
                };
            }
            tracing::warn!(
                version,
                "current points at a version that failed before; running the launcher's own version"
            );
        }
        Launched {
            program: self.own_binary.clone(),
            version: self.own_version.clone(),
            from_current: false,
        }
    }

    /// Start, restart and roll back until `stop` turns true.
    pub async fn run(&self, mut stop: watch::Receiver<bool>) {
        let mut failures = 0usize;
        loop {
            let delay = match self.run_once(&mut stop).await {
                Outcome::Stopped => return,
                Outcome::Exited { code, .. } if code == Some(i32::from(EXIT_RESTART)) => {
                    failures = 0;
                    Duration::ZERO
                }
                Outcome::Exited {
                    verdict: Verdict::RollBack,
                    ..
                } => {
                    failures = 0;
                    Duration::ZERO
                }
                Outcome::Exited { ran_for, .. } => {
                    if ran_for >= STABLE_AFTER {
                        failures = 0;
                    }
                    let delay = backoff(failures);
                    failures = failures.saturating_add(1);
                    delay
                }
            };
            if !delay.is_zero() {
                tracing::info!(seconds = delay.as_secs(), "restarting the agent");
            }
            tokio::select! {
                () = tokio::time::sleep(delay) => {}
                _ = stop.wait_for(|stop| *stop) => return,
            }
        }
    }

    /// One start of the child, through to its exit and the verdict.
    pub async fn run_once(&self, stop: &mut watch::Receiver<bool>) -> Outcome {
        let launched = self.choose();
        tracing::info!(
            version = launched.version,
            program = %launched.program.display(),
            "starting the agent"
        );
        let started = Instant::now();
        let spawned = tokio::process::Command::new(&launched.program)
            .args(&self.child_args)
            .env(SUPERVISED_ENV, "1")
            .stdin(Stdio::null())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn();
        let mut child = match spawned {
            Ok(child) => child,
            Err(e) => {
                tracing::error!(
                    program = %launched.program.display(),
                    error = %e,
                    "could not start the agent"
                );
                return self.exited(&launched, None, started.elapsed());
            }
        };
        // A running child is never rolled back: it ends its own trial once it connects
        // or has run a while (see `ops::update::mark_healthy`), so a server outage can't
        // undo a good update. Only a crash loop can.
        tokio::select! {
            status = child.wait() => {
                let code = status.ok().and_then(|s| s.code());
                self.exited(&launched, code, started.elapsed())
            }
            _ = stop.wait_for(|stop| *stop) => {
                tracing::info!("stopping the agent");
                stop_child(&mut child).await;
                Outcome::Stopped
            }
        }
    }

    fn exited(&self, launched: &Launched, code: Option<i32>, ran_for: Duration) -> Outcome {
        let trial = self.layout.read_trial();
        let verdict = after_exit(trial.as_ref(), launched);
        match (&verdict, trial) {
            (Verdict::Retry { attempts }, Some(mut trial)) => {
                tracing::warn!(
                    version = launched.version,
                    code,
                    attempts = *attempts,
                    "the new agent stopped before it was confirmed healthy; trying again"
                );
                trial.attempts = *attempts;
                if let Err(e) = self.layout.write_trial(&trial) {
                    tracing::error!(error = %e, "could not save the update trial");
                }
            }
            (Verdict::RollBack, _) => {
                tracing::error!(
                    version = launched.version,
                    code,
                    "the new agent failed to start {MAX_ATTEMPTS} times; rolling back"
                );
                self.roll_back(
                    &launched.version,
                    &format!("it stopped {MAX_ATTEMPTS} times before it was confirmed healthy"),
                );
            }
            _ => tracing::warn!(version = launched.version, code, "the agent stopped"),
        }
        Outcome::Exited {
            code,
            ran_for,
            verdict,
        }
    }

    /// Mark `version` bad and point `current` back at `previous`, or remove `current`
    /// so the launcher's own version runs.
    pub fn roll_back(&self, version: &str, reason: &str) {
        let layout = &self.layout;
        let sha256 = layout.read_trial().and_then(|t| t.sha256);
        if let Err(e) = layout.mark_bad(version, sha256, reason) {
            tracing::error!(error = %e, "could not record the failed version");
        }
        if let Err(e) = layout.remove_trial() {
            tracing::error!(error = %e, "could not remove the update trial");
        }
        let previous = layout
            .previous_version()
            .filter(|p| p != version && !layout.is_bad(p, None));
        let result = match &previous {
            Some(previous) => layout.switch_to(previous).map(|_| ()),
            None => crate::layout::remove_if_present(&layout.current_link()),
        };
        let _ = crate::layout::remove_if_present(&layout.previous_link());
        match (result, previous) {
            (Ok(()), Some(previous)) => {
                tracing::error!(from = version, to = previous, "rolled back: {reason}");
            }
            (Ok(()), None) => tracing::error!(
                from = version,
                to = self.own_version,
                "rolled back to the launcher's own version: {reason}"
            ),
            (Err(e), _) => tracing::error!(
                error = %e,
                "could not repoint `current`; the launcher's own version will run"
            ),
        }
    }
}

/// SIGTERM, then SIGKILL after [`STOP_GRACE`].
async fn stop_child(child: &mut tokio::process::Child) {
    if let Some(pid) = child
        .id()
        .and_then(|id| i32::try_from(id).ok())
        .and_then(rustix::process::Pid::from_raw)
    {
        let _ = rustix::process::kill_process(pid, rustix::process::Signal::TERM);
    }
    if tokio::time::timeout(STOP_GRACE, child.wait())
        .await
        .is_err()
    {
        tracing::warn!("the agent did not stop in time; killing it");
        let _ = child.kill().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn launched(version: &str, from_current: bool) -> Launched {
        Launched {
            program: PathBuf::from("/x"),
            version: version.into(),
            from_current,
        }
    }

    fn trial(version: &str, attempts: u32) -> Trial {
        Trial {
            version: version.into(),
            sha256: None,
            started_at: 0,
            attempts,
        }
    }

    #[test]
    fn only_failed_starts_of_the_trial_version_count() {
        let new = launched("2.0.0", true);
        assert_eq!(after_exit(None, &new), Verdict::Restart);
        assert_eq!(
            after_exit(Some(&trial("2.0.0", 0)), &new),
            Verdict::Retry { attempts: 1 }
        );
        assert_eq!(
            after_exit(Some(&trial("2.0.0", 1)), &new),
            Verdict::Retry { attempts: 2 }
        );
        assert_eq!(
            after_exit(Some(&trial("2.0.0", 2)), &new),
            Verdict::RollBack
        );
        // The old version exiting to make way for the new one.
        assert_eq!(
            after_exit(Some(&trial("2.0.0", 0)), &launched("1.0.0", true)),
            Verdict::Restart
        );
        // The launcher's own binary running because `current` was unusable.
        assert_eq!(
            after_exit(Some(&trial("2.0.0", 2)), &launched("2.0.0", false)),
            Verdict::Restart
        );
    }
}
