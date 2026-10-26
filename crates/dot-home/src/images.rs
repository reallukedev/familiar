//! Building the golden macOS image from the portal.
//!
//! 1. Download the macOS recovery image from Apple with OSX-KVM's `fetch-macOS-v2.py`.
//! 2. Boot a blank "builder" Mac with the installer attached; the admin installs macOS
//!    on its live screen, then installs `dot-guest` and runs `dot-guest prepare-image`.
//! 3. When the agent connects, "Use as base image" stops the builder and moves its disk to
//!    `images/base-<time>.qcow2`, pointing `base.qcow2` at it. Existing Macs keep the base
//!    they were created from; only new Macs use the new one.
use crate::{
    auth::Session,
    crypto,
    db::now,
    error::{ApiError, ApiResult},
    state::App,
};
use axum::{Json, extract::State};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::PathBuf;

/// macOS versions OSX-KVM's fetch script knows by short name.
pub const VERSIONS: &[(&str, &str)] = &[
    // macOS 27 and later run only on Apple silicon, so they can't run under KVM on x86.
    // Tahoe is the last release for Intel Macs; real Macs can be added as remote Macs.
    ("tahoe", "macOS Tahoe 26 (newest that runs on this server)"),
    ("sequoia", "macOS Sequoia 15"),
    ("sonoma", "macOS Sonoma 14"),
];

/// Builder progress, kept in settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Builder {
    /// The builder Mac.
    pub instance_id: String,
    /// Short name.
    pub macos: String,
    /// `downloading`, `installing`, `ready_to_promote`, `promoting`, `failed`.
    pub phase: String,
    /// 0 to 1 when known.
    pub progress: Option<f64>,
    /// Sentence.
    pub detail: String,
}

fn images_dir(app: &App) -> PathBuf {
    app.config.data_dir.join("images")
}

async fn load(app: &App) -> Option<Builder> {
    app.db
        .setting("image_builder")
        .await
        .ok()
        .flatten()
        .and_then(|s| serde_json::from_str(&s).ok())
}

async fn save(app: &App, b: Option<&Builder>) {
    let _ = match b {
        Some(b) => {
            app.db
                .set_setting(
                    "image_builder",
                    serde_json::to_string(b).unwrap_or_default(),
                )
                .await
        }
        None => {
            app.db
                .call(|c| {
                    c.execute("DELETE FROM settings WHERE key = 'image_builder'", [])
                        .map(|_| ())
                })
                .await
        }
    };
    let status = status(app).await;
    app.events.admins("image", status);
}

async fn status(app: &App) -> Value {
    let dir = images_dir(app);
    let base = dir.join("base.qcow2");
    let meta = tokio::fs::metadata(&base).await.ok();
    let macos = app.db.setting("base_macos").await.ok().flatten();
    let created = app
        .db
        .setting("base_created_at")
        .await
        .ok()
        .flatten()
        .and_then(|v| v.parse::<i64>().ok());
    let firmware = crate::vm::qemu::Images::locate(&app.config.vm, &app.config.data_dir);
    let mut builder = load(app).await;
    // Reflect the builder Mac's live state.
    if let Some(b) = builder.as_mut()
        && b.phase == "installing"
        && app.fleet.machine(&b.instance_id).status().state == crate::vm::State::Ready
    {
        b.phase = "ready_to_promote".into();
        b.detail = "The agent in the new Mac is connected. Use it as the base image when you're done setting it up.".into();
    }
    json!({
        "base": {
            "present": meta.is_some(),
            "size_bytes": meta.map(|m| m.len()),
            "created_at": crate::db::rfc3339_opt(created),
            "macos": macos
        },
        "firmware": {
            "present": firmware.is_ok(),
            "detail": match &firmware { Ok(_) => "OpenCore and OVMF found.".to_owned(), Err(e) => e.to_string() }
        },
        "versions": VERSIONS.iter().map(|(id, label)| json!({ "id": id, "label": label })).collect::<Vec<_>>(),
        "builder": builder.map(|b| {
            let state_detail = app.fleet.machine(&b.instance_id).status().detail;
            json!({ "instance_id": b.instance_id, "macos": b.macos, "phase": b.phase, "progress": b.progress,
                    "detail": if b.phase == "installing" { state_detail.unwrap_or(b.detail) } else { b.detail } })
        })
    })
}

