//! The owner's clipboard bridge: the screen copies between their browser and the Mac.
//! QEMU's VNC can't reach the macOS pasteboard, so it goes through the agent instead.
use super::owned_instance;
use crate::{
    auth::Session,
    error::{ApiError, ApiResult},
    guest::Link,
    state::App,
};
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use dot_wire::{ClipboardOp, Op};
use serde::Deserialize;
use serde_json::Value;
use std::{sync::Arc, time::Duration};

/// Plenty for text; anything bigger is better sent as a file.
const MAX_TEXT: usize = 1 << 20;

/// The Mac's agent, without waking the Mac: copying only makes sense while it's on screen.
async fn link(app: &App, s: &Session, id: &str) -> ApiResult<Arc<Link>> {
    let id = owned_instance(app, s, id).await?;
    app.fleet
        .machine(&id)
        .link()
        .ok_or_else(|| ApiError::unavailable("The Mac's agent isn't connected yet."))
}

/// `GET /api/instances/:id/clipboard`: `{ "text": String | null }`.
pub async fn get(
    State(app): State<App>,
    s: Session,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let link = link(&app, &s, &id).await?;
    link.call_as::<Value>(Op::Clipboard(ClipboardOp::Get), Duration::from_secs(10))
        .await
        .map(Json)
        .map_err(|e| ApiError::unavailable(format!("Couldn't read the Mac's clipboard: {e}")))
}

#[derive(Deserialize)]
pub struct SetBody {
    text: String,
}

/// `PUT /api/instances/:id/clipboard`.
pub async fn put(
    State(app): State<App>,
    s: Session,
    Path(id): Path<String>,
    Json(b): Json<SetBody>,
) -> ApiResult<StatusCode> {
    if b.text.len() > MAX_TEXT {
        return Err(ApiError::too_large(
            "That's too much to paste. Send it as a file instead.",
        ));
    }
    let link = link(&app, &s, &id).await?;
    link.call_as::<Value>(
        Op::Clipboard(ClipboardOp::Set { text: b.text }),
        Duration::from_secs(10),
    )
    .await
    .map_err(|e| ApiError::unavailable(format!("Couldn't set the Mac's clipboard: {e}")))?;
    Ok(StatusCode::NO_CONTENT)
}
