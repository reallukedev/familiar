//! Auto-updates for the server and each Mac's agent.
//!
//! CI publishes a signed `manifest.json` per channel: `stable` for tagged releases,
//! `nightly` for every push. The server checks it, downloads the artifact, verifies its
//! SHA-256 (the manifest is Ed25519-signed with a key compiled into this binary), and:
//!
//! - **Server**: installs the new binary under `<data>/updates/`, then restarts in place by
//!   exec'ing it. Macs keep running (QEMU is daemonized) and are re-adopted. The new build
//!   is on trial: if it crashes before it has served for a minute, three times, the
//!   launcher marks it bad and goes back to the previous one.
//! - **Agents**: streams the new `dot-guest` to each Mac over its authenticated link
//!   ([`dot_wire::UpdateOp`]); the agent's own supervisor rolls back a version that won't
//!   come up.
use crate::{
    db::{now, rfc3339, rfc3339_opt},
    state::App,
};
use anyhow::{Context, bail};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

/// What this binary is.
pub mod build {
    /// Version, e.g. `0.3.0` or `0.3.0-nightly.202610200312.ad02bf1`.
    pub fn version() -> &'static str {
        // Empty means "not set" (Docker passes empty build args through).
        option_env!("DOT_BUILD_VERSION")
            .filter(|v| !v.is_empty())
            .unwrap_or(env!("CARGO_PKG_VERSION"))
    }
    /// Git commit, when built by CI.
    pub fn commit() -> Option<&'static str> {
        option_env!("DOT_BUILD_COMMIT").filter(|c| !c.is_empty())
    }
    /// Channel the build came from.
    pub fn channel() -> Option<&'static str> {
        option_env!("DOT_BUILD_CHANNEL").filter(|c| matches!(*c, "stable" | "nightly"))
    }
    /// `owner/name` of the GitHub repository releases come from.
    pub fn repository() -> Option<&'static str> {
        option_env!("DOT_UPDATE_REPOSITORY").filter(|r| r.contains('/'))
    }
    /// Base64 Ed25519 public keys (comma separated) trusted to sign manifests.
    pub fn public_keys() -> Vec<&'static str> {
        option_env!("DOT_UPDATE_PUBLIC_KEYS")
            .map(|k| {
                k.split(',')
                    .map(str::trim)
                    .filter(|k| !k.is_empty())
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// One downloadable file in a manifest.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Artifact {
    /// https URL.
    pub url: String,
    /// Hex SHA-256.
    pub sha256: String,
    /// Bytes.
    pub size: u64,
}

/// What CI publishes for a channel.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Manifest {
    /// Format version (1).
    pub schema: u32,
    /// `stable` or `nightly`.
    pub channel: String,
    /// Version string.
    pub version: String,
    /// Git commit.
    pub commit: String,
    /// RFC 3339.
    pub published_at: String,
    /// Release notes (plain text).
    #[serde(default)]
    pub notes: Option<String>,
    /// Release page.
    #[serde(default)]
    pub url: Option<String>,
    /// The Linux x86_64 server binary.
    pub server: Artifact,
    /// The universal macOS agent.
    pub guest: Artifact,
}

/// Verify `sig_b64` over `bytes` with any trusted key, then parse.
pub fn verify_manifest(bytes: &[u8], sig_b64: &str, keys: &[&str]) -> anyhow::Result<Manifest> {
    use ed25519_dalek::{Signature, Verifier, VerifyingKey};
    let sig = STANDARD
        .decode(sig_b64.trim())
        .context("the signature isn't base64")?;
    let sig = Signature::from_slice(&sig).context("the signature has the wrong length")?;
    let trusted = keys.iter().any(|k| {
        STANDARD
            .decode(k)
            .ok()
            .and_then(|b| <[u8; 32]>::try_from(b.as_slice()).ok())
            .and_then(|b| VerifyingKey::from_bytes(&b).ok())
            .is_some_and(|key| key.verify(bytes, &sig).is_ok())
    });
    if !trusted {
        bail!("the update isn't signed by a key this build trusts");
    }
    let m: Manifest = serde_json::from_slice(bytes).context("the update manifest is malformed")?;
    if m.schema != 1 {
        bail!("the update manifest uses a newer format; update Familiar manually once");
    }
    for a in [&m.server, &m.guest] {
        if !a.url.starts_with("https://")
            || a.sha256.len() != 64
            || !a.sha256.chars().all(|c| c.is_ascii_hexdigit())
        {
            bail!("the update manifest has an invalid download");
        }
    }
    if !safe_version(&m.version) {
        bail!("the update manifest has an invalid version");
    }
    Ok(m)
}

