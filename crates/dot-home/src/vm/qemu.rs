//! OSX-KVM on QEMU: files, command lines and process control for one Mac.
//!
//! Encrypted Macs (docs/home/SECURITY.md "Encryption at rest") keep their disk and
//! firmware variables in qcow2 files with QEMU's LUKS encryption, over the shared,
//! unencrypted base image. Their key reaches QEMU through [`secrets::SecretFile`],
//! never a command line. Saved memory goes through [`memfile`] instead of QEMU's
//! plaintext migrate-to-file, and live snapshots are internal disk snapshots plus such
//! a memory file, because QEMU can't `savevm` into an encrypted qcow2 (it aborts on an
//! unaligned write; checked with QEMU 11.1).
use super::{
    Activity, Crypt, Hypervisor, LaunchMode, memfile, qmp,
    secrets::{self, SECRET_ID, SecretFile},
};
use crate::{config::Vm as VmConfig, keys::MacKey};
use anyhow::{Context, bail};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    os::fd::{AsFd, OwnedFd},
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::{io::AsyncReadExt, process::Command};

/// `qemu-img` options that create an encrypted qcow2. The passphrase is a random
/// 256-bit key, so LUKS's own key stretching runs at its minimum.
const ENCRYPT_OPTS: &str = "encrypt.format=luks,encrypt.key-secret=dotkey,encrypt.iter-time=10";

/// The name saved memory's pipe goes by in QEMU (`getfd`, `fd:dotmem`).
const MEM_FD: &str = "dotmem";

/// Drives with internal snapshots: the Mac's disk and its UEFI variables.
const SNAPSHOT_DEVICES: [&str; 2] = ["MacHDD", "pflash1"];

/// Files that belong to one instance.
#[derive(Debug, Clone)]
pub struct Files {
    /// Instance directory.
    pub dir: PathBuf,
}

impl Files {
    /// `<data>/instances/<id>`.
    pub fn new(data_dir: &Path, id: &str) -> Self {
        Self {
            dir: data_dir.join("instances").join(id),
        }
    }
    /// Main disk (qcow2 overlay on the golden image).
    pub fn disk(&self) -> PathBuf {
        self.dir.join("disk.qcow2")
    }
    /// UEFI variables (qcow2 so live snapshots work).
    pub fn vars(&self) -> PathBuf {
        self.dir.join("vars.qcow2")
    }
    /// The DOTCONFIG disk.
    pub fn config_img(&self) -> PathBuf {
        self.dir.join("config.img")
    }
    /// QMP socket.
    pub fn qmp(&self) -> PathBuf {
        self.dir.join("qmp.sock")
    }
    /// VNC socket.
    pub fn vnc(&self) -> PathBuf {
        self.dir.join("vnc.sock")
    }
    /// QEMU pidfile.
    pub fn pid(&self) -> PathBuf {
        self.dir.join("qemu.pid")
    }
    /// Saved RAM while hibernated.
    pub fn hibernate(&self) -> PathBuf {
        self.dir.join("hibernate.state")
    }
    /// QEMU's stderr from the last launch.
    pub fn log(&self) -> PathBuf {
        self.dir.join("qemu.log")
    }
    /// Present while the Mac runs without QEMU's Apple SMC, so a resume matches its boot.
    pub fn no_smc(&self) -> PathBuf {
        self.dir.join("no-smc")
    }
    /// Live snapshots' saved memory, for encrypted Macs (others keep it in the disk).
    pub fn snapshot_memory_dir(&self) -> PathBuf {
        self.dir.join("snapshots")
    }
    /// One live snapshot's saved memory.
    pub fn snapshot_memory(&self, tag: &str) -> PathBuf {
        self.snapshot_memory_dir().join(format!("{tag}.mem"))
    }
}

/// Host-wide image paths.
#[derive(Debug, Clone)]
pub struct Images {
    /// The golden image.
    pub base: PathBuf,
    /// OpenCore boot disk.
    pub opencore: PathBuf,
    /// Read-only UEFI code.
    pub ovmf_code: PathBuf,
    /// UEFI variables template.
    pub ovmf_vars: PathBuf,
    /// The SMC key (empty when QEMU's SMC is off and the key couldn't be read).
    pub osk: String,
}

impl Images {
    /// Locate OSX-KVM's firmware and the golden image. Errors explain what's missing.
    pub fn locate(config: &VmConfig, data_dir: &Path) -> anyhow::Result<Self> {
        let osx = &config.osx_kvm_dir;
        let pick = |names: &[&str]| names.iter().map(|n| osx.join(n)).find(|p| p.exists());
        let opencore = pick(&["OpenCore/OpenCore.qcow2"])
            .with_context(|| format!("OpenCore.qcow2 not found in {}/OpenCore", osx.display()))?;
        let ovmf_code = pick(&["OVMF_CODE_4M.fd", "OVMF_CODE.fd"])
            .with_context(|| format!("OVMF_CODE not found in {}", osx.display()))?;
        let vars_name = format!("OVMF_VARS-{}.fd", config.resolution);
        let ovmf_vars = pick(&[vars_name.as_str(), "OVMF_VARS-1920x1080.fd", "OVMF_VARS.fd"])
            .with_context(|| format!("OVMF_VARS not found in {}", osx.display()))?;
        let osk = if !config.applesmc_osk.is_empty() {
            config.applesmc_osk.clone()
        } else if config.qemu_smc {
            read_osk(osx)?
        } else {
            read_osk(osx).unwrap_or_default()
        };
        Ok(Self {
            base: data_dir.join("images").join("base.qcow2"),
            opencore,
            ovmf_code,
            ovmf_vars,
            osk,
        })
    }
}

/// OSX-KVM's boot script carries the SMC key; read it rather than ship it.
fn read_osk(osx: &Path) -> anyhow::Result<String> {
    for script in ["OpenCore-Boot.sh", "boot-macOS.sh"] {
        if let Ok(text) = std::fs::read_to_string(osx.join(script))
            && let Some(start) = text.find("osk=\"")
        {
            let rest = &text[start + 5..];
            if let Some(end) = rest.find('"') {
                return Ok(rest[..end].to_owned());
            }
        }
    }
    bail!(
        "couldn't read the Apple SMC key from {}/OpenCore-Boot.sh; set vm.applesmc_osk",
        osx.display()
    )
}

/// A stable, locally administered MAC address per instance.
pub fn mac_address(id: &str) -> String {
    let h = crate::crypto::sha256(id.as_bytes());
    format!("52:54:00:{:02x}:{:02x}:{:02x}", h[0], h[1], h[2])
}

/// Everything needed to launch one Mac.
#[derive(Debug, Clone)]
pub struct Spec {
    /// Instance id.
    pub id: String,
    /// vCPUs.
    pub cpus: u32,
    /// RAM.
    pub memory_mb: u32,
    /// Host port forwarded to the guest agent.
    pub port: u16,
    /// Extra raw disk to attach (the macOS installer, for the image builder).
    pub installer: Option<PathBuf>,
}

