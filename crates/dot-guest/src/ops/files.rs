//! [`Op::Files`](dot_wire::Op::Files): file operations as the signed-in user.
//!
//! Nothing is ever deleted outright: replacing a destination and `trash` both move the
//! old item to the Trash, where it can be recovered.
use super::Ctx;
use crate::error::{self, OpResult};
use crate::proc::{self, Command};
use crate::script::{self, sources};
use crate::util::{self, expand_path};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use dot_wire::{FileContent, FileEntry, FilesOp, GuestError};
use serde_json::{Value, json};
use std::fs::{self, Metadata};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

const DEFAULT_READ: u64 = 1024 * 1024;
const MAX_READ: u64 = 32 * 1024 * 1024;
const MAX_WRITE: usize = 32 * 1024 * 1024;
const DEFAULT_LIST: u32 = 500;
const MAX_LIST: u32 = 10_000;
const DEFAULT_SEARCH: u32 = 50;
const MAX_SEARCH: u32 = 1000;

/// Folders macOS protects with Full Disk Access or Files and Folders permissions.
const PROTECTED: &[&str] = &[
    "Library/Mail",
    "Library/Messages",
    "Library/Safari",
    "Library/Calendars",
    "Library/Containers",
    "Library/Group Containers",
    "Library/Application Support/AddressBook",
    "Library/Application Support/com.apple.TCC",
    "Library/Cookies",
    "Library/HomeKit",
    "Library/Metadata/CoreSpotlight",
    "Library/Suggestions",
    "Desktop",
    "Documents",
    "Downloads",
];

/// Map an I/O error on `path` to a [`GuestError`].
pub fn io_error(err: &io::Error, path: &Path, home: &Path) -> GuestError {
    let shown = path.display();
    match err.kind() {
        io::ErrorKind::NotFound => error::not_found(format!("Nothing exists at {shown}.")),
        io::ErrorKind::PermissionDenied => {
            // EPERM comes from macOS privacy controls (TCC); EACCES from file permissions.
            let tcc = err.raw_os_error() == Some(1);
            let protected = path
                .strip_prefix(home)
                .ok()
                .is_some_and(|rel| PROTECTED.iter().any(|p| rel.starts_with(p)));
            if tcc || protected {
                error::permission(
                    format!("macOS privacy settings block access to {shown}."),
                    error::HINT_FULL_DISK,
                )
            } else {
                error::permission(
                    format!("You don't have permission to access {shown}."),
                    "Check the file's owner and permissions (Get Info > Sharing & Permissions).",
                )
            }
        }
        io::ErrorKind::AlreadyExists => error::failed(format!("{shown} already exists.")),
        io::ErrorKind::IsADirectory => error::invalid(format!("{shown} is a folder.")),
        io::ErrorKind::NotADirectory => {
            error::invalid(format!("A part of {shown} is a file, not a folder."))
        }
        io::ErrorKind::StorageFull => error::failed("The disk is full."),
        io::ErrorKind::ReadOnlyFilesystem => error::permission(
            format!("{shown} is on a read-only volume."),
            "Choose another location.",
        ),
        _ => error::failed(format!("{shown}: {err}")),
    }
}

fn kind(meta: &Metadata) -> &'static str {
    let t = meta.file_type();
    if t.is_symlink() {
        "symlink"
    } else if t.is_dir() {
        "dir"
    } else if t.is_file() {
        "file"
    } else {
        "other"
    }
}

/// Describe a path without following a final symlink.
pub fn entry(path: &Path, meta: &Metadata) -> FileEntry {
    FileEntry {
        path: path.to_string_lossy().into_owned(),
        kind: kind(meta).to_owned(),
        size: if meta.is_file() { meta.len() } else { 0 },
        modified: meta.modified().ok().map(util::rfc3339),
    }
}

fn stat(path: &Path, home: &Path) -> OpResult<FileEntry> {
    let meta = fs::symlink_metadata(path).map_err(|e| io_error(&e, path, home))?;
    Ok(entry(path, &meta))
}

/// Run blocking file work off the async threads.
async fn blocking<T, F>(f: F) -> OpResult<T>
where
    T: Send + 'static,
    F: FnOnce() -> OpResult<T> + Send + 'static,
{
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| error::failed(format!("The file operation stopped unexpectedly: {e}")))?
}

