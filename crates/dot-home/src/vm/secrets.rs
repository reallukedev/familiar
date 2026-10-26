//! Handing a Mac's disk key to QEMU without a command line, `ps` or a log seeing it.
//!
//! The key goes into a 0600 file in a private 0700 directory, ideally on a tmpfs, and
//! QEMU or qemu-img reads it through `-object secret,id=dotkey,file=…`. The file is
//! deleted as soon as the process has read it: QEMU reads secrets while it starts, so
//! [`SecretFile`] is dropped right after `-daemonize` returns or qemu-img exits.
use std::{
    io::Write,
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

/// The QEMU object id the drives refer to (`encrypt.key-secret=dotkey`).
pub const SECRET_ID: &str = "dotkey";

/// The private directory for key files: the configured one, else systemd's
/// `$RUNTIME_DIRECTORY`, else `/dev/shm`, else `<data_dir>/run`. Created 0700; refused
/// when it exists but isn't a real directory owned by this user.
pub fn runtime_dir(configured: Option<&Path>, data_dir: &Path) -> anyhow::Result<PathBuf> {
    let uid = nix::unistd::geteuid().as_raw();
    let dir = if let Some(dir) = configured {
        dir.to_path_buf()
    } else if let Some(dir) = std::env::var_os("RUNTIME_DIRECTORY").filter(|v| !v.is_empty()) {
        // systemd may list several, separated by colons; the first is ours.
        let first = dir
            .to_string_lossy()
            .split(':')
            .next()
            .unwrap_or("")
            .to_owned();
        PathBuf::from(first).join("keys")
    } else if Path::new("/dev/shm").is_dir() {
        PathBuf::from(format!("/dev/shm/familiar-{uid}"))
    } else {
        data_dir.join("run")
    };
    match std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&dir)
    {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(anyhow::anyhow!("creating {}: {e}", dir.display())),
    }
    // A shared place like /dev/shm could hold someone else's directory or a symlink.
    let meta = std::fs::symlink_metadata(&dir)?;
    anyhow::ensure!(
        meta.is_dir() && meta.uid() == uid,
        "{} isn't a private directory owned by this user",
        dir.display()
    );
    if meta.permissions().mode() & 0o777 != 0o700 {
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(dir)
}

/// A key in a private file, deleted on drop.
pub struct SecretFile {
    path: PathBuf,
}

impl SecretFile {
    /// Write `value` to a new 0600 file in `dir`.
    pub fn create(dir: &Path, value: &[u8]) -> anyhow::Result<Self> {
        let path = dir.join(format!("{}.key", crate::crypto::random_id::<10>()));
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)?;
        let me = Self { path };
        file.write_all(value)?;
        Ok(me)
    }

    /// The file's path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// `secret,id=dotkey,file=…` for `-object` / `--object`.
    pub fn object(&self) -> String {
        format!(
            "secret,id={SECRET_ID},file={},format=raw",
            escape(&self.path)
        )
    }
}

impl Drop for SecretFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// A path inside a QEMU option list, where a comma must be doubled.
pub fn escape(path: &Path) -> String {
    path.to_string_lossy().replace(',', ",,")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_files_are_private_and_gone_after_use() {
        let dir = tempfile::tempdir().unwrap();
        let run = runtime_dir(Some(&dir.path().join("run")), dir.path()).unwrap();
        let mode = std::fs::metadata(&run).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
        let path = {
            let secret = SecretFile::create(&run, b"passphrase").unwrap();
            let meta = std::fs::metadata(secret.path()).unwrap();
            assert_eq!(meta.permissions().mode() & 0o777, 0o600);
            assert_eq!(std::fs::read(secret.path()).unwrap(), b"passphrase");
            let object = secret.object();
            assert!(object.starts_with("secret,id=dotkey,file="));
            assert!(!object.contains("passphrase"), "only the path is passed");
            secret.path().to_path_buf()
        };
        assert!(!path.exists(), "deleted on drop");
        assert_eq!(escape(Path::new("/a,b")), "/a,,b");
    }

    #[test]
    fn refuses_a_symlinked_runtime_dir() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("elsewhere");
        std::fs::create_dir(&target).unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(runtime_dir(Some(&link), dir.path()).is_err());
    }
}
