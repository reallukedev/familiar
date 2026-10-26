//! [`Op::Apps`](dot_wire::Op::Apps): list, open, quit, windows, install, dictionary,
//! uninstall.
use super::Ctx;
use super::misc::{OPEN, open_app_args, open_error};
use crate::error::{self, OpResult};
use crate::proc::{self, Command};
use crate::script::{self, sources};
use crate::util;
use dot_wire::{AppInfo, AppsOp, GuestErrorCode, InstallSource, WindowInfo};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

const MAX_DOWNLOAD: u64 = 8 * 1024 * 1024 * 1024;
const QUIT_WAIT: Duration = Duration::from_secs(10);

/// [`dot_wire::Op::Apps`].
pub async fn run(ctx: &Ctx, op: AppsOp) -> OpResult {
    match op {
        AppsOp::List { running_only } => super::to_value(list(ctx, running_only).await?),
        AppsOp::Open {
            app,
            open,
            background,
        } => open_app(ctx, &app, &open, background).await,
        AppsOp::Quit { app, force } => {
            quit(ctx, &app, force).await?;
            Ok(json!({}))
        }
        AppsOp::Windows { app } => {
            let windows: Vec<WindowInfo> = script::jxa(
                ctx.deadline,
                sources::APPS,
                &json!({ "mode": "windows", "app": app }),
            )
            .await
            .map_err(accessibility_hint)?;
            super::to_value(windows)
        }
        AppsOp::Install { source } => super::to_value(install(ctx, source).await?),
        AppsOp::Dictionary { app } => super::to_value(dictionary(ctx, &app).await?),
        AppsOp::Uninstall { app } => uninstall(ctx, &app).await,
    }
}

fn accessibility_hint(mut e: dot_wire::GuestError) -> dot_wire::GuestError {
    if e.code == GuestErrorCode::PermissionDenied && e.hint.is_none() {
        e.hint = Some(error::HINT_ACCESSIBILITY.to_owned());
    }
    e
}

fn app_folders(home: &Path) -> Vec<PathBuf> {
    vec![
        PathBuf::from("/Applications"),
        home.join("Applications"),
        PathBuf::from("/System/Applications"),
    ]
}

/// `.app` bundles in the standard folders and one level of subfolders (Utilities,
/// vendor folders).
fn scan_apps(home: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for root in app_folders(home) {
        let Ok(entries) = std::fs::read_dir(&root) else {
            continue;
        };
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "app") {
                out.push(path);
            } else if entry.file_type().is_ok_and(|t| t.is_dir())
                && let Ok(inner) = std::fs::read_dir(&path)
            {
                out.extend(
                    inner
                        .filter_map(Result::ok)
                        .map(|e| e.path())
                        .filter(|p| p.extension().is_some_and(|e| e == "app")),
                );
            }
        }
    }
    out
}

async fn list(ctx: &Ctx, running_only: bool) -> OpResult<Vec<AppInfo>> {
    let home = ctx.state.home.clone();
    let paths = if running_only {
        Vec::new()
    } else {
        tokio::task::spawn_blocking(move || scan_apps(&home))
            .await
            .unwrap_or_default()
    };
    script::jxa(
        ctx.deadline,
        sources::APPS,
        &json!({ "mode": "list", "paths": paths, "running_only": running_only }),
    )
    .await
}

/// Look up an installed or running app by name, bundle id or `.app` path.
pub async fn resolve(ctx: &Ctx, app: &str) -> OpResult<AppInfo> {
    let app = app.trim();
    if app.is_empty() {
        return Err(error::invalid("Give the app's name or bundle id."));
    }
    script::jxa(
        ctx.deadline,
        sources::APPS,
        &json!({ "mode": "resolve", "app": app }),
    )
    .await
}

async fn open_app(ctx: &Ctx, app: &str, items: &[String], background: bool) -> OpResult {
    let app = app.trim();
    if app.is_empty() {
        return Err(error::invalid("Give the app's name or bundle id."));
    }
    let mut cmd = Command::new(OPEN, ctx.deadline);
    if background {
        cmd = cmd.arg("-g");
    }
    cmd = cmd.args(open_app_args(app));
    for item in items {
        let item = item.trim();
        if super::misc::has_scheme(item) && !item.starts_with('/') {
            cmd = cmd.arg(item);
        } else {
            let path = util::expand_path(item, &ctx.state.home)?;
            if !path.exists() {
                return Err(error::not_found(format!(
                    "Nothing exists at {}.",
                    path.display()
                )));
            }
            cmd = cmd.arg(path);
        }
    }
    let out = proc::tool(cmd).await?;
    if !out.success() {
        return Err(open_error(&out.stderr_text(), Some(app)));
    }
    match resolve(ctx, app).await {
        Ok(mut info) => {
            info.running = true;
            super::to_value(info)
        }
        Err(_) => Ok(json!({ "opened": app })),
    }
}