/// [`dot_wire::Op::Files`].
pub async fn run(ctx: &Ctx, op: FilesOp) -> OpResult {
    let home = ctx.state.home.clone();
    match op {
        FilesOp::Read {
            path,
            offset,
            length,
        } => {
            let path = expand_path(&path, &home)?;
            super::to_value(blocking(move || read(&path, offset, length, &home)).await?)
        }
        FilesOp::Write {
            path,
            content,
            base64,
            append,
            create_only,
            parents,
        } => {
            let path = expand_path(&path, &home)?;
            let bytes = if base64 {
                STANDARD
                    .decode(content.trim())
                    .map_err(|_| error::invalid("`content` is not valid base64."))?
            } else {
                content.into_bytes()
            };
            if bytes.len() > MAX_WRITE {
                return Err(error::invalid(
                    "Write at most 32 MiB at a time; use append for more.",
                ));
            }
            let opts = WriteOpts {
                append,
                create_only,
                parents,
            };
            super::to_value(blocking(move || write(&path, &bytes, opts, &home)).await?)
        }
        FilesOp::List {
            path,
            depth,
            hidden,
            limit,
        } => {
            let path = expand_path(&path, &home)?;
            let limit = limit.unwrap_or(DEFAULT_LIST).clamp(1, MAX_LIST) as usize;
            let depth = depth.min(20);
            super::to_value(blocking(move || list(&path, depth, hidden, limit, &home)).await?)
        }
        FilesOp::Stat { path } => {
            let path = expand_path(&path, &home)?;
            super::to_value(blocking(move || stat(&path, &home)).await?)
        }
        FilesOp::Mkdir { path } => {
            let path = expand_path(&path, &home)?;
            super::to_value(blocking(move || mkdir(&path, &home)).await?)
        }
        FilesOp::Move {
            from,
            to,
            overwrite,
        } => {
            let from = expand_path(&from, &home)?;
            let to = expand_path(&to, &home)?;
            super::to_value(transfer(ctx, &from, &to, overwrite, Transfer::Move).await?)
        }
        FilesOp::Copy {
            from,
            to,
            overwrite,
        } => {
            let from = expand_path(&from, &home)?;
            let to = expand_path(&to, &home)?;
            super::to_value(transfer(ctx, &from, &to, overwrite, Transfer::Copy).await?)
        }
        FilesOp::Trash { path } => {
            let path = expand_path(&path, &home)?;
            refuse_system_path(&path, &home)?;
            fs::symlink_metadata(&path).map_err(|e| io_error(&e, &path, &home))?;
            trash(ctx, &path).await?;
            Ok(json!({ "trashed": path.to_string_lossy() }))
        }
        FilesOp::Search {
            query,
            scope,
            limit,
        } => {
            let scope = scope.map(|s| expand_path(&s, &home)).transpose()?;
            let limit = limit.unwrap_or(DEFAULT_SEARCH).clamp(1, MAX_SEARCH) as usize;
            super::to_value(search(ctx, &query, scope.as_deref(), limit).await?)
        }
    }
}

fn read(path: &Path, offset: u64, length: Option<u64>, home: &Path) -> OpResult<FileContent> {
    let length = length.unwrap_or(DEFAULT_READ);
    if length > MAX_READ {
        return Err(error::invalid(
            "Read at most 32 MiB at a time; use offset to page.",
        ));
    }
    let mut file = fs::File::open(path).map_err(|e| io_error(&e, path, home))?;
    let meta = file.metadata().map_err(|e| io_error(&e, path, home))?;
    if meta.is_dir() {
        return Err(error::invalid(format!(
            "{} is a folder; list it instead.",
            path.display()
        )));
    }
    let size = meta.len();
    file.seek(SeekFrom::Start(offset))
        .map_err(|e| io_error(&e, path, home))?;
    let mut bytes = Vec::with_capacity(usize::try_from(length.min(size)).unwrap_or(0));
    file.take(length)
        .read_to_end(&mut bytes)
        .map_err(|e| io_error(&e, path, home))?;
    let end = offset.saturating_add(bytes.len() as u64);
    let truncated = end < size;
    let mime = util::mime_for(path).to_owned();
    let (content, base64) = match util::utf8_text(&bytes, truncated) {
        Some(text) if !text.contains('\0') => (text, false),
        _ => (STANDARD.encode(&bytes), true),
    };
    Ok(FileContent {
        path: path.to_string_lossy().into_owned(),
        content,
        base64,
        size,
        truncated,
        mime,
    })
}

