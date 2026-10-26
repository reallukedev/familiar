//! The per-instance `DOTCONFIG` disk: a tiny FAT image holding `dot-guest.json`, which
//! macOS mounts automatically so the shared golden image learns which instance it is.
use base64::{Engine, engine::general_purpose::STANDARD};
use std::{io::Write, path::Path};

/// 2 MiB is the smallest size every FAT tool and macOS handle without complaint.
const SIZE: u64 = 2 * 1024 * 1024;

/// Write `config.img` for an instance. The file is replaced atomically.
pub fn write(path: &Path, instance_id: &str, secret: &[u8]) -> anyhow::Result<()> {
    let body = serde_json::json!({
        "instance_id": instance_id,
        "secret": STANDARD.encode(secret),
        "port": dot_wire::GUEST_PORT,
    })
    .to_string();
    let tmp = path.with_extension("img.tmp");
    {
        use std::os::unix::fs::OpenOptionsExt;
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)?;
        file.set_len(SIZE)?;
        let mut label = [b' '; 11];
        label[..dot_wire::CONFIG_VOLUME.len()].copy_from_slice(dot_wire::CONFIG_VOLUME.as_bytes());
        fatfs::format_volume(
            &file,
            fatfs::FormatVolumeOptions::new()
                .volume_label(label)
                .fat_type(fatfs::FatType::Fat12),
        )?;
        let fs = fatfs::FileSystem::new(&file, fatfs::FsOptions::new())?;
        let mut f = fs.root_dir().create_file("dot-guest.json")?;
        f.truncate()?;
        f.write_all(body.as_bytes())?;
        f.flush()?;
        drop(f);
        fs.unmount()?;
        file.sync_all()?;
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn image_has_label_and_readable_config() {
        let dir = tempfile::tempdir().unwrap();
        let img = dir.path().join("config.img");
        write(&img, "inst42", b"s3cret").unwrap();
        let file = std::fs::File::open(&img).unwrap();
        let fs = fatfs::FileSystem::new(&file, fatfs::FsOptions::new()).unwrap();
        assert_eq!(fs.volume_label().trim(), "DOTCONFIG");
        let mut text = String::new();
        fs.root_dir()
            .open_file("dot-guest.json")
            .unwrap()
            .read_to_string(&mut text)
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["instance_id"], "inst42");
        assert_eq!(
            STANDARD.decode(v["secret"].as_str().unwrap()).unwrap(),
            b"s3cret"
        );
        assert_eq!(v["port"], 7466);
    }
}