/// Sign manifest bytes (CI): `seed_b64` is the 32-byte private seed.
pub fn sign_manifest(bytes: &[u8], seed_b64: &str) -> anyhow::Result<String> {
    use ed25519_dalek::{Signer, SigningKey};
    let seed: [u8; 32] = STANDARD
        .decode(seed_b64.trim())?
        .try_into()
        .map_err(|_| anyhow::anyhow!("the signing key must be 32 bytes"))?;
    Ok(STANDARD.encode(SigningKey::from_bytes(&seed).sign(bytes).to_bytes()))
}

/// New key pair: (private seed, public key), both base64.
pub fn keygen() -> (String, String) {
    let seed = dot_wire::random_bytes::<32>();
    let key = ed25519_dalek::SigningKey::from_bytes(&seed);
    (
        STANDARD.encode(seed),
        STANDARD.encode(key.verifying_key().to_bytes()),
    )
}

/// Letters, digits, `.`, `-`, `+`, `_`; it becomes a file name.
pub fn safe_version(v: &str) -> bool {
    !v.is_empty()
        && v.len() <= 64
        && !v.starts_with('.')
        && v.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+' | '_'))
}

/// Ordering key: numeric `major.minor.patch`, then releases above pre-releases, then the
/// pre-release text (nightly versions embed a sortable timestamp).
fn version_key(v: &str) -> (Vec<u64>, bool, String) {
    let (core, pre) = v.split_once('-').unwrap_or((v, ""));
    let core = core.split('+').next().unwrap_or(core);
    let nums = core.split('.').map(|p| p.parse().unwrap_or(0)).collect();
    (nums, pre.is_empty(), pre.to_owned())
}

/// Is `candidate` newer than `current`?
pub fn is_newer(candidate: &str, current: &str) -> bool {
    candidate != current && version_key(candidate) > version_key(current)
}

/// Where the server's update files live.
fn dir(app: &App) -> PathBuf {
    app.config.data_dir.join("updates")
}

/// The launcher's record of which server binary to run.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Launch {
    /// Installed binary to run instead of the built-in one.
    pub current: Option<String>,
    /// Version of `current`.
    pub current_version: Option<String>,
    /// What to go back to.
    pub previous: Option<String>,
    /// Version of `previous`.
    pub previous_version: Option<String>,
    /// Starts of `current` that haven't yet proved healthy.
    pub trial_attempts: u32,
    /// Versions that failed their trial; never run again.
    #[serde(default)]
    pub bad: Vec<String>,
}

impl Launch {
    fn path(data_dir: &Path) -> PathBuf {
        data_dir.join("updates").join("launch.json")
    }
    /// Read (missing or unreadable means "run the built-in binary").
    pub fn load(data_dir: &Path) -> Self {
        std::fs::read(Self::path(data_dir))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }
    /// Write atomically.
    pub fn save(&self, data_dir: &Path) -> anyhow::Result<()> {
        crate::crypto::write_private(&Self::path(data_dir), &serde_json::to_vec_pretty(self)?)
    }
}

/// What the launcher decided.
#[derive(Debug, PartialEq, Eq)]
pub enum LaunchDecision {
    /// Run the binary that's already running.
    Stay,
    /// Exec this installed binary.
    Exec(PathBuf),
}

/// Decide which binary to run, updating `launch` (trial counts, rollbacks). Pure, so the
/// rules are testable; [`launch`] applies the decision.
pub fn decide(launch: &mut Launch, built_in_version: &str, already_execed: bool) -> LaunchDecision {
    const MAX_TRIALS: u32 = 3;
    let Some(current) = launch.current.clone() else {
        return LaunchDecision::Stay;
    };
    let version = launch.current_version.clone().unwrap_or_default();
    // A newer image (e.g. `docker pull`) beats an older self-installed update.
    if !is_newer(&version, built_in_version)
        && version != built_in_version
        && !launch.bad.contains(&version)
    {
        launch.current = None;
        launch.current_version = None;
        launch.trial_attempts = 0;
        return LaunchDecision::Stay;
    }
    if version == built_in_version || already_execed {
        return LaunchDecision::Stay;
    }
    if launch.bad.contains(&version) || !Path::new(&current).exists() {
        launch.current = launch.previous.take();
        launch.current_version = launch.previous_version.take();
        launch.trial_attempts = 0;
        return decide(launch, built_in_version, already_execed);
    }
    if launch.trial_attempts > 0 {
        if launch.trial_attempts >= MAX_TRIALS {
            launch.bad.push(version);
            launch.current = launch.previous.take();
            launch.current_version = launch.previous_version.take();
            launch.trial_attempts = 0;
            return decide(launch, built_in_version, already_execed);
        }
        launch.trial_attempts += 1;
    }
    LaunchDecision::Exec(PathBuf::from(current))
}

