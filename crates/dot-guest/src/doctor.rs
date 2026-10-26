//! `dot-guest doctor`: a checklist of everything the agent needs, each with a fix.
use crate::config::{self, Config};
use crate::launchd;
use crate::ops::info::{self, Status};
use crate::script::{self, sources};
use dot_wire::Permissions;
use serde::Deserialize;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::time::Instant;

/// A status file older than this is ignored.
const STATUS_FRESH: Duration = Duration::from_secs(15 * 60);

/// One checklist line.
#[derive(Debug)]
pub struct Check {
    /// Passed, failed, or could not tell.
    pub state: CheckState,
    /// What was checked and what was found.
    pub label: String,
    /// What to do when it failed.
    pub fix: Option<String>,
}

/// Outcome of a check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckState {
    /// Fine.
    Ok,
    /// Needs fixing.
    Fail,
    /// Could not tell.
    Unknown,
}

impl Check {
    fn ok(label: impl Into<String>) -> Self {
        Self {
            state: CheckState::Ok,
            label: label.into(),
            fix: None,
        }
    }
    fn fail(label: impl Into<String>, fix: impl Into<String>) -> Self {
        Self {
            state: CheckState::Fail,
            label: label.into(),
            fix: Some(fix.into()),
        }
    }
    fn unknown(label: impl Into<String>, fix: impl Into<String>) -> Self {
        Self {
            state: CheckState::Unknown,
            label: label.into(),
            fix: Some(fix.into()),
        }
    }
    fn from(value: Option<bool>, label: &str, fix: &str) -> Self {
        match value {
            Some(true) => Self::ok(format!("{label}: yes")),
            Some(false) => Self::fail(format!("{label}: no"), fix),
            None => Self::unknown(format!("{label}: could not tell"), fix),
        }
    }
}

#[derive(Debug, Default, Deserialize)]
struct LocalInfo {
    accessibility: Option<bool>,
    screen_recording: Option<bool>,
    default_browser: Option<String>,
}

fn fresh_status(home: &Path, agent_running: bool) -> Option<Status> {
    let status = info::read_status(home)?;
    let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
    (agent_running && now.saturating_sub(status.updated_unix) < STATUS_FRESH.as_secs())
        .then_some(status)
}

/// Run every check.
pub async fn checks(home: &Path) -> Vec<Check> {
    let mut out = Vec::new();
    let uid = rustix::process::getuid().as_raw();

    let config = match config::find_volume_config()
        .map(|p| (Config::load(&p), p))
        .or_else(|| {
            let p = config::user_config_path(home);
            p.is_file().then(|| (Config::load(&p), p))
        }) {
        Some((Ok(c), path)) => {
            out.push(Check::ok(format!(
                "Configuration found: {} (instance {}, port {})",
                path.display(),
                c.instance_id,
                c.port
            )));
            Some(c)
        }
        Some((Err(e), path)) => {
            out.push(Check::fail(
                format!("Configuration at {} is invalid: {e:#}", path.display()),
                "Re-run `dot-guest pair` with a fresh pairing string from the Dot Home portal.",
            ));
            None
        }
        None => {
            out.push(Check::fail(
                "No configuration found",
                "On a Dot Home VM the server attaches a DOTCONFIG disk; on another Mac run `dot-guest pair <pairing string>` from the portal.",
            ));
            None
        }
    };

    let agent = launchd::agent_state(uid);
    let running = agent.as_deref().is_some_and(|s| s.starts_with("running"));
    match &agent {
        Some(state) => out.push(Check::ok(format!(
            "LaunchAgent {}: {state}",
            launchd::LABEL
        ))),
        None => out.push(Check::fail(
            format!("LaunchAgent {} is not loaded", launchd::LABEL),
            "Run `dot-guest install` while logged in to the Mac's desktop.",
        )),
    }

    let plist = std::fs::read_to_string(launchd::Paths::new(home).plist).ok();
    out.push(supervisor_check(
        plist.as_deref(),
        &crate::layout::Layout::new(home).launcher(),
    ));

    let port = config.as_ref().map_or(dot_wire::GUEST_PORT, |c| c.port);
    match std::net::TcpListener::bind(("0.0.0.0", port)) {
        Ok(_) => out.push(if running {
            Check::fail(
                format!("Port {port} is free, but the agent should be listening on it"),
                format!("Check {} for errors.", launchd::Paths::new(home).log_file.display()),
            )
        } else {
            Check::ok(format!("Port {port} is free"))
        }),
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse && running => {
            out.push(Check::ok(format!("Port {port} is in use by the running agent")));
        }
        Err(e) => out.push(Check::fail(
            format!("Port {port} is not available: {e}"),
            format!("Quit whatever else listens on port {port} (`lsof -nP -iTCP:{port}`), or choose another port when pairing."),
        )),
    }

    let status = fresh_status(home, running);
    let (perms, browser, probes, source) = match &status {
        Some(s) => (
            s.permissions.clone(),
            s.default_browser.clone(),
            Some(s.probes),
            "as the agent sees them",
        ),
        None => {
            let deadline = Instant::now() + Duration::from_secs(10);
            let local: LocalInfo = script::jxa(deadline, sources::INFO, &serde_json::json!({}))
                .await
                .unwrap_or_default();
            (
                Permissions {
                    accessibility: local.accessibility,
                    screen_recording: local.screen_recording,
                    full_disk_access: info::full_disk_access(home),
                },
                local.default_browser,
                None,
                "for this Terminal, not the agent: start the agent for an exact answer",
            )
        }
    };
    println!("Permissions are checked {source}.\n");
    out.push(Check::from(
        perms.accessibility,
        "Accessibility (UI tree, clicks, typing)",
        crate::error::HINT_ACCESSIBILITY,
    ));
    out.push(Check::from(
        perms.screen_recording,
        "Screen Recording (screenshots)",
        crate::error::HINT_SCREEN_RECORDING,
    ));
    out.push(Check::from(
        perms.full_disk_access,
        "Full Disk Access (Mail, Messages, Safari data)",
        crate::error::HINT_FULL_DISK,
    ));
    out.push(Check::from(
        probes.and_then(|p| p.automation_system_events),
        "Automation for System Events",
        "Open System Settings > Privacy & Security > Automation, find dot-guest and turn on System Events. If dot-guest is not listed, restart the agent (`dot-guest install`) to make macOS ask.",
    ));
    let safari_js = match probes.and_then(|p| p.safari_javascript) {
        Some(v) => Some(v),
        None => safari_js_local().await,
    };
    out.push(Check::from(
        safari_js,
        "Safari allows JavaScript from Apple Events",
        crate::error::HINT_SAFARI_JS,
    ));
    out.push(match browser.as_deref() {
        Some(id) if id.eq_ignore_ascii_case("com.apple.safari") => {
            Check::ok("Safari is the default browser")
        }
        Some(other) => Check::fail(
            format!("The default browser is {other}, not Safari"),
            "Open System Settings > Desktop & Dock > Default web browser and choose Safari.",
        ),
        None => Check::unknown(
            "Default browser: could not tell",
            "Open System Settings > Desktop & Dock > Default web browser and choose Safari.",
        ),
    });
    // Homebrew is how apps are added on Apple silicon, so it's required there; Intel Macs
    // (Homebrew no longer supports them) use download links. The App Store is optional.
    for (tool, required, fix) in [
        (
            "brew",
            std::env::consts::ARCH == "aarch64",
            "Run `dot-guest prepare-image`, or install Homebrew from https://brew.sh.",
        ),
        (
            "mas",
            false,
            "Run `brew install mas` and sign in to the App Store to allow App Store installs (optional).",
        ),
    ] {
        let found = ["/opt/homebrew/bin", "/usr/local/bin"]
            .iter()
            .map(|d| Path::new(d).join(tool))
            .find(|p| p.is_file());
        out.push(match found {
            Some(p) => Check::ok(format!("{tool} found at {}", p.display())),
            None if required => Check::fail(format!("{tool} is not installed"), fix),
            None => Check::unknown(format!("{tool} is not installed"), fix),
        });
    }
    out
}