/// Build QEMU's argument list. `smc` adds QEMU's Apple SMC. With `secret`, the disk and
/// firmware variables are opened with the key in that file; the key itself never appears here.
pub fn args(
    config: &VmConfig,
    images: &Images,
    files: &Files,
    spec: &Spec,
    mode: LaunchMode,
    smc: bool,
    secret: Option<&SecretFile>,
) -> Vec<String> {
    let p = |p: &Path| p.to_string_lossy().into_owned();
    let key = if secret.is_some() {
        format!(",encrypt.key-secret={SECRET_ID}")
    } else {
        String::new()
    };
    let cores = if spec.cpus >= 2 { spec.cpus / 2 } else { 1 };
    let threads = if spec.cpus >= 2 { 2 } else { 1 };
    let mut net = format!(
        "user,id=net0,hostfwd=tcp:{}:{}-:{}",
        config.forward_address,
        spec.port,
        dot_wire::GUEST_PORT
    );
    if config.restrict_network {
        net.push_str(",restrict=on");
    }
    let mut a: Vec<String> = [
        "-name".into(),
        format!("dot-{},process=dot-vm-{}", spec.id, spec.id),
        "-enable-kvm".into(),
        "-machine".into(),
        config.machine.clone(),
        "-cpu".into(),
        config.cpu_model.clone(),
        "-smp".into(),
        format!("{},cores={cores},threads={threads},sockets=1", spec.cpus),
        "-m".into(),
        format!("{}M", spec.memory_mb),
        "-smbios".into(),
        "type=2".into(),
        "-rtc".into(),
        "base=utc".into(),
        "-drive".into(),
        format!("if=pflash,format=raw,readonly=on,file={}", p(&images.ovmf_code)),
        "-drive".into(),
        format!("if=pflash,format=qcow2,file={}{key}", p(&files.vars())),
        "-device".into(),
        "qemu-xhci,id=xhci".into(),
        "-device".into(),
        "usb-kbd,bus=xhci.0".into(),
        "-device".into(),
        "usb-tablet,bus=xhci.0".into(),
        "-device".into(),
        "ich9-ahci,id=sata".into(),
        "-drive".into(),
        format!("id=OpenCoreBoot,if=none,snapshot=on,format=qcow2,file={}", p(&images.opencore)),
        "-device".into(),
        "ide-hd,bus=sata.2,drive=OpenCoreBoot,bootindex=1".into(),
        "-drive".into(),
        format!(
            "id=MacHDD,if=none,format=qcow2,cache=writeback,discard=unmap,detect-zeroes=unmap,file={}{key}",
            p(&files.disk())
        ),
        "-device".into(),
        "ide-hd,bus=sata.4,drive=MacHDD".into(),
        "-drive".into(),
        format!("id=DotConfig,if=none,format=raw,readonly=on,file={}", p(&files.config_img())),
        "-device".into(),
        "usb-storage,bus=xhci.0,drive=DotConfig,removable=on".into(),
        "-netdev".into(),
        net,
        "-device".into(),
        format!("virtio-net-pci,netdev=net0,id=net0,mac={}", mac_address(&spec.id)),
        "-device".into(),
        "vmware-svga".into(),
        "-display".into(),
        "none".into(),
        "-vnc".into(),
        format!("unix:{}", p(&files.vnc())),
        "-qmp".into(),
        format!("unix:{},server=on,wait=off", p(&files.qmp())),
        "-pidfile".into(),
        p(&files.pid()),
        "-daemonize".into(),
        "-no-user-config".into(),
    ]
    .into();
    if smc {
        a.extend(["-device".into(), format!("isa-applesmc,osk={}", images.osk)]);
    }
    if let Some(installer) = &spec.installer {
        a.extend([
            "-drive".into(),
            format!("id=InstallMedia,if=none,format=raw,file={}", p(installer)),
            "-device".into(),
            "ide-hd,bus=sata.3,drive=InstallMedia".into(),
        ]);
    }
    if mode == LaunchMode::Resume {
        a.extend(["-incoming".into(), "defer".into()]);
    }
    if let Some(secret) = secret {
        a.splice(2..2, ["-object".to_owned(), secret.object()]);
    }
    a
}

/// A pipe whose ends don't leak into other processes we start.
fn pipe() -> anyhow::Result<(OwnedFd, OwnedFd)> {
    #[cfg(target_os = "linux")]
    let (r, w) = nix::unistd::pipe2(nix::fcntl::OFlag::O_CLOEXEC)?;
    #[cfg(not(target_os = "linux"))]
    let (r, w) = {
        use nix::fcntl::{FcntlArg, FdFlag, fcntl};
        let (r, w) = nix::unistd::pipe()?;
        fcntl(&r, FcntlArg::F_SETFD(FdFlag::FD_CLOEXEC))?;
        fcntl(&w, FcntlArg::F_SETFD(FdFlag::FD_CLOEXEC))?;
        (r, w)
    };
    Ok((r, w))
}

/// `driver=qcow2,…` for `--image-opts`, with the key reference when encrypted.
fn image_opts(path: &Path, encrypted: bool) -> String {
    let mut s = format!("driver=qcow2,file.filename={}", secrets::escape(path));
    if encrypted {
        s.push_str(",encrypt.key-secret=");
        s.push_str(SECRET_ID);
    }
    s
}

/// `(12.34/100%)` from `qemu-img convert -p`, as 0 to 1.
pub fn parse_progress(line: &str) -> Option<f64> {
    let inner = line.trim().strip_prefix('(')?.strip_suffix("/100%)")?;
    inner
        .trim()
        .parse::<f64>()
        .ok()
        .map(|v| (v / 100.0).clamp(0.0, 1.0))
}

/// Save a stopped VM's memory to `dest`: plain `file:` without a key, else through
/// [`memfile::encrypt`] from a pipe. The file appears only once it's complete.
async fn save_memory(q: &mut qmp::Qmp, dest: &Path, key: Option<&MacKey>) -> anyhow::Result<()> {
    let partial = dest.with_extension("partial");
    if let Some(dir) = dest.parent() {
        tokio::fs::create_dir_all(dir).await?;
    }
    let writer = match key {
        None => {
            let uri = format!("file:{}", dest.to_string_lossy());
            q.execute("migrate", Some(json!({ "uri": uri }))).await?;
            None
        }
        Some(key) => {
            let (rd, wr) = pipe()?;
            q.send_fd(MEM_FD, wr.as_fd()).await?;
            drop(wr);
            let out = {
                use std::os::unix::fs::OpenOptionsExt;
                std::fs::OpenOptions::new()
                    .write(true)
                    .create(true)
                    .truncate(true)
                    .mode(0o600)
                    .open(&partial)?
            };
            let key = key.clone();
            let task = tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
                let mut w = std::io::BufWriter::with_capacity(1 << 20, out);
                memfile::encrypt(std::fs::File::from(rd), &mut w, &key)?;
                w.into_inner().map_err(|e| e.into_error())?.sync_all()?;
                Ok(())
            });
            if let Err(e) = q
                .execute("migrate", Some(json!({ "uri": format!("fd:{MEM_FD}") })))
                .await
            {
                // QEMU still holds the pipe; closing it ends the writer.
                let _ = q
                    .execute("closefd", Some(json!({ "fdname": MEM_FD })))
                    .await;
                let _ = task.await;
                let _ = tokio::fs::remove_file(&partial).await;
                return Err(e);
            }
            Some(task)
        }
    };
    let started = std::time::Instant::now();
    let outcome = loop {
        let info = q.execute("query-migrate", None).await?;
        match info.get("status").and_then(Value::as_str) {
            Some("completed") => break Ok(()),
            Some("failed" | "cancelled") => {
                break Err(anyhow::anyhow!("saving the Mac's memory failed"));
            }
            _ => {}
        }
        if started.elapsed() > Duration::from_secs(600) {
            let _ = q.execute("migrate_cancel", None).await;
            break Err(anyhow::anyhow!("saving the Mac's memory took too long"));
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    };
    match (outcome, writer) {
        (Ok(()), None) => Ok(()),
        (Err(e), None) => {
            let _ = tokio::fs::remove_file(dest).await;
            Err(e)
        }
        (outcome, Some(task)) => {
            // QEMU closes its end once the migration is cleaned up, which ends the writer.
            let written = tokio::time::timeout(Duration::from_secs(60), task)
                .await
                .map_err(|_| anyhow::anyhow!("writing the Mac's memory didn't finish"))
                .and_then(|r| r.map_err(anyhow::Error::from))
                .and_then(|r| r);
            match outcome.and(written) {
                Ok(()) => {
                    tokio::fs::rename(&partial, dest).await?;
                    Ok(())
                }
                Err(e) => {
                    let _ = tokio::fs::remove_file(&partial).await;
                    Err(e)
                }
            }
        }
    }
}