/// Call first thing in `serve`: exec an installed update when there is one. Returns only
/// when this binary should keep running.
pub fn launch(data_dir: &Path, args: &[String]) {
    let already = std::env::var_os("DOT_HOME_EXECED").is_some();
    if let LaunchDecision::Exec(path) = resolve(data_dir, already) {
        exec(&path, args);
    }
}

/// After a shutdown for an update or rollback: start the image's own binary again as a
/// fresh process; its [`launch`] then picks the installed update (or not). Returns only
/// if exec fails, and the caller exits so the container or systemd restarts it.
pub fn relaunch(args: &[String]) {
    use std::os::unix::process::CommandExt;
    let builtin = builtin_path();
    eprintln!("dot-home: restarting");
    let err = std::process::Command::new(&builtin)
        .args(args.iter().skip(1))
        .env_remove("DOT_HOME_EXECED")
        .env("DOT_HOME_BUILTIN", &builtin)
        .exec();
    eprintln!("dot-home: couldn't restart ({err})");
}

/// The binary the container or systemd started (before any update was exec'd).
fn builtin_path() -> PathBuf {
    std::env::var_os("DOT_HOME_BUILTIN")
        .map(PathBuf::from)
        .or_else(|| std::env::current_exe().ok())
        .unwrap_or_default()
}

fn exec(path: &Path, args: &[String]) {
    use std::os::unix::process::CommandExt;
    eprintln!("dot-home: starting installed update {}", path.display());
    let err = std::process::Command::new(path)
        .args(args.iter().skip(1))
        .env("DOT_HOME_EXECED", "1")
        .env("DOT_HOME_BUILTIN", builtin_path())
        .exec();
    eprintln!(
        "dot-home: couldn't start {} ({err}); running the built-in version",
        path.display()
    );
}

fn resolve(data_dir: &Path, already: bool) -> LaunchDecision {
    let mut state = Launch::load(data_dir);
    let before = serde_json::to_string(&state).unwrap_or_default();
    let decision = decide(&mut state, build::version(), already);
    if serde_json::to_string(&state).unwrap_or_default() != before
        && let Err(e) = state.save(data_dir)
    {
        eprintln!("dot-home: couldn't record the update state: {e:#}");
    }
    decision
}

/// Settings kept in the database.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    /// `stable`, `nightly` or `off`.
    pub channel: String,
    /// Install server updates in the window without asking.
    pub auto_install: bool,
    /// 0 to 23.
    pub window_start_hour: u8,
    /// What Macs on `default` follow.
    pub guest_default_channel: String,
    /// Agents update themselves when their Mac is idle.
    pub guest_auto_install: bool,
}

impl Default for Settings {
    fn default() -> Self {
        let channel = build::channel().unwrap_or("stable").to_owned();
        Self {
            guest_default_channel: channel.clone(),
            channel,
            auto_install: true,
            window_start_hour: 3,
            guest_auto_install: true,
        }
    }
}

/// Live updater state.
#[derive(Default)]
pub struct Updater {
    state: Mutex<Live>,
    /// Set when the server should exec the new binary after shutting down.
    pub restart_requested: AtomicBool,
    /// Wakes `serve` to shut down for a restart.
    pub restart: tokio::sync::Notify,
    updating: Mutex<HashSet<String>>,
}

#[derive(Default)]
struct Live {
    state: &'static str,
    detail: Option<String>,
    manifests: HashMap<String, Manifest>,
    last_checked: Option<i64>,
}

fn http() -> anyhow::Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .timeout(Duration::from_secs(600))
        .connect_timeout(Duration::from_secs(15))
        .user_agent(concat!("dot-home/", env!("CARGO_PKG_VERSION")))
        .build()?)
}

/// Why updates can't run, if they can't.
pub fn disabled_reason() -> Option<&'static str> {
    if build::repository().is_none() || build::public_keys().is_empty() {
        Some(
            "This build wasn't made by the release pipeline, so it can't verify updates. Install a release build, or update by rebuilding the container.",
        )
    } else {
        None
    }
}

impl Updater {
    fn set(&self, state: &'static str, detail: Option<String>) {
        let mut s = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        s.state = state;
        s.detail = detail;
    }

    /// The cached manifest for a channel.
    pub fn manifest(&self, channel: &str) -> Option<Manifest> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .manifests
            .get(channel)
            .cloned()
    }

    /// Is this Mac's agent being updated right now.
    pub fn is_updating(&self, instance_id: &str) -> bool {
        self.updating
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains(instance_id)
    }
}

