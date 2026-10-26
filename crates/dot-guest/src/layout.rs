//! Where versions of the agent live, and the small state files self-update keeps.
//!
//! ```text
//! ~/Library/Application Support/Dot Guest/
//!   launcher/dot-guest        what launchd runs (`supervise`); replaced only by `dot-guest install`
//!   versions/<version>/dot-guest
//!   current  -> versions/<version>
//!   previous -> versions/<version>
//!   update/<version>.partial  a transfer in progress
//!   trial.json                a new version that hasn't proven itself yet
//!   bad.json                  versions that failed and were rolled back
//! ```
use serde::{Deserialize, Serialize};
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// The binary's name inside `launcher/` and each version folder.
pub const BINARY: &str = "dot-guest";
/// Versions kept on disk (`current` and `previous` are always kept on top of this).
pub const KEEP_VERSIONS: usize = 3;
/// Longest accepted version string.
pub const MAX_VERSION_LEN: usize = 64;

/// The self-update folders under one support folder.
#[derive(Debug, Clone)]
pub struct Layout {
    /// `~/Library/Application Support/Dot Guest`, or a temporary folder in tests.
    pub root: PathBuf,
}

impl Layout {
    /// The layout for the user whose home is `home`.
    pub fn new(home: &Path) -> Self {
        Self::at(crate::config::support_dir(home))
    }