async fn quit(ctx: &Ctx, app: &str, force: bool) -> OpResult<()> {
    let wait = ctx
        .deadline
        .saturating_duration_since(tokio::time::Instant::now())
        .saturating_sub(Duration::from_millis(500))
        .min(QUIT_WAIT);
    let _: Value = script::jxa(
        ctx.deadline,
        sources::APPS,
        &json!({
            "mode": "quit",
            "app": app.trim(),
            "force": force,
            "wait_ms": u64::try_from(wait.as_millis()).unwrap_or(10_000),
        }),
    )
    .await?;
    Ok(())
}

async fn dictionary(ctx: &Ctx, app: &str) -> OpResult<dot_wire::AppDictionary> {
    let info = resolve(ctx, app).await?;
    let path = info.path.clone().ok_or_else(|| {
        error::not_found(format!("Could not find where {} is installed.", info.name))
    })?;
    let out = proc::tool(
        Command::new("/usr/bin/sdef", ctx.deadline)
            .arg(&path)
            .max_output(64 * 1024 * 1024),
    )
    .await?;
    let xml = out.stdout_text();
    if !out.success() || xml.trim().is_empty() {
        return Err(error::not_found(format!(
            "{} has no AppleScript dictionary, so it can only be driven through its UI.",
            info.name
        )));
    }
    let name = info.name.clone();
    tokio::task::spawn_blocking(move || {
        super::sdef::condense(&name, &xml, &|p| std::fs::read_to_string(p).ok())
    })
    .await
    .map_err(|e| error::failed(format!("Reading the dictionary stopped unexpectedly: {e}")))?
    .map_err(error::failed)
}

async fn uninstall(ctx: &Ctx, app: &str) -> OpResult {
    let info = resolve(ctx, app).await?;
    let Some(path) = info.path.clone() else {
        return Err(error::not_found(format!(
            "Could not find where {} is installed.",
            info.name
        )));
    };
    if path.starts_with("/System/") || info.bundle_id.as_deref() == Some("com.apple.Safari") {
        return Err(error::invalid(format!(
            "{} is part of macOS and can't be uninstalled.",
            info.name
        )));
    }
    if info.running {
        let id = info.bundle_id.as_deref().unwrap_or(&info.name);
        quit(ctx, id, false).await.map_err(|e| {
            error::failed(format!(
                "{} is running and did not quit: {}",
                info.name, e.message
            ))
        })?;
    }
    super::files::trash(ctx, Path::new(&path)).await?;
    Ok(json!({}))
}

// ---- install -------------------------------------------------------------------------

/// Top-level `.app` bundles in the install destinations, with their modification times.
fn installed_now(home: &Path) -> HashMap<PathBuf, Option<SystemTime>> {
    [PathBuf::from("/Applications"), home.join("Applications")]
        .iter()
        .filter_map(|d| std::fs::read_dir(d).ok())
        .flat_map(|r| r.filter_map(Result::ok))
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "app"))
        .map(|p| {
            let modified = std::fs::metadata(&p).and_then(|m| m.modified()).ok();
            (p, modified)
        })
        .collect()
}

fn changed_app(
    before: &HashMap<PathBuf, Option<SystemTime>>,
    after: &HashMap<PathBuf, Option<SystemTime>>,
) -> Option<PathBuf> {
    let mut changed: Vec<&PathBuf> = after
        .iter()
        .filter(|(p, m)| before.get(*p) != Some(*m))
        .map(|(p, _)| p)
        .collect();
    changed.sort();
    changed.first().map(|p| (*p).clone())
}

