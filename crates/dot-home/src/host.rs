//! Facts about the host machine and the health checks shown to admins.
use crate::config::Config;
use serde::Serialize;
use std::path::Path;

/// (total, available) memory in MB, from `/proc/meminfo`.
pub fn memory() -> Option<(u64, u64)> {
    let text = std::fs::read_to_string("/proc/meminfo").ok()?;
    let field = |name: &str| {
        text.lines()
            .find(|l| l.starts_with(name))?
            .split_whitespace()
            .nth(1)?
            .parse::<u64>()
            .ok()
    };
    Some((field("MemTotal:")? / 1024, field("MemAvailable:")? / 1024))
}

/// 1-minute load average.
pub fn load() -> Option<f64> {
    std::fs::read_to_string("/proc/loadavg")
        .ok()?
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

/// Seconds since the host booted.
pub fn uptime() -> Option<u64> {
    let text = std::fs::read_to_string("/proc/uptime").ok()?;
    let secs: f64 = text.split_whitespace().next()?.parse().ok()?;
    Some(secs as u64)
}

/// (total, free) bytes on the filesystem holding `path`.
pub fn disk(path: &Path) -> Option<(u64, u64)> {
    let stat = nix::sys::statvfs::statvfs(path).ok()?;
    let frag = stat.fragment_size() as u64;
    Some((
        stat.blocks() as u64 * frag,
        stat.blocks_available() as u64 * frag,
    ))
}

/// Free bytes on the filesystem holding `path`.
pub fn disk_free(path: &Path) -> Option<u64> {
    disk(path).map(|(_, free)| free)
}

/// One health check.
#[derive(Debug, Clone, Serialize)]
pub struct Check {
    /// Stable id.
    pub id: &'static str,
    /// What is checked.
    pub label: &'static str,
    /// Passed.
    pub ok: bool,
    /// Result or how to fix it.
    pub detail: String,
}

/// Everything `dot-home doctor` and the admin page check. `reserve_gb` is the free
/// space Macs must leave (Admin > Storage).
pub async fn checks(config: &Config, reserve_gb: u64) -> Vec<Check> {
    let mut out = Vec::new();
    let kvm = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/kvm");
    out.push(Check {
        id: "kvm",
        label: "Hardware virtualization",
        ok: kvm.is_ok(),
        detail: match kvm {
            Ok(_) => "/dev/kvm is available.".into(),
            Err(e) => format!(
                "/dev/kvm can't be opened ({e}). Enable VT-x/AMD-V in the BIOS and pass --device /dev/kvm to Docker."
            ),
        },
    });
    let qemu = tokio::process::Command::new(&config.vm.qemu)
        .arg("--version")
        .output()
        .await;
    out.push(match qemu {
        Ok(o) if o.status.success() => {
            let v = String::from_utf8_lossy(&o.stdout);
            let line = v.lines().next().unwrap_or("").to_owned();
            let ok = qemu_version_ok(&line);
            Check {
                id: "qemu",
                label: "QEMU",
                ok,
                detail: if ok {
                    line
                } else {
                    format!("{line}. Familiar needs QEMU 8.2 or later for hibernation.")
                },
            }
        }
        _ => Check {
            id: "qemu",
            label: "QEMU",
            ok: false,
            detail: format!(
                "{} wasn't found. Install qemu-system-x86.",
                config.vm.qemu.display()
            ),
        },
    });
    let images = crate::vm::qemu::Images::locate(&config.vm, &config.data_dir);
    out.push(Check {
        id: "firmware",
        label: "OpenCore and OVMF",
        ok: images.is_ok(),
        detail: match &images {
            Ok(_) => format!("Found in {}.", config.vm.osx_kvm_dir.display()),
            Err(e) => format!("{e}. The Docker image includes OSX-KVM; outside Docker, clone it and set vm.osx_kvm_dir."),
        },
    });
    let base = config.data_dir.join("images/base.qcow2");
    out.push(Check {
        id: "base_image",
        label: "macOS base image",
        ok: base.exists(),
        detail: if base.exists() {
            "Ready. New Macs are copies of it.".into()
        } else {
            "Not built yet. Open Admin > macOS Image to build one.".into()
        },
    });
    let (total, free) = disk(&config.data_dir).unwrap_or((0, 0));
    let min = reserve_gb.saturating_mul(crate::storage::GB);
    out.push(Check {
        id: "disk",
        label: "Free disk space",
        ok: free >= min,
        detail: format!(
            "{} GB free of {} GB{}",
            free / 1_073_741_824,
            total / 1_073_741_824,
            if free >= min {
                "."
            } else {
                ". Macs won't start below the minimum."
            }
        ),
    });
    out.push(Check {
        id: "https",
        label: "HTTPS",
        ok: config.public_url.scheme() == "https",
        detail: if config.public_url.scheme() == "https" {
            format!("Served at {}.", config.origin())
        } else {
            "Plain HTTP on localhost. Set public_url to an https address before using it outside your network.".into()
        },
    });
    out
}

fn qemu_version_ok(line: &str) -> bool {
    let version = line
        .split_whitespace()
        .find(|w| w.chars().next().is_some_and(|c| c.is_ascii_digit()))
        .unwrap_or("0");
    let mut parts = version.split('.').map(|p| p.parse::<u32>().unwrap_or(0));
    let major = parts.next().unwrap_or(0);
    let minor = parts.next().unwrap_or(0);
    (major, minor) >= (8, 2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qemu_versions() {
        assert!(qemu_version_ok(
            "QEMU emulator version 9.2.0 (Debian 1:9.2.0+ds-5)"
        ));
        assert!(qemu_version_ok("QEMU emulator version 8.2.2"));
        assert!(!qemu_version_ok("QEMU emulator version 7.2.15"));
        assert!(!qemu_version_ok("garbage"));
        assert!(disk(Path::new("/")).is_some_and(|(t, f)| t >= f));
    }
}
