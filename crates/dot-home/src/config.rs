//! Server configuration: a TOML file, with a few environment overrides for containers.
use anyhow::{Context, bail};
use serde::Deserialize;
use std::{net::SocketAddr, path::PathBuf};
use url::Url;

/// Everything the server needs to start.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// The URL people and assistants use, e.g. `https://home.example.com`.
    pub public_url: Url,
    /// Where to listen.
    #[serde(default = "default_bind")]
    pub bind: SocketAddr,
    /// State directory (database, keys, images, instances).
    #[serde(default = "default_data_dir")]
    pub data_dir: PathBuf,
    /// HTTPS.
    #[serde(default)]
    pub tls: Tls,
    /// Virtual machines.
    #[serde(default)]
    pub vm: Vm,
    /// Limits that protect the host.
    #[serde(default)]
    pub limits: Limits,
    /// Proxies whose `X-Forwarded-For` is trusted for client IPs (exact addresses).
    #[serde(default)]
    pub trusted_proxies: Vec<Cidr>,
    /// Log as JSON lines.
    #[serde(default)]
    pub json_logs: bool,
}

/// An address or a network (`10.0.0.1`, `172.16.0.0/12`, `2606:4700::/32`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cidr {
    addr: std::net::IpAddr,
    prefix: u8,
}

impl Cidr {
    /// Is `ip` inside this network.
    pub fn contains(&self, ip: std::net::IpAddr) -> bool {
        use std::net::IpAddr::{V4, V6};
        let ip = match ip {
            V6(v6) => v6.to_ipv4_mapped().map_or(V6(v6), V4),
            v4 => v4,
        };
        match (self.addr, ip) {
            (V4(net), V4(ip)) => {
                let mask = u32::MAX
                    .checked_shl(32 - u32::from(self.prefix))
                    .unwrap_or(0);
                u32::from(net) & mask == u32::from(ip) & mask
            }
            (V6(net), V6(ip)) => {
                let mask = u128::MAX
                    .checked_shl(128 - u32::from(self.prefix))
                    .unwrap_or(0);
                u128::from(net) & mask == u128::from(ip) & mask
            }
            _ => false,
        }
    }
}

impl std::str::FromStr for Cidr {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        let (addr, prefix) = s
            .trim()
            .split_once('/')
            .map_or((s.trim(), None), |(a, p)| (a, Some(p)));
        let addr: std::net::IpAddr = addr
            .parse()
            .map_err(|_| format!("{s} isn't an IP address or network"))?;
        let max = if addr.is_ipv4() { 32 } else { 128 };
        let prefix = match prefix {
            Some(p) => p
                .parse::<u8>()
                .ok()
                .filter(|p| *p <= max)
                .ok_or_else(|| format!("{s} has an invalid prefix"))?,
            None => max,
        };
        Ok(Self { addr, prefix })
    }
}

impl<'de> Deserialize<'de> for Cidr {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        String::deserialize(d)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

/// How HTTPS is handled.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum Tls {
    /// Plain HTTP. Only for loopback development or behind a TLS-terminating proxy.
    #[default]
    Off,
    /// Let's Encrypt via TLS-ALPN-01 on the bind port (must be reachable on 443).
    Acme {
        /// Contact email for the certificate authority.
        email: String,
        /// Use the staging CA (for testing).
        #[serde(default)]
        staging: bool,
    },
    /// Certificate files (PEM).
    Files {
        /// Full chain.
        cert: PathBuf,
        /// Private key.
        key: PathBuf,
    },
}

/// QEMU and OSX-KVM settings.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Vm {
    /// QEMU binary.
    pub qemu: PathBuf,
    /// qemu-img binary.
    pub qemu_img: PathBuf,
    /// OSX-KVM checkout (OpenCore image, OVMF, `fetch-macOS-v2.py`).
    pub osx_kvm_dir: PathBuf,
    /// The Apple SMC key from OSX-KVM's boot script. Read from `osx_kvm_dir` when empty.
    pub applesmc_osk: String,
    /// Give the Mac QEMU's own Apple SMC (`isa-applesmc`). Off by default so OpenCore's
    /// VirtualSMC answers instead: QEMU's SMC only handles reads, so macOS's shutdown
    /// watchdog write fails and Tahoe freezes on every restart and shutdown.
    pub qemu_smc: bool,
    /// `-cpu` model and flags.
    pub cpu_model: String,
    /// Default vCPUs for new Macs.
    pub default_cpus: u32,
    /// Default memory for new Macs.
    pub default_memory_mb: u32,
    /// Default disk size for new Macs (the overlay can grow to this).
    pub default_disk_gb: u32,
    /// First host port for guest forwarding; each instance takes one.
    pub port_base: u16,
    /// Loopback address the agent ports are forwarded on. QEMU user networking lets a guest
    /// reach the host's 127.0.0.1 (as 10.0.2.2), so this must be a different 127.x address
    /// or one Mac could connect to another Mac's agent port.
    pub forward_address: std::net::Ipv4Addr,
    /// QEMU machine type, pinned so hibernation files survive QEMU upgrades.
    pub machine: String,
    /// Display resolution (must match an OVMF_VARS variant).
    pub resolution: String,
    /// Optional command prefix for resource limits, e.g. `["systemd-run", "--scope", "--user"]`.
    pub wrapper: Vec<String>,
    /// Cut guest-initiated network connections (no internet in the Mac).
    pub restrict_network: bool,
    /// Reset a Mac whose agent stops answering for this long (0 disables).
    pub unresponsive_reset_minutes: u64,
    /// Reset a Mac that has been frozen in the macOS kernel for this long, busy with no disk
    /// activity (what a failed restart looks like); while stopping, it's turned off sooner.
    /// 0 disables.
    pub frozen_reset_seconds: u64,
    /// When the server stops: `leave` Macs running (systemd) or `hibernate` them (Docker,
    /// where stopping the container would otherwise cut their power).
    pub on_shutdown: OnShutdown,
    /// Private directory (ideally on a tmpfs) where keys are handed to QEMU in files that
    /// live only until QEMU has read them. Unset: `$RUNTIME_DIRECTORY` (systemd), then
    /// `/dev/shm`, then `<data_dir>/run`.
    pub runtime_dir: Option<PathBuf>,
}

