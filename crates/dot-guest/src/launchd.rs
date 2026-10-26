//! Installing and removing the LaunchAgent that runs `dot-guest supervise` at login.
use crate::layout::{self, Layout};
use crate::util::xml_escape;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// The LaunchAgent label.
pub const LABEL: &str = "dev.dot.guest";

/// Paths the agent installs to, relative to a home folder.
#[derive(Debug, Clone)]
pub struct Paths {
    /// `~/Library/LaunchAgents/dev.dot.guest.plist`.
    pub plist: PathBuf,
    /// `~/Library/Logs/Dot Guest`.
    pub log_dir: PathBuf,
    /// `~/Library/Logs/Dot Guest/dot-guest.log`.
    pub log_file: PathBuf,
}

impl Paths {
    /// Paths under `home`.
    pub fn new(home: &Path) -> Self {
        let log_dir = home.join("Library/Logs/Dot Guest");
        Self {
            plist: home
                .join("Library/LaunchAgents")
                .join(format!("{LABEL}.plist")),
            log_file: log_dir.join("dot-guest.log"),
            log_dir,
        }
    }
}

/// The LaunchAgent property list that runs `launcher supervise`.
pub fn plist(launcher: &Path, paths: &Paths) -> String {
    let exe = xml_escape(&launcher.to_string_lossy());
    let log = xml_escape(&paths.log_file.to_string_lossy());
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{LABEL}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{exe}</string>
        <string>supervise</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>ThrottleInterval</key>
    <integer>10</integer>
    <key>ProcessType</key>
    <string>Interactive</string>
    <key>LimitLoadToSessionType</key>
    <string>Aqua</string>
    <key>EnvironmentVariables</key>
    <dict>
        <key>PATH</key>
        <string>/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin</string>
        <key>LANG</key>
        <string>en_US.UTF-8</string>
    </dict>
    <key>StandardOutPath</key>
    <string>{log}</string>
    <key>StandardErrorPath</key>
    <string>{log}</string>
</dict>
</plist>
"#
    )
}

/// `ProgramArguments` of a LaunchAgent property list.
pub fn program_arguments(plist: &str) -> Option<Vec<String>> {
    let doc = roxmltree::Document::parse_with_options(
        plist,
        roxmltree::ParsingOptions {
            allow_dtd: true,
            ..Default::default()
        },
    )
    .ok()?;
    let key = doc
        .descendants()
        .find(|n| n.has_tag_name("key") && n.text() == Some("ProgramArguments"))?;
    let array = key
        .next_siblings()
        .skip(1)
        .find(roxmltree::Node::is_element)?;
    Some(
        array
            .children()
            .filter(|n| n.has_tag_name("string"))
            .map(|n| n.text().unwrap_or_default().to_owned())
            .collect(),
    )
}

/// One step of an install or uninstall.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// Create a folder (and parents).
    CreateDir(PathBuf),
    /// Write a file.
    WriteFile(PathBuf, String),
    /// Remove a file if present.
    RemoveFile(PathBuf),
    /// Remove a folder and everything in it, if present.
    RemoveDir(PathBuf),
    /// Copy an executable (mode 0755) through a temporary file and a rename, so copying
    /// a binary over itself or over a running one is safe.
    CopyExecutable {
        /// Source.
        from: PathBuf,
        /// Destination.
        to: PathBuf,
    },
    /// Point a symlink at `target`, atomically.
    Link {
        /// The symlink.
        link: PathBuf,
        /// Where it points (relative to the link's folder).
        target: PathBuf,
    },
    /// Run `launchctl` with these arguments. `tolerate_failure` for steps that fail
    /// harmlessly when the agent isn't loaded.
    Launchctl {
        /// Arguments.
        args: Vec<String>,
        /// Ignore a nonzero exit.
        tolerate_failure: bool,
    },
}

fn domain(uid: u32) -> String {
    format!("gui/{uid}")
}

/// Steps to put `exe` (whose version is `version`) in place as both the launcher and
/// the current version.
pub fn layout_plan(exe: &Path, version: &str, layout: &Layout) -> Vec<Step> {
    let launcher = layout.launcher();
    let binary = layout.version_binary(version);
    let mut steps = Vec::new();
    for to in [&launcher, &binary] {
        if let Some(dir) = to.parent() {
            steps.push(Step::CreateDir(dir.to_path_buf()));
        }
        steps.push(Step::CopyExecutable {
            from: exe.to_path_buf(),
            to: to.clone(),
        });
    }
    steps.push(Step::Link {
        link: layout.current_link(),
        target: Path::new("versions").join(version),
    });
    // A manual install is trusted: nothing is on trial any more.
    steps.push(Step::RemoveFile(layout.trial_path()));
    steps
}

