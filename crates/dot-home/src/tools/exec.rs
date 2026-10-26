//! Executing tools against the Mac.
use super::{Caller, Def, ToolError};
use crate::{
    guest::{Link, LinkError},
    packs::Language,
    state::App,
    vm::Machine,
};
use dot_wire::{
    AppsOp, ClipboardOp, DomAction, FilesOp, ImageFormat, InstallSource, KeyboardArgs, McpOp, Op,
    PointerArgs, Rect, SafariOp, Screenshot, ScreenshotArgs, Script, ScriptLanguage, ShortcutsOp,
    UiAction, UiActionArgs, UiFindArgs, UiTree, UiTreeArgs,
};
use rmcp::model::{CallToolResult, ContentBlock as Content};
use serde_json::{Map, Value, json};
use std::{sync::Arc, time::Duration};

/// Everything a tool needs.
pub struct Ctx<'a> {
    pub app: &'a App,
    pub caller: &'a Caller,
    pub machine: &'a Arc<Machine>,
    pub link: Arc<Link>,
    pub deadline: Duration,
}

type Out = Result<CallToolResult, ToolError>;

fn terr(code: &str, message: impl Into<String>) -> ToolError {
    ToolError {
        code: code.to_owned(),
        message: message.into(),
        extra: json!({}),
    }
}

impl From<LinkError> for ToolError {
    fn from(e: LinkError) -> Self {
        match e {
            LinkError::Disconnected => terr(
                "MAC_UNAVAILABLE",
                "The Mac's agent disconnected. Wait a few seconds and try again; if the action changed something, check before repeating it.",
            ),
            LinkError::Timeout => terr(
                "TIMEOUT",
                "The Mac didn't finish in time. The action may have partly happened; check before repeating it.",
            ),
            LinkError::Guest(g) => ToolError {
                code: serde_json::to_value(g.code)
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_owned))
                    .unwrap_or_else(|| "FAILED".into()),
                message: g.message,
                extra: g.hint.map_or_else(|| json!({}), |h| json!({ "hint": h })),
            },
            LinkError::Decode(d) => terr(
                "FAILED",
                format!(
                    "The Mac's agent sent something unexpected ({d}). Update dot-guest in the Mac."
                ),
            ),
        }
    }
}

fn s<'a>(args: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str)
}
fn n(args: &Map<String, Value>, key: &str) -> Option<f64> {
    args.get(key).and_then(Value::as_f64)
}
fn u(args: &Map<String, Value>, key: &str) -> Option<u64> {
    args.get(key).and_then(Value::as_u64)
}
fn b(args: &Map<String, Value>, key: &str) -> bool {
    args.get(key).and_then(Value::as_bool).unwrap_or(false)
}
fn req<'a>(args: &'a Map<String, Value>, key: &str) -> Result<&'a str, ToolError> {
    s(args, key).ok_or_else(|| terr("INVALID_ARGS", format!("{key} is required.")))
}

fn ok(value: Value) -> Out {
    Ok(CallToolResult::structured(value))
}

impl Ctx<'_> {
    async fn call(&self, op: Op) -> Result<Value, ToolError> {
        Ok(self.link.call(op, self.deadline).await?)
    }
    async fn call_for(&self, op: Op, seconds: u64) -> Result<Value, ToolError> {
        let d = Duration::from_secs(seconds).min(self.deadline);
        Ok(self.link.call(op, d).await?)
    }
    /// Input actions run one at a time per Mac.
    async fn input(&self, op: Op) -> Result<Value, ToolError> {
        let _guard = self.machine.input.lock().await;
        self.call(op).await
    }

    /// Map a position to screen points.
    fn point(
        &self,
        args: &Map<String, Value>,
        xk: &str,
        yk: &str,
    ) -> Result<(f64, f64), ToolError> {
        let (x, y) = match (n(args, xk), n(args, yk)) {
            (Some(x), Some(y)) => (x, y),
            _ => {
                return Err(terr(
                    "INVALID_ARGS",
                    format!("Give {xk} and {yk}, or an element ref."),
                ));
            }
        };
        if s(args, "space") == Some("points") {
            return Ok((x, y));
        }
        let area = *self
            .machine
            .last_screenshot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let scale = *self
            .machine
            .last_scale
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(area) = area else {
            return Err(terr(
                "INVALID_ARGS",
                "Take a mac_screenshot first (positions are pixels in it), or pass space: \"points\".",
            ));
        };
        Ok((area.x + x * scale, area.y + y * scale))
    }

    async fn screenshot(&self, shot: ScreenshotArgs, remember: bool) -> Out {
        let value = self.call(Op::Screenshot(shot)).await?;
        let shot: Screenshot = serde_json::from_value(value)
            .map_err(|e| terr("FAILED", format!("Unexpected screenshot reply: {e}")))?;
        if remember {
            *self
                .machine
                .last_screenshot
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(shot.area);
            *self
                .machine
                .last_scale
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = shot.points_per_pixel;
        }
        let meta = json!({
            "width": shot.width, "height": shot.height, "area_points": shot.area,
            "points_per_pixel": shot.points_per_pixel,
            "note": "Pass pixel positions in this image to mac_click, mac_scroll, mac_drag and mac_hover."
        });
        Ok(CallToolResult::success(vec![
            Content::image(shot.data, shot.mime),
            Content::text(meta.to_string()),
        ]))
    }
}

