//! Portal preferences, saved per account so they follow the person to every device.
use crate::{
    auth::Session,
    error::{ApiError, ApiResult},
    state::App,
};
use axum::{Json, extract::State};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// `Preferences` in docs/home/API.md. Missing keys in storage fall back to the defaults.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Preferences {
    appearance: String,
    start_page: String,
    screen_mode: String,
    screen_quality: String,
    screen_scale: String,
    clipboard_sync: bool,
    map_ctrl_to_cmd: bool,
    approval_notifications: bool,
    approval_sound: bool,
    /// Seconds a destructive action waits, with Undo, before it runs. 0 runs at once.
    undo_seconds: u32,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            appearance: "system".into(),
            start_page: "home".into(),
            screen_mode: "view".into(),
            screen_quality: "auto".into(),
            screen_scale: "fit".into(),
            clipboard_sync: true,
            map_ctrl_to_cmd: true,
            approval_notifications: false,
            approval_sound: false,
            undo_seconds: 3,
        }
    }
}

/// A rejected key and the sentence explaining it.
type Rejection = (&'static str, &'static str);

impl Preferences {
    /// Apply the known keys of `patch`, all or nothing. Unknown keys are ignored.
    pub fn merge(&mut self, patch: &Map<String, Value>) -> Result<(), Rejection> {
        let mut next = self.clone();
        for (key, value) in patch {
            let (field, slot): (&'static str, Slot<'_>) = match key.as_str() {
                "appearance" => (
                    "appearance",
                    Slot::Choice(
                        &mut next.appearance,
                        &["system", "light", "dark"],
                        "Choose System, Light or Dark.",
                    ),
                ),
                "start_page" => (
                    "start_page",
                    Slot::Choice(
                        &mut next.start_page,
                        &["home", "last_mac"],
                        "Choose Home or the last Mac you used.",
                    ),
                ),
                "screen_mode" => (
                    "screen_mode",
                    Slot::Choice(
                        &mut next.screen_mode,
                        &["view", "control"],
                        "Choose View or Control.",
                    ),
                ),
                "screen_quality" => (
                    "screen_quality",
                    Slot::Choice(
                        &mut next.screen_quality,
                        &["auto", "sharp", "balanced", "fast"],
                        "Choose Automatic, Sharp, Balanced or Fast.",
                    ),
                ),
                "screen_scale" => (
                    "screen_scale",
                    Slot::Choice(
                        &mut next.screen_scale,
                        &["fit", "actual"],
                        "Choose Fit or Actual Size.",
                    ),
                ),
                "clipboard_sync" => ("clipboard_sync", Slot::Switch(&mut next.clipboard_sync)),
                "map_ctrl_to_cmd" => ("map_ctrl_to_cmd", Slot::Switch(&mut next.map_ctrl_to_cmd)),
                "approval_notifications" => (
                    "approval_notifications",
                    Slot::Switch(&mut next.approval_notifications),
                ),
                "approval_sound" => ("approval_sound", Slot::Switch(&mut next.approval_sound)),
                "undo_seconds" => (
                    "undo_seconds",
                    Slot::Number(
                        &mut next.undo_seconds,
                        &[0, 3, 5, 10],
                        "Choose Off, 3, 5 or 10 seconds.",
                    ),
                ),
                _ => continue,
            };
            match slot {
                Slot::Choice(target, allowed, message) => match value.as_str() {
                    Some(v) if allowed.contains(&v) => v.clone_into(target),
                    _ => return Err((field, message)),
                },
                Slot::Number(target, allowed, message) => match value.as_u64() {
                    Some(v) if allowed.iter().any(|a| u64::from(*a) == v) => {
                        *target = u32::try_from(v).map_err(|_| (field, message))?;
                    }
                    _ => return Err((field, message)),
                },
                Slot::Switch(target) => {
                    *target = value
                        .as_bool()
                        .ok_or((field, "Turn this on or off (true or false)."))?;
                }
            }
        }
        *self = next;
        Ok(())
    }
}