/// Load memory saved by [`save_memory`] into a QEMU started with `-incoming defer`, then
/// run it.
async fn load_memory(qmp_socket: &Path, state: &Path, key: Option<&MacKey>) -> anyhow::Result<()> {
    let mut q = qmp::Qmp::connect(qmp_socket).await?;
    let feeder = match key {
        None => {
            let uri = format!("file:{}", state.to_string_lossy());
            q.execute("migrate-incoming", Some(json!({ "uri": uri })))
                .await?;
            None
        }
        Some(key) => {
            let src = std::fs::File::open(state)
                .with_context(|| format!("opening {}", state.display()))?;
            let (rd, wr) = pipe()?;
            q.send_fd(MEM_FD, rd.as_fd()).await?;
            drop(rd);
            let key = key.clone();
            // If QEMU stops reading (it failed), the write fails and this ends.
            let task = tokio::task::spawn_blocking(move || {
                memfile::decrypt(
                    std::io::BufReader::with_capacity(1 << 20, src),
                    std::fs::File::from(wr),
                    &key,
                )
            });
            q.execute(
                "migrate-incoming",
                Some(json!({ "uri": format!("fd:{MEM_FD}") })),
            )
            .await?;
            Some(task)
        }
    };
    let started = std::time::Instant::now();
    loop {
        let status = q.status().await?;
        if status != "inmigrate" {
            break;
        }
        if started.elapsed() > Duration::from_secs(900) {
            bail!("restoring the Mac's memory took too long");
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    if let Some(task) = feeder {
        task.await??;
    }
    q.execute("cont", None).await?;
    Ok(())
}

/// The real hypervisor.
pub struct Qemu {
    /// Settings.
    pub config: VmConfig,
    /// Data directory.
    pub data_dir: PathBuf,
}

impl Qemu {
    async fn run(program: &Path, args: &[String]) -> anyhow::Result<String> {
        let out = Command::new(program)
            .args(args)
            .kill_on_drop(true)
            .output()
            .await
            .with_context(|| format!("running {}", program.display()))?;
        if !out.status.success() {
            bail!(
                "{} failed: {}",
                program.display(),
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    async fn qemu_img(&self, args: &[&str]) -> anyhow::Result<String> {
        self.qemu_img_with(None, args).await
    }

    /// `qemu-img <subcommand>` with the key available as `dotkey` when given.
    async fn qemu_img_with(&self, key: Option<&MacKey>, args: &[&str]) -> anyhow::Result<String> {
        let secret = key.map(|k| self.secret(k)).transpose()?;
        let mut all: Vec<String> = Vec::with_capacity(args.len() + 2);
        if let Some((sub, rest)) = args.split_first() {
            all.push((*sub).to_owned());
            if let Some(s) = &secret {
                all.extend(["--object".to_owned(), s.object()]);
            }
            all.extend(rest.iter().map(|s| (*s).to_owned()));
        }
        let out = Self::run(&self.config.qemu_img, &all).await;
        drop(secret);
        out
    }

    /// `qemu-img snapshot <op> <tag>` on one image, opening it with the key if encrypted.
    async fn img_snapshot(
        &self,
        key: Option<&MacKey>,
        op: &str,
        tag: &str,
        image: &Path,
    ) -> anyhow::Result<()> {
        match key {
            None => {
                self.qemu_img(&["snapshot", op, tag, &image.to_string_lossy()])
                    .await?
            }
            Some(key) => {
                self.qemu_img_with(
                    Some(key),
                    &[
                        "snapshot",
                        "--image-opts",
                        op,
                        tag,
                        &image_opts(image, true),
                    ],
                )
                .await?
            }
        };
        Ok(())
    }

    fn images(&self) -> anyhow::Result<Images> {
        Images::locate(&self.config, &self.data_dir)
    }

    /// The key in a private file for QEMU to read (deleted when dropped).
    fn secret(&self, key: &MacKey) -> anyhow::Result<SecretFile> {
        let dir = secrets::runtime_dir(self.config.runtime_dir.as_deref(), &self.data_dir)?;
        SecretFile::create(&dir, key.passphrase().as_bytes())
    }

    /// Start QEMU in the background, waiting for incoming memory when `incoming`.
    async fn start(
        &self,
        files: &Files,
        spec: &Spec,
        incoming: bool,
        key: Option<&MacKey>,
    ) -> anyhow::Result<()> {
        let images = self.images()?;
        anyhow::ensure!(
            self.status(files).await.is_none(),
            "a QEMU process for this Mac is still running; stop it first"
        );
        for stale in [files.qmp(), files.vnc(), files.pid()] {
            let _ = tokio::fs::remove_file(stale).await;
        }
        let mode = if incoming {
            LaunchMode::Resume
        } else {
            LaunchMode::Fresh
        };
        // A resumed kernel still has the SMC it booted with mapped, so keep it the same.
        let smc = match mode {
            LaunchMode::Resume => !files.no_smc().exists(),
            LaunchMode::Fresh => self.config.qemu_smc,
        };
        anyhow::ensure!(
            !smc || !images.osk.is_empty(),
            "couldn't read the Apple SMC key from {}/OpenCore-Boot.sh; set vm.applesmc_osk",
            self.config.osx_kvm_dir.display()
        );
        if smc {
            let _ = tokio::fs::remove_file(files.no_smc()).await;
        } else {
            tokio::fs::write(files.no_smc(), b"")
                .await
                .with_context(|| format!("writing {}", files.no_smc().display()))?;
        }
        let secret = key.map(|k| self.secret(k)).transpose()?;
        let args = args(
            &self.config,
            &images,
            files,
            spec,
            mode,
            smc,
            secret.as_ref(),
        );
        let (program, all_args) = match self.config.wrapper.split_first() {
            Some((first, rest)) => {
                let mut v: Vec<String> = rest
                    .iter()
                    .map(|a| {
                        a.replace("{id}", &spec.id)
                            .replace("{memory_mb}", &spec.memory_mb.to_string())
                            .replace("{cpus}", &spec.cpus.to_string())
                    })
                    .collect();
                v.push(self.config.qemu.to_string_lossy().into_owned());
                v.extend(args);
                (PathBuf::from(first), v)
            }
            None => (self.config.qemu.clone(), args),
        };
        // snapshot=on keeps its scratch copy in TMPDIR (/var/tmp by default), which a
        // read-only container can't write; point it at the data directory instead.
        let tmp = self.data_dir.join("tmp");
        tokio::fs::create_dir_all(&tmp)
            .await
            .with_context(|| format!("creating {}", tmp.display()))?;
        // -daemonize returns once QEMU has initialized (or failed), so the VM outlives us.
        let out = Command::new(&program)
            .args(&all_args)
            .env("TMPDIR", &tmp)
            .kill_on_drop(false)
            .output()
            .await
            .with_context(|| format!("starting {}", program.display()));
        // QEMU reads its secrets while it initializes; the key file isn't needed again.
        drop(secret);
        let out = out?;
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        let _ = tokio::fs::write(files.log(), &stderr).await;
        if !out.status.success() {
            bail!("QEMU didn't start: {}", first_line(&stderr));
        }
        Ok(())
    }

    /// Wait for this Mac's QEMU to be gone (after `kill`).
    async fn wait_gone(&self, files: &Files) {
        for _ in 0..100 {
            if self.status(files).await.is_none() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// Encrypted live snapshot: internal snapshots of the disks plus the memory in a
    /// [`memfile`], taken while the vCPUs are stopped.
    async fn live_snapshot(&self, files: &Files, tag: &str, key: &MacKey) -> anyhow::Result<()> {
        let mut q = qmp::Qmp::connect(&files.qmp()).await?;
        let running = q.status().await? == "running";
        q.execute("stop", None).await?;
        let mut taken = 0;
        let mut result = Ok(());
        for device in SNAPSHOT_DEVICES {
            if let Err(e) = q
                .execute(
                    "blockdev-snapshot-internal-sync",
                    Some(json!({ "device": device, "name": tag })),
                )
                .await
            {
                result = Err(e);
                break;
            }
            taken += 1;
        }
        if result.is_ok() {
            result = save_memory(&mut q, &files.snapshot_memory(tag), Some(key)).await;
        }
        if result.is_err() {
            for device in &SNAPSHOT_DEVICES[..taken] {
                let _ = q
                    .execute(
                        "blockdev-snapshot-delete-internal-sync",
                        Some(json!({ "device": device, "name": tag })),
                    )
                    .await;
            }
        }
        if running {
            // After a completed migration this also takes the disks back.
            let _ = q.execute("cont", None).await;
        }
        result
    }

    /// Run `qemu-img convert -p …`, reporting progress.
    async fn convert_with_progress(
        &self,
        args: &[String],
        progress: &(dyn Fn(f64) + Send + Sync),
    ) -> anyhow::Result<()> {
        let mut child = Command::new(&self.config.qemu_img)
            .args(args)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("running {}", self.config.qemu_img.display()))?;
        let mut stdout = child.stdout.take().context("no stdout")?;
        let mut stderr = child.stderr.take().context("no stderr")?;
        let errors = tokio::spawn(async move {
            let mut text = String::new();
            let _ = stderr.read_to_string(&mut text).await;
            text
        });
        let mut buf = [0u8; 512];
        let mut pending = String::new();
        loop {
            let n = stdout.read(&mut buf).await?;
            if n == 0 {
                break;
            }
            pending.push_str(&String::from_utf8_lossy(&buf[..n]));
            while let Some(i) = pending.find(['\r', '\n']) {
                if let Some(p) = parse_progress(&pending[..i]) {
                    progress(p);
                }
                pending.drain(..=i);
            }
        }
        let status = child.wait().await?;
        let errors = errors.await.unwrap_or_default();
        if !status.success() {
            bail!("qemu-img failed: {}", first_line(&errors));
        }
        Ok(())
    }
}

impl Hypervisor for Qemu {
    async fn provision(
        &self,
        files: &Files,
        disk_gb: u32,
        from_base: bool,
        crypt: &Crypt,
    ) -> anyhow::Result<()> {
        let images = self.images()?;
        let key = crypt.key()?;
        tokio::fs::create_dir_all(&files.dir).await?;
        if !files.disk().exists() {
            let disk = files.disk().to_string_lossy().into_owned();
            let size = format!("{disk_gb}G");
            let mut args = vec!["create", "-q", "-f", "qcow2"];
            if key.is_some() {
                // Only this Mac's own blocks are encrypted; the shared base stays as is.
                args.extend(["-o", ENCRYPT_OPTS]);
            }
            let base;
            if from_base {
                anyhow::ensure!(
                    images.base.exists(),
                    "there's no macOS base image yet; an admin needs to build one first"
                );
                // Resolve the base.qcow2 link so this Mac keeps its own base if a newer one is promoted.
                base = std::fs::canonicalize(&images.base)?
                    .to_string_lossy()
                    .into_owned();
                args.extend(["-b", &base, "-F", "qcow2"]);
            }
            args.extend([disk.as_str(), size.as_str()]);
            self.qemu_img_with(key, &args).await?;
        }
        if !files.vars().exists() {
            let src = images.ovmf_vars.to_string_lossy().into_owned();
            let dst = files.vars().to_string_lossy().into_owned();
            let mut args = vec!["convert", "-q", "-f", "raw", "-O", "qcow2"];
            if key.is_some() {
                args.extend(["-o", ENCRYPT_OPTS]);
            }
            args.extend([src.as_str(), dst.as_str()]);
            self.qemu_img_with(key, &args).await?;
        }
        Ok(())
    }

    async fn launch(
        &self,
        files: &Files,
        spec: &Spec,
        mode: LaunchMode,
        crypt: &Crypt,
    ) -> anyhow::Result<()> {
        let key = crypt.key()?;
        let resume = mode == LaunchMode::Resume;
        self.start(files, spec, resume, key).await?;
        if resume {
            if let Err(e) = load_memory(&files.qmp(), &files.hibernate(), key).await {
                // Never leave an unmanaged QEMU behind; the saved memory is no longer trusted.
                let _ = self.kill(files).await;
                let _ = tokio::fs::remove_file(files.hibernate()).await;
                return Err(e.context(
                    "restoring the saved memory failed, so the Mac will start fresh next time",
                ));
            }
            let _ = tokio::fs::remove_file(files.hibernate()).await;
        }
        Ok(())
    }

    async fn status(&self, files: &Files) -> Option<String> {
        if let Ok(mut q) = qmp::Qmp::connect(&files.qmp()).await
            && let Ok(status) = q.status().await
        {
            return Some(status);
        }
        // QMP takes one client at a time, so a busy socket doesn't mean QEMU is gone.
        live_pid(files).map(|_| "unreachable".to_owned())
    }

    async fn activity(&self, files: &Files) -> Option<Activity> {
        let pid = live_pid(files)?;
        let mut q = qmp::Qmp::connect(&files.qmp()).await.ok()?;
        let io_ops = block_ops(&q.execute("query-blockstats", None).await.ok()?);
        let rips = instruction_pointers(&q.hmp("info registers -a").await.ok()?);
        let stat = tokio::fs::read_to_string(format!("/proc/{pid}/stat"))
            .await
            .ok()?;
        Some(Activity {
            io_ops,
            cpu_seconds: cpu_seconds(&stat)?,
            rips,
        })
    }

    async fn powerdown(&self, files: &Files) -> anyhow::Result<()> {
        qmp::once(&files.qmp(), "system_powerdown", None)
            .await
            .map(|_| ())
    }

    async fn kill(&self, files: &Files) -> anyhow::Result<()> {
        if qmp::once(&files.qmp(), "quit", None).await.is_ok() {
            return Ok(());
        }
        if let Some(pid) = live_pid(files) {
            let _ = nix::sys::signal::kill(
                nix::unistd::Pid::from_raw(pid),
                nix::sys::signal::Signal::SIGKILL,
            );
        }
        Ok(())
    }

    async fn reset(&self, files: &Files) -> anyhow::Result<()> {
        qmp::once(&files.qmp(), "system_reset", None)
            .await
            .map(|_| ())
    }

    async fn pause(&self, files: &Files) -> anyhow::Result<()> {
        qmp::once(&files.qmp(), "stop", None).await.map(|_| ())
    }

    async fn resume(&self, files: &Files) -> anyhow::Result<()> {
        qmp::once(&files.qmp(), "cont", None).await.map(|_| ())
    }

    async fn hibernate(&self, files: &Files, crypt: &Crypt) -> anyhow::Result<()> {
        let key = crypt.key()?;
        let mut q = qmp::Qmp::connect(&files.qmp()).await?;
        q.execute("stop", None).await?;
        if let Err(e) = save_memory(&mut q, &files.hibernate(), key).await {
            let _ = q.execute("cont", None).await;
            return Err(e);
        }
        let _ = q.execute("quit", None).await;
        Ok(())
    }

    async fn snapshot(
        &self,
        files: &Files,
        tag: &str,
        live: bool,
        crypt: &Crypt,
    ) -> anyhow::Result<()> {
        let key = crypt.key()?;
        match (live, key) {
            (true, None) => {
                qmp::Qmp::connect(&files.qmp())
                    .await?
                    .hmp(&format!("savevm {tag}"))
                    .await?;
            }
            (true, Some(key)) => self.live_snapshot(files, tag, key).await?,
            (false, key) => self.img_snapshot(key, "-c", tag, &files.disk()).await?,
        }
        Ok(())
    }

    async fn restore(
        &self,
        files: &Files,
        tag: &str,
        running: bool,
        crypt: &Crypt,
        spec: &Spec,
    ) -> anyhow::Result<()> {
        let Some(key) = crypt.key()? else {
            if running {
                qmp::Qmp::connect(&files.qmp())
                    .await?
                    .hmp(&format!("loadvm {tag}"))
                    .await?;
            } else {
                let disk = files.disk().to_string_lossy().into_owned();
                self.qemu_img(&["snapshot", "-a", tag, &disk]).await?;
            }
            return Ok(());
        };
        // No loadvm for encrypted disks: stop QEMU, revert the disks, and start again
        // from the snapshot's saved memory (or cold, for a disk-only snapshot).
        if running {
            self.kill(files).await?;
            self.wait_gone(files).await;
        }
        self.img_snapshot(Some(key), "-a", tag, &files.disk())
            .await?;
        // Disk-only snapshots taken while off have no copy of the variables.
        let _ = self.img_snapshot(Some(key), "-a", tag, &files.vars()).await;
        if running {
            let memory = files.snapshot_memory(tag);
            let has_memory = memory.exists();
            self.start(files, spec, has_memory, Some(key)).await?;
            if has_memory && let Err(e) = load_memory(&files.qmp(), &memory, Some(key)).await {
                let _ = self.kill(files).await;
                return Err(e.context("restoring the snapshot's memory failed"));
            }
        }
        Ok(())
    }

    async fn delete_snapshot(
        &self,
        files: &Files,
        tag: &str,
        running: bool,
        crypt: &Crypt,
    ) -> anyhow::Result<()> {
        if !crypt.is_encrypted() {
            if running {
                qmp::Qmp::connect(&files.qmp())
                    .await?
                    .hmp(&format!("delvm {tag}"))
                    .await?;
            } else {
                let disk = files.disk().to_string_lossy().into_owned();
                self.qemu_img(&["snapshot", "-d", tag, &disk]).await?;
                let vars = files.vars().to_string_lossy().into_owned();
                let _ = self.qemu_img(&["snapshot", "-d", tag, &vars]).await;
            }
            return Ok(());
        }
        if running {
            // QEMU already has the key, so this works even while the owner is locked.
            let mut q = qmp::Qmp::connect(&files.qmp()).await?;
            for (i, device) in SNAPSHOT_DEVICES.iter().enumerate() {
                let r = q
                    .execute(
                        "blockdev-snapshot-delete-internal-sync",
                        Some(json!({ "device": device, "name": tag })),
                    )
                    .await;
                if i == 0 {
                    r?;
                }
            }
        } else {
            let key = crypt.key()?;
            self.img_snapshot(key, "-d", tag, &files.disk()).await?;
            let _ = self.img_snapshot(key, "-d", tag, &files.vars()).await;
        }
        match tokio::fs::remove_file(files.snapshot_memory(tag)).await {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
            _ => Ok(()),
        }
    }

    async fn encrypt(
        &self,
        files: &Files,
        key: &MacKey,
        progress: &(dyn Fn(f64) + Send + Sync),
    ) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.status(files).await.is_none(),
            "the Mac is still running; shut it down first"
        );
        let disk = files.disk();
        let info: Value = serde_json::from_str(
            &self
                .qemu_img(&["info", "--output=json", &disk.to_string_lossy()])
                .await?,
        )?;
        anyhow::ensure!(
            !info["encrypted"].as_bool().unwrap_or(false),
            "this Mac's disk is already encrypted"
        );
        let backing = info
            .get("full-backing-filename")
            .or_else(|| info.get("backing-filename"))
            .and_then(Value::as_str)
            .map(str::to_owned);
        let new_disk = files.dir.join("disk.qcow2.encrypting");
        let new_vars = files.dir.join("vars.qcow2.encrypting");
        let cleanup = || {
            let _ = std::fs::remove_file(&new_disk);
            let _ = std::fs::remove_file(&new_vars);
        };
        cleanup();
        let secret = self.secret(key)?;
        let p = |p: &Path| p.to_string_lossy().into_owned();
        let mut args: Vec<String> = ["convert", "-p", "--object"]
            .into_iter()
            .map(str::to_owned)
            .collect();
        args.push(secret.object());
        args.extend(
            ["-f", "qcow2", "-O", "qcow2", "-o", ENCRYPT_OPTS]
                .into_iter()
                .map(str::to_owned),
        );
        if let Some(base) = &backing {
            // Keep sharing the base: only this Mac's own blocks are copied (and encrypted).
            args.extend([
                "-B".to_owned(),
                base.clone(),
                "-F".to_owned(),
                "qcow2".to_owned(),
            ]);
        }
        args.extend([p(&disk), p(&new_disk)]);
        let result = async {
            self.convert_with_progress(&args, &|f| progress(f * 0.97))
                .await?;
            if files.vars().exists() {
                let mut v: Vec<String> = vec!["convert".into(), "-q".into(), "--object".into()];
                v.push(secret.object());
                v.extend(
                    ["-f", "qcow2", "-O", "qcow2", "-o", ENCRYPT_OPTS]
                        .into_iter()
                        .map(str::to_owned),
                );
                v.extend([p(&files.vars()), p(&new_vars)]);
                Self::run(&self.config.qemu_img, &v).await?;
            }
            anyhow::Ok(())
        }
        .await;
        drop(secret);
        if let Err(e) = result {
            cleanup();
            return Err(e);
        }
        tokio::fs::rename(&new_disk, &disk).await?;
        if new_vars.exists() {
            tokio::fs::rename(&new_vars, files.vars()).await?;
        }
        // Converting drops internal snapshots, and any saved memory belongs to the old disk.
        let _ = tokio::fs::remove_file(files.hibernate()).await;
        let _ = tokio::fs::remove_dir_all(files.snapshot_memory_dir()).await;
        progress(1.0);
        Ok(())
    }

    async fn disk_bytes(&self, files: &Files) -> Option<u64> {
        let out = self
            .qemu_img(&["info", "--output=json", &files.disk().to_string_lossy()])
            .await
            .ok()?;
        serde_json::from_str::<Value>(&out)
            .ok()?
            .get("actual-size")?
            .as_u64()
    }

    async fn snapshot_state_bytes(&self, files: &Files) -> Option<u64> {
        if !files.disk().exists() {
            return Some(0);
        }
        // A running QEMU holds the image lock; reading its metadata is still safe.
        let out = self
            .qemu_img(&[
                "info",
                "--force-share",
                "--output=json",
                &files.disk().to_string_lossy(),
            ])
            .await
            .ok()?;
        vm_state_bytes(&out)
    }

    async fn snapshot_sizes(&self, files: &Files) -> Option<HashMap<String, u64>> {
        if !files.disk().exists() {
            return Some(HashMap::new());
        }
        let out = self
            .qemu_img(&[
                "info",
                "--force-share",
                "--output=json",
                &files.disk().to_string_lossy(),
            ])
            .await
            .ok()?;
        let mut sizes = vm_state_by_tag(&out)?;
        // Encrypted Macs keep live snapshots' memory in files of their own.
        if let Ok(entries) = std::fs::read_dir(files.snapshot_memory_dir()) {
            for entry in entries.filter_map(Result::ok) {
                let path = entry.path();
                if path.extension().is_some_and(|e| e == "mem")
                    && let Some(tag) = path.file_stem().and_then(|s| s.to_str())
                {
                    *sizes.entry(tag.to_owned()).or_default() += crate::storage::allocated(&path);
                }
            }
        }
        Some(sizes)
    }
}

/// `snapshots[].vm-state-size` by `snapshots[].name` in `qemu-img info --output=json`.
pub fn vm_state_by_tag(info_json: &str) -> Option<HashMap<String, u64>> {
    let info: Value = serde_json::from_str(info_json).ok()?;
    Some(
        info.get("snapshots")
            .and_then(Value::as_array)
            .map(|snaps| {
                snaps
                    .iter()
                    .filter_map(|s| {
                        let name = s.get("name")?.as_str()?.to_owned();
                        let size = s.get("vm-state-size").and_then(Value::as_u64).unwrap_or(0);
                        Some((name, size))
                    })
                    .collect()
            })
            .unwrap_or_default(),
    )
}

/// Sum of `snapshots[].vm-state-size` in `qemu-img info --output=json`.
pub fn vm_state_bytes(info_json: &str) -> Option<u64> {
    let info: Value = serde_json::from_str(info_json).ok()?;
    Some(
        info.get("snapshots")
            .and_then(Value::as_array)
            .map(|snaps| {
                snaps
                    .iter()
                    .filter_map(|s| s.get("vm-state-size").and_then(Value::as_u64))
                    .sum()
            })
            .unwrap_or(0),
    )
}

/// The QEMU process in the pidfile, if it's alive and still this Mac's (pids get reused).
fn live_pid(files: &Files) -> Option<i32> {
    let pid: i32 = std::fs::read_to_string(files.pid())
        .ok()?
        .trim()
        .parse()
        .ok()?;
    if pid <= 1 {
        return None;
    }
    match std::fs::read(format!("/proc/{pid}/cmdline")) {
        Ok(cmdline) => {
            let qmp = files.qmp();
            let needle = qmp.as_os_str().as_encoded_bytes();
            cmdline
                .windows(needle.len())
                .any(|w| w == needle)
                .then_some(pid)
        }
        Err(_) if Path::new("/proc/self").exists() => None,
        // No /proc (a macOS dev machine): settle for "some process has this pid".
        Err(_) => nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None)
            .is_ok()
            .then_some(pid),
    }
}

/// Reads, writes and flushes across every disk in `query-blockstats`.
fn block_ops(stats: &Value) -> u64 {
    stats
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|dev| dev.get("stats"))
        .map(|s| {
            ["rd_operations", "wr_operations", "flush_operations"]
                .iter()
                .filter_map(|k| s.get(*k).and_then(Value::as_u64))
                .sum::<u64>()
        })
        .sum()
}

/// Every vCPU's `RIP=` in HMP's `info registers -a`.
fn instruction_pointers(registers: &str) -> Vec<u64> {
    registers
        .split_whitespace()
        .filter_map(|w| w.strip_prefix("RIP="))
        .filter_map(|hex| u64::from_str_radix(hex, 16).ok())
        .collect()
}

/// User plus system CPU time from `/proc/<pid>/stat`.
fn cpu_seconds(stat: &str) -> Option<f64> {
    // The command name can hold spaces and parentheses; the fields after it can't.
    let fields: Vec<&str> = stat
        .get(stat.rfind(')')? + 1..)?
        .split_whitespace()
        .collect();
    let utime: u64 = fields.get(11)?.parse().ok()?;
    let stime: u64 = fields.get(12)?.parse().ok()?;
    // /proc reports in USER_HZ, which Linux fixes at 100 for userspace.
    #[allow(clippy::cast_precision_loss)]
    Some((utime + stime) as f64 / 100.0)
}

fn first_line(text: &str) -> String {
    let line = text
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("no output");
    crate::util::truncate(line.trim(), 300)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_snapshot_memory_is_summed_from_qemu_img_info() {
        let info = r#"{"virtual-size": 137438953472, "actual-size": 21474836480,
            "snapshots": [
                {"id": "1", "name": "dot-a", "vm-state-size": 4294967296, "date-sec": 1},
                {"id": "2", "name": "dot-b", "vm-state-size": 0, "date-sec": 2},
                {"id": "3", "name": "dot-c", "vm-state-size": 1073741824, "date-sec": 3}
            ]}"#;
        assert_eq!(vm_state_bytes(info), Some(5_368_709_120));
        let by_tag = vm_state_by_tag(info).unwrap();
        assert_eq!(by_tag["dot-a"], 4_294_967_296);
        assert_eq!(by_tag["dot-b"], 0);
        assert_eq!(by_tag.len(), 3);
        assert_eq!(
            vm_state_bytes(r#"{"actual-size": 1}"#),
            Some(0),
            "no snapshots"
        );
        assert_eq!(vm_state_bytes("qemu-img: Could not open"), None);
    }

    #[test]
    fn command_line_isolates_the_guest_and_resumes() {
        let config = VmConfig {
            restrict_network: true,
            ..VmConfig::default()
        };
        let images = Images {
            base: "/d/images/base.qcow2".into(),
            opencore: "/o/OpenCore/OpenCore.qcow2".into(),
            ovmf_code: "/o/OVMF_CODE.fd".into(),
            ovmf_vars: "/o/OVMF_VARS-1920x1080.fd".into(),
            osk: "key".into(),
        };
        let files = Files::new(Path::new("/d"), "abc");
        let spec = Spec {
            id: "abc".into(),
            cpus: 4,
            memory_mb: 8192,
            port: 17001,
            installer: None,
        };
        let a = args(
            &config,
            &images,
            &files,
            &spec,
            LaunchMode::Fresh,
            false,
            None,
        )
        .join(" ");
        assert!(
            a.contains("hostfwd=tcp:127.77.0.1:17001-:7466,restrict=on"),
            "guests can't reach this address"
        );
        assert!(a.contains("-machine pc-q35-8.2"));
        assert!(a.contains("-vnc unix:/d/instances/abc/vnc.sock"));
        assert!(a.contains("-smp 4,cores=2,threads=2,sockets=1"));
        assert!(a.contains("readonly=on,file=/d/instances/abc/config.img"));
        assert!(a.contains("-daemonize"));
        assert!(!a.contains("-incoming"));
        assert!(!a.contains("0.0.0.0"), "nothing listens publicly");
        let r = args(
            &config,
            &images,
            &files,
            &spec,
            LaunchMode::Resume,
            false,
            None,
        )
        .join(" ");
        assert!(r.ends_with("-incoming defer"));
        assert!(
            !a.contains("isa-applesmc"),
            "VirtualSMC answers unless QEMU's SMC is asked for"
        );
        let smc = args(
            &config,
            &images,
            &files,
            &spec,
            LaunchMode::Fresh,
            true,
            None,
        )
        .join(" ");
        assert!(smc.contains("-device isa-applesmc,osk=key"));
        assert_eq!(mac_address("abc"), mac_address("abc"));
        assert_ne!(mac_address("abc"), mac_address("abd"));
    }

    #[test]
    fn reads_what_a_frozen_guest_looks_like() {
        let stats = json!([
            {"device": "", "qdev": "sata.2", "stats": {"rd_operations": 10, "wr_operations": 0, "flush_operations": 0}},
            {"device": "", "qdev": "sata.4", "stats": {"rd_operations": 100, "wr_operations": 40, "flush_operations": 2}}
        ]);
        assert_eq!(block_ops(&stats), 152);
        assert_eq!(block_ops(&json!({})), 0);
        let regs = "CPU#0\nRAX=0000000000000000 RBX=0000000000000001\nRIP=ffffff8012a4c3b0 RFL=00000246 [---Z-P-] CPL=0\n\
                    CPU#1\nRIP=000000007fe1a2b4 RFL=00000002\n";
        assert_eq!(
            instruction_pointers(regs),
            vec![0xffff_ff80_12a4_c3b0, 0x7fe1_a2b4]
        );
        let stat =
            "4242 (dot-vm-a b) S 1 4242 4242 0 -1 4194560 100 0 0 0 1234 566 0 0 20 0 9 0 50 1 2";
        assert_eq!(cpu_seconds(stat), Some(18.0));
        assert_eq!(cpu_seconds("garbage"), None);
    }

    #[test]
    fn a_dead_or_foreign_pid_is_not_this_macs_qemu() {
        let dir = tempfile::tempdir().unwrap();
        let files = Files::new(dir.path(), "abc");
        std::fs::create_dir_all(&files.dir).unwrap();
        assert_eq!(live_pid(&files), None, "no pidfile");
        std::fs::write(files.pid(), "1\n").unwrap();
        assert_eq!(live_pid(&files), None, "never init");
        // This test process is alive, but its command line doesn't name this Mac's socket.
        std::fs::write(files.pid(), std::process::id().to_string()).unwrap();
        if Path::new("/proc/self").exists() {
            assert_eq!(live_pid(&files), None);
        }
    }

    #[test]
    fn reads_the_smc_key_from_the_boot_script() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("OpenCore-Boot.sh"),
            "args=( -device isa-applesmc,osk=\"some(c)key\" )",
        )
        .unwrap();
        assert_eq!(read_osk(dir.path()).unwrap(), "some(c)key");
        assert!(read_osk(Path::new("/nonexistent")).is_err());
    }

    fn test_images() -> Images {
        Images {
            base: "/d/images/base.qcow2".into(),
            opencore: "/o/OpenCore/OpenCore.qcow2".into(),
            ovmf_code: "/o/OVMF_CODE.fd".into(),
            ovmf_vars: "/o/OVMF_VARS-1920x1080.fd".into(),
            osk: "key".into(),
        }
    }

    fn test_spec() -> Spec {
        Spec {
            id: "abc".into(),
            cpus: 4,
            memory_mb: 8192,
            port: 17001,
            installer: None,
        }
    }

    #[test]
    fn command_line_names_the_key_file_but_never_the_key() {
        let dir = tempfile::tempdir().unwrap();
        let run = secrets::runtime_dir(Some(&dir.path().join("run")), dir.path()).unwrap();
        let key = MacKey::from_bytes([9; 32]);
        let passphrase = key.passphrase();
        let secret = SecretFile::create(&run, passphrase.as_bytes()).unwrap();
        let files = Files::new(Path::new("/d"), "abc");
        let config = VmConfig::default();
        let line = args(
            &config,
            &test_images(),
            &files,
            &test_spec(),
            LaunchMode::Fresh,
            false,
            Some(&secret),
        )
        .join(" ");
        assert!(
            !line.contains(passphrase.as_str()),
            "the key never reaches ps"
        );
        assert!(line.contains(&format!(
            "-object secret,id=dotkey,file={},format=raw",
            secret.path().display()
        )));
        assert!(line.contains("file=/d/instances/abc/disk.qcow2,encrypt.key-secret=dotkey"));
        assert!(line.contains("file=/d/instances/abc/vars.qcow2,encrypt.key-secret=dotkey"));
        assert_eq!(
            line.matches("encrypt.key-secret").count(),
            2,
            "only the Mac's own disk and variables; the shared images stay plain"
        );
        let plain = args(
            &config,
            &test_images(),
            &files,
            &test_spec(),
            LaunchMode::Fresh,
            false,
            None,
        )
        .join(" ");
        assert!(!plain.contains("secret") && !plain.contains("encrypt"));
        assert_eq!(parse_progress("    (42.50/100%)"), Some(0.425));
        assert_eq!(parse_progress("(100.00/100%)"), Some(1.0));
        assert_eq!(parse_progress("Image resized."), None);
    }

    /// An OSX-KVM checkout with just the files `Images::locate` looks for.
    fn fake_osx(dir: &Path, vars: Option<&Path>) -> VmConfig {
        let osx = dir.join("osx");
        std::fs::create_dir_all(osx.join("OpenCore")).unwrap();
        std::fs::write(osx.join("OpenCore/OpenCore.qcow2"), b"").unwrap();
        std::fs::write(osx.join("OVMF_CODE.fd"), b"").unwrap();
        match vars {
            Some(v) => std::fs::copy(v, osx.join("OVMF_VARS-1920x1080.fd")).map(|_| ()),
            None => std::fs::write(osx.join("OVMF_VARS-1920x1080.fd"), vec![0u8; 1 << 20]),
        }
        .unwrap();
        std::fs::create_dir_all(dir.join("data/images")).unwrap();
        VmConfig {
            osx_kvm_dir: osx,
            applesmc_osk: "key".into(),
            runtime_dir: Some(dir.join("run")),
            ..VmConfig::default()
        }
    }

    #[tokio::test]
    async fn encrypted_disks_are_created_with_the_key_in_a_file_that_is_then_deleted() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let mut config = fake_osx(dir.path(), None);
        let log = dir.path().join("calls");
        // A stand-in qemu-img that records its arguments and what the key file held.
        let script = dir.path().join("qemu-img");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\necho \"ARGS $*\" >> '{log}'\nfor a in \"$@\"; do case \"$a\" in secret,id=dotkey,file=*) f=\"${{a#secret,id=dotkey,file=}}\"; f=\"${{f%,format=raw}}\"; echo \"KEY $(cat \"$f\")\" >> '{log}';; esac; done\n",
                log = log.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        config.qemu_img = script;
        let data = dir.path().join("data");
        std::fs::write(data.join("images/base.qcow2"), b"").unwrap();
        let hv = Qemu {
            config: config.clone(),
            data_dir: data.clone(),
        };
        let key = MacKey::from_bytes([5; 32]);
        let passphrase = key.passphrase();
        hv.provision(
            &Files::new(&data, "enc"),
            64,
            true,
            &Crypt::Key(key.clone()),
        )
        .await
        .unwrap();
        let text = std::fs::read_to_string(&log).unwrap();
        let args: Vec<&str> = text.lines().filter(|l| l.starts_with("ARGS")).collect();
        assert_eq!(args.len(), 2, "{text}");
        assert!(args[0].starts_with("ARGS create --object secret,id=dotkey,file="));
        assert!(args[0].contains(&format!("-o {ENCRYPT_OPTS} -b ")));
        assert!(args[0].contains("base.qcow2 -F qcow2"));
        assert!(args[0].ends_with("instances/enc/disk.qcow2 64G"));
        assert!(args[1].starts_with("ARGS convert --object secret,id=dotkey,file="));
        assert!(args[1].contains(&format!("-O qcow2 -o {ENCRYPT_OPTS} ")));
        assert!(
            !text
                .lines()
                .any(|l| l.starts_with("ARGS") && l.contains(passphrase.as_str()))
        );
        let keys: Vec<&str> = text.lines().filter(|l| l.starts_with("KEY ")).collect();
        assert_eq!(keys.len(), 2);
        assert!(
            keys.iter().all(|k| k[4..] == *passphrase),
            "qemu-img read the key from the file"
        );
        assert_eq!(
            std::fs::read_dir(dir.path().join("run")).unwrap().count(),
            0,
            "key files are gone once qemu-img is done"
        );

        std::fs::remove_file(&log).unwrap();
        hv.provision(&Files::new(&data, "plain"), 64, true, &Crypt::Plain)
            .await
            .unwrap();
        let text = std::fs::read_to_string(&log).unwrap();
        assert!(
            !text.contains("encrypt") && !text.contains("secret"),
            "{text}"
        );

        let locked = hv
            .provision(&Files::new(&data, "locked"), 64, true, &Crypt::Locked)
            .await
            .unwrap_err();
        assert!(locked.downcast_ref::<super::super::LockedError>().is_some());
    }

    fn tool(name: &str) -> bool {
        std::process::Command::new(name)
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success())
    }

    fn info(path: &Path) -> Value {
        let out = std::process::Command::new("qemu-img")
            .args(["info", "--force-share", "--output=json"])
            .arg(path)
            .output()
            .unwrap();
        serde_json::from_slice(&out.stdout).unwrap()
    }

    /// With the real qemu-img (skipped when it isn't installed).
    #[tokio::test]
    async fn real_qemu_img_encrypts_new_and_existing_disks() {
        if !tool("qemu-img") {
            eprintln!("qemu-img isn't installed; skipping");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let mut config = fake_osx(dir.path(), None);
        config.qemu_img = "qemu-img".into();
        let data = dir.path().join("data");
        let base = data.join("images/base.qcow2");
        assert!(
            std::process::Command::new("qemu-img")
                .args(["create", "-q", "-f", "qcow2"])
                .arg(&base)
                .arg("1G")
                .status()
                .unwrap()
                .success()
        );
        let hv = Qemu {
            config,
            data_dir: data.clone(),
        };
        let key = MacKey::from_bytes([3; 32]);
        let crypt = Crypt::Key(key.clone());
        let files = Files::new(&data, "enc");
        hv.provision(&files, 1, true, &crypt).await.unwrap();
        let disk = info(&files.disk());
        assert_eq!(disk["encrypted"], true);
        assert!(
            disk["backing-filename"]
                .as_str()
                .unwrap()
                .ends_with("base.qcow2")
        );
        assert_eq!(info(&files.vars())["encrypted"], true);
        assert_eq!(
            std::fs::read_dir(dir.path().join("run")).unwrap().count(),
            0
        );

        // Snapshots while off need the key; measuring doesn't.
        hv.snapshot(&files, "dot-s1", false, &crypt).await.unwrap();
        let wrong = Crypt::Key(MacKey::from_bytes([4; 32]));
        assert!(hv.snapshot(&files, "dot-s2", false, &wrong).await.is_err());
        assert!(
            hv.snapshot(&files, "dot-s2", false, &Crypt::Locked)
                .await
                .is_err()
        );
        assert!(
            hv.snapshot_sizes(&files)
                .await
                .unwrap()
                .contains_key("dot-s1")
        );
        assert_eq!(hv.snapshot_state_bytes(&files).await, Some(0));
        assert!(hv.disk_bytes(&files).await.unwrap() > 0);
        hv.restore(&files, "dot-s1", false, &crypt, &test_spec())
            .await
            .unwrap();
        hv.delete_snapshot(&files, "dot-s1", false, &crypt)
            .await
            .unwrap();
        assert!(hv.snapshot_sizes(&files).await.unwrap().is_empty());

        // An existing plain Mac, with data in it, converts in place and keeps its base.
        let plain = Files::new(&data, "plain");
        hv.provision(&plain, 1, true, &Crypt::Plain).await.unwrap();
        let io = tool("qemu-io");
        if io {
            assert!(
                std::process::Command::new("qemu-io")
                    .args(["-c", "write -P 0x41 0 1M"])
                    .arg(plain.disk())
                    .output()
                    .unwrap()
                    .status
                    .success()
            );
        }
        hv.snapshot(&plain, "dot-old", false, &Crypt::Plain)
            .await
            .unwrap();
        let seen = std::sync::Mutex::new(Vec::new());
        hv.encrypt(&plain, &key, &|p| seen.lock().unwrap().push(p))
            .await
            .unwrap();
        assert_eq!(seen.lock().unwrap().last(), Some(&1.0));
        let converted = info(&plain.disk());
        assert_eq!(converted["encrypted"], true);
        assert!(
            converted["backing-filename"]
                .as_str()
                .unwrap()
                .ends_with("base.qcow2")
        );
        assert_eq!(info(&plain.vars())["encrypted"], true);
        assert!(
            converted.get("snapshots").is_none(),
            "snapshots don't carry over"
        );
        assert!(!plain.dir.join("disk.qcow2.encrypting").exists());
        let bytes = std::fs::read(plain.disk()).unwrap();
        assert!(!bytes.windows(64).any(|w| w.iter().all(|b| *b == 0x41)));
        if io {
            let secret = hv.secret(&key).unwrap();
            let out = std::process::Command::new("qemu-io")
                .args([
                    "--object",
                    &secret.object(),
                    "--image-opts",
                    "-c",
                    "read -P 0x41 0 1M",
                ])
                .arg(image_opts(&plain.disk(), true))
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            assert!(!String::from_utf8_lossy(&out.stdout).contains("Pattern verification failed"));
        }
        assert!(
            hv.encrypt(&plain, &key, &|_| {}).await.is_err(),
            "only once"
        );
    }

    /// Live snapshots and hibernation of an encrypted Mac with a real QEMU (TCG, UEFI
    /// firmware, no macOS). Run with
    /// `cargo test -p dot-home real_qemu_saves -- --ignored`.
    #[tokio::test]
    #[ignore = "needs qemu-system-x86_64 and edk2 firmware"]
    async fn real_qemu_saves_encrypted_memory_and_live_snapshots() {
        let firmware = [
            "/opt/homebrew/share/qemu",
            "/usr/local/share/qemu",
            "/usr/share/qemu",
        ]
        .iter()
        .map(Path::new)
        .find(|d| d.join("edk2-x86_64-code.fd").exists())
        .expect("edk2 firmware");
        let dir = tempfile::tempdir().unwrap();
        let mut config = fake_osx(dir.path(), Some(&firmware.join("edk2-i386-vars.fd")));
        config.qemu_img = "qemu-img".into();
        let data = dir.path().join("data");
        assert!(
            std::process::Command::new("qemu-img")
                .args(["create", "-q", "-f", "qcow2"])
                .arg(data.join("images/base.qcow2"))
                .arg("1G")
                .status()
                .unwrap()
                .success()
        );
        let hv = Qemu {
            config,
            data_dir: data.clone(),
        };
        let key = MacKey::from_bytes([7; 32]);
        let crypt = Crypt::Key(key.clone());
        let files = Files::new(&data, "m");
        hv.provision(&files, 1, true, &crypt).await.unwrap();
        let code = firmware.join("edk2-x86_64-code.fd");
        let boot = |incoming: bool| {
            let secret = hv.secret(&key).unwrap();
            let p = |p: PathBuf| p.to_string_lossy().into_owned();
            let mut cmd = std::process::Command::new("qemu-system-x86_64");
            cmd.args([
                "-machine",
                "q35",
                "-accel",
                "tcg",
                "-m",
                "128",
                "-display",
                "none",
                "-nodefaults",
            ])
            .args(["-object", &secret.object()])
            .args([
                "-drive",
                &format!("if=pflash,format=raw,readonly=on,file={}", code.display()),
            ])
            .args([
                "-drive",
                &format!(
                    "if=pflash,format=qcow2,file={},encrypt.key-secret=dotkey",
                    p(files.vars())
                ),
            ])
            .args([
                "-drive",
                &format!(
                    "id=MacHDD,if=none,format=qcow2,file={},encrypt.key-secret=dotkey",
                    p(files.disk())
                ),
            ])
            .args([
                "-device",
                "ich9-ahci,id=sata",
                "-device",
                "ide-hd,bus=sata.0,drive=MacHDD",
            ])
            .args([
                "-qmp",
                &format!("unix:{},server=on,wait=off", p(files.qmp())),
            ])
            .args(["-pidfile", &p(files.pid()), "-daemonize"]);
            if incoming {
                cmd.args(["-incoming", "defer"]);
            }
            let out = cmd.output().unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            drop(secret);
        };
        let no_plain_stream = |path: &Path| {
            let bytes = std::fs::read(path).unwrap();
            assert!(bytes.starts_with(memfile::MAGIC));
            assert!(
                !bytes.windows(4).any(|w| w == b"QEVM"),
                "no plaintext migration stream"
            );
            bytes.len()
        };

        boot(false);
        tokio::time::sleep(Duration::from_secs(2)).await;
        assert_eq!(hv.status(&files).await.as_deref(), Some("running"));
        assert_eq!(
            std::fs::read_dir(dir.path().join("run")).unwrap().count(),
            0
        );

        // A live snapshot: internal disk snapshots plus encrypted memory; it keeps running.
        hv.snapshot(&files, "dot-live", true, &crypt).await.unwrap();
        assert_eq!(hv.status(&files).await.as_deref(), Some("running"));
        assert!(no_plain_stream(&files.snapshot_memory("dot-live")) > 1 << 20);
        assert!(hv.snapshot_sizes(&files).await.unwrap()["dot-live"] > 0);

        // Hibernate, then resume from the encrypted file.
        hv.hibernate(&files, &crypt).await.unwrap();
        hv.wait_gone(&files).await;
        assert!(hv.status(&files).await.is_none());
        no_plain_stream(&files.hibernate());
        boot(true);
        load_memory(&files.qmp(), &files.hibernate(), Some(&key))
            .await
            .unwrap();
        assert_eq!(hv.status(&files).await.as_deref(), Some("running"));

        // A wrong key never feeds QEMU anything.
        hv.hibernate(&files, &crypt).await.unwrap();
        hv.wait_gone(&files).await;
        boot(true);
        let wrong = MacKey::from_bytes([8; 32]);
        assert!(
            load_memory(&files.qmp(), &files.hibernate(), Some(&wrong))
                .await
                .is_err()
        );
        hv.kill(&files).await.unwrap();
        hv.wait_gone(&files).await;

        // Deleting a live snapshot of a running Mac doesn't need the key.
        boot(false);
        tokio::time::sleep(Duration::from_secs(1)).await;
        hv.delete_snapshot(&files, "dot-live", true, &Crypt::Locked)
            .await
            .unwrap();
        assert!(!files.snapshot_memory("dot-live").exists());
        hv.kill(&files).await.unwrap();
        hv.wait_gone(&files).await;
    }
}