#[derive(Debug, Clone, Copy)]
struct WriteOpts {
    append: bool,
    create_only: bool,
    parents: bool,
}

fn write(path: &Path, bytes: &[u8], opts: WriteOpts, home: &Path) -> OpResult<FileEntry> {
    // Write through a symlink to its target, so dotfile links stay links.
    let path = match fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_symlink() => {
            fs::canonicalize(path).map_err(|e| io_error(&e, path, home))?
        }
        _ => path.to_path_buf(),
    };
    let path = path.as_path();
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .ok_or_else(|| error::invalid("Give a file path, not the root folder."))?;
    if opts.parents {
        fs::create_dir_all(parent).map_err(|e| io_error(&e, parent, home))?;
    } else if !parent.is_dir() {
        return Err(error::not_found(format!(
            "The folder {} does not exist. Pass parents to create it.",
            parent.display()
        )));
    }
    let existing = fs::metadata(path).ok();
    if existing.as_ref().is_some_and(Metadata::is_dir) {
        return Err(error::invalid(format!("{} is a folder.", path.display())));
    }
    if opts.create_only && existing.is_some() {
        return Err(error::failed(format!(
            "{} already exists and create_only was set.",
            path.display()
        )));
    }
    if opts.append {
        let mut file = fs::OpenOptions::new()
            .append(true)
            .create(!opts.create_only)
            .create_new(opts.create_only)
            .mode(0o644)
            .open(path)
            .map_err(|e| io_error(&e, path, home))?;
        file.write_all(bytes)
            .map_err(|e| io_error(&e, path, home))?;
        return stat(path, home);
    }

    let name = path
        .file_name()
        .ok_or_else(|| error::invalid("Give a file path, not a folder."))?
        .to_string_lossy();
    let tmp = parent.join(format!(
        ".{name}.dot-guest-{}.tmp",
        util::hex(&dot_wire::random_bytes::<6>())
    ));
    let mode = existing
        .as_ref()
        .map_or(0o644, |m| m.permissions().mode() & 0o7777);
    let result = (|| -> io::Result<()> {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp)?;
        file.write_all(bytes)?;
        file.set_permissions(fs::Permissions::from_mode(mode))?;
        file.sync_all()?;
        if opts.create_only {
            // Hard-linking fails if the name was taken meanwhile, unlike rename.
            fs::hard_link(&tmp, path)?;
            fs::remove_file(&tmp)?;
        } else {
            fs::rename(&tmp, path)?;
        }
        Ok(())
    })();
    if let Err(e) = result {
        let _ = fs::remove_file(&tmp);
        return Err(io_error(&e, path, home));
    }
    stat(path, home)
}

fn list(
    root: &Path,
    depth: u32,
    hidden: bool,
    limit: usize,
    home: &Path,
) -> OpResult<Vec<FileEntry>> {
    let meta = fs::metadata(root).map_err(|e| io_error(&e, root, home))?;
    if !meta.is_dir() {
        return Err(error::invalid(format!(
            "{} is not a folder.",
            root.display()
        )));
    }
    let mut out = Vec::new();
    list_into(root, depth, hidden, limit, home, &mut out, true)?;
    Ok(out)
}