/// Steps to install (or reinstall, which restarts a running agent): copy `exe` into
/// the layout and point the LaunchAgent at the launcher.
pub fn install_plan(
    exe: &Path,
    version: &str,
    layout: &Layout,
    paths: &Paths,
    uid: u32,
) -> Vec<Step> {
    let mut steps = layout_plan(exe, version, layout);
    if let Some(dir) = paths.plist.parent() {
        steps.push(Step::CreateDir(dir.to_path_buf()));
    }
    steps.push(Step::CreateDir(paths.log_dir.clone()));
    steps.push(Step::WriteFile(
        paths.plist.clone(),
        plist(&layout.launcher(), paths),
    ));
    steps.extend(unload_steps(paths, uid));
    steps.push(Step::Launchctl {
        args: vec!["enable".into(), format!("{}/{LABEL}", domain(uid))],
        tolerate_failure: true,
    });
    steps.push(Step::Launchctl {
        args: vec![
            "bootstrap".into(),
            domain(uid),
            paths.plist.to_string_lossy().into_owned(),
        ],
        tolerate_failure: false,
    });
    steps
}

fn unload_steps(_paths: &Paths, uid: u32) -> Vec<Step> {
    vec![Step::Launchctl {
        args: vec!["bootout".into(), format!("{}/{LABEL}", domain(uid))],
        tolerate_failure: true,
    }]
}

/// Steps to uninstall. `purge` also removes the support folder: every installed
/// version and the paired configuration.
pub fn uninstall_plan(paths: &Paths, layout: &Layout, purge: bool, uid: u32) -> Vec<Step> {
    let mut steps = unload_steps(paths, uid);
    steps.push(Step::RemoveFile(paths.plist.clone()));
    if purge {
        steps.push(Step::RemoveDir(layout.root.clone()));
    }
    steps
}

/// Print or carry out steps.
pub fn execute(steps: &[Step], dry_run: bool) -> Result<()> {
    for step in steps {
        match step {
            Step::CreateDir(dir) => {
                println!("create folder  {}", dir.display());
                if !dry_run {
                    std::fs::create_dir_all(dir)
                        .with_context(|| format!("could not create {}", dir.display()))?;
                }
            }
            Step::WriteFile(path, body) => {
                println!("write          {}", path.display());
                if dry_run {
                    println!("{body}");
                } else {
                    std::fs::write(path, body)
                        .with_context(|| format!("could not write {}", path.display()))?;
                }
            }
            Step::RemoveFile(path) => {
                println!("remove         {}", path.display());
                if !dry_run {
                    match std::fs::remove_file(path) {
                        Ok(()) => {}
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                        Err(e) => {
                            return Err(e)
                                .with_context(|| format!("could not remove {}", path.display()));
                        }
                    }
                }
            }
            Step::RemoveDir(dir) => {
                println!("remove folder  {}", dir.display());
                if !dry_run {
                    match std::fs::remove_dir_all(dir) {
                        Ok(()) => {}
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                        Err(e) => {
                            return Err(e)
                                .with_context(|| format!("could not remove {}", dir.display()));
                        }
                    }
                }
            }
            Step::CopyExecutable { from, to } => {
                println!("copy           {} -> {}", from.display(), to.display());
                if !dry_run {
                    copy_executable(from, to).with_context(|| {
                        format!("could not copy {} to {}", from.display(), to.display())
                    })?;
                }
            }
            Step::Link { link, target } => {
                println!("link           {} -> {}", link.display(), target.display());
                if !dry_run {
                    layout::point(link, target).with_context(|| {
                        format!("could not point {} at {}", link.display(), target.display())
                    })?;
                }
            }
            Step::Launchctl {
                args,
                tolerate_failure,
            } => {
                println!("launchctl      {}", args.join(" "));
                if dry_run {
                    continue;
                }
                let out = std::process::Command::new("/bin/launchctl")
                    .args(args)
                    .output()
                    .context("could not run launchctl")?;
                if !out.status.success() && !tolerate_failure {
                    anyhow::bail!(
                        "launchctl {} failed: {}{}",
                        args.join(" "),
                        String::from_utf8_lossy(&out.stderr).trim(),
                        if args.first().is_some_and(|a| a == "bootstrap") {
                            ". Run this from the logged-in user's session (not over SSH as another user)."
                        } else {
                            ""
                        }
                    );
                }
            }
        }
    }
    Ok(())
}

fn copy_executable(from: &Path, to: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let name = to.file_name().map(|n| n.to_string_lossy().into_owned());
    let tmp = to.with_file_name(format!(
        ".{}.{}.tmp",
        name.as_deref().unwrap_or("copy"),
        std::process::id()
    ));
    std::fs::copy(from, &tmp)?;
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))?;
    std::fs::rename(&tmp, to).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// The running binary. `install` copies it into the support folder, so it may live
/// anywhere, even a temporary download.
pub fn current_exe() -> Result<PathBuf> {
    let exe = std::env::current_exe().context("could not find the dot-guest binary's path")?;
    Ok(std::fs::canonicalize(&exe).unwrap_or(exe))
}