/// `GET /api/admin/images`.
pub async fn images(State(app): State<App>, s: Session) -> ApiResult<Json<Value>> {
    s.require_admin()?;
    Ok(Json(status(&app).await))
}

#[derive(Deserialize)]
pub struct StartBody {
    macos: String,
}

/// `POST /api/admin/images/builder`.
pub async fn builder_start(
    State(app): State<App>,
    s: Session,
    Json(b): Json<StartBody>,
) -> ApiResult<Json<Value>> {
    s.require_admin()?;
    s.require_confirmed()?;
    if !VERSIONS.iter().any(|(id, _)| *id == b.macos) {
        return Err(ApiError::invalid(
            "macos",
            "Choose one of the listed macOS versions.",
        ));
    }
    if load(&app).await.is_some() {
        return Err(ApiError::conflict(
            "An image is already being built. Finish or cancel it first.",
        ));
    }
    crate::vm::qemu::Images::locate(&app.config.vm, &app.config.data_dir)
        .map_err(|e| ApiError::unavailable(format!("OSX-KVM isn't set up: {e}")))?;
    let memory_mb = builder_memory_mb(app.fleet.effective_memory_budget_mb().await).ok_or_else(|| {
        ApiError::budget(
            "Building an image needs at least 4 GB of memory for running Macs. Raise it in Server > Budget for Macs.",
        )
    })?;
    let id = crypto::random_id::<10>();
    let secret = dot_wire::random_bytes::<32>();
    let sealed = app.key.seal(&format!("guest:{id}"), &secret);
    let (iid, uid) = (id.clone(), s.user.id.clone());
    let port = app.config.vm.port_base.saturating_add(999);
    app.db
        .call(move |c| {
            c.execute("DELETE FROM instances WHERE port = ?1", [port])?;
            c.execute(
                "INSERT INTO instances (id, user_id, name, driver, cpus, memory_mb, disk_gb, port, secret, is_builder, sleep_after_minutes, hibernate_after_minutes, created_at)
                 VALUES (?1, ?2, 'macOS image builder', 'qemu', 4, ?6, 160, ?3, ?4, 1, 0, 0, ?5)",
                rusqlite::params![iid, uid, port, sealed, now(), memory_mb],
            )
        })
        .await?;
    let builder = Builder {
        instance_id: id.clone(),
        macos: b.macos.clone(),
        phase: "downloading".into(),
        progress: None,
        detail: "Downloading the macOS installer from Apple.".into(),
    };
    save(&app, Some(&builder)).await;
    let app2 = app.clone();
    tokio::spawn(async move { run(app2, builder).await });
    Ok(Json(status(&app).await))
}

/// Memory for the builder Mac: 8 GB when the budget allows, otherwise whatever whole
/// gigabytes fit, down to the 4 GB the installer needs. `None` when even that doesn't fit.
fn builder_memory_mb(budget_mb: u64) -> Option<u32> {
    let fit = (budget_mb / 1024 * 1024).min(8192);
    (fit >= 4096).then(|| u32::try_from(fit).unwrap_or(8192))
}

async fn run(app: App, mut b: Builder) {
    let dir = images_dir(&app);
    let work = dir.join("build");
    let result: anyhow::Result<()> = async {
        tokio::fs::create_dir_all(&work).await?;
        let script = app.config.vm.osx_kvm_dir.join("fetch-macOS-v2.py");
        let out = tokio::process::Command::new("python3")
            .arg(&script)
            .args(["--shortname", &b.macos])
            .current_dir(&work)
            .kill_on_drop(true)
            .output()
            .await?;
        if !out.status.success() {
            anyhow::bail!(
                "the download failed: {}",
                String::from_utf8_lossy(&out.stderr)
                    .lines()
                    .last()
                    .unwrap_or("no output")
            );
        }
        let dmg = work.join("BaseSystem.dmg");
        anyhow::ensure!(dmg.exists(), "the download finished without BaseSystem.dmg");
        let img = dir.join("BaseSystem.img");
        let q = tokio::process::Command::new(&app.config.vm.qemu_img)
            .args(["convert", "-O", "raw"])
            .arg(&dmg)
            .arg(&img)
            .output()
            .await?;
        anyhow::ensure!(
            q.status.success(),
            "couldn't convert the installer: {}",
            String::from_utf8_lossy(&q.stderr).trim()
        );
        let _ = tokio::fs::remove_dir_all(&work).await;
        Ok(())
    }
    .await;
    if let Err(e) = result {
        b.phase = "failed".into();
        b.detail = format!("Building the image failed: {e:#}. Cancel and try again.");
        save(&app, Some(&b)).await;
        return;
    }
    b.phase = "installing".into();
    b.progress = None;
    b.detail = "Open the builder's screen and install macOS on the blank disk. Then install dot-guest and run `dot-guest prepare-image` in Terminal.".into();
    save(&app, Some(&b)).await;
    if let Err(e) = app.fleet.machine(&b.instance_id).start().await {
        b.phase = "failed".into();
        b.detail = e;
        save(&app, Some(&b)).await;
    }
}