fn list_into(
    dir: &Path,
    depth: u32,
    hidden: bool,
    limit: usize,
    home: &Path,
    out: &mut Vec<FileEntry>,
    top: bool,
) -> OpResult<()> {
    let reader = match fs::read_dir(dir) {
        Ok(r) => r,
        // Unreadable subfolders are skipped; only the top folder is an error.
        Err(e) if top => return Err(io_error(&e, dir, home)),
        Err(_) => return Ok(()),
    };
    let mut items: Vec<(PathBuf, Metadata)> = reader
        .filter_map(Result::ok)
        .filter(|e| hidden || !e.file_name().to_string_lossy().starts_with('.'))
        .filter_map(|e| {
            let path = e.path();
            fs::symlink_metadata(&path).ok().map(|m| (path, m))
        })
        .collect();
    items.sort_by(|(a, am), (b, bm)| {
        bm.is_dir().cmp(&am.is_dir()).then_with(|| {
            let a = a.file_name().map(|n| n.to_string_lossy().to_lowercase());
            let b = b.file_name().map(|n| n.to_string_lossy().to_lowercase());
            a.cmp(&b)
        })
    });
    for (path, meta) in items {
        if out.len() >= limit {
            return Ok(());
        }
        out.push(entry(&path, &meta));
        if depth > 0 && meta.is_dir() {
            list_into(&path, depth - 1, hidden, limit, home, out, false)?;
        }
    }
    Ok(())
}