/// Read settings.
pub async fn settings(app: &App) -> Settings {
    app.db
        .setting("updates")
        .await
        .ok()
        .flatten()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// Save settings.
pub async fn save_settings(app: &App, s: &Settings) -> anyhow::Result<()> {
    app.db
        .set_setting("updates", serde_json::to_string(s)?)
        .await?;
    Ok(())
}

/// Fetch and verify one channel's manifest.
pub async fn fetch(channel: &str) -> anyhow::Result<Manifest> {
    let repo = build::repository().context("updates aren't configured in this build")?;
    let base = match channel {
        "stable" => format!("https://github.com/{repo}/releases/latest/download"),
        "nightly" => format!("https://github.com/{repo}/releases/download/nightly"),
        _ => bail!("unknown channel"),
    };
    let client = http()?;
    let get = |url: String| {
        let client = client.clone();
        async move {
            let r = client
                .get(&url)
                .timeout(Duration::from_secs(30))
                .send()
                .await?;
            if !r.status().is_success() {
                bail!("{url} answered HTTP {}", r.status().as_u16());
            }
            let bytes = r.bytes().await?;
            anyhow::ensure!(bytes.len() < 1_000_000, "the manifest is too large");
            Ok(bytes)
        }
    };
    let (body, sig) = tokio::try_join!(
        get(format!("{base}/manifest.json")),
        get(format!("{base}/manifest.json.sig"))
    )?;
    let m = verify_manifest(&body, &String::from_utf8_lossy(&sig), &build::public_keys())?;
    anyhow::ensure!(
        m.channel == channel,
        "the {channel} manifest says it's for {}",
        m.channel
    );
    Ok(m)
}

/// Check every channel in use. Errors become the visible state, never a crash.
pub async fn check(app: &App) {
    if disabled_reason().is_some() {
        return;
    }
    app.updates.set("checking", None);
    publish(app).await;
    let s = settings(app).await;
    let mut channels: HashSet<String> = [s.channel.clone(), s.guest_default_channel.clone()].into();
    let per_mac: Vec<String> = app
        .db
        .call(|c| {
            let mut st = c.prepare("SELECT DISTINCT agent_channel FROM instances")?;
            let rows = st.query_map([], |r| r.get(0))?;
            rows.collect()
        })
        .await
        .unwrap_or_default();
    channels.extend(per_mac);
    channels.retain(|c| c == "stable" || c == "nightly");
    let mut failure = None;
    for ch in channels {
        match fetch(&ch).await {
            Ok(m) => {
                app.updates
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .manifests
                    .insert(ch, m);
            }
            Err(e) => {
                tracing::warn!(channel = %ch, error = %format!("{e:#}"), "update check failed");
                failure = Some(format!("Couldn't check for updates: {e:#}"));
            }
        }
    }
    app.updates
        .state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .last_checked = Some(now());
    match failure {
        Some(f) => app.updates.set("failed", Some(f)),
        None => app.updates.set("idle", None),
    }
    publish(app).await;
}

async fn download(app: &App, artifact: &Artifact, to: &Path) -> anyhow::Result<()> {
    use sha2::{Digest, Sha256};
    use tokio::io::AsyncWriteExt;
    anyhow::ensure!(
        artifact.size < 400 * 1024 * 1024,
        "the download is unexpectedly large"
    );
    if let Some(parent) = to.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    if tokio::fs::read(to)
        .await
        .is_ok_and(|b| hex(&Sha256::digest(&b)) == artifact.sha256)
    {
        return Ok(());
    }
    let partial = to.with_extension("partial");
    let mut file = tokio::fs::File::create(&partial).await?;
    let mut res = http()?.get(&artifact.url).send().await?;
    anyhow::ensure!(
        res.status().is_success(),
        "the download answered HTTP {}",
        res.status().as_u16()
    );
    let mut hasher = Sha256::new();
    let mut total = 0u64;
    while let Some(chunk) = res.chunk().await? {
        total += chunk.len() as u64;
        anyhow::ensure!(
            total <= artifact.size,
            "the download is larger than the manifest says"
        );
        hasher.update(&chunk);
        file.write_all(&chunk).await?;
        if total % (8 * 1024 * 1024) < chunk.len() as u64 {
            app.updates.set(
                "downloading",
                Some(format!(
                    "Downloaded {} of {} MB",
                    total / 1_048_576,
                    artifact.size / 1_048_576
                )),
            );
        }
    }
    file.sync_all().await?;
    drop(file);
    anyhow::ensure!(total == artifact.size, "the download was cut short");
    anyhow::ensure!(
        hex(&hasher.finalize()) == artifact.sha256,
        "the download doesn't match its signed checksum"
    );
    tokio::fs::rename(&partial, to).await?;
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Download, verify and stage the server update, then restart into it.
pub async fn install_server(app: &App, manifest: &Manifest) -> anyhow::Result<()> {
    if let Some(r) = disabled_reason() {
        bail!(r);
    }
    let bin = dir(app).join(format!("dot-home-{}", manifest.version));
    app.updates.set(
        "downloading",
        Some(format!("Downloading Familiar {}", manifest.version)),
    );
    publish(app).await;
    let result = async {
        download(app, &manifest.server, &bin).await?;
        {
            use std::os::unix::fs::PermissionsExt;
            tokio::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).await?;
        }
        app.updates.set(
            "installing",
            Some(format!("Checking Familiar {}", manifest.version)),
        );
        publish(app).await;
        let out = tokio::time::timeout(
            Duration::from_secs(20),
            tokio::process::Command::new(&bin).arg("--version").output(),
        )
        .await
        .context("the new version didn't answer")??;
        let printed = String::from_utf8_lossy(&out.stdout);
        anyhow::ensure!(
            out.status.success() && printed.contains(&manifest.version),
            "the downloaded binary reports {:?} instead of {}",
            printed.trim(),
            manifest.version
        );
        let data_dir = app.config.data_dir.clone();
        let mut launch = Launch::load(&data_dir);
        let running = std::env::current_exe()
            .ok()
            .map(|p| p.to_string_lossy().into_owned());
        launch.previous = launch.current.take().or(running);
        launch.previous_version = launch
            .current_version
            .take()
            .or_else(|| Some(build::version().to_owned()));
        launch.current = Some(bin.to_string_lossy().into_owned());
        launch.current_version = Some(manifest.version.clone());
        launch.trial_attempts = 1;
        launch.bad.retain(|v| v != &manifest.version);
        launch.save(&data_dir)?;
        anyhow::Ok(())
    }
    .await;
    match result {
        Ok(()) => {
            log(
                app,
                "server",
                None,
                "Familiar",
                Some(build::version()),
                &manifest.version,
                "installed",
                None,
            )
            .await;
            restart(
                app,
                &format!("Restarting into Familiar {}", manifest.version),
            )
            .await;
            Ok(())
        }
        Err(e) => {
            let msg = format!("{e:#}");
            log(
                app,
                "server",
                None,
                "Familiar",
                Some(build::version()),
                &manifest.version,
                "failed",
                Some(&msg),
            )
            .await;
            app.updates
                .set("failed", Some(format!("The update didn't install: {msg}")));
            publish(app).await;
            Err(e)
        }
    }
}

/// Go back to the previous server binary.
pub async fn rollback(app: &App) -> anyhow::Result<()> {
    let data_dir = app.config.data_dir.clone();
    let mut launch = Launch::load(&data_dir);
    let (Some(prev), Some(prev_version)) = (launch.previous.take(), launch.previous_version.take())
    else {
        bail!("there's no previous version to go back to");
    };
    if let Some(v) = launch.current_version.take() {
        launch.bad.push(v.clone());
        log(
            app,
            "server",
            None,
            "Familiar",
            Some(&v),
            &prev_version,
            "rolled_back",
            Some("Rolled back by an admin"),
        )
        .await;
    }
    let running = std::env::current_exe()
        .ok()
        .map(|p| p.to_string_lossy().into_owned());
    launch.current = (Some(prev.clone()) != running).then_some(prev);
    launch.current_version = launch.current.as_ref().map(|_| prev_version.clone());
    launch.trial_attempts = 0;
    launch.save(&data_dir)?;
    restart(app, &format!("Restarting into Familiar {prev_version}")).await;
    Ok(())
}

async fn restart(app: &App, detail: &str) {
    app.updates.set("restarting", Some(detail.to_owned()));
    publish(app).await;
    app.updates.restart_requested.store(true, Ordering::SeqCst);
    // Give the portal a moment to receive the event before the server goes away.
    let app = app.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(2)).await;
        app.updates.restart.notify_one();
    });
}