async fn safari_js_local() -> Option<bool> {
    let out =
        crate::proc::Command::new("/usr/bin/defaults", Instant::now() + Duration::from_secs(5))
            .args(["read", "com.apple.Safari", "AllowJavaScriptFromAppleEvents"])
            .run()
            .await
            .ok()?;
    match (out.success(), out.stdout_text().trim()) {
        (true, "1") => Some(true),
        (true, "0") => Some(false),
        _ => None,
    }
}

/// Does the LaunchAgent run `launcher/dot-guest supervise`? Without it updates can't
/// restart the agent, and updated binaries would lose the privacy permissions.
pub fn supervisor_check(plist: Option<&str>, launcher: &Path) -> Check {
    const FIX: &str = "Run `dot-guest install` again so the LaunchAgent starts the launcher, which lets the agent update itself.";
    let Some(args) = plist.and_then(launchd::program_arguments) else {
        return Check::fail(
            "The LaunchAgent's property list is missing or unreadable",
            FIX,
        );
    };
    let launcher = launcher.to_string_lossy();
    if args == [launcher.as_ref(), "supervise"] {
        Check::ok(format!(
            "LaunchAgent runs {launcher} supervise (self-update on)"
        ))
    } else {
        Check::fail(
            format!(
                "LaunchAgent runs `{}`, not {launcher} supervise (self-update off)",
                args.join(" ")
            ),
            FIX,
        )
    }
}

/// Print the checklist; returns whether everything required passed.
pub async fn run(home: &Path) -> bool {
    println!("dot-guest {} doctor\n", crate::VERSION);
    let checks = checks(home).await;
    let mut all_ok = true;
    for check in &checks {
        let mark = match check.state {
            CheckState::Ok => "[ok]",
            CheckState::Fail => {
                all_ok = false;
                "[!!]"
            }
            CheckState::Unknown => "[??]",
        };
        println!("{mark} {}", check.label);
        if let Some(fix) = &check.fix {
            println!("     {fix}");
        }
    }
    println!();
    if all_ok {
        println!("Everything required is in place.");
    } else {
        println!("Fix the [!!] items above, then run `dot-guest doctor` again.");
    }
    all_ok
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_launch_agent_must_run_the_launcher_supervisor() {
        let home = Path::new("/Users/a b");
        let paths = launchd::Paths::new(home);
        let launcher = crate::layout::Layout::new(home).launcher();
        let current = launchd::plist(&launcher, &paths);
        assert_eq!(
            supervisor_check(Some(&current), &launcher).state,
            CheckState::Ok
        );

        // What older installs wrote: the binary itself, started with `run`.
        let old = current.replace(
            &format!(
                "<string>{}</string>\n        <string>supervise</string>",
                launcher.display()
            ),
            "<string>/usr/local/bin/dot-guest</string>\n        <string>run</string>",
        );
        let check = supervisor_check(Some(&old), &launcher);
        assert_eq!(check.state, CheckState::Fail);
        assert!(
            check.label.contains("/usr/local/bin/dot-guest run"),
            "{}",
            check.label
        );
        assert!(check.fix.unwrap_or_default().contains("dot-guest install"));

        assert_eq!(supervisor_check(None, &launcher).state, CheckState::Fail);
    }
}