async fn install(ctx: &Ctx, source: InstallSource) -> OpResult<AppInfo> {
    let home = ctx.state.home.clone();
    let before = installed_now(&home);
    let installed = match source {
        InstallSource::Url { url, sha256 } => install_url(ctx, &url, sha256.as_deref()).await?,
        InstallSource::AppStore { id } => {
            install_mas(ctx, id).await?;
            None
        }
        InstallSource::Brew { cask } => install_brew(ctx, &cask).await?,
    };
    let after = installed_now(&home);
    let path = installed.or_else(|| changed_app(&before, &after)).ok_or_else(|| {
        error::failed("The installer finished, but no new app appeared in /Applications or ~/Applications.")
    })?;
    resolve(ctx, &path.to_string_lossy()).await
}

/// Validate an https URL without a URL parser: scheme, a host, no whitespace.
pub fn check_https_url(url: &str) -> OpResult<()> {
    let rest = url
        .strip_prefix("https://")
        .ok_or_else(|| error::invalid("Only https:// download URLs are allowed."))?;
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    if host.is_empty() || url.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(error::invalid(format!("`{url}` is not a valid https URL.")));
    }
    Ok(())
}

/// What a downloaded file is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Archive {
    /// Disk image.
    Dmg,
    /// Zip archive.
    Zip,
    /// Installer package (xar).
    Pkg,
}

/// Detect the archive type from magic bytes, falling back to the URL's extension.
pub fn detect_archive(head: &[u8], tail: &[u8], url: &str) -> Option<Archive> {
    if head.starts_with(b"PK\x03\x04") {
        return Some(Archive::Zip);
    }
    if head.starts_with(b"xar!") {
        return Some(Archive::Pkg);
    }
    if tail.starts_with(b"koly") {
        return Some(Archive::Dmg);
    }
    let path = url.split(['?', '#']).next().unwrap_or(url).to_lowercase();
    if path.ends_with(".dmg") {
        Some(Archive::Dmg)
    } else if path.ends_with(".zip") {
        Some(Archive::Zip)
    } else if path.ends_with(".pkg") || path.ends_with(".mpkg") {
        Some(Archive::Pkg)
    } else {
        None
    }
}

fn file_head_tail(path: &Path) -> std::io::Result<(Vec<u8>, Vec<u8>, String)> {
    use std::io::{Seek, SeekFrom};
    let mut file = std::fs::File::open(path)?;
    let len = file.metadata()?.len();
    let mut head = vec![0u8; 8];
    let n = file.read(&mut head)?;
    head.truncate(n);
    let mut tail = Vec::new();
    if len >= 512 {
        file.seek(SeekFrom::Start(len - 512))?;
        Read::by_ref(&mut file).take(512).read_to_end(&mut tail)?;
    }
    file.seek(SeekFrom::Start(0))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1024 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok((head, tail, util::hex(&hasher.finalize())))
}

async fn install_url(ctx: &Ctx, url: &str, sha256: Option<&str>) -> OpResult<Option<PathBuf>> {
    let url = url.trim();
    check_https_url(url)?;
    let expected = sha256
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty());
    if let Some(s) = &expected
        && (s.len() != 64 || !s.chars().all(|c| c.is_ascii_hexdigit()))
    {
        return Err(error::invalid("sha256 must be 64 hex characters."));
    }
    let dir = util::temp_dir()?;
    let file = dir.path().join("download");
    let out = proc::tool(
        Command::new("/usr/bin/curl", ctx.deadline)
            .args([
                "--fail",
                "--location",
                "--silent",
                "--show-error",
                "--proto",
                "=https",
                "--proto-redir",
                "=https",
                "--retry",
                "2",
                "--connect-timeout",
                "30",
                "--max-filesize",
                &MAX_DOWNLOAD.to_string(),
                "--output",
            ])
            .arg(&file)
            .arg(url),
    )
    .await?;
    if !out.success() {
        return Err(error::failed(format!(
            "The download failed: {}",
            out.stderr_text()
        )));
    }
    let probe = file.clone();
    let (head, tail, digest) = tokio::task::spawn_blocking(move || file_head_tail(&probe))
        .await
        .map_err(|e| error::failed(format!("Checking the download stopped: {e}")))?
        .map_err(|e| error::failed(format!("Could not read the download: {e}")))?;
    if let Some(expected) = expected
        && digest != expected
    {
        return Err(error::failed(format!(
            "The download's SHA-256 is {digest}, not the expected {expected}. Nothing was installed."
        )));
    }
    let kind = detect_archive(&head, &tail, url)
        .ok_or_else(|| error::invalid("The download is not a .dmg, .zip or .pkg file."))?;
    install_archive(ctx, &file, kind, dir.path(), 0).await
}

