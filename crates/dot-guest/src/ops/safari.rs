//! [`Op::Safari`](dot_wire::Op::Safari) through `scripts/safari.js`.
use super::Ctx;
use crate::error::{self, OpResult};
use crate::script::{self, sources};
use dot_wire::{DomAction, SafariOp, SafariPage, SafariTab};
use serde_json::{Value, json};
use std::time::Duration;

const DEFAULT_MAX_CHARS: u32 = 60_000;
const MAX_CHARS: u32 = 2_000_000;
/// Time kept back for the script to report a timeout itself.
const SCRIPT_MARGIN: Duration = Duration::from_millis(1000);

fn index(name: &str, value: Option<u32>) -> OpResult<()> {
    match value {
        Some(0) => Err(error::invalid(format!("{name} indexes start at 1."))),
        _ => Ok(()),
    }
}

/// Safari needs Automation permission for every op; name it when it's missing.
fn automation_hint(mut e: dot_wire::GuestError) -> dot_wire::GuestError {
    if e.code == dot_wire::GuestErrorCode::PermissionDenied && e.hint.is_none() {
        e.hint = Some(error::automation_hint(Some("Safari")));
    }
    e
}

async fn call<T: serde::de::DeserializeOwned>(ctx: &Ctx, input: Value) -> OpResult<T> {
    script::jxa(ctx.deadline, sources::SAFARI, &input)
        .await
        .map_err(automation_hint)
}

/// [`dot_wire::Op::Safari`].
pub async fn run(ctx: &Ctx, op: SafariOp) -> OpResult {
    match op {
        SafariOp::Tabs => {
            super::to_value(call::<Vec<SafariTab>>(ctx, json!({ "action": "tabs" })).await?)
        }
        SafariOp::Open { url, new_tab } => {
            let url = url.trim();
            if !super::misc::has_scheme(url) {
                return Err(error::invalid(format!(
                    "`{url}` is not a URL. Include the scheme, for example https://example.com."
                )));
            }
            super::to_value(
                call::<SafariTab>(
                    ctx,
                    json!({ "action": "open", "url": url, "new_tab": new_tab }),
                )
                .await?,
            )
        }
        SafariOp::Activate { window, tab } => {
            index("Window", Some(window))?;
            index("Tab", Some(tab))?;
            super::to_value(
                call::<SafariTab>(
                    ctx,
                    json!({ "action": "activate", "window": window, "tab": tab }),
                )
                .await?,
            )
        }
        SafariOp::Close { window, tab } => {
            index("Window", Some(window))?;
            index("Tab", Some(tab))?;
            call::<Value>(
                ctx,
                json!({ "action": "close", "window": window, "tab": tab }),
            )
            .await?;
            Ok(json!({}))
        }
        SafariOp::Read {
            window,
            tab,
            max_chars,
        } => {
            index("Window", window)?;
            index("Tab", tab)?;
            let max_chars = max_chars.unwrap_or(DEFAULT_MAX_CHARS).clamp(1, MAX_CHARS);
            super::to_value(
                call::<SafariPage>(
                    ctx,
                    json!({ "action": "read", "window": window, "tab": tab, "max_chars": max_chars }),
                )
                .await?,
            )
        }
        SafariOp::Eval {
            window,
            tab,
            script,
        } => {
            index("Window", window)?;
            index("Tab", tab)?;
            if script.trim().is_empty() {
                return Err(error::invalid("The script is empty."));
            }
            call(
                ctx,
                json!({ "action": "eval", "window": window, "tab": tab, "script": script }),
            )
            .await
        }
        SafariOp::Dom {
            window,
            tab,
            selector,
            act,
        } => {
            index("Window", window)?;
            index("Tab", tab)?;
            if selector.trim().is_empty() {
                return Err(error::invalid("The CSS selector is empty."));
            }
            let act = match act {
                DomAction::Click => json!({ "type": "click" }),
                DomAction::Fill { value } => json!({ "type": "fill", "value": value }),
                DomAction::Select { value } => json!({ "type": "select", "value": value }),
                DomAction::Submit => json!({ "type": "submit" }),
                DomAction::Scroll => json!({ "type": "scroll" }),
                DomAction::Inspect => json!({ "type": "inspect" }),
            };
            call(
                ctx,
                json!({ "action": "dom", "window": window, "tab": tab, "selector": selector, "do": act }),
            )
            .await
        }
        SafariOp::Navigate { direction } => {
            let direction = direction.trim().to_lowercase();
            if !matches!(direction.as_str(), "back" | "forward" | "reload") {
                return Err(error::invalid("direction must be back, forward or reload."));
            }
            super::to_value(
                call::<SafariTab>(ctx, json!({ "action": "navigate", "direction": direction }))
                    .await?,
            )
        }
        SafariOp::WaitLoaded => super::to_value(
            call::<SafariTab>(
                ctx,
                json!({ "action": "wait_loaded", "budget_ms": ctx.budget_ms(SCRIPT_MARGIN) }),
            )
            .await?,
        ),
    }
}
