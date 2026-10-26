//! Where the agent learns its instance id and secret.
//!
//! The JSON format is a contract with the host, which writes it to the `DOTCONFIG` disk:
//!
//! ```json
//! { "instance_id": "inst_123", "secret": "<standard base64>", "port": 7466 }
//! ```
//!
//! `port` is optional and defaults to [`dot_wire::GUEST_PORT`].
use anyhow::{Context, Result, bail};
use base64::Engine;
use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// File name on the `DOTCONFIG` volume.
pub const CONFIG_FILE_NAME: &str = "dot-guest.json";
/// Shortest secret accepted, in bytes.
pub const MIN_SECRET_BYTES: usize = 16;
/// How long `run` waits at boot for the `DOTCONFIG` USB volume to mount.
pub const VOLUME_WAIT: Duration = Duration::from_secs(60);

/// The on-disk JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigFile {
    /// Which instance this Mac is.
    pub instance_id: String,
    /// The shared secret, standard base64.
    pub secret: String,
    /// Port to listen on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
}

/// A validated configuration.
#[derive(Clone, PartialEq, Eq)]
pub struct Config {
    /// Which instance this Mac is.
    pub instance_id: String,
    /// The shared secret.
    pub secret: Vec<u8>,
    /// Port to listen on.
    pub port: u16,
}

impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Config")
            .field("instance_id", &self.instance_id)
            .field("secret", &"<redacted>")
            .field("port", &self.port)
            .finish()
    }
}

impl Config {
    /// Validate a parsed file.
    pub fn from_file(file: ConfigFile) -> Result<Self> {
        validate_instance_id(&file.instance_id)?;
        let secret = decode_secret(&file.secret)?;
        let port = file.port.unwrap_or(dot_wire::GUEST_PORT);
        if port == 0 {
            bail!("port must be between 1 and 65535");
        }
        Ok(Self {
            instance_id: file.instance_id,
            secret,
            port,
        })
    }

    /// Parse and validate JSON text.
    pub fn parse(json: &str) -> Result<Self> {
        let file: ConfigFile = serde_json::from_str(json).context(
            "the configuration is not valid JSON with `instance_id`, `secret` and optional `port`",
        )?;
        Self::from_file(file)
    }

    /// Read and validate a file.
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("could not read {}", path.display()))?;
        Self::parse(&text)
            .with_context(|| format!("{} is not a valid configuration", path.display()))
    }

    /// The JSON form, with the secret as standard base64.
    pub fn to_file(&self) -> ConfigFile {
        ConfigFile {
            instance_id: self.instance_id.clone(),
            secret: STANDARD.encode(&self.secret),
            port: (self.port != dot_wire::GUEST_PORT).then_some(self.port),
        }
    }
}

fn validate_instance_id(id: &str) -> Result<()> {
    if id.is_empty() || id.len() > 128 {
        bail!("instance_id must be 1 to 128 characters");
    }
    if !id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    {
        bail!("instance_id may only contain letters, digits, `-`, `_` and `.`");
    }
    Ok(())
}

/// Accept standard base64 (the contract) and, leniently, the URL-safe and unpadded forms.
fn decode_secret(text: &str) -> Result<Vec<u8>> {
    let text = text.trim();
    let secret = [STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD]
        .iter()
        .find_map(|engine| engine.decode(text).ok())
        .context("secret is not valid base64")?;
    if secret.len() < MIN_SECRET_BYTES {
        bail!("secret must be at least {MIN_SECRET_BYTES} bytes");
    }
    Ok(secret)
}

/// `~/Library/Application Support/Dot Guest`.
pub fn support_dir(home: &Path) -> PathBuf {
    home.join("Library/Application Support/Dot Guest")
}

/// The per-user configuration written by `dot-guest pair`.
pub fn user_config_path(home: &Path) -> PathBuf {
    support_dir(home).join("config.json")
}

/// Status the daemon writes for `dot-guest doctor`.
pub fn status_path(home: &Path) -> PathBuf {
    support_dir(home).join("status.json")
}