fn shot_args(args: &Map<String, Value>, app: Option<String>) -> ScreenshotArgs {
    ScreenshotArgs {
        display: u(args, "display").and_then(|d| u32::try_from(d).ok()),
        region: args
            .get("region")
            .and_then(|r| serde_json::from_value::<Rect>(r.clone()).ok()),
        app,
        max_width: u(args, "max_width")
            .and_then(|w| u32::try_from(w).ok())
            .or(Some(1440)),
        format: if s(args, "format") == Some("png") {
            ImageFormat::Png
        } else {
            ImageFormat::Jpeg
        },
        cursor: b(args, "cursor"),
    }
}

fn wt(args: &Map<String, Value>) -> (Option<u32>, Option<u32>) {
    let f = |k| u(args, k).and_then(|v| u32::try_from(v).ok());
    (f("window"), f("tab"))
}

/// Run a resolved tool.
#[allow(clippy::too_many_lines)]
pub async fn run(ctx: &Ctx<'_>, def: &Def, args: &Map<String, Value>) -> Out {
    let name = match def {
        Def::Pack(pack, i) => return run_pack(ctx, &pack.tools[*i], args).await,
        Def::App(tool) => return run_app_mcp(ctx, tool, args).await,
        Def::Builtin(t) => t.name,
    };
    match name {
        "mac_status" => status(ctx).await,
        "mac_screenshot" => {
            ctx.screenshot(
                shot_args(args, s(args, "app").map(str::to_owned)),
                s(args, "app").is_none() && args.get("region").is_none(),
            )
            .await
        }
        "safari_screenshot" => {
            ctx.screenshot(shot_args(args, Some("Safari".into())), false)
                .await
        }
        "mac_ui_tree" => ok(ctx
            .call(Op::UiTree(UiTreeArgs {
                app: s(args, "app").map(str::to_owned),
                max_depth: u(args, "max_depth").map(|v| v as u32),
                max_nodes: u(args, "max_nodes").map(|v| v as u32),
                include_menus: b(args, "include_menus"),
            }))
            .await?),
        "mac_ui_find" => ok(ctx.call(Op::UiFind(find_args(args))).await?),
        "mac_windows" => ok(
            json!({ "windows": ctx.call(Op::Apps(AppsOp::Windows { app: s(args, "app").map(str::to_owned) })).await? }),
        ),
        "mac_apps" => ok(
            json!({ "apps": ctx.call(Op::Apps(AppsOp::List { running_only: b(args, "running_only") })).await? }),
        ),
        "app_dictionary" => ok(ctx
            .call(Op::Apps(AppsOp::Dictionary {
                app: req(args, "app")?.to_owned(),
            }))
            .await?),
        "mac_wait" => wait(ctx, args).await,
        "mac_click" => {
            if let Some(r) = s(args, "ref") {
                ctx.input(Op::UiAction(UiActionArgs {
                    reference: r.to_owned(),
                    action: UiAction::Press,
                }))
                .await?;
                return ok(json!({ "pressed": r }));
            }
            let (x, y) = ctx.point(args, "x", "y")?;
            let button =
                serde_json::from_value(args.get("button").cloned().unwrap_or(json!("left")))
                    .unwrap_or_default();
            let modifiers = args
                .get("modifiers")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|m| m.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default();
            let count = u(args, "count").unwrap_or(1) as u32;
            ctx.input(Op::Pointer(PointerArgs::Click {
                x,
                y,
                button,
                count,
                modifiers,
            }))
            .await?;
            ok(json!({ "clicked_points": [x, y] }))
        }
        "mac_hover" => {
            let (x, y) = ctx.point(args, "x", "y")?;
            ctx.input(Op::Pointer(PointerArgs::Move { x, y })).await?;
            ok(json!({ "pointer_points": [x, y] }))
        }
        "mac_scroll" => {
            let (x, y) = ctx.point(args, "x", "y")?;
            let dx = args.get("dx").and_then(Value::as_i64).unwrap_or(0) as i32;
            let dy = args.get("dy").and_then(Value::as_i64).unwrap_or(5) as i32;
            ctx.input(Op::Pointer(PointerArgs::Scroll { x, y, dx, dy }))
                .await?;
            ok(json!({ "scrolled": true }))
        }
        "mac_drag" => {
            let (from_x, from_y) = ctx.point(args, "from_x", "from_y")?;
            let (to_x, to_y) = ctx.point(args, "to_x", "to_y")?;
            ctx.input(Op::Pointer(PointerArgs::Drag {
                from_x,
                from_y,
                to_x,
                to_y,
            }))
            .await?;
            ok(json!({ "dragged": true }))
        }
        "mac_ui_action" => {
            let action = match s(args, "action").unwrap_or("press") {
                "focus" => UiAction::Focus,
                "set_value" => UiAction::SetValue {
                    value: s(args, "value").unwrap_or_default().to_owned(),
                },
                "show_menu" => UiAction::ShowMenu,
                "raise" => UiAction::Raise,
                "scroll_into_view" => UiAction::ScrollIntoView,
                "perform" => UiAction::Perform {
                    name: req(args, "value")?.to_owned(),
                },
                _ => UiAction::Press,
            };
            let v = ctx
                .input(Op::UiAction(UiActionArgs {
                    reference: req(args, "ref")?.to_owned(),
                    action,
                }))
                .await?;
            ok(json!({ "done": true, "result": v }))
        }
        "mac_type" => {
            ctx.input(Op::Keyboard(KeyboardArgs {
                text: Some(req(args, "text")?.to_owned()),
                keys: None,
                app: s(args, "app").map(str::to_owned),
            }))
            .await?;
            ok(json!({ "typed_chars": req(args, "text")?.chars().count() }))
        }
        "mac_key" => {
            let keys = req(args, "keys")?.to_owned();
            let repeat = u(args, "repeat").unwrap_or(1);
            let _guard = ctx.machine.input.lock().await;
            for i in 0..repeat {
                let app = if i == 0 {
                    s(args, "app").map(str::to_owned)
                } else {
                    None
                };
                ctx.call(Op::Keyboard(KeyboardArgs {
                    text: None,
                    keys: Some(keys.clone()),
                    app,
                }))
                .await?;
            }
            ok(json!({ "pressed": keys, "times": repeat }))
        }
        "mac_open_app" => {
            let open = args
                .get("open")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default();
            ok(ctx
                .call(Op::Apps(AppsOp::Open {
                    app: req(args, "app")?.to_owned(),
                    open,
                    background: b(args, "background"),
                }))
                .await?)
        }
        "mac_quit_app" => ok(ctx
            .call(Op::Apps(AppsOp::Quit {
                app: req(args, "app")?.to_owned(),
                force: b(args, "force"),
            }))
            .await?),
        "mac_open_url" => ok(ctx
            .call(Op::OpenUrl(dot_wire::OpenUrlArgs {
                url: req(args, "url")?.to_owned(),
                app: s(args, "app").map(str::to_owned),
            }))
            .await?),
        "mac_notify" => ok(ctx
            .call(Op::Notify(dot_wire::NotifyArgs {
                title: req(args, "title")?.to_owned(),
                message: req(args, "message")?.to_owned(),
            }))
            .await?),
        "shortcuts_list" => {
            ok(json!({ "shortcuts": ctx.call(Op::Shortcuts(ShortcutsOp::List)).await? }))
        }
        "shortcuts_run" => ok(ctx
            .call_for(
                Op::Shortcuts(ShortcutsOp::Run {
                    name: req(args, "name")?.to_owned(),
                    input: s(args, "input").map(str::to_owned),
                }),
                u(args, "timeout_s").unwrap_or(45),
            )
            .await?),
        "safari_tabs" => ok(json!({ "tabs": ctx.call(Op::Safari(SafariOp::Tabs)).await? })),
        "safari_open" => {
            let tab = ctx
                .call(Op::Safari(SafariOp::Open {
                    url: req(args, "url")?.to_owned(),
                    new_tab: args.get("new_tab").and_then(Value::as_bool).unwrap_or(true),
                }))
                .await?;
            let loaded = ctx
                .call_for(Op::Safari(SafariOp::WaitLoaded), 20)
                .await
                .unwrap_or(tab);
            ok(loaded)
        }
        "safari_read" => {
            let (window, tab) = wt(args);
            ok(ctx
                .call(Op::Safari(SafariOp::Read {
                    window,
                    tab,
                    max_chars: u(args, "max_chars").map(|v| v as u32),
                }))
                .await?)
        }
        "safari_eval" => {
            let (window, tab) = wt(args);
            ok(ctx
                .call(Op::Safari(SafariOp::Eval {
                    window,
                    tab,
                    script: req(args, "script")?.to_owned(),
                }))
                .await?)
        }
        "safari_dom" => {
            let (window, tab) = wt(args);
            let value = s(args, "value").unwrap_or_default().to_owned();
            let act = match req(args, "action")? {
                "click" => DomAction::Click,
                "fill" => DomAction::Fill { value },
                "select" => DomAction::Select { value },
                "submit" => DomAction::Submit,
                "scroll" => DomAction::Scroll,
                _ => DomAction::Inspect,
            };
            ok(ctx
                .call(Op::Safari(SafariOp::Dom {
                    window,
                    tab,
                    selector: req(args, "selector")?.to_owned(),
                    act,
                }))
                .await?)
        }
        "safari_navigate" => ok(ctx
            .call(Op::Safari(SafariOp::Navigate {
                direction: req(args, "direction")?.to_owned(),
            }))
            .await?),
        "safari_activate_tab" | "safari_close_tab" => {
            let (Some(window), Some(tab)) = wt(args) else {
                return Err(terr("INVALID_ARGS", "window and tab are required."));
            };
            let op = if name == "safari_close_tab" {
                SafariOp::Close { window, tab }
            } else {
                SafariOp::Activate { window, tab }
            };
            ok(ctx.call(Op::Safari(op)).await?)
        }
        "file_read" => ok(ctx
            .call(Op::Files(FilesOp::Read {
                path: req(args, "path")?.to_owned(),
                offset: u(args, "offset").unwrap_or(0),
                length: u(args, "length"),
            }))
            .await?),
        "file_list" => ok(json!({ "entries": ctx
            .call(Op::Files(FilesOp::List { path: req(args, "path")?.to_owned(), depth: u(args, "depth").unwrap_or(0) as u32, hidden: b(args, "hidden"), limit: u(args, "limit").map(|v| v as u32) }))
            .await? })),
        "file_stat" => ok(ctx
            .call(Op::Files(FilesOp::Stat {
                path: req(args, "path")?.to_owned(),
            }))
            .await?),
        "file_search" => ok(json!({ "results": ctx
            .call(Op::Files(FilesOp::Search { query: req(args, "query")?.to_owned(), scope: s(args, "folder").map(str::to_owned), limit: u(args, "limit").map(|v| v as u32) }))
            .await? })),
        "file_write" => ok(ctx
            .call(Op::Files(FilesOp::Write {
                path: req(args, "path")?.to_owned(),
                content: req(args, "content")?.to_owned(),
                base64: b(args, "base64"),
                append: b(args, "append"),
                create_only: b(args, "create_only"),
                parents: args.get("parents").and_then(Value::as_bool).unwrap_or(true),
            }))
            .await?),
        "file_move" => ok(ctx
            .call(Op::Files(FilesOp::Move {
                from: req(args, "from")?.to_owned(),
                to: req(args, "to")?.to_owned(),
                overwrite: b(args, "overwrite"),
            }))
            .await?),
        "file_copy" => ok(ctx
            .call(Op::Files(FilesOp::Copy {
                from: req(args, "from")?.to_owned(),
                to: req(args, "to")?.to_owned(),
                overwrite: b(args, "overwrite"),
            }))
            .await?),
        "file_mkdir" => ok(ctx
            .call(Op::Files(FilesOp::Mkdir {
                path: req(args, "path")?.to_owned(),
            }))
            .await?),
        "file_trash" => ok(ctx
            .call(Op::Files(FilesOp::Trash {
                path: req(args, "path")?.to_owned(),
            }))
            .await?),
        "mac_shell" => {
            let env = args
                .get("env")
                .and_then(Value::as_object)
                .map(|m| {
                    m.iter()
                        .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_owned())))
                        .collect()
                })
                .unwrap_or_default();
            ok(ctx
                .call_for(
                    Op::Exec(dot_wire::Exec {
                        command: req(args, "command")?.to_owned(),
                        cwd: s(args, "cwd").map(str::to_owned),
                        env,
                        stdin: s(args, "stdin").map(str::to_owned),
                        max_output_bytes: 256 * 1024,
                    }),
                    u(args, "timeout_s").unwrap_or(30),
                )
                .await?)
        }
        "mac_script" => {
            let language = if s(args, "language") == Some("javascript") {
                ScriptLanguage::Javascript
            } else {
                ScriptLanguage::Applescript
            };
            let script_args = args
                .get("args")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .map(|v| v.as_str().map_or_else(|| v.to_string(), str::to_owned))
                        .collect()
                })
                .unwrap_or_default();
            let v = ctx
                .call_for(
                    Op::Script(Script {
                        language,
                        source: req(args, "source")?.to_owned(),
                        args: script_args,
                    }),
                    u(args, "timeout_s").unwrap_or(30),
                )
                .await?;
            ok(script_output(v))
        }
        "clipboard_read" => ok(ctx.call(Op::Clipboard(ClipboardOp::Get)).await?),
        "clipboard_write" => ok(ctx
            .call(Op::Clipboard(ClipboardOp::Set {
                text: req(args, "text")?.to_owned(),
            }))
            .await?),
        "mac_checkpoint" => {
            let snap = crate::snapshots::create(
                ctx.app,
                &ctx.caller.instance_id,
                req(args, "name")?,
                "agent",
            )
            .await
            .map_err(|e| {
                use crate::snapshots::CreateError;
                match e {
                    CreateError::Invalid(m) => terr("INVALID_ARGS", m),
                    CreateError::Limit(m) => terr("STORAGE_LIMIT", m),
                    CreateError::Failed(m) => terr("FAILED", m),
                }
            })?;
            ok(serde_json::to_value(snap).unwrap_or_default())
        }
        "mac_checkpoints" => {
            let list = crate::snapshots::list(ctx.app, &ctx.caller.instance_id)
                .await
                .map_err(|e| terr("FAILED", format!("{e:#}")))?;
            ok(json!({ "checkpoints": list }))
        }
        "mac_restore_checkpoint" => {
            crate::snapshots::restore(
                ctx.app,
                &ctx.caller.instance_id,
                req(args, "checkpoint_id")?,
            )
            .await
            .map_err(|e| terr("FAILED", e))?;
            ok(
                json!({ "restored": true, "note": "The Mac is reconnecting. Take a screenshot to see where it is." }),
            )
        }
        "app_install" => install(ctx, args).await,
        "app_uninstall" => ok(ctx
            .call(Op::Apps(AppsOp::Uninstall {
                app: req(args, "app")?.to_owned(),
            }))
            .await?),
        "mac_restart" => {
            let machine = ctx.machine.clone();
            tokio::spawn(async move {
                let _ = machine.restart().await;
            });
            ok(
                json!({ "restarting": true, "note": "Calls will wait for the Mac to come back (about a minute)." }),
            )
        }
        other => Err(terr("UNKNOWN_TOOL", format!("{other} isn't implemented."))),
    }
}

