//! `dot-guest prepare-image`: one-time setup of the golden macOS image, run by the admin
//! inside the VM. Every step is safe to repeat.
use std::io::{BufRead, IsTerminal, Write};
use std::path::Path;
use std::process::Command;

const SAFARI_DEFAULT_SCRIPT: &str = "ObjC.import('CoreServices'); \
    [$.LSSetDefaultHandlerForURLScheme($('http'), $('com.apple.Safari')), \
     $.LSSetDefaultHandlerForURLScheme($('https'), $('com.apple.Safari'))].join(',')";

/// Installs Homebrew unless it's there. Homebrew also installs the Xcode command line
/// tools, which takes a few minutes.
const HOMEBREW_SCRIPT: &str = "if [ -x /usr/local/bin/brew ] || [ -x /opt/homebrew/bin/brew ]; then \
    echo '     Homebrew is already installed.'; exit 0; fi; \
    sudo -v && NONINTERACTIVE=1 /bin/bash -c \"$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)\"";

/// One thing to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Run a command. `sudo` runs it through `sudo` (asks for the admin password).
    Run {
        /// Program and arguments.
        argv: Vec<String>,
        /// Needs administrator rights.
        sudo: bool,
    },
    /// Make Safari the default browser if it isn't (macOS asks to confirm).
    SafariDefault,
    /// Open a System Settings pane and wait for the admin.
    OpenPane {
        /// `x-apple.systempreferences:` URL.
        url: String,
        /// What to do there.
        instructions: String,
    },
    /// Something only a person can do.
    Manual(String),
}

/// A titled group of actions.
#[derive(Debug, Clone)]
pub struct Task {
    /// Heading.
    pub title: &'static str,
    /// Steps.
    pub actions: Vec<Action>,
}

fn run(argv: &[&str], sudo: bool) -> Action {
    Action::Run {
        argv: argv.iter().map(|s| (*s).to_owned()).collect(),
        sudo,
    }
}

fn pane(anchor: &str, name: &str) -> Action {
    Action::OpenPane {
        url: format!("x-apple.systempreferences:com.apple.preference.security?{anchor}"),
        instructions: format!(
            "In {name}, drag dot-guest from the Finder window into the list (or switch it on if it's listed), then enter your password if asked."
        ),
    }
}

/// Homebrew only supports Apple silicon now; the universal binary runs its arm64 half there.
fn apple_silicon() -> bool {
    std::env::consts::ARCH == "aarch64"
}

/// Everything prepare-image does, in order.
pub fn plan(exe: &Path) -> Vec<Task> {
    let safari = |key: &str| {
        run(
            &[
                "/usr/bin/defaults",
                "write",
                "com.apple.Safari",
                key,
                "-bool",
                "true",
            ],
            false,
        )
    };
    vec![
        Task {
            title: "Safari as the default browser",
            actions: vec![Action::SafariDefault],
        },
        Task {
            title: "Safari developer features",
            actions: vec![
                Action::Manual("Quit Safari before this step so it doesn't overwrite the settings.".into()),
                safari("IncludeDevelopMenu"),
                safari("WebKitDeveloperExtrasEnabledPreferenceKey"),
                safari("com.apple.Safari.ContentPageGroupIdentifier.WebKit2DeveloperExtrasEnabled"),
                safari("AllowJavaScriptFromAppleEvents"),
                Action::Manual(
                    "Recent Safari versions only honour \"Allow JavaScript from Apple Events\" when it is switched on by hand: open Safari > Settings > Advanced, turn on \"Show features for web developers\", then choose Develop > Allow JavaScript from Apple Events and enter your password.".into(),
                ),
            ],
        },
        Task {
            title: "Never sleep, no screen saver",
            actions: vec![
                run(&["/usr/bin/pmset", "-a", "sleep", "0", "displaysleep", "0", "disksleep", "0", "powernap", "0"], true),
                run(&["/usr/bin/defaults", "-currentHost", "write", "com.apple.screensaver", "idleTime", "-int", "0"], false),
                Action::Manual(
                    "In System Settings > Lock Screen, set \"Require password after screen saver begins or display is turned off\" to Never.".into(),
                ),
            ],
        },
        Task {
            // A VM has no GPU, so every blur and animation costs real CPU on the server.
            title: "Lighter on the server",
            actions: vec![
                run(&["/usr/bin/defaults", "write", "com.apple.universalaccess", "reduceTransparency", "-bool", "true"], false),
                run(&["/usr/bin/defaults", "write", "com.apple.universalaccess", "reduceMotion", "-bool", "true"], false),
                run(&["/usr/bin/defaults", "write", "com.apple.dock", "launchanim", "-bool", "false"], false),
                run(&["/usr/bin/defaults", "write", "NSGlobalDomain", "NSAutomaticWindowAnimationsEnabled", "-bool", "false"], false),
                run(&["/usr/bin/defaults", "write", "com.apple.assistant.support", "Assistant Enabled", "-bool", "false"], false),
                run(&["/usr/bin/defaults", "write", "com.apple.photoanalysisd", "PhotoAnalysisDisabled", "-bool", "true"], false),
                run(&["/usr/bin/defaults", "write", "com.apple.SubmitDiagInfo", "AutoSubmit", "-bool", "false"], true),
                run(&["/usr/bin/tmutil", "disable"], true),
                Action::Manual(
                    "Spotlight stays on (file_search uses it). If the Mac feels slow right after setup, it's indexing for the first time; it settles within an hour.".into(),
                ),
            ],
        },
        Task {
            title: "No surprise restarts for updates",
            actions: vec![run(
                &[
                    "/usr/bin/defaults",
                    "write",
                    "/Library/Preferences/com.apple.SoftwareUpdate",
                    "AutomaticallyInstallMacOSUpdates",
                    "-bool",
                    "false",
                ],
                true,
            )],
        },
        Task {
            // Every Mac is copied from this image, so each one starts with Homebrew; it's
            // how the assistant adds apps by default. The installer refuses to run as root,
            // so it runs as this user and asks for the password itself. Homebrew no longer
            // supports Intel Macs (every OSX-KVM Mac is one), so those use download links.
            title: "Homebrew",
            actions: if apple_silicon() {
                vec![
                    run(&["/bin/bash", "-c", HOMEBREW_SCRIPT], false),
                    run(&["/bin/zsh", "-lc", "brew analytics off && brew install mas"], false),
                ]
            } else {
                vec![Action::Manual(
                    "Skipped: Homebrew only supports Apple silicon, and this Mac is Intel. Apps install from download links instead.".into(),
                )]
            },
        },
        Task {
            title: "Automatic login",
            actions: vec![Action::Manual(
                "In System Settings > Users & Groups, set \"Automatically log in as\" to this user (FileVault must be off). The agent only runs in a logged-in desktop session.".into(),
            )],
        },
        Task {
            title: "Install the agent",
            actions: vec![Action::Manual(format!(
                "Run `dot-guest install` if you haven't (the install script does). It copies the agent to {}, which is what launchd runs and what the permissions below are for.",
                exe.display()
            ))],
        },
        Task {
            title: "Privacy permissions for dot-guest",
            actions: vec![
                // A Finder window with dot-guest selected, to drag into each list: much easier
                // than finding a hidden folder in the + sheet.
                run(&["/usr/bin/open", "-R", &exe.to_string_lossy()], false),
                pane("Privacy_Accessibility", "Accessibility"),
                pane("Privacy_ScreenCapture", "Screen & System Audio Recording"),
                pane("Privacy_AllFiles", "Full Disk Access"),
                Action::Manual(
                    "Restart the agent so it picks these up: `dot-guest install` (or log out and back in).".into(),
                ),
                Action::Manual(
                    "Automation can't be added by hand: once the agent runs it asks to control System Events (and later Safari and other apps); click Allow. Check with `dot-guest doctor`.".into(),
                ),
            ],
        },
    ]
}