/// After a minute of serving, a trial build is considered healthy.
pub fn spawn_health_commit(app: App) {
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(60)).await;
        if app
            .db
            .call(|c| c.query_row("SELECT 1", [], |r| r.get::<_, i64>(0)))
            .await
            .is_err()
        {
            return;
        }
        let mut launch = Launch::load(&app.config.data_dir);
        if launch.trial_attempts > 0 && launch.current_version.as_deref() == Some(build::version())
        {
            launch.trial_attempts = 0;
            if let Err(e) = launch.save(&app.config.data_dir) {
                tracing::warn!(error = %e, "couldn't record that this update is healthy");
            } else {
                tracing::info!(version = build::version(), "update is healthy");
            }
        }
    });
}

/// The channel a Mac's agent follows.
pub fn effective_channel(agent_channel: &str, s: &Settings) -> String {
    match agent_channel {
        "stable" | "nightly" | "off" => agent_channel.to_owned(),
        _ => s.guest_default_channel.clone(),
    }
}

/// Stream a new agent to a Mac over its link and wait for it to come back.
pub async fn update_agent(app: &App, instance_id: &str, manifest: &Manifest) -> anyhow::Result<()> {
    {
        let mut set = app
            .updates
            .updating
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !set.insert(instance_id.to_owned()) {
            bail!("this Mac's agent is already updating");
        }
    }
    crate::api::publish_instance(app, instance_id).await;
    let result = push_agent(app, instance_id, manifest).await;
    app.updates
        .updating
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .remove(instance_id);
    crate::api::publish_instance(app, instance_id).await;
    result
}

