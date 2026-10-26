//! [`GuestInfo`]: what the agent knows about its Mac, plus the status file for `doctor`.
use super::{CAPABILITIES, Ctx, State, UPDATE_CAPABILITY};
use crate::proc::Command;
use crate::script::{self, sources};
use dot_wire::{Display, GuestInfo, Permissions};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::OnceCell;
use tokio::time::Instant;

/// How long a gathered [`GuestInfo`] is reused for handshakes.
pub const CACHE_FOR: Duration = Duration::from_secs(10);

/// Cached info and the slower one-time probes.
#[derive(Debug, Default)]
pub struct InfoCache {
    latest: Mutex<Option<(Instant, GuestInfo)>>,
    os: OnceCell<(String, String)>,
    probes: Mutex<Probes>,
}

/// Results of probes that may show a privacy prompt, run once at startup.
#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct Probes {
    /// Automation permission for System Events.
    pub automation_system_events: Option<bool>,
    /// Safari's "Allow JavaScript from Apple Events".
    pub safari_javascript: Option<bool>,
}

/// What the daemon last saw, for `dot-guest doctor`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Status {
    /// Seconds since the Unix epoch.
    pub updated_unix: u64,
    /// Agent version.
    pub version: String,
    /// Daemon process id.
    pub pid: u32,
    /// Listening port.
    pub port: u16,
    /// Privacy permissions as the daemon sees them.
    pub permissions: Permissions,
    /// Default browser bundle id.
    pub default_browser: Option<String>,
    /// One-time probes.
    pub probes: Probes,
}

#[derive(Debug, Default, Deserialize)]
struct ScriptInfo {
    #[serde(default)]
    displays: Vec<Display>,
    accessibility: Option<bool>,
    screen_recording: Option<bool>,
    frontmost_app: Option<String>,
    default_browser: Option<String>,
    #[serde(default)]
    uptime_s: u64,
}

/// Gather now, update the cache and the status file.
pub async fn fresh(ctx: &Ctx) -> GuestInfo {
    let info = gather(&ctx.state, ctx.deadline).await;
    store(&ctx.state, &info);
    info
}

/// Reuse a recent result, or gather within `budget`.
pub async fn cached(state: &State, budget: Duration) -> GuestInfo {
    if let Some((at, info)) = state
        .info
        .latest
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_ref()
        && at.elapsed() < CACHE_FOR
    {
        return info.clone();
    }
    let info = gather(state, Instant::now() + budget).await;
    store(state, &info);
    info
}

fn store(state: &State, info: &GuestInfo) {
    *state
        .info
        .latest
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some((Instant::now(), info.clone()));
    let probes = *state
        .info
        .probes
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let status = Status {
        updated_unix: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
        version: crate::VERSION.to_owned(),
        pid: std::process::id(),
        port: state.port,
        permissions: info.permissions.clone(),
        default_browser: info.default_browser.clone(),
        probes,
    };
    if let Err(e) = write_status(&state.home, &status) {
        tracing::debug!(error = %e, "could not write the status file");
    }
}

fn write_status(home: &Path, status: &Status) -> std::io::Result<()> {
    let path = crate::config::status_path(home);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(status)?)?;
    std::fs::rename(tmp, path)
}

/// Read the status file the daemon writes.
pub fn read_status(home: &Path) -> Option<Status> {
    let text = std::fs::read_to_string(crate::config::status_path(home)).ok()?;
    serde_json::from_str(&text).ok()
}

async fn gather(state: &State, deadline: Instant) -> GuestInfo {
    let no_input = serde_json::json!({});
    let (os, hostname, script) = tokio::join!(
        os_version(state, deadline),
        hostname(deadline),
        script::jxa::<_, ScriptInfo>(deadline, sources::INFO, &no_input),
    );
    let script = script.unwrap_or_else(|e| {
        tracing::warn!(error = %e, "could not read displays and permissions");
        ScriptInfo::default()
    });
    GuestInfo {
        os_version: os.0,
        os_build: os.1,
        hostname,
        user: user_name(),
        uptime_s: script.uptime_s,
        displays: script.displays,
        permissions: Permissions {
            accessibility: script.accessibility,
            screen_recording: script.screen_recording,
            full_disk_access: full_disk_access(&state.home),
        },
        disk_free_bytes: disk_free(),
        frontmost_app: script.frontmost_app,
        default_browser: script.default_browser,
        capabilities: CAPABILITIES
            .iter()
            .copied()
            .chain(state.supervised.then_some(UPDATE_CAPABILITY))
            .map(str::to_owned)
            .collect(),
    }
}

