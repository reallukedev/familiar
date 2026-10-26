//! Small ops: open a URL, clipboard, Shortcuts, notifications.
use super::Ctx;
use crate::error::{self, OpResult};
use crate::proc::{self, Command};
use crate::script::{self, sources};
use dot_wire::{ClipboardOp, NotifyArgs, OpenUrlArgs, ShortcutsOp};
use serde_json::{Value, json};

/// `open`.
pub const OPEN: &str = "/usr/bin/open";

/// Is `url` shaped like `scheme:rest`, with a scheme `open` will not mistake for a flag?
pub fn has_scheme(url: &str) -> bool {
    let Some((scheme, rest)) = url.split_once(':') else {
        return false;
    };
    let mut chars = scheme.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
        && !rest.is_empty()
}

/// Looks like `com.apple.Safari` rather than a display name or a path.
pub fn looks_like_bundle_id(app: &str) -> bool {
    app.contains('.')
        && !app.contains('/')
        && !app.contains(' ')
        && !app.ends_with(".app")
        && app
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
}

/// `open -a <name>` / `open -b <id>` / `open <path.app>` arguments for an app.
pub fn open_app_args(app: &str) -> Vec<String> {
    if app.starts_with('/') {
        vec!["-a".into(), app.into()]
    } else if looks_like_bundle_id(app) {
        vec!["-b".into(), app.into()]
    } else {
        vec!["-a".into(), app.into()]
    }
}

/// Map `open` failures: an unknown app is NotFound.
pub fn open_error(stderr: &str, app: Option<&str>) -> dot_wire::GuestError {
    let lower = stderr.to_lowercase();
    if lower.contains("unable to find application")
        || lower.contains("-10814")
        || lower.contains("lscopyapplicationurlsforbundleidentifier")
    {
        return error::not_found(format!(
            "No app called \"{}\" is installed.",
            app.unwrap_or("that")
        ));
    }
    if lower.contains("does not exist") {
        return error::not_found(stderr.trim().to_owned());
    }
    error::failed(format!("`open` failed: {}", stderr.trim()))
}

/// [`dot_wire::Op::OpenUrl`].
pub async fn open_url(ctx: &Ctx, args: OpenUrlArgs) -> OpResult {
    let url = args.url.trim();
    if !has_scheme(url) {
        return Err(error::invalid(format!(
            "`{url}` is not a URL. Include the scheme, for example https://example.com."
        )));
    }
    let mut cmd = Command::new(OPEN, ctx.deadline);
    if let Some(app) = args.app.as_deref() {
        cmd = cmd.args(open_app_args(app));
    }
    let out = proc::tool(cmd.arg(url)).await?;
    if !out.success() {
        return Err(open_error(&out.stderr_text(), args.app.as_deref()));
    }
    Ok(json!({}))
}

/// [`dot_wire::Op::Clipboard`].
pub async fn clipboard(ctx: &Ctx, args: ClipboardOp) -> OpResult {
    match args {
        ClipboardOp::Get => {
            let out = proc::tool_ok(
                Command::new("/usr/bin/pbpaste", ctx.deadline)
                    .env("LANG", "en_US.UTF-8")
                    .env("LC_CTYPE", "en_US.UTF-8")
                    .max_output(16 * 1024 * 1024),
            )
            .await?;
            if out.stdout.is_empty() {
                return Ok(json!({ "text": Value::Null }));
            }
            Ok(json!({ "text": out.stdout_text(), "truncated": out.truncated }))
        }
        ClipboardOp::Set { text } => {
            proc::tool_ok(
                Command::new("/usr/bin/pbcopy", ctx.deadline)
                    .env("LANG", "en_US.UTF-8")
                    .env("LC_CTYPE", "en_US.UTF-8")
                    .stdin(text.into_bytes()),
            )
            .await?;
            Ok(json!({}))
        }
    }
}

/// [`dot_wire::Op::Shortcuts`].
pub async fn shortcuts(ctx: &Ctx, args: ShortcutsOp) -> OpResult {
    const SHORTCUTS: &str = "/usr/bin/shortcuts";
    match args {
        ShortcutsOp::List => {
            let out = proc::tool_ok(Command::new(SHORTCUTS, ctx.deadline).arg("list")).await?;
            let names: Vec<String> = out
                .stdout_text()
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(str::to_owned)
                .collect();
            super::to_value(names)
        }
        ShortcutsOp::Run { name, input } => {
            if name.trim().is_empty() || name.starts_with('-') {
                return Err(error::invalid(
                    "Give the shortcut's name as it appears in the Shortcuts app.",
                ));
            }
            let dir = crate::util::temp_dir()?;
            let output_path = dir.path().join("output");
            let mut cmd = Command::new(SHORTCUTS, ctx.deadline).args(["run", &name]);
            if let Some(input) = input {
                let input_path = dir.path().join("input.txt");
                std::fs::write(&input_path, input).map_err(|e| {
                    error::failed(format!("Could not save the shortcut input: {e}"))
                })?;
                cmd = cmd.arg("--input-path").arg(&input_path);
            }
            let out = proc::tool(cmd.arg("--output-path").arg(&output_path)).await?;
            if !out.success() {
                let stderr = out.stderr_text();
                let lower = stderr.to_lowercase();
                if lower.contains("couldn\u{2019}t find") || lower.contains("couldn't find") {
                    return Err(error::not_found(format!(
                        "There is no shortcut called \"{name}\". List shortcuts to see the exact names."
                    )));
                }
                return Err(error::failed(format!("The shortcut failed: {stderr}")));
            }
            let output = match std::fs::read(&output_path) {
                Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
                Err(_) => out.stdout_text(),
            };
            Ok(json!({ "output": output }))
        }
    }
}

/// [`dot_wire::Op::Notify`].
pub async fn notify(ctx: &Ctx, args: NotifyArgs) -> OpResult {
    if args.title.trim().is_empty() && args.message.trim().is_empty() {
        return Err(error::invalid("A notification needs a title or a message."));
    }
    let _: Value = script::jxa(ctx.deadline, sources::NOTIFY, &args).await?;
    Ok(json!({}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_scheme_check_accepts_real_urls_and_refuses_flags() {
        for ok in [
            "https://example.com",
            "mailto:a@b.c",
            "x-apple.systempreferences:com.apple",
        ] {
            assert!(has_scheme(ok), "{ok}");
        }
        for bad in ["example.com", "-a Safari", "://x", "1http://x", "https:"] {
            assert!(!has_scheme(bad), "{bad}");
        }
    }

    #[test]
    fn app_arguments_pick_bundle_id_or_name() {
        assert_eq!(
            open_app_args("com.apple.Safari"),
            ["-b", "com.apple.Safari"]
        );
        assert_eq!(
            open_app_args("Visual Studio Code"),
            ["-a", "Visual Studio Code"]
        );
        assert_eq!(open_app_args("Safari.app"), ["-a", "Safari.app"]);
        assert_eq!(
            open_app_args("/Applications/X.app"),
            ["-a", "/Applications/X.app"]
        );
    }
}