async fn push_agent(app: &App, instance_id: &str, manifest: &Manifest) -> anyhow::Result<()> {
    use dot_wire::{Op, UPDATE_CHUNK, UpdateOp};
    let machine = app.fleet.machine(instance_id);
    let link = machine.link().context("the Mac isn't connected")?;
    anyhow::ensure!(
        link.info.capabilities.iter().any(|c| c == "update"),
        "this Mac's agent is too old to update itself; reinstall it from the Connect page"
    );
    let from = link.agent_version.clone();
    if from == manifest.version {
        return Ok(());
    }
    let iid = instance_id.to_owned();
    let name: String = app
        .db
        .call(move |c| {
            c.query_row("SELECT name FROM instances WHERE id = ?1", [iid], |r| {
                r.get(0)
            })
        })
        .await
        .unwrap_or_else(|_| "Mac".into());
    let path = dir(app)
        .join("guest")
        .join(format!("dot-guest-{}", manifest.version));
    let outcome: anyhow::Result<()> = async {
        download(app, &manifest.guest, &path).await?;
        let bytes = tokio::fs::read(&path).await?;
        let step = Duration::from_secs(60);
        link.call(
            Op::Update(UpdateOp::Begin {
                version: manifest.version.clone(),
                sha256: manifest.guest.sha256.clone(),
                size: bytes.len() as u64,
            }),
            step,
        )
        .await?;
        let mut offset = 0u64;
        for chunk in bytes.chunks(UPDATE_CHUNK) {
            link.call(
                Op::Update(UpdateOp::Chunk {
                    offset,
                    data: STANDARD.encode(chunk),
                }),
                step,
            )
            .await?;
            offset += chunk.len() as u64;
        }
        link.call(Op::Update(UpdateOp::Commit), Duration::from_secs(60))
            .await?;
        // The agent restarts; wait for the supervisor to reconnect to the new one.
        let deadline = std::time::Instant::now() + Duration::from_secs(180);
        loop {
            tokio::time::sleep(Duration::from_secs(2)).await;
            if let Some(l) = machine.link()
                && !std::sync::Arc::ptr_eq(&l, &link)
            {
                if l.agent_version == manifest.version {
                    return Ok(());
                }
                bail!(
                    "the Mac went back to {} (its agent rolled the update back)",
                    l.agent_version
                );
            }
            if std::time::Instant::now() > deadline {
                bail!("the Mac didn't reconnect within 3 minutes of updating");
            }
        }
    }
    .await;
    let (result, detail) = match &outcome {
        Ok(()) => ("installed", None),
        Err(e) if e.to_string().contains("rolled the update back") => {
            ("rolled_back", Some(format!("{e:#}")))
        }
        Err(e) => ("failed", Some(format!("{e:#}"))),
    };
    log(
        app,
        "agent",
        Some(instance_id),
        &name,
        Some(&from),
        &manifest.version,
        result,
        detail.as_deref(),
    )
    .await;
    if outcome.is_ok() {
        let (iid, v) = (instance_id.to_owned(), manifest.version.clone());
        let _ = app
            .db
            .call(move |c| {
                c.execute(
                    "UPDATE instances SET agent_version = ?2 WHERE id = ?1",
                    [iid, v],
                )
            })
            .await;
    }
    outcome
}

#[allow(clippy::too_many_arguments)]
async fn log(
    app: &App,
    target: &str,
    instance_id: Option<&str>,
    name: &str,
    from: Option<&str>,
    to: &str,
    outcome: &str,
    detail: Option<&str>,
) {
    let (t, i, n, f, to, o, d) = (
        target.to_owned(),
        instance_id.map(str::to_owned),
        name.to_owned(),
        from.map(str::to_owned),
        to.to_owned(),
        outcome.to_owned(),
        detail.map(|d| crate::util::truncate(d, 500)),
    );
    let _ = app
        .db
        .call(move |c| {
            c.execute(
                "INSERT INTO update_log (at, target, instance_id, target_name, from_version, to_version, outcome, detail)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                rusqlite::params![now(), t, i, n, f, to, o, d],
            )
        })
        .await;
}