fn default_browser_is_safari() -> Option<bool> {
    let out = Command::new("/usr/bin/osascript")
        .args(["-l", "JavaScript", "-e", crate::script::sources::INFO, "{}"])
        .output()
        .ok()?;
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).ok()?;
    let id = value.get("default_browser")?.as_str()?;
    Some(id.eq_ignore_ascii_case("com.apple.safari"))
}

fn wait_for_enter(interactive: bool) {
    if interactive {
        print!("     Press Return when done... ");
        let _ = std::io::stdout().flush();
        let _ = std::io::stdin().lock().read_line(&mut String::new());
    }
}

/// Carry out (or with `dry_run`, only print) the plan. Returns whether every command
/// succeeded.
pub fn execute(tasks: &[Task], dry_run: bool) -> bool {
    let interactive = std::io::stdin().is_terminal() && !dry_run;
    let mut all_ok = true;
    for (n, task) in tasks.iter().enumerate() {
        println!("\n{}. {}", n + 1, task.title);
        for action in &task.actions {
            match action {
                Action::Run { argv, sudo } => {
                    let shown = format!("{}{}", if *sudo { "sudo " } else { "" }, argv.join(" "));
                    println!("   run: {shown}");
                    if dry_run {
                        continue;
                    }
                    if *sudo && !interactive {
                        println!(
                            "     Skipped: needs an administrator password. Run it yourself in Terminal."
                        );
                        all_ok = false;
                        continue;
                    }
                    let mut cmd = if *sudo {
                        let mut c = Command::new("/usr/bin/sudo");
                        c.args(argv);
                        c
                    } else {
                        let mut c = Command::new(&argv[0]);
                        c.args(&argv[1..]);
                        c
                    };
                    match cmd.status() {
                        Ok(s) if s.success() => println!("     done"),
                        Ok(s) => {
                            all_ok = false;
                            println!(
                                "     failed ({s}). If this is a Safari setting, give Terminal Full Disk Access or set it by hand as described."
                            );
                        }
                        Err(e) => {
                            all_ok = false;
                            println!("     could not run: {e}");
                        }
                    }
                }
                Action::SafariDefault => {
                    println!(
                        "   check the default browser, and make Safari the default if it isn't"
                    );
                    if dry_run {
                        continue;
                    }
                    match default_browser_is_safari() {
                        Some(true) => println!("     Safari is already the default browser."),
                        _ => {
                            println!(
                                "     Asking macOS to make Safari the default; confirm the dialog that appears."
                            );
                            let ok = Command::new("/usr/bin/osascript")
                                .args(["-l", "JavaScript", "-e", SAFARI_DEFAULT_SCRIPT])
                                .status()
                                .is_ok_and(|s| s.success());
                            if !ok {
                                all_ok = false;
                                println!(
                                    "     Could not change it. Use System Settings > Desktop & Dock > Default web browser."
                                );
                            }
                        }
                    }
                }
                Action::OpenPane { url, instructions } => {
                    println!("   open: {url}");
                    println!("     {instructions}");
                    if dry_run {
                        continue;
                    }
                    let _ = Command::new("/usr/bin/open").arg(url).status();
                    wait_for_enter(interactive);
                }
                Action::Manual(text) => {
                    println!("   by hand: {text}");
                }
            }
        }
    }
    println!();
    if dry_run {
        println!("Dry run: nothing was changed.");
    } else if all_ok {
        println!("Done. Run `dot-guest doctor` to check everything.");
    } else {
        println!("Some steps need attention (see above). Run `dot-guest doctor` afterwards.");
    }
    all_ok
}