/// What happens to running Macs when the server stops.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OnShutdown {
    /// Keep running (they're daemonized).
    #[default]
    Leave,
    /// Save their memory to disk so they resume where they were.
    Hibernate,
}

impl Default for Vm {
    fn default() -> Self {
        Self {
            qemu: "qemu-system-x86_64".into(),
            qemu_img: "qemu-img".into(),
            osx_kvm_dir: "/opt/osx-kvm".into(),
            applesmc_osk: String::new(),
            qemu_smc: false,
            // OSX-KVM's model for Sequoia and Tahoe; Tahoe hangs at the Apple logo on Haswell.
            cpu_model: "Skylake-Client,-hle,-rtm,kvm=on,vendor=GenuineIntel,+invtsc,vmware-cpuid-freq=on,+ssse3,+sse4.2,+popcnt,+avx,+aes,+xsave,+xsaveopt,check".into(),
            default_cpus: 4,
            default_memory_mb: 8192,
            default_disk_gb: 128,
            port_base: 17_000,
            forward_address: std::net::Ipv4Addr::new(127, 77, 0, 1),
            machine: "pc-q35-8.2".into(),
            resolution: "1920x1080".into(),
            wrapper: Vec::new(),
            restrict_network: false,
            unresponsive_reset_minutes: 5,
            frozen_reset_seconds: 90,
            on_shutdown: OnShutdown::Leave,
            runtime_dir: None,
        }
    }
}

/// Host protection.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Limits {
    /// RAM all running Macs may use together (0 = 75% of host RAM).
    pub memory_budget_mb: u64,
    /// Running Macs at once.
    pub max_running: u32,
    /// Macs per member (admins have no limit).
    pub max_instances_per_user: u32,
    /// Refuse to start Macs or take snapshots below this much free disk.
    pub min_free_disk_gb: u64,
    /// Concurrent tool calls per Mac.
    pub calls_per_instance: usize,
    /// Activity retention.
    pub activity_days: u32,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            memory_budget_mb: 0,
            max_running: 4,
            max_instances_per_user: 2,
            min_free_disk_gb: 20,
            calls_per_instance: 4,
            activity_days: 90,
        }
    }
}

fn default_bind() -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], 8470))
}

fn default_data_dir() -> PathBuf {
    "/var/lib/dot-home".into()
}

impl Config {
    /// Read the file at `path`, then apply `DOT_HOME_*` overrides.
    pub fn load(path: Option<&std::path::Path>) -> anyhow::Result<Self> {
        let mut text = match path {
            Some(p) => {
                std::fs::read_to_string(p).with_context(|| format!("reading {}", p.display()))?
            }
            None => String::new(),
        };
        if let Ok(url) = std::env::var("DOT_HOME_PUBLIC_URL")
            && !text.contains("public_url")
        {
            text = format!("public_url = {}\n{text}", toml::Value::String(url));
        }
        let mut config: Config = toml::from_str(&text).context("invalid configuration")?;
        if let Ok(bind) = std::env::var("DOT_HOME_BIND") {
            config.bind = bind.parse().context("DOT_HOME_BIND")?;
        }
        if let Ok(dir) = std::env::var("DOT_HOME_DATA_DIR") {
            config.data_dir = dir.into();
        }
        if let Ok(email) = std::env::var("DOT_HOME_ACME_EMAIL")
            && !email.is_empty()
        {
            config.tls = Tls::Acme {
                email,
                staging: std::env::var("DOT_HOME_ACME_STAGING")
                    .is_ok_and(|v| v == "1" || v == "true"),
            };
        }
        if let Ok(v) = std::env::var("DOT_HOME_ON_SHUTDOWN") {
            config.vm.on_shutdown = match v.as_str() {
                "hibernate" => OnShutdown::Hibernate,
                "leave" => OnShutdown::Leave,
                _ => bail!("DOT_HOME_ON_SHUTDOWN must be leave or hibernate"),
            };
        }
        if let Ok(v) = std::env::var("DOT_HOME_TRUSTED_PROXIES") {
            config.trusted_proxies = v
                .split(',')
                .filter(|s| !s.trim().is_empty())
                .map(|s| s.trim().parse::<Cidr>())
                .collect::<Result<_, _>>()
                .map_err(|e| anyhow::anyhow!("DOT_HOME_TRUSTED_PROXIES: {e}"))?;
        }
        config.validate()?;
        Ok(config)
    }