fn find_args(args: &Map<String, Value>) -> UiFindArgs {
    UiFindArgs {
        app: s(args, "app").map(str::to_owned),
        text: s(args, "text").map(str::to_owned),
        role: s(args, "role").map(str::to_owned),
        limit: u(args, "limit").map(|v| v as u32),
    }
}

fn script_output(v: Value) -> Value {
    let text = v.get("output").and_then(Value::as_str).unwrap_or_default();
    match serde_json::from_str::<Value>(text) {
        Ok(parsed) if parsed.is_object() || parsed.is_array() => json!({ "result": parsed }),
        _ => json!({ "output": text }),
    }
}

async fn status(ctx: &Ctx<'_>) -> Out {
    let info = ctx.call_for(Op::Info, 10).await?;
    let st = ctx.machine.status();
    let iid = ctx.caller.instance_id.clone();
    let enabled: Vec<String> = super::enabled_packs(ctx.app, &iid)
        .await
        .map(|p| p.iter().map(|p| p.name.clone()).collect())
        .unwrap_or_default();
    ok(json!({
        "state": st.state, "detail": st.detail, "mac": info,
        "app_tools_enabled": enabled,
        "agent_version": ctx.link.agent_version,
        "tips": [
            "Prefer mac_ui_find / mac_ui_action over clicking pixels; fall back to mac_screenshot + mac_click.",
            "Safari is the default browser; safari_read and safari_dom are faster than screenshots for web pages.",
            "Save a mac_checkpoint before risky changes."
        ]
    }))
}