/// `dot-guest.json` on a mounted `DOTCONFIG` volume, if any. macOS names a second
/// volume with the same label `DOTCONFIG 1`, so any `DOTCONFIG*` volume counts.
pub fn find_volume_config() -> Option<PathBuf> {
    let direct = Path::new("/Volumes")
        .join(dot_wire::CONFIG_VOLUME)
        .join(CONFIG_FILE_NAME);
    if direct.is_file() {
        return Some(direct);
    }
    let mut candidates: Vec<PathBuf> = std::fs::read_dir("/Volumes")
        .ok()?
        .filter_map(Result::ok)
        .filter(|e| {
            e.file_name()
                .to_str()
                .is_some_and(|n| n.starts_with(dot_wire::CONFIG_VOLUME))
        })
        .map(|e| e.path().join(CONFIG_FILE_NAME))
        .filter(|p| p.is_file())
        .collect();
    candidates.sort();
    candidates.into_iter().next()
}

/// Where a configuration came from.
#[derive(Debug, Clone)]
pub enum Source {
    /// `--config`.
    Flag(PathBuf),
    /// The `DOTCONFIG` disk.
    Volume(PathBuf),
    /// `~/Library/Application Support/Dot Guest/config.json`.
    User(PathBuf),
}

impl Source {
    /// The file path.
    pub fn path(&self) -> &Path {
        match self {
            Source::Flag(p) | Source::Volume(p) | Source::User(p) => p,
        }
    }
}

