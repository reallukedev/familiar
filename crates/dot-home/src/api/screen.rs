//! Live screen: a WebSocket carrying raw RFB (VNC) between noVNC and QEMU's unix socket.
//! QEMU's VNC server is only reachable through this authenticated proxy.
use super::owned_instance;
use crate::{auth::Session, error::ApiError, state::App};
use axum::{
    extract::{
        Path, State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    http::{HeaderMap, header},
    response::{IntoResponse, Response},
};
use std::time::Duration;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::UnixStream,
};

/// `GET /api/instances/:id/screen` (WebSocket).
pub async fn screen(
    State(app): State<App>,
    s: Session,
    headers: HeaderMap,
    Path(id): Path<String>,
    ws: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    // Cross-site WebSocket hijacking: browsers send Origin on upgrades; it must be ours.
    let origin = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok());
    if origin != Some(app.config.origin().as_str()) {
        return Err(ApiError::csrf());
    }
    let id = owned_instance(&app, &s, &id).await?;
    let iid = id.clone();
    let driver: String = app
        .db
        .call(move |c| {
            c.query_row("SELECT driver FROM instances WHERE id = ?1", [iid], |r| {
                r.get(0)
            })
        })
        .await?;
    if driver != "qemu" {
        return Err(ApiError::bad(
            "The live screen is available for Macs on this server. Use Screen Sharing for remote Macs.",
        ));
    }
    let machine = app.fleet.machine(&id);
    if !machine.status().state.uses_memory() {
        return Err(ApiError::unavailable("Start the Mac to see its screen."));
    }
    machine.wake().await;
    let socket = app.fleet.files(&id).vnc();
    let session_key = s.id_hash.clone();
    Ok(ws
        .protocols(["binary"])
        .max_message_size(4 * 1024 * 1024)
        .on_upgrade(move |ws| async move {
            let _viewer = machine.viewer();
            if let Err(e) = proxy(ws, &socket, &app, &session_key).await {
                tracing::debug!(error = %e, "screen proxy ended");
            }
        })
        .into_response())
}

async fn proxy(
    mut ws: WebSocket,
    socket: &std::path::Path,
    app: &App,
    session_key: &[u8],
) -> anyhow::Result<()> {
    let vnc = tokio::time::timeout(Duration::from_secs(5), UnixStream::connect(socket)).await??;
    let (mut vr, mut vw) = vnc.into_split();
    let mut buf = vec![0u8; 64 * 1024];
    // Signing out elsewhere, a password change or a disabled account ends the screen within 30 s.
    let mut recheck = tokio::time::interval(Duration::from_secs(30));
    recheck.tick().await;
    loop {
        tokio::select! {
            _ = recheck.tick() => {
                if !crate::auth::still_valid(app, session_key).await {
                    let _ = ws.send(Message::Close(None)).await;
                    break;
                }
            }
            n = vr.read(&mut buf) => {
                let n = n?;
                if n == 0 { break; }
                ws.send(Message::Binary(buf[..n].to_vec().into())).await?;
            }
            msg = tokio::time::timeout(Duration::from_secs(600), ws.recv()) => {
                match msg {
                    Ok(Some(Ok(Message::Binary(data)))) => vw.write_all(&data).await?,
                    Ok(Some(Ok(Message::Close(_)) | Err(_)) | None) | Err(_) => break,
                    Ok(Some(Ok(_))) => {}
                }
            }
        }
    }
    Ok(())
}
