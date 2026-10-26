//! Running AppleScript and JXA through `osascript`.
//!
//! Bundled scripts live in `scripts/` and are compiled into the binary. Each takes one
//! argument, a JSON object, and returns JSON. Values never go into script source.
use crate::error::{self, OpResult};
use crate::proc::{self, Command};
use dot_wire::{ScriptLanguage, ScriptResult};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use tokio::time::Instant;

/// `osascript`.
pub const OSASCRIPT: &str = "/usr/bin/osascript";
/// Largest JSON input passed through argv (macOS `ARG_MAX` is 1 MiB).
const MAX_ARGV_JSON: usize = 512 * 1024;

/// Bundled JXA sources.
pub mod sources {
    /// Displays, permissions, frontmost app, default browser.
    pub const INFO: &str = include_str!("../scripts/info.js");
    /// Displays and window lookup for screenshots.
    pub const SCREEN: &str = include_str!("../scripts/screen.js");
    /// Accessibility tree, find and actions.
    pub const UI: &str = include_str!("../scripts/ui.js");
    /// Mouse and keyboard events.
    pub const INPUT: &str = include_str!("../scripts/input.js");
    /// App list, resolve, quit and windows.
    pub const APPS: &str = include_str!("../scripts/apps.js");
    /// Move to Trash.
    pub const FILES: &str = include_str!("../scripts/files.js");
    /// Safari.
    pub const SAFARI: &str = include_str!("../scripts/safari.js");
    /// Notification banner.
    pub const NOTIFY: &str = include_str!("../scripts/notify.js");
}

/// Run a bundled JXA script with `input` as `argv[0]` and parse its JSON result.
pub async fn jxa<I, O>(deadline: Instant, source: &str, input: &I) -> OpResult<O>
where
    I: Serialize + ?Sized,
    O: DeserializeOwned,
{
    let json = serde_json::to_string(input)
        .map_err(|e| error::failed(format!("Could not encode the script input: {e}")))?;
    if json.len() > MAX_ARGV_JSON {
        return Err(dot_wire::GuestError::new(
            dot_wire::GuestErrorCode::TooLarge,
            format!(
                "The input is {} KiB; the limit is {} KiB.",
                json.len() / 1024,
                MAX_ARGV_JSON / 1024
            ),
        ));
    }
    let out = proc::tool(Command::new(OSASCRIPT, deadline).args([
        "-l",
        "JavaScript",
        "-e",
        source,
        &json,
    ]))
    .await?;
    if !out.success() {
        return Err(error::osascript_error(&out.stderr_text()));
    }
    let text = out.stdout_text();
    let text = text.strip_suffix('\n').unwrap_or(&text);
    let value: Value = if text.trim().is_empty() {
        Value::Null
    } else {
        serde_json::from_str(text).map_err(|e| {
            error::failed(format!(
                "A bundled script returned something that isn't JSON: {e}"
            ))
        })?
    };
    serde_json::from_value(value).map_err(|e| {
        error::failed(format!(
            "A bundled script returned an unexpected shape: {e}"
        ))
    })
}

/// Run a caller-supplied script from a private temporary file.
pub async fn run_user(
    deadline: Instant,
    language: ScriptLanguage,
    source: &str,
    args: &[String],
) -> OpResult<ScriptResult> {
    if source.trim().is_empty() {
        return Err(error::invalid("The script is empty."));
    }
    let dir = crate::util::temp_dir()?;
    let (lang, file_name) = match language {
        ScriptLanguage::Applescript => ("AppleScript", "script.applescript"),
        ScriptLanguage::Javascript => ("JavaScript", "script.js"),
    };
    let path = dir.path().join(file_name);
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)
        .and_then(|mut f| f.write_all(source.as_bytes()))
        .map_err(|e| {
            error::failed(format!(
                "Could not save the script to a temporary file: {e}"
            ))
        })?;
    let out = proc::tool(
        Command::new(OSASCRIPT, deadline)
            .args(["-l", lang])
            .arg(&path)
            .args(args),
    )
    .await?;
    drop(dir);
    if !out.success() {
        return Err(error::osascript_error(&out.stderr_text()));
    }
    if out.truncated {
        return Err(dot_wire::GuestError::new(
            dot_wire::GuestErrorCode::TooLarge,
            "The script's output was larger than 16 MiB.",
        ));
    }
    let mut output = out.stdout_text();
    if output.ends_with('\n') {
        output.pop();
    }
    Ok(ScriptResult { output })
}