/// The portal's view.
pub async fn status(app: &App) -> Value {
    let s = settings(app).await;
    let (state, detail, last_checked, manifest) = {
        let l = app
            .updates
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        (
            if l.state.is_empty() { "idle" } else { l.state },
            l.detail.clone(),
            l.last_checked,
            l.manifests.get(&s.channel).cloned(),
        )
    };
    let history: Vec<Value> = app
        .db
        .call(|c| {
            let mut st = c.prepare(
                "SELECT id, at, target, target_name, from_version, to_version, outcome, detail FROM update_log ORDER BY id DESC LIMIT 50",
            )?;
            let rows = st.query_map([], |r| {
                Ok(json!({
                    "id": r.get::<_, i64>(0)?.to_string(), "at": rfc3339(r.get(1)?), "target": r.get::<_, String>(2)?,
                    "target_name": r.get::<_, String>(3)?, "from_version": r.get::<_, Option<String>>(4)?,
                    "to_version": r.get::<_, String>(5)?, "outcome": r.get::<_, String>(6)?, "detail": r.get::<_, Option<String>>(7)?
                }))
            })?;
            rows.collect()
        })
        .await
        .unwrap_or_default();
    let launch = Launch::load(&app.config.data_dir);
    json!({
        "enabled": disabled_reason().is_none(),
        "disabled_reason": disabled_reason(),
        "current": { "version": build::version(), "commit": build::commit(), "channel": build::channel() },
        "previous": launch.previous_version.map(|v| json!({ "version": v })),
        "settings": s,
        "available": manifest.map(|m| json!({
            "version": m.version, "commit": m.commit, "published_at": m.published_at, "notes": m.notes, "url": m.url,
            "newer": is_newer(&m.version, build::version()),
        })),
        "last_checked_at": rfc3339_opt(last_checked),
        "state": state,
        "detail": detail,
        "history": history,
    })
}

/// Send the status to admins' live streams.
pub async fn publish(app: &App) {
    let v = status(app).await;
    app.events.admins("updates", v);
}

/// Is `hour` inside the 2-hour window starting at `start` (server local time).
pub fn in_window(hour: u8, start: u8) -> bool {
    let end = (start + 2) % 24;
    if start < end {
        (start..end).contains(&hour)
    } else {
        hour >= start || hour < end
    }
}

/// Background: check every 6 hours, install in the window, update idle agents.
pub fn spawn_scheduler(app: App) {
    if disabled_reason().is_some() {
        return;
    }
    tokio::spawn(async move {
        // Spread checks so a fleet of servers doesn't hit GitHub at once.
        let jitter = u64::from(dot_wire::random_bytes::<1>()[0]) * 2;
        tokio::time::sleep(Duration::from_secs(60 + jitter)).await;
        let mut tick = tokio::time::interval(Duration::from_secs(900));
        loop {
            tick.tick().await;
            let s = settings(&app).await;
            let stale = app
                .updates
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .last_checked
                .is_none_or(|t| now() - t > 6 * 3600);
            if stale && (s.channel != "off" || s.guest_default_channel != "off") {
                check(&app).await;
            }
            let hour = jiff::Zoned::now().hour().clamp(0, 23) as u8;
            if s.auto_install
                && s.channel != "off"
                && in_window(hour, s.window_start_hour)
                && let Some(m) = app.updates.manifest(&s.channel)
                && is_newer(&m.version, build::version())
            {
                tracing::info!(version = %m.version, "installing server update in the maintenance window");
                let _ = install_server(&app, &m).await;
                continue;
            }
            if s.guest_auto_install {
                auto_update_agents(&app, &s).await;
            }
        }
    });
}