fn mkdir(path: &Path, home: &Path) -> OpResult<FileEntry> {
    if let Ok(meta) = fs::metadata(path)
        && !meta.is_dir()
    {
        return Err(error::failed(format!(
            "{} already exists and is a file.",
            path.display()
        )));
    }
    fs::create_dir_all(path).map_err(|e| io_error(&e, path, home))?;
    stat(path, home)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Transfer {
    Move,
    Copy,
}

async fn transfer(
    ctx: &Ctx,
    from: &Path,
    to: &Path,
    overwrite: bool,
    how: Transfer,
) -> OpResult<FileEntry> {
    let home = &ctx.state.home;
    let source = fs::symlink_metadata(from).map_err(|e| io_error(&e, from, home))?;
    if from == to {
        return Err(error::invalid("The source and destination are the same."));
    }
    if source.is_dir() && to.starts_with(from) {
        return Err(error::invalid(
            "A folder can't be moved or copied into itself.",
        ));
    }
    if how == Transfer::Move {
        refuse_system_path(from, home)?;
    }
    let parent = to
        .parent()
        .ok_or_else(|| error::invalid("Give a destination path, not the root folder."))?;
    if !parent.is_dir() {
        return Err(error::not_found(format!(
            "The destination folder {} does not exist.",
            parent.display()
        )));
    }
    if fs::symlink_metadata(to).is_ok() {
        if !overwrite {
            return Err(error::failed(format!(
                "{} already exists. Pass overwrite to replace it (the old one goes to the Trash).",
                to.display()
            )));
        }
        trash(ctx, to).await?;
    }
    match how {
        Transfer::Move => match fs::rename(from, to) {
            Ok(()) => {}
            // Different volumes: copy, then send the original to the Trash.
            Err(e) if e.raw_os_error() == Some(18) => {
                ditto(ctx, from, to).await?;
                trash(ctx, from).await?;
            }
            Err(e) => return Err(io_error(&e, from, home)),
        },
        Transfer::Copy if source.is_dir() => ditto(ctx, from, to).await?,
        Transfer::Copy => {
            let (src, dst, home2) = (from.to_path_buf(), to.to_path_buf(), home.clone());
            blocking(move || {
                fs::copy(&src, &dst)
                    .map(|_| ())
                    .map_err(|e| io_error(&e, &src, &home2))
            })
            .await?;
        }
    }
    stat(to, home)
}

async fn ditto(ctx: &Ctx, from: &Path, to: &Path) -> OpResult<()> {
    proc::tool_ok(
        Command::new("/usr/bin/ditto", ctx.deadline)
            .arg(from)
            .arg(to),
    )
    .await?;
    Ok(())
}

/// Refuse to move or trash anything that belongs to macOS itself.
fn refuse_system_path(path: &Path, home: &Path) -> OpResult<()> {
    let system = [
        "/System",
        "/usr",
        "/bin",
        "/sbin",
        "/Library/Apple",
        "/private/var/db",
    ];
    if path == Path::new("/") || path == home || system.iter().any(|s| path.starts_with(s)) {
        return Err(error::invalid(format!(
            "{} belongs to macOS or is your home folder; the agent won't move or trash it.",
            path.display()
        )));
    }
    Ok(())
}

/// Move to the Trash (recoverable).
pub async fn trash(ctx: &Ctx, path: &Path) -> OpResult<()> {
    let _: Value = script::jxa(
        ctx.deadline,
        sources::FILES,
        &json!({ "path": path.to_string_lossy() }),
    )
    .await?;
    Ok(())
}

async fn search(
    ctx: &Ctx,
    query: &str,
    scope: Option<&Path>,
    limit: usize,
) -> OpResult<Vec<FileEntry>> {
    let query = query.trim();
    if query.is_empty() {
        return Err(error::invalid("The search query is empty."));
    }
    if query.starts_with('-') {
        return Err(error::invalid("A search query can't start with `-`."));
    }
    let mut cmd = Command::new("/usr/bin/mdfind", ctx.deadline).max_output(4 * 1024 * 1024);
    if let Some(scope) = scope {
        if !scope.is_dir() {
            return Err(error::not_found(format!(
                "The folder {} does not exist.",
                scope.display()
            )));
        }
        cmd = cmd.arg("-onlyin").arg(scope);
    }
    let out = proc::tool_ok(cmd.arg(query)).await?;
    let text = out.stdout_text();
    let paths: Vec<PathBuf> = text
        .lines()
        .filter(|l| !l.is_empty())
        .take(limit)
        .map(PathBuf::from)
        .collect();
    blocking(move || {
        Ok(paths
            .iter()
            .filter_map(|p| fs::symlink_metadata(p).ok().map(|m| entry(p, &m)))
            .collect())
    })
    .await
}

#[cfg(test)]
mod tests {
    #![expect(
        clippy::unwrap_used,
        reason = "Test assertions fail immediately on malformed setup"
    )]
    use super::*;

    #[test]
    fn privacy_denials_point_at_full_disk_access() {
        let home = Path::new("/Users/dot");
        let eperm = io::Error::from_raw_os_error(1);
        let err = io_error(&eperm, Path::new("/Users/dot/Library/Mail/V10"), home);
        assert_eq!(err.code, dot_wire::GuestErrorCode::PermissionDenied);
        assert_eq!(err.hint.as_deref(), Some(error::HINT_FULL_DISK));
        let eacces = io::Error::from_raw_os_error(13);
        let err = io_error(&eacces, Path::new("/etc/sudoers"), home);
        assert_ne!(err.hint.as_deref(), Some(error::HINT_FULL_DISK));
        let missing = io::Error::from_raw_os_error(2);
        assert_eq!(
            io_error(&missing, Path::new("/x"), home).code,
            dot_wire::GuestErrorCode::NotFound
        );
    }

    #[test]
    fn create_only_never_replaces_and_atomic_write_keeps_the_mode() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let path = dir.path().join("a.txt");
        let opts = WriteOpts {
            append: false,
            create_only: true,
            parents: false,
        };
        write(&path, b"one", opts, home).unwrap();
        assert!(write(&path, b"two", opts, home).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"one");

        fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
        let replace = WriteOpts {
            append: false,
            create_only: false,
            parents: false,
        };
        write(&path, b"three", replace, home).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"three");
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o640
        );
        let leftovers: Vec<_> = fs::read_dir(dir.path()).unwrap().collect();
        assert_eq!(leftovers.len(), 1, "no temp files left behind");
    }

    #[test]
    fn writing_through_a_symlink_keeps_the_link() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("real.txt");
        let link = dir.path().join("link.txt");
        fs::write(&target, "old").unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let opts = WriteOpts {
            append: false,
            create_only: false,
            parents: false,
        };
        write(&link, b"new", opts, dir.path()).unwrap();
        assert!(
            fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(fs::read_to_string(&target).unwrap(), "new");
    }

    #[test]
    fn system_locations_are_never_moved_or_trashed() {
        let home = Path::new("/Users/dot");
        for p in [
            "/",
            "/System/Applications/Safari.app",
            "/usr/bin/true",
            "/Users/dot",
        ] {
            assert!(refuse_system_path(Path::new(p), home).is_err(), "{p}");
        }
        assert!(refuse_system_path(Path::new("/Users/dot/Desktop/a.txt"), home).is_ok());
    }
}