async fn wait(ctx: &Ctx<'_>, args: &Map<String, Value>) -> Out {
    let seconds = n(args, "seconds").unwrap_or(10.0).clamp(0.1, 50.0);
    let budget =
        Duration::from_secs_f64(seconds).min(ctx.deadline.saturating_sub(Duration::from_secs(2)));
    if s(args, "text").is_none() && s(args, "role").is_none() {
        tokio::time::sleep(budget).await;
        return ok(json!({ "waited_s": budget.as_secs_f64() }));
    }
    let gone = b(args, "gone");
    let started = std::time::Instant::now();
    let mut find = find_args(args);
    find.limit = Some(5);
    loop {
        let remaining = budget.saturating_sub(started.elapsed());
        let tree: Option<UiTree> = ctx
            .link
            .call(
                Op::UiFind(find.clone()),
                remaining.max(Duration::from_secs(2)),
            )
            .await
            .ok()
            .and_then(|v| serde_json::from_value(v).ok());
        let found = tree.as_ref().is_some_and(|t| !t.nodes.is_empty());
        if found != gone {
            return ok(
                json!({ "matched": !gone, "after_s": started.elapsed().as_secs_f64(), "elements": tree.map(|t| t.nodes) }),
            );
        }
        if started.elapsed() + Duration::from_millis(700) >= budget {
            return Err(ToolError {
                code: "TIMEOUT".into(),
                message: format!(
                    "Still {} after {:.0} s.",
                    if gone { "there" } else { "not there" },
                    budget.as_secs_f64()
                ),
                extra: json!({ "hint": "Take a mac_screenshot to see what's on screen." }),
            });
        }
        tokio::time::sleep(Duration::from_millis(700)).await;
    }
}