fn install_archive<'a>(
    ctx: &'a Ctx,
    file: &'a Path,
    kind: Archive,
    work: &'a Path,
    nesting: u8,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = OpResult<Option<PathBuf>>> + Send + 'a>> {
    Box::pin(async move {
        match kind {
            Archive::Pkg => {
                install_pkg(ctx, file).await?;
                Ok(None)
            }
            Archive::Zip => {
                let out_dir = work.join(format!("unzipped{nesting}"));
                proc::tool_ok(
                    Command::new("/usr/bin/ditto", ctx.deadline)
                        .args(["-x", "-k"])
                        .arg(file)
                        .arg(&out_dir),
                )
                .await?;
                install_from_folder(ctx, &out_dir, work, nesting).await
            }
            Archive::Dmg => {
                let mount = work.join(format!("mount{nesting}"));
                std::fs::create_dir_all(&mount)
                    .map_err(|e| error::failed(format!("Could not prepare a mount point: {e}")))?;
                let out = proc::tool(
                    Command::new("/usr/bin/hdiutil", ctx.deadline)
                        .args([
                            "attach",
                            "-nobrowse",
                            "-readonly",
                            "-noautoopen",
                            "-mountpoint",
                        ])
                        .arg(&mount)
                        .arg(file)
                        // Disk images with a licence ask to agree on stdin.
                        .stdin(b"Y\n".to_vec()),
                )
                .await?;
                if !out.success() {
                    return Err(error::failed(format!(
                        "Could not open the disk image: {}",
                        out.stderr_text()
                    )));
                }
                let guard = DetachGuard(Some(mount.clone()));
                let result = install_from_folder(ctx, &mount, work, nesting).await;
                let detached = proc::tool(
                    Command::new("/usr/bin/hdiutil", ctx.deadline)
                        .args(["detach", "-force"])
                        .arg(&mount),
                )
                .await;
                if detached.is_ok_and(|o| o.success()) {
                    guard.disarm();
                }
                result
            }
        }
    })
}

/// Detaches a disk image if the install is cancelled or the polite detach failed.
struct DetachGuard(Option<PathBuf>);

impl DetachGuard {
    fn disarm(mut self) {
        self.0 = None;
    }
}

impl Drop for DetachGuard {
    fn drop(&mut self) {
        if let Some(mount) = self.0.take() {
            // Fire and forget: Drop can't await, and this must not block the runtime.
            let _ = std::process::Command::new("/usr/bin/hdiutil")
                .args(["detach", "-force"])
                .arg(mount)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn();
        }
    }
}

fn find_top(dir: &Path, ext: &str) -> Option<PathBuf> {
    let mut found: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == ext))
        .filter(|p| {
            !p.file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with('.'))
        })
        .collect();
    found.sort();
    found.into_iter().next()
}

async fn install_from_folder(
    ctx: &Ctx,
    dir: &Path,
    work: &Path,
    nesting: u8,
) -> OpResult<Option<PathBuf>> {
    let mut search = vec![dir.to_path_buf()];
    if let Ok(entries) = std::fs::read_dir(dir) {
        search.extend(
            entries
                .filter_map(Result::ok)
                .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
                .map(|e| e.path())
                .filter(|p| p.extension().is_none_or(|e| e != "app")),
        );
    }
    for folder in &search {
        if let Some(app) = find_top(folder, "app") {
            return copy_app(ctx, &app).await.map(Some);
        }
    }
    if nesting == 0 {
        for (ext, kind) in [
            ("pkg", Archive::Pkg),
            ("mpkg", Archive::Pkg),
            ("dmg", Archive::Dmg),
        ] {
            if let Some(inner) = search.iter().find_map(|f| find_top(f, ext)) {
                return install_archive(ctx, &inner, kind, work, nesting + 1).await;
            }
        }
    }
    Err(error::failed(
        "The download contains no app, package or disk image to install.",
    ))
}

/// `/Applications` when writable, else `~/Applications`.
fn install_dir(home: &Path) -> OpResult<PathBuf> {
    let system = Path::new("/Applications");
    if rustix::fs::access(system, rustix::fs::Access::WRITE_OK).is_ok() {
        return Ok(system.to_path_buf());
    }
    let user = home.join("Applications");
    std::fs::create_dir_all(&user)
        .map_err(|e| error::failed(format!("Could not create {}: {e}", user.display())))?;
    Ok(user)
}

