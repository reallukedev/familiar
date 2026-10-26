//! Small pure helpers shared by the operations.
use crate::error::{self, OpResult};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// The signed-in user's home folder.
pub fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/var/root"))
}

/// Expand a leading `~` and require an absolute path.
pub fn expand_path(raw: &str, home: &Path) -> OpResult<PathBuf> {
    if raw.is_empty() {
        return Err(error::invalid("The path is empty."));
    }
    if raw.contains('\0') {
        return Err(error::invalid("The path contains a NUL character."));
    }
    let path = if raw == "~" {
        home.to_path_buf()
    } else if let Some(rest) = raw.strip_prefix("~/") {
        home.join(rest)
    } else if raw.starts_with('~') {
        return Err(error::invalid(format!(
            "`{raw}`: only `~` for your own home folder is supported, not `~user`."
        )));
    } else {
        PathBuf::from(raw)
    };
    if !path.is_absolute() {
        return Err(error::invalid(format!(
            "`{raw}` is a relative path. Use an absolute path or one starting with `~/`."
        )));
    }
    Ok(path)
}

/// RFC 3339 in UTC, to the second.
pub fn rfc3339(time: SystemTime) -> String {
    let secs = match time.duration_since(UNIX_EPOCH) {
        Ok(d) => i64::try_from(d.as_secs()).unwrap_or(i64::MAX),
        Err(e) => -i64::try_from(e.duration().as_secs()).unwrap_or(i64::MAX),
    };
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// Days since 1970-01-01 to a proleptic Gregorian date (Howard Hinnant's algorithm).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m as u32, d as u32)
}

/// Escape text for XML element content and attribute values.
pub fn xml_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c => out.push(c),
        }
    }
    out
}

/// Cut `text` to at most `max` characters. Returns whether it was cut.
pub fn truncate_chars(text: &mut String, max: usize) -> bool {
    match text.char_indices().nth(max) {
        Some((at, _)) => {
            text.truncate(at);
            true
        }
        None => false,
    }
}

/// Decode bytes as UTF-8 text. When the bytes were cut mid-character at `cut_end`, the
/// incomplete tail is dropped. Returns `None` for binary data.
pub fn utf8_text(bytes: &[u8], cut_end: bool) -> Option<String> {
    match std::str::from_utf8(bytes) {
        Ok(text) => Some(text.to_owned()),
        Err(e) if cut_end && e.error_len().is_none() => {
            Some(String::from_utf8_lossy(&bytes[..e.valid_up_to()]).into_owned())
        }
        Err(_) => None,
    }
}

/// MIME type guessed from the file extension.
pub fn mime_for(path: &Path) -> &'static str {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    match ext.as_str() {
        "txt" | "log" | "text" => "text/plain",
        "md" | "markdown" => "text/markdown",
        "html" | "htm" => "text/html",
        "css" => "text/css",
        "csv" => "text/csv",
        "tsv" => "text/tab-separated-values",
        "js" | "mjs" => "text/javascript",
        "json" => "application/json",
        "xml" | "plist" | "sdef" => "application/xml",
        "yaml" | "yml" => "application/yaml",
        "toml" => "application/toml",
        "rs" | "py" | "rb" | "go" | "swift" | "c" | "h" | "cpp" | "m" | "java" | "kt" | "ts"
        | "tsx" | "jsx" | "sh" | "zsh" | "applescript" => "text/plain",
        "pdf" => "application/pdf",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "heic" => "image/heic",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "tif" | "tiff" => "image/tiff",
        "mp3" => "audio/mpeg",
        "m4a" => "audio/mp4",
        "wav" => "audio/wav",
        "mp4" | "m4v" => "video/mp4",
        "mov" => "video/quicktime",
        "zip" => "application/zip",
        "gz" | "tgz" => "application/gzip",
        "dmg" => "application/x-apple-diskimage",
        "pkg" => "application/vnd.apple.installer+xml",
        "doc" => "application/msword",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "pages" => "application/vnd.apple.pages",
        "numbers" => "application/vnd.apple.numbers",
        "key" => "application/vnd.apple.keynote",
        "ics" => "text/calendar",
        "vcf" => "text/vcard",
        "eml" => "message/rfc822",
        _ => "application/octet-stream",
    }
}

/// A private temporary folder (0700) removed when dropped.
pub fn temp_dir() -> OpResult<tempfile::TempDir> {
    tempfile::Builder::new()
        .prefix("dot-guest-")
        .tempdir()
        .map_err(|e| error::failed(format!("Could not create a temporary folder: {e}")))
}

/// Lowercase hex.
pub fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
}

#[cfg(test)]
mod tests {
    #![expect(
        clippy::unwrap_used,
        reason = "Test assertions fail immediately on malformed setup"
    )]
    use super::*;
    use std::time::Duration;

    #[test]
    fn tilde_expands_only_for_the_own_home_and_paths_must_be_absolute() {
        let home = Path::new("/Users/dot");
        assert_eq!(expand_path("~", home).unwrap(), PathBuf::from("/Users/dot"));
        assert_eq!(
            expand_path("~/Documents/a b.txt", home).unwrap(),
            PathBuf::from("/Users/dot/Documents/a b.txt")
        );
        assert_eq!(
            expand_path("/tmp/x", home).unwrap(),
            PathBuf::from("/tmp/x")
        );
        for bad in ["", "relative/x", "~root/x", "a\0b"] {
            assert!(expand_path(bad, home).is_err(), "{bad:?} should be refused");
        }
    }

    #[test]
    fn rfc3339_formats_known_instants() {
        assert_eq!(rfc3339(UNIX_EPOCH), "1970-01-01T00:00:00Z");
        let leap_day = UNIX_EPOCH + Duration::from_secs(951_782_400 + 3_723);
        assert_eq!(rfc3339(leap_day), "2000-02-29T01:02:03Z");
        let late = UNIX_EPOCH + Duration::from_secs(4_102_444_799);
        assert_eq!(rfc3339(late), "2099-12-31T23:59:59Z");
    }

    #[test]
    fn utf8_text_drops_only_a_cut_trailing_character() {
        let bytes = "h\u{e9}llo \u{1f600}".as_bytes();
        let cut = &bytes[..bytes.len() - 2];
        assert_eq!(utf8_text(cut, true).as_deref(), Some("h\u{e9}llo "));
        assert_eq!(utf8_text(cut, false), None);
        assert_eq!(utf8_text(&[0xff, 0x00, 0x41], true), None);
    }

    #[test]
    fn truncate_chars_counts_characters_not_bytes() {
        let mut s = "\u{e9}\u{e9}\u{e9}".to_owned();
        assert!(truncate_chars(&mut s, 2));
        assert_eq!(s, "\u{e9}\u{e9}");
        assert!(!truncate_chars(&mut s, 5));
    }
}