async fn install(ctx: &Ctx<'_>, args: &Map<String, Value>) -> Out {
    let source = if let Some(url) = s(args, "url") {
        if !url.starts_with("https://") {
            return Err(terr(
                "INVALID_ARGS",
                "Only https download links are allowed.",
            ));
        }
        InstallSource::Url {
            url: url.to_owned(),
            sha256: s(args, "sha256").map(str::to_owned),
        }
    } else if let Some(id) = u(args, "app_store_id") {
        InstallSource::AppStore { id }
    } else if let Some(cask) = s(args, "brew_cask") {
        InstallSource::Brew {
            cask: cask.to_owned(),
        }
    } else {
        return Err(terr("INVALID_ARGS", "Give url, app_store_id or brew_cask."));
    };
    // Installs can take minutes: let it finish in the background and report if it's quick.
    let link = ctx.link.clone();
    let task = tokio::spawn(async move {
        link.call(
            Op::Apps(AppsOp::Install { source }),
            Duration::from_secs(900),
        )
        .await
    });
    match tokio::time::timeout(ctx.deadline.saturating_sub(Duration::from_secs(3)), task).await {
        Ok(Ok(result)) => ok(json!({ "installed": result? })),
        Ok(Err(_)) => Err(terr("FAILED", "The install task stopped unexpectedly.")),
        Err(_) => ok(
            json!({ "installing": true, "note": "Still installing. Check mac_apps in a minute or two." }),
        ),
    }
}