async fn copy_app(ctx: &Ctx, app: &Path) -> OpResult<PathBuf> {
    let name = app
        .file_name()
        .ok_or_else(|| error::failed("The app in the download has no name."))?;
    let dest = install_dir(&ctx.state.home)?.join(name);
    if dest.exists() {
        // An update: quit the running copy and keep the old one recoverable.
        let dest_text = dest.to_string_lossy().into_owned();
        if let Ok(info) = resolve(ctx, &dest_text).await
            && info.running
        {
            let id = info.bundle_id.as_deref().unwrap_or(&info.name);
            quit(ctx, id, false).await.map_err(|e| {
                error::failed(format!(
                    "{} is running and did not quit: {}",
                    info.name, e.message
                ))
            })?;
        }
        super::files::trash(ctx, &dest).await?;
    }
    proc::tool_ok(
        Command::new("/usr/bin/ditto", ctx.deadline)
            .arg(app)
            .arg(&dest),
    )
    .await?;
    let _ = proc::tool(
        Command::new("/usr/bin/xattr", ctx.deadline)
            .args(["-dr", "com.apple.quarantine"])
            .arg(&dest),
    )
    .await;
    Ok(dest)
}

async fn install_pkg(ctx: &Ctx, pkg: &Path) -> OpResult<()> {
    let out = proc::tool(
        Command::new("/usr/sbin/installer", ctx.deadline)
            .arg("-pkg")
            .arg(pkg)
            .args(["-target", "CurrentUserHomeDirectory"]),
    )
    .await?;
    if out.success() {
        return Ok(());
    }
    let text = format!("{}\n{}", out.stdout_text(), out.stderr_text());
    let lower = text.to_lowercase();
    if lower.contains("root") || lower.contains("authoriz") || lower.contains("privilege") {
        return Err(error::permission(
            "This package installs for every user and needs an administrator password.",
            "Install it by hand in the Mac, or use the app's .dmg or .zip download, or its Homebrew cask.",
        ));
    }
    Err(error::failed(format!(
        "The package did not install for this user: {}",
        text.trim()
    ))
    .hint("Many packages only install system-wide. Try the app's .dmg or .zip download, or its Homebrew cask."))
}

fn find_tool(name: &str) -> Option<PathBuf> {
    ["/opt/homebrew/bin", "/usr/local/bin"]
        .iter()
        .map(|d| Path::new(d).join(name))
        .find(|p| p.is_file())
}

async fn install_mas(ctx: &Ctx, id: u64) -> OpResult<()> {
    let mas = find_tool("mas").ok_or_else(|| {
        error::unsupported(
            "The `mas` command is not installed, so the App Store can't be used from the agent.",
        )
        .hint("Install it with `brew install mas`, then sign in to the App Store in the Mac.")
    })?;
    let out =
        proc::tool(Command::new(&mas, ctx.deadline).args(["install", &id.to_string()])).await?;
    if out.success() {
        return Ok(());
    }
    let text = format!("{} {}", out.stdout_text(), out.stderr_text());
    let lower = text.to_lowercase();
    if lower.contains("not signed in") || lower.contains("sign in") {
        return Err(error::failed("The Mac is not signed in to the App Store.")
            .hint("Open the App Store in the Mac and sign in with an Apple Account."));
    }
    if lower.contains("no apps found") || lower.contains("not found") {
        return Err(error::not_found(format!(
            "The App Store has no app with id {id}."
        )));
    }
    Err(error::failed(format!("mas could not install app {id}: {}", text.trim()))
        .hint("Apps never downloaded with this Apple Account must be \"bought\" once in the App Store, even when free."))
}

/// Homebrew cask tokens are lowercase letters, digits and `-`, `_`, `.`, `@`, `+`.
pub fn valid_cask(token: &str) -> bool {
    !token.is_empty()
        && token.len() <= 100
        && !token.starts_with('-')
        && token
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "-_.@+".contains(c))
}

#[derive(Debug, Deserialize)]
struct BrewInfo {
    #[serde(default)]
    casks: Vec<BrewCask>,
}