/// `POST /api/admin/images/promote`.
pub async fn builder_promote(State(app): State<App>, s: Session) -> ApiResult<Json<Value>> {
    s.require_admin()?;
    s.require_confirmed()?;
    let mut b = load(&app)
        .await
        .ok_or_else(|| ApiError::conflict("There's no image being built."))?;
    let machine = app.fleet.machine(&b.instance_id);
    if machine.status().state != crate::vm::State::Ready {
        return Err(ApiError::conflict(
            "The builder's agent isn't connected yet. Finish installing dot-guest in it first.",
        ));
    }
    b.phase = "promoting".into();
    b.detail = "Shutting down the builder and saving its disk.".into();
    save(&app, Some(&b)).await;
    machine.stop(false).await.map_err(ApiError::unavailable)?;
    let dir = images_dir(&app);
    let t = now();
    let target = dir.join(format!("base-{t}.qcow2"));
    let disk = app.fleet.files(&b.instance_id).disk();
    tokio::fs::rename(&disk, &target)
        .await
        .map_err(ApiError::internal)?;
    let link = dir.join("base.qcow2");
    let tmp = dir.join("base.qcow2.new");
    let _ = tokio::fs::remove_file(&tmp).await;
    tokio::fs::symlink(target.file_name().unwrap_or_default(), &tmp)
        .await
        .map_err(ApiError::internal)?;
    tokio::fs::rename(&tmp, &link)
        .await
        .map_err(ApiError::internal)?;
    app.db.set_setting("base_macos", b.macos.clone()).await?;
    app.db.set_setting("base_created_at", t.to_string()).await?;
    cleanup(&app, &b.instance_id).await?;
    save(&app, None).await;
    tracing::info!(image = %target.display(), "new base image");
    Ok(Json(status(&app).await))
}

/// `DELETE /api/admin/images/builder`.
pub async fn builder_delete(State(app): State<App>, s: Session) -> ApiResult<Json<Value>> {
    s.require_admin()?;
    if let Some(b) = load(&app).await {
        let _ = app.fleet.machine(&b.instance_id).stop(true).await;
        cleanup(&app, &b.instance_id).await?;
    }
    let _ = tokio::fs::remove_dir_all(images_dir(&app).join("build")).await;
    save(&app, None).await;
    Ok(Json(status(&app).await))
}

async fn cleanup(app: &App, instance_id: &str) -> ApiResult<()> {
    app.fleet.forget(instance_id).await;
    let _ = tokio::fs::remove_dir_all(app.fleet.files(instance_id).dir).await;
    let _ = tokio::fs::remove_file(images_dir(app).join("BaseSystem.img")).await;
    let id = instance_id.to_owned();
    app.db
        .call(move |c| c.execute("DELETE FROM instances WHERE id = ?1", [id]))
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::builder_memory_mb;

    #[test]
    fn builder_fits_inside_small_budgets() {
        assert_eq!(builder_memory_mb(64 * 1024), Some(8192));
        // A 7 GB budget used to refuse the 8 GB builder outright.
        assert_eq!(builder_memory_mb(7 * 1024), Some(7168));
        assert_eq!(builder_memory_mb(7 * 1024 + 900), Some(7168));
        assert_eq!(builder_memory_mb(4096), Some(4096));
        assert_eq!(builder_memory_mb(4095), None);
    }
}