async fn run_pack(ctx: &Ctx<'_>, tool: &crate::packs::PackTool, args: &Map<String, Value>) -> Out {
    let language = match tool.language {
        Language::Javascript => ScriptLanguage::Javascript,
        Language::Applescript => ScriptLanguage::Applescript,
    };
    let input = Value::Object(args.clone()).to_string();
    let v = ctx
        .call_for(
            Op::Script(Script {
                language,
                source: tool.script.clone(),
                args: vec![input],
            }),
            tool.timeout_s.unwrap_or(30),
        )
        .await?;
    ok(script_output(v))
}

async fn run_app_mcp(
    ctx: &Ctx<'_>,
    tool: &super::appmcp::AppTool,
    args: &Map<String, Value>,
) -> Out {
    if !ctx.link.info.capabilities.iter().any(|c| c == "mcp") {
        return Err(terr(
            "UNSUPPORTED",
            "The agent in this Mac is too old for app MCP servers. Ask the person to update it in Familiar.",
        ));
    }
    let op = Op::Mcp(McpOp::CallTool {
        server: tool.server.clone(),
        tool: tool.original.clone(),
        arguments: args.clone(),
        // Agents that predate fingerprints report none, and get none to check.
        expected_fingerprint: Some(tool.fingerprint.clone()).filter(|f| !f.is_empty()),
    });
    let v = ctx.call(op).await?;
    // The server's own result passes through; anything unreadable is shown as text.
    Ok(serde_json::from_value::<CallToolResult>(v.clone())
        .unwrap_or_else(|_| CallToolResult::success(vec![Content::text(v.to_string())])))
}