#[derive(Debug, Deserialize)]
struct BrewCask {
    #[serde(default)]
    artifacts: Vec<Value>,
}

/// `.app` names a cask installs, from `brew info --json=v2`.
fn cask_apps(json: &str) -> Vec<String> {
    let Ok(info) = serde_json::from_str::<BrewInfo>(json) else {
        return Vec::new();
    };
    info.casks
        .iter()
        .flat_map(|c| c.artifacts.iter())
        .filter_map(|a| a.get("app")?.as_array())
        .flat_map(|apps| apps.iter())
        .filter_map(|v| v.as_str().map(str::to_owned))
        .collect()
}

async fn install_brew(ctx: &Ctx, cask: &str) -> OpResult<Option<PathBuf>> {
    let cask = cask.trim();
    if !valid_cask(cask) {
        return Err(error::invalid(format!(
            "`{cask}` is not a Homebrew cask name."
        )));
    }
    let brew = find_tool("brew").ok_or_else(|| {
        error::unsupported("Homebrew is not installed on this Mac.")
            .hint("Install it from https://brew.sh, or install the app from a .dmg URL.")
    })?;
    let out = proc::tool(
        Command::new(&brew, ctx.deadline)
            .args(["install", "--cask", cask])
            .env("NONINTERACTIVE", "1")
            .env("HOMEBREW_NO_ENV_HINTS", "1"),
    )
    .await?;
    if !out.success() {
        let text = format!("{}\n{}", out.stdout_text(), out.stderr_text());
        let lower = text.to_lowercase();
        if lower.contains("sudo") || lower.contains("password") {
            return Err(error::permission(
                format!("Installing {cask} needs an administrator password."),
                "Install it by hand in the Mac with `brew install --cask` in Terminal.",
            ));
        }
        if lower.contains("no available cask") || lower.contains("no cask") {
            return Err(error::not_found(format!(
                "There is no Homebrew cask called {cask}."
            )));
        }
        return Err(error::failed(format!(
            "brew could not install {cask}: {}",
            text.trim()
        )));
    }
    let info =
        proc::tool(Command::new(&brew, ctx.deadline).args(["info", "--cask", "--json=v2", cask]))
            .await?;
    let home = &ctx.state.home;
    Ok(cask_apps(&info.stdout_text())
        .into_iter()
        .flat_map(|name| {
            [
                Path::new("/Applications").join(&name),
                home.join("Applications").join(&name),
            ]
        })
        .find(|p| p.exists()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archive_type_comes_from_magic_bytes_before_the_url() {
        assert_eq!(
            detect_archive(b"PK\x03\x04", b"", "https://x/a.dmg"),
            Some(Archive::Zip)
        );
        assert_eq!(
            detect_archive(b"xar!\x00\x1c", b"", "https://x/a"),
            Some(Archive::Pkg)
        );
        assert_eq!(
            detect_archive(b"\x00\x00", b"koly\x00\x00", "https://x/a"),
            Some(Archive::Dmg)
        );
        assert_eq!(
            detect_archive(b"", b"", "https://x/App.DMG?dl=1"),
            Some(Archive::Dmg)
        );
        assert_eq!(detect_archive(b"<html>", b"", "https://x/download"), None);
    }

    #[test]
    fn only_https_urls_with_a_host_are_downloaded() {
        assert!(check_https_url("https://example.com/app.dmg").is_ok());
        for bad in [
            "http://example.com/a.dmg",
            "https:///a.dmg",
            "file:///etc/passwd",
            "https://a b.com/x",
            "ftp://x",
        ] {
            assert!(check_https_url(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn cask_tokens_cannot_smuggle_flags() {
        for ok in [
            "visual-studio-code",
            "firefox@developer-edition",
            "1password",
        ] {
            assert!(valid_cask(ok), "{ok}");
        }
        for bad in ["--force", "Firefox", "a b", "x;rm", ""] {
            assert!(!valid_cask(bad), "{bad}");
        }
    }

    #[test]
    fn cask_app_names_are_read_from_brew_info() {
        let json = r#"{"formulae":[],"casks":[{"token":"vscode","artifacts":[{"uninstall":[]},{"app":["Visual Studio Code.app"]},{"binary":["code"]}]}]}"#;
        assert_eq!(cask_apps(json), ["Visual Studio Code.app"]);
        assert!(cask_apps("not json").is_empty());
    }
}
