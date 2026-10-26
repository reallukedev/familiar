//! Serving `dot-guest` to Macs: `curl -fsSL https://<server>/guest/install.sh | sh`.
//!
//! The binary is a macOS universal build placed at `<data>/guest/dot-guest` (see
//! `scripts/build-guest.sh`). Neither file is secret; the script pins the binary's SHA-256.
use crate::state::App;
use axum::{
    body::Body,
    extract::State,
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};

fn binary_path(app: &App) -> std::path::PathBuf {
    app.config.data_dir.join("guest").join("dot-guest")
}

type Cached = Option<(std::time::SystemTime, bytes::Bytes, String)>;
static CACHE: tokio::sync::Mutex<Cached> = tokio::sync::Mutex::const_new(None);

/// The binary and its SHA-256, re-read only when the file changes.
async fn load(app: &App) -> Option<(bytes::Bytes, String)> {
    let path = binary_path(app);
    let modified = tokio::fs::metadata(&path).await.ok()?.modified().ok()?;
    let mut cache = CACHE.lock().await;
    if let Some((m, b, h)) = cache.as_ref()
        && *m == modified
    {
        return Some((b.clone(), h.clone()));
    }
    let data = bytes::Bytes::from(tokio::fs::read(&path).await.ok()?);
    let sha: String = crate::crypto::sha256(&data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    *cache = Some((modified, data.clone(), sha.clone()));
    Some((data, sha))
}

/// `GET /guest/dot-guest`.
pub async fn binary(State(app): State<App>) -> Response {
    match load(&app).await {
        Some((bytes, _)) => (
            [
                (header::CONTENT_TYPE, "application/octet-stream"),
                (header::CACHE_CONTROL, "no-cache"),
            ],
            Body::from(bytes),
        )
            .into_response(),
        None => (
            StatusCode::NOT_FOUND,
            "dot-guest hasn't been added to this server yet. See docs/home/SETUP.md.",
        )
            .into_response(),
    }
}

/// `GET /guest/install.sh`.
pub async fn script(State(app): State<App>, headers: HeaderMap) -> Response {
    let Some((_, sha)) = load(&app).await else {
        return (
            StatusCode::NOT_FOUND,
            "echo 'dot-guest has not been added to this server yet.' >&2; exit 1\n",
        )
            .into_response();
    };
    // Fetched from inside one of this server's Macs: download over the same internal
    // address, since the public one may not loop back. The SHA-256 still pins the binary.
    let vm = app.config.vm_origin();
    let internal = headers
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .is_some_and(|h| vm.strip_prefix("http://") == Some(h));
    let origin = if internal { vm } else { app.config.origin() };
    let script = format!(
        r#"#!/bin/sh
# Installs Familiar's agent into this Mac. Safe to run again.
set -eu
tmp=$(mktemp -t dot-guest)
trap 'rm -f "$tmp"' EXIT
echo "Downloading dot-guest from {origin}"
curl -fsSL --proto '=https' --tlsv1.2 "{origin}/guest/dot-guest" -o "$tmp" || curl -fsSL "{origin}/guest/dot-guest" -o "$tmp"
echo "{sha}  $tmp" | shasum -a 256 -c - >/dev/null || {{ echo "The download didn't match. Try again." >&2; exit 1; }}
chmod 755 "$tmp"
xattr -c "$tmp" 2>/dev/null || true
# install copies the agent into ~/Library/Application Support/Dot Guest (no admin needed).
"$tmp" install
dest="$HOME/Library/Application Support/Dot Guest/launcher/dot-guest"
echo
echo "Installed at $dest"
echo "Next: run \"$dest\" prepare-image (for the base image) or \"$dest\" pair <code> (for a remote Mac),"
echo "then \"$dest\" doctor to check permissions."
"#
    );
    (
        [
            (header::CONTENT_TYPE, "text/x-shellscript; charset=utf-8"),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        script,
    )
        .into_response()
}