/// Is the LaunchAgent loaded, and with which pid?
pub fn agent_state(uid: u32) -> Option<String> {
    let out = std::process::Command::new("/bin/launchctl")
        .args(["print", &format!("{}/{LABEL}", domain(uid))])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let field = |key: &str| {
        text.lines()
            .find_map(|l| l.trim().strip_prefix(key))
            .map(|v| v.trim().to_owned())
    };
    Some(match (field("state = "), field("pid = ")) {
        (Some(state), Some(pid)) => format!("{state} (pid {pid})"),
        (Some(state), None) => state,
        _ => "loaded".to_owned(),
    })
}

#[cfg(test)]
mod tests {
    #![expect(
        clippy::unwrap_used,
        reason = "Test assertions fail immediately on malformed setup"
    )]
    use super::*;

    #[test]
    fn plist_runs_the_launcher_supervisor_escapes_paths_and_carries_the_session_keys() {
        let paths = Paths::new(Path::new("/Users/a&b"));
        let body = plist(Path::new("/opt/dot <guest>/dot-guest"), &paths);
        assert!(body.contains(
            "<string>/opt/dot &lt;guest&gt;/dot-guest</string>\n        <string>supervise</string>"
        ));
        assert!(body.contains("/Users/a&amp;b/Library/Logs/Dot Guest/dot-guest.log"));
        for key in [
            "RunAtLoad",
            "KeepAlive",
            "LimitLoadToSessionType",
            "<string>Aqua</string>",
            "<string>Interactive</string>",
        ] {
            assert!(body.contains(key), "{key}");
        }
        roxmltree::Document::parse_with_options(
            &body,
            roxmltree::ParsingOptions {
                allow_dtd: true,
                ..Default::default()
            },
        )
        .unwrap();
    }

    #[test]
    fn install_reloads_and_bootstraps_into_the_gui_domain() {
        let home = Path::new("/Users/dot");
        let paths = Paths::new(home);
        let layout = Layout::new(home);
        let steps = install_plan(Path::new("/tmp/dl"), "1.0.0", &layout, &paths, 501);
        let launchctl: Vec<String> = steps
            .iter()
            .filter_map(|s| match s {
                Step::Launchctl { args, .. } => Some(args.join(" ")),
                _ => None,
            })
            .collect();
        assert_eq!(
            launchctl,
            [
                "bootout gui/501/dev.dot.guest",
                "enable gui/501/dev.dot.guest",
                "bootstrap gui/501 /Users/dot/Library/LaunchAgents/dev.dot.guest.plist",
            ]
        );
        let uninstall = uninstall_plan(&paths, &layout, false, 501);
        assert_eq!(
            uninstall.last(),
            Some(&Step::RemoveFile(paths.plist.clone()))
        );
        let purge = uninstall_plan(&paths, &layout, true, 501);
        assert_eq!(purge.last(), Some(&Step::RemoveDir(layout.root.clone())));
    }

    #[test]
    fn layout_install_is_idempotent_even_when_run_from_the_launcher_itself() {
        let home = tempfile::tempdir().unwrap();
        let layout = Layout::new(home.path());
        let download = home.path().join("download");
        std::fs::write(&download, "#!/bin/sh\necho v1\n").unwrap();
        layout
            .write_trial(&layout::Trial {
                version: "0.9.0".into(),
                sha256: None,
                started_at: 0,
                attempts: 1,
            })
            .unwrap();

        execute(&layout_plan(&download, "1.0.0", &layout), false).unwrap();
        // Again, now copying the launcher onto itself.
        execute(&layout_plan(&layout.launcher(), "1.0.0", &layout), false).unwrap();

        for bin in [layout.launcher(), layout.version_binary("1.0.0")] {
            assert_eq!(
                std::fs::read_to_string(&bin).unwrap(),
                "#!/bin/sh\necho v1\n"
            );
            let mode = std::os::unix::fs::PermissionsExt::mode(
                &std::fs::metadata(&bin).unwrap().permissions(),
            );
            assert_eq!(mode & 0o777, 0o755, "{}", bin.display());
        }
        assert_eq!(layout.current_version().as_deref(), Some("1.0.0"));
        assert_eq!(layout.read_trial(), None);
    }

    #[test]
    fn dry_run_touches_nothing() {
        let home = tempfile::tempdir().unwrap();
        let paths = Paths::new(home.path());
        let layout = Layout::new(home.path());
        // Not a real binary, so nothing could start even if a step ran for real.
        let steps = install_plan(
            Path::new("/nonexistent/dot-guest"),
            "1.0.0",
            &layout,
            &paths,
            u32::MAX,
        );
        execute(&steps, true).unwrap();
        execute(&uninstall_plan(&paths, &layout, true, u32::MAX), true).unwrap();
        assert_eq!(std::fs::read_dir(home.path()).unwrap().count(), 0);
    }
}
