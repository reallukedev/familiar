//! Approval modes and the Review list (docs/home/API.md "Approvals" and "Review").
use super::{instances::groups, owned_instance};
use crate::{
    auth::Session,
    error::{ApiError, ApiResult},
    state::App,
    tools::{modes, review},
};
use axum::{
    Json,
    extract::{Path, Query, State},
};
use serde::Deserialize;
use serde_json::{Value, json};

/// `GET /api/instances/:id/permissions/mode`.
pub async fn mode(
    State(app): State<App>,
    s: Session,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let id = owned_instance(&app, &s, &id).await?;
    Ok(Json(json!({ "mode": modes::current(&app, &id).await? })))
}

#[derive(Deserialize)]
pub struct ModeBody {
    mode: String,
}

/// `PUT /api/instances/:id/permissions/mode`: sets every group, returns them all.
pub async fn set_mode(
    State(app): State<App>,
    s: Session,
    Path(id): Path<String>,
    Json(b): Json<ModeBody>,
) -> ApiResult<Json<Value>> {
    let id = owned_instance(&app, &s, &id).await?;
    if !modes::MODES.contains(&b.mode.as_str()) {
        return Err(ApiError::invalid(
            "mode",
            "Choose Ask for Everything, Ask Before Changes or Allow Everything.",
        ));
    }
    modes::set(&app, &id, &b.mode).await?;
    Ok(Json(json!({
        "mode": modes::current(&app, &id).await?,
        "groups": groups(&app, &id).await?,
    })))
}

#[derive(Deserialize)]
pub struct ListQuery {
    status: Option<String>,
}

/// `GET /api/instances/:id/review?status=pending|done`.
pub async fn list(
    State(app): State<App>,
    s: Session,
    Path(id): Path<String>,
    Query(q): Query<ListQuery>,
) -> ApiResult<Json<Vec<review::ReviewItem>>> {
    let id = owned_instance(&app, &s, &id).await?;
    Ok(Json(
        review::list(&app, &id, q.status.as_deref().unwrap_or("pending")).await?,
    ))
}

#[derive(Deserialize)]
pub struct DecideBody {
    decision: String,
    note: Option<String>,
}

/// `POST /api/instances/:id/review/:item`.
pub async fn decide(
    State(app): State<App>,
    s: Session,
    Path((id, item)): Path<(String, String)>,
    Json(b): Json<DecideBody>,
) -> ApiResult<Json<review::ReviewItem>> {
    let id = owned_instance(&app, &s, &id).await?;
    let accept = match b.decision.as_str() {
        "accept" => true,
        "decline" => false,
        _ => return Err(ApiError::invalid("decision", "Use accept or decline.")),
    };
    Ok(Json(
        review::decide(&app, &id, &item, accept, b.note).await?,
    ))
}

#[derive(Deserialize, Default)]
pub struct AcceptAllBody {
    session: Option<String>,
    ids: Option<Vec<String>>,
}

/// `POST /api/instances/:id/review/accept-all`.
pub async fn accept_all(
    State(app): State<App>,
    s: Session,
    Path(id): Path<String>,
    body: Option<Json<AcceptAllBody>>,
) -> ApiResult<Json<Value>> {
    let id = owned_instance(&app, &s, &id).await?;
    let b = body.map(|Json(b)| b).unwrap_or_default();
    let n = review::accept_all(&app, &id, b.session, b.ids).await?;
    Ok(Json(json!({ "accepted": n })))
}