enum Slot<'a> {
    Choice(&'a mut String, &'static [&'static str], &'static str),
    Switch(&'a mut bool),
    Number(&'a mut u32, &'static [u32], &'static str),
}

/// Whether stored preferences (the `users.preferences` column) turn approval
/// notifications on. Missing or unreadable preferences mean the default: off.
pub fn approval_notifications(stored: Option<&str>) -> bool {
    stored
        .and_then(|s| serde_json::from_str::<Preferences>(s).ok())
        .unwrap_or_default()
        .approval_notifications
}

async fn load(app: &App, user_id: &str) -> ApiResult<Preferences> {
    let uid = user_id.to_owned();
    let stored: Option<String> = app
        .db
        .call(move |c| {
            c.query_row("SELECT preferences FROM users WHERE id = ?1", [uid], |r| {
                r.get(0)
            })
            .optional()
            .map(Option::flatten)
        })
        .await?;
    Ok(stored
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default())
}

/// `GET /api/account/preferences`.
pub async fn get(State(app): State<App>, s: Session) -> ApiResult<Json<Preferences>> {
    Ok(Json(load(&app, &s.user.id).await?))
}

/// `PUT /api/account/preferences`: merges the keys it gets.
pub async fn put(
    State(app): State<App>,
    s: Session,
    Json(body): Json<Value>,
) -> ApiResult<Json<Preferences>> {
    let patch = body
        .as_object()
        .ok_or_else(|| ApiError::bad("Send the preferences to change as a JSON object."))?;
    let mut prefs = load(&app, &s.user.id).await?;
    prefs
        .merge(patch)
        .map_err(|(field, message)| ApiError::invalid(field, message))?;
    let json = serde_json::to_string(&prefs).map_err(ApiError::internal)?;
    let uid = s.user.id.clone();
    app.db
        .call(move |c| {
            c.execute(
                "UPDATE users SET preferences = ?2 WHERE id = ?1",
                (uid, json),
            )
        })
        .await?;
    Ok(Json(prefs))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn patch(v: Value) -> Map<String, Value> {
        v.as_object().cloned().unwrap_or_default()
    }

    #[test]
    fn merge_changes_only_the_keys_given_and_ignores_unknown_ones() {
        let mut p = Preferences::default();
        p.merge(&patch(
            json!({ "appearance": "dark", "approval_sound": true, "theme": "neon" }),
        ))
        .unwrap();
        let expected = Preferences {
            appearance: "dark".into(),
            approval_sound: true,
            ..Preferences::default()
        };
        assert_eq!(p, expected);
    }

    #[test]
    fn a_bad_value_names_its_field_and_changes_nothing() {
        let mut p = Preferences::default();
        let err = p
            .merge(&patch(
                json!({ "screen_mode": "control", "screen_quality": "ultra" }),
            ))
            .unwrap_err();
        assert_eq!(err.0, "screen_quality");
        assert_eq!(p, Preferences::default(), "all or nothing");
        let err = p
            .merge(&patch(json!({ "clipboard_sync": "yes" })))
            .unwrap_err();
        assert_eq!(err.0, "clipboard_sync");
    }

    #[tokio::test]
    async fn preferences_round_trip_through_the_migrated_column() {
        let db = crate::db::Db::memory().unwrap();
        db.call(|c| {
            c.execute(
                "INSERT INTO users (id, username, display_name, password_hash, role, created_at) VALUES ('u1','alex','Alex','x','admin',0)",
                [],
            )
        })
        .await
        .unwrap();
        let mut p = Preferences::default();
        p.merge(&patch(json!({ "start_page": "last_mac" })))
            .unwrap();
        let stored = serde_json::to_string(&p).unwrap();
        db.call(move |c| {
            c.execute(
                "UPDATE users SET preferences = ?1 WHERE id = 'u1'",
                [stored],
            )
        })
        .await
        .unwrap();
        let back: String = db
            .call(|c| {
                c.query_row("SELECT preferences FROM users WHERE id = 'u1'", [], |r| {
                    r.get(0)
                })
            })
            .await
            .unwrap();
        assert_eq!(serde_json::from_str::<Preferences>(&back).unwrap(), p);
        // Keys added in later versions fall back to their defaults.
        let old: Preferences = serde_json::from_str(r#"{"appearance":"light"}"#).unwrap();
        assert!(old.clipboard_sync);
    }
}