async fn auto_update_agents(app: &App, s: &Settings) {
    let rows: Vec<(String, String)> = app
        .db
        .call(|c| {
            let mut st =
                c.prepare("SELECT id, agent_channel FROM instances WHERE is_builder = 0")?;
            let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
            rows.collect()
        })
        .await
        .unwrap_or_default();
    for (id, ch) in rows {
        let ch = effective_channel(&ch, s);
        let Some(m) = app.updates.manifest(&ch) else {
            continue;
        };
        let machine = app.fleet.machine(&id);
        let Some(link) = machine.link() else { continue };
        // Only when nobody is using the Mac and only forward on the same channel.
        if link.agent_version == m.version
            || !is_newer(&m.version, &link.agent_version)
            || !machine.is_idle(600)
        {
            continue;
        }
        if let Err(e) = update_agent(app, &id, &m).await {
            tracing::warn!(instance = %id, error = %format!("{e:#}"), "agent update failed");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_order_releases_and_nightlies() {
        assert!(is_newer("0.3.1", "0.3.0"));
        assert!(is_newer("0.10.0", "0.9.9"));
        assert!(
            is_newer("0.3.0", "0.3.0-nightly.202610200312.ad02bf1"),
            "a release beats its nightlies"
        );
        assert!(is_newer(
            "0.3.1-nightly.202610210000.bbbbbbb",
            "0.3.1-nightly.202610200000.aaaaaaa"
        ));
        assert!(!is_newer("0.3.0", "0.3.0"));
        assert!(!is_newer("0.2.9", "0.3.0"));
        assert!(safe_version("0.3.0-nightly.202610200312.ad02bf1"));
        assert!(!safe_version("../evil"));
        assert!(!safe_version(""));
    }

    #[test]
    fn manifests_must_be_signed_by_a_trusted_key() {
        let (seed, public) = keygen();
        let (_, other) = keygen();
        let m = Manifest {
            schema: 1,
            channel: "stable".into(),
            version: "0.4.0".into(),
            commit: "abc".into(),
            published_at: "2026-10-20T00:00:00Z".into(),
            notes: Some("Faster screenshots".into()),
            url: None,
            server: Artifact {
                url: "https://e.x/s".into(),
                sha256: "a".repeat(64),
                size: 10,
            },
            guest: Artifact {
                url: "https://e.x/g".into(),
                sha256: "b".repeat(64),
                size: 10,
            },
        };
        let bytes = serde_json::to_vec(&m).unwrap();
        let sig = sign_manifest(&bytes, &seed).unwrap();
        assert_eq!(verify_manifest(&bytes, &sig, &[&public]).unwrap(), m);
        assert!(
            verify_manifest(&bytes, &sig, &[&other]).is_err(),
            "wrong key"
        );
        let mut tampered = bytes.clone();
        tampered[10] ^= 1;
        assert!(
            verify_manifest(&tampered, &sig, &[&public]).is_err(),
            "tampered"
        );
        let mut http = m.clone();
        http.server.url = "http://e.x/s".into();
        let b = serde_json::to_vec(&http).unwrap();
        assert!(
            verify_manifest(&b, &sign_manifest(&b, &seed).unwrap(), &[&public]).is_err(),
            "https only"
        );
    }

    #[test]
    fn launcher_runs_updates_on_trial_and_rolls_back_crash_loops() {
        let dir = tempfile::tempdir().unwrap();
        let new = dir.path().join("dot-home-0.4.0");
        std::fs::write(&new, b"x").unwrap();
        let mut l = Launch {
            current: Some(new.to_string_lossy().into_owned()),
            current_version: Some("0.4.0".into()),
            previous: None,
            previous_version: None,
            trial_attempts: 1,
            bad: vec![],
        };
        // First and second starts run the update while it's on trial.
        assert_eq!(
            decide(&mut l, "0.3.0", false),
            LaunchDecision::Exec(new.clone())
        );
        assert_eq!(l.trial_attempts, 2);
        assert_eq!(
            decide(&mut l, "0.3.0", false),
            LaunchDecision::Exec(new.clone())
        );
        // It never proved healthy: the third start gives up on it.
        assert_eq!(decide(&mut l, "0.3.0", false), LaunchDecision::Stay);
        assert_eq!(l.bad, vec!["0.4.0".to_owned()]);
        assert!(l.current.is_none());

        // Healthy (trial cleared): runs every time; an exec'd child never execs again.
        let mut ok = Launch {
            current: Some(new.to_string_lossy().into_owned()),
            current_version: Some("0.4.0".into()),
            ..Launch::default()
        };
        assert_eq!(
            decide(&mut ok, "0.3.0", false),
            LaunchDecision::Exec(new.clone())
        );
        assert_eq!(decide(&mut ok, "0.3.0", true), LaunchDecision::Stay);
        // A newer image wins over an older installed update.
        assert_eq!(decide(&mut ok, "0.5.0", false), LaunchDecision::Stay);
        assert!(ok.current.is_none());
    }

    #[test]
    fn maintenance_window_wraps_midnight() {
        assert!(in_window(3, 3) && in_window(4, 3) && !in_window(5, 3));
        assert!(in_window(23, 23) && in_window(0, 23) && !in_window(1, 23));
        assert_eq!(
            effective_channel(
                "default",
                &Settings {
                    guest_default_channel: "nightly".into(),
                    ..Settings::default()
                }
            ),
            "nightly"
        );
        assert_eq!(effective_channel("off", &Settings::default()), "off");
    }
}