/// Resolve the configuration for `run`: `--config`, then the `DOTCONFIG` volume, then
/// the user file. The volume is polled for up to `volume_wait` when there is no user
/// file, because USB disks mount late at boot.
pub async fn resolve(
    flag: Option<&Path>,
    home: &Path,
    volume_wait: Duration,
) -> Result<(Config, Source)> {
    if let Some(path) = flag {
        return Ok((Config::load(path)?, Source::Flag(path.to_owned())));
    }
    let user = user_config_path(home);
    let wait = if user.is_file() {
        Duration::ZERO
    } else {
        volume_wait
    };
    let give_up = tokio::time::Instant::now() + wait;
    loop {
        if let Some(path) = find_volume_config() {
            return Ok((Config::load(&path)?, Source::Volume(path)));
        }
        if tokio::time::Instant::now() >= give_up {
            break;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    if user.is_file() {
        return Ok((Config::load(&user)?, Source::User(user)));
    }
    bail!(
        "no configuration found: expected /Volumes/{}/{} (attached by the Dot Home server) or {} (written by `dot-guest pair`)",
        dot_wire::CONFIG_VOLUME,
        CONFIG_FILE_NAME,
        user.display()
    )
}

/// Prefix of a pairing string.
pub const PAIRING_PREFIX: &str = "dotpair:v1:";

/// Parse a pairing string from the portal:
/// `dotpair:v1:<instance_id>:<secret as unpadded base64url>:<port>`.
pub fn parse_pairing(text: &str) -> Result<Config> {
    let text = text.trim();
    let Some(rest) = text.strip_prefix(PAIRING_PREFIX) else {
        if text.starts_with("dotpair:") {
            bail!("this pairing string is from a newer Dot Home; update dot-guest");
        }
        bail!(
            "a pairing string starts with `{PAIRING_PREFIX}`; copy the whole string from the Dot Home portal"
        );
    };
    let parts: Vec<&str> = rest.split(':').collect();
    let [instance_id, secret, port] = parts.as_slice() else {
        bail!(
            "the pairing string should have three parts after `{PAIRING_PREFIX}`: instance, secret and port"
        );
    };
    validate_instance_id(instance_id)?;
    let secret = URL_SAFE_NO_PAD
        .decode(secret)
        .or_else(|_| URL_SAFE.decode(secret))
        .context("the secret in the pairing string is not valid base64url")?;
    if secret.len() < MIN_SECRET_BYTES {
        bail!("the secret in the pairing string is too short");
    }
    let port: u16 = port
        .parse()
        .ok()
        .filter(|p| *p != 0)
        .context("the port in the pairing string must be a number from 1 to 65535")?;
    Ok(Config {
        instance_id: (*instance_id).to_owned(),
        secret,
        port,
    })
}

/// Format a pairing string (the inverse of [`parse_pairing`]).
pub fn format_pairing(config: &Config) -> String {
    format!(
        "{PAIRING_PREFIX}{}:{}:{}",
        config.instance_id,
        URL_SAFE_NO_PAD.encode(&config.secret),
        config.port
    )
}

/// Write the user configuration atomically with mode 0600 in a 0700 folder.
pub fn write_user_config(home: &Path, config: &Config) -> Result<PathBuf> {
    use std::io::Write;
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
    let dir = support_dir(home);
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&dir)
        .with_context(|| format!("could not create {}", dir.display()))?;
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))
        .with_context(|| format!("could not secure {}", dir.display()))?;
    let path = user_config_path(home);
    let tmp = dir.join(format!(
        ".config.{}.tmp",
        crate::util::hex(&dot_wire::random_bytes::<6>())
    ));
    let body = serde_json::to_vec_pretty(&config.to_file())?;
    let result = (|| -> Result<()> {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp)?;
        file.write_all(&body)?;
        file.sync_all()?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result.with_context(|| format!("could not write {}", path.display()))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    #![expect(
        clippy::unwrap_used,
        reason = "Test assertions fail immediately on malformed setup"
    )]
    use super::*;

    const SECRET_B64: &str = "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=";

    #[test]
    fn host_contract_json_parses_with_default_port() {
        let config = Config::parse(&format!(
            r#"{{"instance_id":"inst_1","secret":"{SECRET_B64}"}}"#
        ))
        .unwrap();
        assert_eq!(config.instance_id, "inst_1");
        assert_eq!(config.secret, (0u8..32).collect::<Vec<_>>());
        assert_eq!(config.port, dot_wire::GUEST_PORT);
    }

    #[test]
    fn invalid_configs_are_refused_with_a_reason() {
        let cases = [
            (
                r#"{"instance_id":"","secret":"AAAAAAAAAAAAAAAAAAAAAA=="}"#,
                "instance_id",
            ),
            (
                r#"{"instance_id":"a:b","secret":"AAAAAAAAAAAAAAAAAAAAAA=="}"#,
                "instance_id",
            ),
            (r#"{"instance_id":"a","secret":"AAAA"}"#, "at least"),
            (r#"{"instance_id":"a","secret":"!!!"}"#, "base64"),
            (
                r#"{"instance_id":"a","secret":"AAAAAAAAAAAAAAAAAAAAAA==","port":0}"#,
                "port",
            ),
            (
                r#"{"instance":"a","secret":"AAAAAAAAAAAAAAAAAAAAAA=="}"#,
                "JSON",
            ),
        ];
        for (json, needle) in cases {
            let err = format!("{:#}", Config::parse(json).unwrap_err());
            assert!(err.contains(needle), "{json}: {err}");
        }
    }

    #[test]
    fn pairing_string_round_trips_and_rejects_malformed_input() {
        let config = Config {
            instance_id: "mac-mini_2".into(),
            secret: (0u8..32).collect(),
            port: 7470,
        };
        let text = format_pairing(&config);
        assert!(text.starts_with("dotpair:v1:mac-mini_2:"));
        assert!(!text.contains('='), "base64url without padding: {text}");
        assert_eq!(parse_pairing(&format!("  {text}\n")).unwrap(), config);

        for bad in [
            "dotpair:v2:a:AAAAAAAAAAAAAAAAAAAAAA:7466",
            "pair:v1:a:AAAAAAAAAAAAAAAAAAAAAA:7466",
            "dotpair:v1:a:AAAAAAAAAAAAAAAAAAAAAA",
            "dotpair:v1:a:AAAA:7466",
            "dotpair:v1:a:AAAAAAAAAAAAAAAAAAAAAA:0",
            "dotpair:v1:a:AAAAAAAAAAAAAAAAAAAAAA:99999",
            "dotpair:v1::AAAAAAAAAAAAAAAAAAAAAA:7466",
        ] {
            assert!(parse_pairing(bad).is_err(), "{bad} should be refused");
        }
    }

    #[test]
    fn user_config_is_private_and_reloads() {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().unwrap();
        let config = Config {
            instance_id: "inst".into(),
            secret: vec![7; 32],
            port: dot_wire::GUEST_PORT,
        };
        let path = write_user_config(home.path(), &config).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        assert_eq!(Config::load(&path).unwrap(), config);
    }

    #[test]
    fn debug_output_never_shows_the_secret() {
        let config = Config {
            instance_id: "inst".into(),
            secret: b"super-secret-value!".to_vec(),
            port: 1,
        };
        assert!(!format!("{config:?}").contains("super"));
    }
}