async fn os_version(state: &State, deadline: Instant) -> (String, String) {
    state
        .info
        .os
        .get_or_init(|| async {
            let out = Command::new("/usr/bin/sw_vers", deadline).run().await;
            out.map(|o| parse_sw_vers(&o.stdout_text()))
                .unwrap_or_default()
        })
        .await
        .clone()
}

/// `ProductVersion:\t26.0` lines from `sw_vers`.
fn parse_sw_vers(text: &str) -> (String, String) {
    let field = |key: &str| {
        text.lines()
            .find_map(|l| l.strip_prefix(key))
            .map(|v| v.trim_start_matches(':').trim().to_owned())
            .unwrap_or_default()
    };
    (field("ProductVersion"), field("BuildVersion"))
}

async fn hostname(deadline: Instant) -> String {
    let out = Command::new("/usr/sbin/scutil", deadline)
        .args(["--get", "ComputerName"])
        .run()
        .await;
    match out {
        Ok(o) if o.success() => o.stdout_text().trim().to_owned(),
        _ => std::env::var("HOSTNAME").unwrap_or_else(|_| "Mac".to_owned()),
    }
}

fn user_name() -> String {
    std::env::var("USER")
        .ok()
        .filter(|u| !u.is_empty())
        .unwrap_or_else(|| rustix::process::getuid().as_raw().to_string())
}

fn disk_free() -> u64 {
    rustix::fs::statvfs("/")
        .map(|s| s.f_bavail.saturating_mul(s.f_frsize))
        .unwrap_or(0)
}

/// The TCC database is readable only with Full Disk Access, and exists on every Mac.
pub fn full_disk_access(home: &Path) -> Option<bool> {
    let probes = [
        home.join("Library/Application Support/com.apple.TCC/TCC.db"),
        home.join("Library/Safari/Bookmarks.plist"),
        home.join("Library/Mail"),
    ];
    for path in probes {
        let result = if path.is_dir() {
            std::fs::read_dir(&path).map(|_| ())
        } else {
            std::fs::File::open(&path).map(|_| ())
        };
        match result {
            Ok(()) => return Some(true),
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => return Some(false),
            Err(_) => continue,
        }
    }
    None
}

/// One-time startup probes. Sending an Apple event to System Events shows the
/// Automation prompt once, which is what the golden-image setup wants.
pub async fn run_probes(state: &State) {
    let deadline = Instant::now() + Duration::from_secs(20);
    let automation = Command::new(script::OSASCRIPT, deadline)
        .args([
            "-e",
            "tell application \"System Events\" to count processes",
        ])
        .run()
        .await
        .ok()
        .and_then(|o| {
            if o.success() {
                Some(true)
            } else if o.stderr_text().contains("-1743") {
                Some(false)
            } else {
                None
            }
        });
    let safari = Command::new("/usr/bin/defaults", deadline)
        .args(["read", "com.apple.Safari", "AllowJavaScriptFromAppleEvents"])
        .run()
        .await
        .ok()
        .and_then(|o| match (o.success(), o.stdout_text().trim()) {
            (true, "1") => Some(true),
            (true, "0") => Some(false),
            _ => None,
        });
    *state
        .info
        .probes
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Probes {
        automation_system_events: automation,
        safari_javascript: safari,
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sw_vers_output_is_parsed() {
        let text = "ProductName:\t\tmacOS\nProductVersion:\t\t26.0.1\nBuildVersion:\t\t25A362\n";
        assert_eq!(
            parse_sw_vers(text),
            ("26.0.1".to_owned(), "25A362".to_owned())
        );
    }
}