    /// The layout under `root`.
    pub fn at(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// `launcher/dot-guest`.
    pub fn launcher(&self) -> PathBuf {
        self.root.join("launcher").join(BINARY)
    }

    /// `versions/`.
    pub fn versions_dir(&self) -> PathBuf {
        self.root.join("versions")
    }

    /// `versions/<version>/dot-guest`.
    pub fn version_binary(&self, version: &str) -> PathBuf {
        self.versions_dir().join(version).join(BINARY)
    }

    /// The `current` symlink.
    pub fn current_link(&self) -> PathBuf {
        self.root.join("current")
    }

    /// The `previous` symlink.
    pub fn previous_link(&self) -> PathBuf {
        self.root.join("previous")
    }

    /// `update/`.
    pub fn update_dir(&self) -> PathBuf {
        self.root.join("update")
    }

    /// `update/<version>.partial`.
    pub fn partial(&self, version: &str) -> PathBuf {
        self.update_dir().join(format!("{version}.partial"))
    }

    /// `trial.json`.
    pub fn trial_path(&self) -> PathBuf {
        self.root.join("trial.json")
    }

    /// `bad.json`.
    pub fn bad_path(&self) -> PathBuf {
        self.root.join("bad.json")
    }

    /// The version `current` points at, if its binary is there.
    pub fn current_version(&self) -> Option<String> {
        self.link_version(&self.current_link())
    }

    /// The version `previous` points at, if its binary is there.
    pub fn previous_version(&self) -> Option<String> {
        self.link_version(&self.previous_link())
    }

    fn link_version(&self, link: &Path) -> Option<String> {
        let target = std::fs::read_link(link).ok()?;
        let version = target.file_name()?.to_str()?.to_owned();
        (valid_version(&version).is_ok() && self.version_binary(&version).is_file())
            .then_some(version)
    }

    /// Point `current` at `version` and `previous` at what `current` was before (unless
    /// that is the same version). Each link changes atomically. Returns the old current.
    pub fn switch_to(&self, version: &str) -> io::Result<Option<String>> {
        let old = self.current_version();
        if let Some(old) = old.as_deref().filter(|old| *old != version) {
            point(&self.previous_link(), &version_target(old))?;
        }
        point(&self.current_link(), &version_target(version))?;
        Ok(old)
    }

    /// Remove version folders beyond the newest [`KEEP_VERSIONS`], never touching
    /// `current` or `previous`. Returns the removed versions.
    pub fn prune(&self) -> Vec<String> {
        let Ok(entries) = std::fs::read_dir(self.versions_dir()) else {
            return Vec::new();
        };
        let found: Vec<(String, SystemTime)> = entries
            .filter_map(Result::ok)
            .filter_map(|e| {
                let name = e.file_name().to_str()?.to_owned();
                let modified = e.metadata().ok()?.modified().ok()?;
                Some((name, modified))
            })
            .collect();
        let current = self.current_version();
        let previous = self.previous_version();
        let protected: Vec<&str> = [current.as_deref(), previous.as_deref()]
            .into_iter()
            .flatten()
            .collect();
        let doomed = prune_plan(&found, KEEP_VERSIONS, &protected);
        doomed
            .into_iter()
            .filter(|v| valid_version(v).is_ok())
            .filter(|v| std::fs::remove_dir_all(self.versions_dir().join(v)).is_ok())
            .collect()
    }
}

/// The relative target a link uses for `version`, so the support folder can move.
fn version_target(version: &str) -> PathBuf {
    Path::new("versions").join(version)
}

/// Is `version` safe as a single path component? Letters, digits, `.`, `-`, `+` and `_`,
/// starting with a letter or digit, at most [`MAX_VERSION_LEN`] characters.
pub fn valid_version(version: &str) -> Result<(), String> {
    if version.is_empty() || version.len() > MAX_VERSION_LEN {
        return Err(format!(
            "The version must be 1 to {MAX_VERSION_LEN} characters long."
        ));
    }
    if !version.starts_with(|c: char| c.is_ascii_alphanumeric()) {
        return Err(format!(
            "The version `{version}` must start with a letter or digit."
        ));
    }
    if let Some(c) = version
        .chars()
        .find(|c| !(c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+' | '_')))
    {
        return Err(format!(
            "The version `{}` contains `{c}`; only letters, digits, `.`, `-`, `+` and `_` are allowed.",
            version.escape_default()
        ));
    }
    Ok(())
}

/// Which versions to delete: everything but the newest `keep` and the `protected` ones.
pub fn prune_plan(found: &[(String, SystemTime)], keep: usize, protected: &[&str]) -> Vec<String> {
    let mut newest_first: Vec<&(String, SystemTime)> = found.iter().collect();
    newest_first.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| b.0.cmp(&a.0)));
    newest_first
        .into_iter()
        .skip(keep)
        .filter(|(name, _)| !protected.contains(&name.as_str()))
        .map(|(name, _)| name.clone())
        .collect()
}

/// Make `link` point at `target`, atomically: a new symlink under a temporary name,
/// then renamed over the old one, so a reader sees either the old or the new target.
pub fn point(link: &Path, target: &Path) -> io::Result<()> {
    let name = link
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "link has no file name"))?;
    let tmp = link.with_file_name(format!(".{name}.{}.tmp", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    std::os::unix::fs::symlink(target, &tmp)?;
    std::fs::rename(&tmp, link).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// Remove a file or symlink; already gone is fine.
pub fn remove_if_present(path: &Path) -> io::Result<()> {
    match std::fs::remove_file(path) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// A version that was just installed and has not yet connected to the host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Trial {
    /// The version on trial.
    pub version: String,
    /// SHA-256 of its binary (hex), so a rebuilt binary with the same version is not
    /// blocked by this one failing.
    #[serde(default)]
    pub sha256: Option<String>,
    /// When it was installed, seconds since the Unix epoch.
    pub started_at: u64,
    /// Starts that ended before it became healthy.
    pub attempts: u32,
}

/// A version that failed its trial.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BadVersion {
    /// The version.
    pub version: String,
    /// SHA-256 of the binary that failed, when known.
    #[serde(default)]
    pub sha256: Option<String>,
    /// Why it was rolled back.
    pub reason: String,
    /// When, seconds since the Unix epoch.
    pub at: u64,
}

/// Seconds since the Unix epoch.
pub fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(value)?)?;
    std::fs::rename(tmp, path)
}

impl Layout {
    /// The trial in progress, if any. An unreadable file counts as none.
    pub fn read_trial(&self) -> Option<Trial> {
        let text = std::fs::read_to_string(self.trial_path()).ok()?;
        serde_json::from_str(&text).ok()
    }

    /// Save the trial.
    pub fn write_trial(&self, trial: &Trial) -> io::Result<()> {
        write_json(&self.trial_path(), trial)
    }

    /// End the trial.
    pub fn remove_trial(&self) -> io::Result<()> {
        remove_if_present(&self.trial_path())
    }