/// A short, non-content note for the activity log.
pub fn detail_for(tool: &str, args: &Map<String, Value>) -> Option<String> {
    let pick = |k: &str| s(args, k).map(|v| crate::util::truncate(v, 80));
    let d = match tool {
        "mac_open_app" | "mac_quit_app" | "app_dictionary" | "app_uninstall" => pick("app"),
        "mac_open_url" | "safari_open" => s(args, "url")
            .and_then(|u| url::Url::parse(u).ok())
            .and_then(|u| u.host_str().map(str::to_owned)),
        "mac_key" => pick("keys"),
        "shortcuts_run" => pick("name"),
        "mac_screenshot" => Some(pick("app").unwrap_or_else(|| "Screen".into())),
        "mac_ui_tree" | "mac_ui_find" | "mac_windows" => pick("app"),
        t if t.starts_with("file_") => pick("path")
            .or_else(|| pick("from"))
            .or_else(|| pick("query")),
        "mac_shell" => s(args, "command")
            .map(|c| crate::util::truncate(c.split_whitespace().next().unwrap_or(""), 40)),
        "mac_checkpoint" => pick("name"),
        "safari_dom" => pick("action"),
        _ => None,
    };
    d.filter(|v| !v.is_empty())
}

/// A sentence describing what an approval would allow.
pub fn summary_for(def: &Def, args: &Map<String, Value>) -> String {
    let list = |k: &str| {
        args.get(k)
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .filter(|s| !s.is_empty())
    };
    let title = def.title();
    let mut parts = Vec::new();
    for key in ["to", "recipient", "chat_id"] {
        if let Some(v) = list(key).or_else(|| s(args, key).map(str::to_owned)) {
            parts.push(format!("to {}", crate::util::truncate(&v, 120)));
            break;
        }
    }
    for key in [
        "subject",
        "name",
        "app",
        "url",
        "command",
        "path",
        "brew_cask",
        "checkpoint_id",
    ] {
        if let Some(v) = s(args, key) {
            parts.push(format!("\u{201c}{}\u{201d}", crate::util::truncate(v, 120)));
            break;
        }
    }
    if let Some(text) = s(args, "text").or_else(|| s(args, "body")) {
        parts.push(format!(
            "\u{201c}{}\u{201d}",
            crate::util::truncate(text, 160)
        ));
    }
    if parts.is_empty() {
        title
    } else {
        format!("{title} {}", parts.join(" "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::catalog;

    #[test]
    fn details_and_summaries_say_what_happened_without_content() {
        let args = |v: Value| v.as_object().cloned().unwrap();
        assert_eq!(
            detail_for(
                "mac_open_url",
                &args(json!({"url": "https://example.com/private?q=1"}))
            )
            .as_deref(),
            Some("example.com")
        );
        assert_eq!(
            detail_for("mac_shell", &args(json!({"command": "brew install jq"}))).as_deref(),
            Some("brew")
        );
        assert_eq!(detail_for("mac_click", &args(json!({"x": 1}))), None);
        let def = Def::Builtin(catalog::builtin("app_install").unwrap());
        assert_eq!(
            summary_for(&def, &args(json!({"brew_cask": "firefox"}))),
            "Install an app \u{201c}firefox\u{201d}"
        );
        assert_eq!(
            script_output(json!({"output": "{\"a\":1}"})),
            json!({"result": {"a": 1}})
        );
        assert_eq!(
            script_output(json!({"output": "hello"})),
            json!({"output": "hello"})
        );
    }
}