    /// Reject settings that would make the server unsafe.
    pub fn validate(&self) -> anyhow::Result<()> {
        let https = self.public_url.scheme() == "https";
        let loopback_url = matches!(
            self.public_url.host_str(),
            Some("localhost" | "127.0.0.1" | "[::1]")
        );
        if !https && !loopback_url {
            bail!("public_url must be https unless it is localhost");
        }
        if self.public_url.path() != "/" || self.public_url.query().is_some() {
            bail!("public_url must be an origin like https://home.example.com, without a path");
        }
        // In the container, 0.0.0.0 is the container's own interface; the published port decides exposure.
        let in_container = std::env::var("DOT_HOME_IN_CONTAINER").is_ok_and(|v| v == "1");
        let plain_public = matches!(self.tls, Tls::Off) && !self.bind.ip().is_loopback();
        if plain_public && !https && !(in_container && loopback_url) {
            bail!("plain HTTP may only listen on loopback; set [tls] or bind to 127.0.0.1");
        }
        if plain_public && https && self.trusted_proxies.is_empty() {
            bail!(
                "public_url is https but this server would answer plain HTTP on {}. Turn on [tls] (or DOT_HOME_ACME_EMAIL), or put a TLS proxy in front and list it in trusted_proxies (DOT_HOME_TRUSTED_PROXIES)",
                self.bind
            );
        }
        if !self.vm.forward_address.is_loopback()
            || self.vm.forward_address == std::net::Ipv4Addr::LOCALHOST
        {
            bail!("vm.forward_address must be a loopback address other than 127.0.0.1");
        }
        if self.limits.calls_per_instance == 0 || self.limits.calls_per_instance > 32 {
            bail!("limits.calls_per_instance must be between 1 and 32");
        }
        Ok(())
    }

    /// Public origin without a trailing slash.
    pub fn origin(&self) -> String {
        self.public_url.origin().ascii_serialization()
    }

    /// Familiar as Macs on this server reach it: QEMU's user network maps 10.0.2.2 to
    /// the container, which is more dependable than looping out to the public address.
    pub fn vm_origin(&self) -> String {
        format!("http://10.0.2.2:{}", self.bind.port())
    }

    /// Cookies get `Secure` and the `__Host-` prefix over HTTPS.
    pub fn secure_cookies(&self) -> bool {
        self.public_url.scheme() == "https"
    }

    /// The MCP endpoint URL.
    pub fn mcp_url(&self) -> String {
        format!("{}/mcp", self.origin())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> anyhow::Result<Config> {
        let c: Config = toml::from_str(text)?;
        c.validate()?;
        Ok(c)
    }

    #[test]
    fn refuses_unsafe_public_exposure() {
        assert!(parse("public_url = 'http://home.example.com'").is_err());
        assert!(parse("public_url = 'https://home.example.com/sub'").is_err());
        assert!(
            parse("public_url = 'http://localhost:8470'\nbind = '0.0.0.0:8470'").is_err(),
            "plain HTTP on a public interface"
        );
        // Plain HTTP on a public interface needs a named TLS proxy in front.
        assert!(parse("public_url = 'https://home.example.com'\nbind = '0.0.0.0:8470'").is_err());
        assert!(parse("public_url = 'https://home.example.com'\nbind = '0.0.0.0:8470'\ntrusted_proxies = ['172.17.0.1']").is_ok());
        assert!(
            parse("public_url = 'https://h.example'\n[vm]\nforward_address = '127.0.0.1'").is_err()
        );
        let ok = parse(
            "public_url = 'https://home.example.com'\nbind = '0.0.0.0:443'\n[tls]\nmode = 'acme'\nemail = 'a@b.c'",
        )
        .unwrap();
        assert_eq!(ok.mcp_url(), "https://home.example.com/mcp");
        assert!(ok.secure_cookies());
        assert!(parse("public_url = 'http://localhost:8470'").is_ok());
        assert!(parse("public_url = 'https://h.example'\n[vm]\nbogus = 1").is_err());
    }
}