    /// Versions that failed. An unreadable file counts as empty.
    pub fn read_bad(&self) -> Vec<BadVersion> {
        std::fs::read_to_string(self.bad_path())
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    /// Did `version` fail before? With `sha256`, only that exact binary counts.
    pub fn is_bad(&self, version: &str, sha256: Option<&str>) -> bool {
        self.read_bad().iter().any(|bad| {
            bad.version == version
                && match (sha256, bad.sha256.as_deref()) {
                    (Some(want), Some(was)) => want.eq_ignore_ascii_case(was),
                    _ => true,
                }
        })
    }

    /// Record that `version` failed.
    pub fn mark_bad(&self, version: &str, sha256: Option<String>, reason: &str) -> io::Result<()> {
        let mut bad = self.read_bad();
        bad.retain(|b| b.version != version);
        bad.push(BadVersion {
            version: version.to_owned(),
            sha256,
            reason: reason.to_owned(),
            at: now_unix(),
        });
        write_json(&self.bad_path(), &bad)
    }

    /// Forget that `version` failed (a different binary with that version was installed).
    pub fn forget_bad(&self, version: &str) -> io::Result<()> {
        let mut bad = self.read_bad();
        let before = bad.len();
        bad.retain(|b| b.version != version);
        if bad.len() == before {
            return Ok(());
        }
        write_json(&self.bad_path(), &bad)
    }
}

#[cfg(test)]
mod tests {
    #![expect(
        clippy::unwrap_used,
        reason = "Test assertions fail immediately on malformed setup"
    )]
    use super::*;
    use std::time::Duration;

    #[test]
    fn versions_must_be_one_safe_path_component() {
        for good in ["0.3.0", "1.2.3-nightly.20261019+abc123", "v2_rc1", "a"] {
            assert_eq!(valid_version(good), Ok(()), "{good}");
        }
        let long = "1".repeat(MAX_VERSION_LEN + 1);
        for bad in [
            "", "..", ".", ".hidden", "-rf", "1/2", "../x", "1 2", "1\0", "é1", &long,
        ] {
            assert!(valid_version(bad).is_err(), "{bad:?} should be refused");
        }
        assert_eq!(valid_version(&"1".repeat(MAX_VERSION_LEN)), Ok(()));
    }

    #[test]
    fn prune_keeps_the_newest_and_never_current_or_previous() {
        let t = |s| UNIX_EPOCH + Duration::from_secs(s);
        let found: Vec<(String, SystemTime)> = [
            ("old", 1),
            ("older", 0),
            ("mid", 5),
            ("new", 9),
            ("newer", 10),
            ("newest", 11),
        ]
        .into_iter()
        .map(|(n, s)| (n.to_owned(), t(s)))
        .collect();
        let mut doomed = prune_plan(&found, 3, &["older"]);
        doomed.sort();
        assert_eq!(doomed, ["mid", "old"]);
        assert!(prune_plan(&found[..2], 3, &[]).is_empty());
    }

    #[test]
    fn switching_moves_current_to_previous_through_atomic_renames() {
        let dir = tempfile::tempdir().unwrap();
        let layout = Layout::at(dir.path());
        for v in ["1.0.0", "1.1.0"] {
            let bin = layout.version_binary(v);
            std::fs::create_dir_all(bin.parent().unwrap()).unwrap();
            std::fs::write(&bin, "#!/bin/sh\n").unwrap();
        }
        assert_eq!(layout.switch_to("1.0.0").unwrap(), None);
        assert_eq!(layout.current_version().as_deref(), Some("1.0.0"));
        assert_eq!(layout.previous_version(), None);

        assert_eq!(layout.switch_to("1.1.0").unwrap().as_deref(), Some("1.0.0"));
        assert_eq!(layout.current_version().as_deref(), Some("1.1.0"));
        assert_eq!(layout.previous_version().as_deref(), Some("1.0.0"));
        assert_eq!(
            std::fs::read_link(layout.current_link()).unwrap(),
            Path::new("versions/1.1.0")
        );

        // Reinstalling the same version leaves `previous` alone.
        layout.switch_to("1.1.0").unwrap();
        assert_eq!(layout.previous_version().as_deref(), Some("1.0.0"));
        // No temporary links are left behind.
        let names: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".tmp"))
            .collect();
        assert!(names.is_empty(), "{names:?}");
    }

    #[test]
    fn bad_versions_match_by_hash_when_both_sides_know_it() {
        let dir = tempfile::tempdir().unwrap();
        let layout = Layout::at(dir.path());
        layout
            .mark_bad("2.0.0", Some("AA".into()), "never connected")
            .unwrap();
        assert!(layout.is_bad("2.0.0", None));
        assert!(layout.is_bad("2.0.0", Some("aa")));
        assert!(!layout.is_bad("2.0.0", Some("bb")));
        assert!(!layout.is_bad("2.0.1", None));
        layout.forget_bad("2.0.0").unwrap();
        assert!(!layout.is_bad("2.0.0", None));
    }
}
