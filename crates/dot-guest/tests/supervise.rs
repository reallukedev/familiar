//! The supervisor's restart and rollback rules, with shell scripts standing in for
//! agent versions inside a temporary support folder.
#![expect(clippy::unwrap_used, reason = "A setup error must fail the test")]
use dot_guest::layout::{Layout, Trial};
use dot_guest::supervise::{Outcome, Supervisor, Verdict};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use tokio::sync::watch;

struct Fixture {
    dir: tempfile::TempDir,
    layout: Layout,
    supervisor: Supervisor,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let layout = Layout::at(dir.path().join("support"));
        let own = dir.path().join("launcher-binary");
        let fixture_log = dir.path().join("starts.log");
        write_script(
            &own,
            &format!("echo \"launcher $*\" >> '{}'\n", fixture_log.display()),
        );
        let supervisor = Supervisor {
            layout: layout.clone(),
            own_binary: own,
            own_version: "0.1.0".into(),
            child_args: vec!["run".into(), "--port".into(), "1".into()],
        };
        Self {
            dir,
            layout,
            supervisor,
        }
    }

    fn log(&self) -> PathBuf {
        self.dir.path().join("starts.log")
    }

    /// A version whose binary logs its start and then runs `body`.
    fn version(&self, version: &str, body: &str) {
        let bin = self.layout.version_binary(version);
        std::fs::create_dir_all(bin.parent().unwrap()).unwrap();
        write_script(
            &bin,
            &format!(
                "echo \"{version} $* supervised=$DOT_GUEST_SUPERVISED\" >> '{}'\n{body}\n",
                self.log().display()
            ),
        );
    }

    fn starts(&self) -> Vec<String> {
        std::fs::read_to_string(self.log())
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    fn trial(&self, version: &str, attempts: u32) {
        self.layout
            .write_trial(&Trial {
                version: version.into(),
                sha256: Some("ab".repeat(32)),
                started_at: 0,
                attempts,
            })
            .unwrap();
    }

    async fn once(&self) -> Outcome {
        let (_tx, mut rx) = watch::channel(false);
        self.supervisor.run_once(&mut rx).await
    }
}

fn write_script(path: &Path, body: &str) {
    std::fs::write(path, format!("#!/bin/sh\n{body}")).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn verdict(outcome: Outcome) -> Verdict {
    match outcome {
        Outcome::Exited { verdict, .. } => verdict,
        Outcome::Stopped => panic!("unexpected stop"),
    }
}

#[tokio::test]
async fn a_version_that_keeps_crashing_rolls_back_to_previous_after_three_starts() {
    let f = Fixture::new();
    f.version("1.0.0", "exit 0");
    f.version("2.0.0", "exit 3");
    f.layout.switch_to("1.0.0").unwrap();
    f.layout.switch_to("2.0.0").unwrap();
    f.trial("2.0.0", 0);

    assert_eq!(verdict(f.once().await), Verdict::Retry { attempts: 1 });
    assert_eq!(f.layout.read_trial().unwrap().attempts, 1);
    assert_eq!(verdict(f.once().await), Verdict::Retry { attempts: 2 });
    assert_eq!(verdict(f.once().await), Verdict::RollBack);

    assert_eq!(f.layout.current_version().as_deref(), Some("1.0.0"));
    assert_eq!(f.layout.previous_version(), None);
    assert_eq!(f.layout.read_trial(), None);
    let bad = f.layout.read_bad();
    assert_eq!(bad.len(), 1);
    assert_eq!(bad[0].version, "2.0.0");
    assert_eq!(bad[0].sha256.as_deref(), Some("ab".repeat(32).as_str()));

    // The next start runs the previous version, with the arguments and the marker.
    assert_eq!(verdict(f.once().await), Verdict::Restart);
    assert_eq!(
        f.starts(),
        [
            "2.0.0 run --port 1 supervised=1",
            "2.0.0 run --port 1 supervised=1",
            "2.0.0 run --port 1 supervised=1",
            "1.0.0 run --port 1 supervised=1",
        ]
    );
}

#[tokio::test]
async fn without_a_previous_version_rollback_falls_back_to_the_launcher() {
    let f = Fixture::new();
    f.version("2.0.0", "exit 3");
    f.layout.switch_to("2.0.0").unwrap();
    f.trial("2.0.0", 2);

    assert_eq!(verdict(f.once().await), Verdict::RollBack);
    assert!(!f.layout.current_link().exists());
    assert!(f.layout.is_bad("2.0.0", None));

    assert_eq!(verdict(f.once().await), Verdict::Restart);
    assert_eq!(
        f.starts().last().map(String::as_str),
        Some("launcher run --port 1")
    );
}

#[tokio::test]
async fn a_current_version_marked_bad_is_skipped_for_the_launcher() {
    let f = Fixture::new();
    f.version("2.0.0", "exit 0");
    f.layout.switch_to("2.0.0").unwrap();
    f.layout.mark_bad("2.0.0", None, "test").unwrap();

    assert_eq!(verdict(f.once().await), Verdict::Restart);
    assert_eq!(f.starts(), ["launcher run --port 1"]);
}

#[tokio::test]
async fn stopping_forwards_sigterm_to_the_child() {
    let f = Fixture::new();
    let got = f.dir.path().join("got-term");
    let ready = f.dir.path().join("ready");
    f.version(
        "1.0.0",
        &format!(
            "trap 'echo term > \"{}\"; exit 0' TERM\ntouch \"{}\"\nwhile :; do sleep 0.05; done",
            got.display(),
            ready.display()
        ),
    );
    f.layout.switch_to("1.0.0").unwrap();
    let (tx, mut rx) = watch::channel(false);
    let started = tokio::spawn(async move {
        for _ in 0..500 {
            if ready.exists() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        let _ = tx.send(true);
    });
    assert_eq!(f.supervisor.run_once(&mut rx).await, Outcome::Stopped);
    started.await.unwrap();
    assert_eq!(std::fs::read_to_string(got).unwrap().trim(), "term");
}
