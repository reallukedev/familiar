//! Portal API errors: a stable code and a sentence that is safe to show as-is.
use axum::{
    Json,
    http::{HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use serde_json::json;

/// An API failure.
#[derive(Debug)]
pub struct ApiError {
    /// HTTP status.
    pub status: StatusCode,
    /// Stable code from docs/home/API.md.
    pub code: &'static str,
    /// Plain sentence for the person.
    pub message: String,
    /// Which input field was wrong.
    pub field: Option<&'static str>,
    /// Seconds before retrying makes sense.
    pub retry_after_s: Option<u64>,
    /// Extra top-level fields (e.g. the TOTP `challenge`).
    pub extra: Option<(&'static str, String)>,
}

/// Shorthand.
pub type ApiResult<T> = Result<T, ApiError>;

impl ApiError {
    fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
            field: None,
            retry_after_s: None,
            extra: None,
        }
    }
    /// 401.
    pub fn unauthenticated() -> Self {
        Self::new(
            StatusCode::UNAUTHORIZED,
            "unauthenticated",
            "Your session ended. Sign in again to continue.",
        )
    }
    /// 403.
    pub fn forbidden(message: impl Into<String>) -> Self {
        Self::new(StatusCode::FORBIDDEN, "forbidden", message)
    }
    /// 404. Also used for other people's resources so their existence isn't revealed.
    pub fn not_found(what: &str) -> Self {
        Self::new(
            StatusCode::NOT_FOUND,
            "not_found",
            format!("That {what} doesn't exist or isn't yours."),
        )
    }
    /// 404 with its own sentence (a file or folder in a Mac, say).
    pub fn missing(message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, "not_found", message)
    }
    /// 413: the body is bigger than allowed.
    pub fn too_large(message: impl Into<String>) -> Self {
        Self::new(StatusCode::PAYLOAD_TOO_LARGE, "invalid", message)
    }
    /// 400 for a specific field.
    pub fn invalid(field: &'static str, message: impl Into<String>) -> Self {
        let mut e = Self::new(StatusCode::BAD_REQUEST, "invalid", message);
        e.field = Some(field);
        e
    }
    /// 400 without a field.
    pub fn bad(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "invalid", message)
    }
    /// 409.
    pub fn conflict(message: impl Into<String>) -> Self {
        Self::new(StatusCode::CONFLICT, "conflict", message)
    }
    /// 429.
    pub fn rate_limited(retry_after_s: u64) -> Self {
        let mut e = Self::new(
            StatusCode::TOO_MANY_REQUESTS,
            "rate_limited",
            format!(
                "Too many attempts. Try again in {}.",
                crate::util::human_duration(retry_after_s)
            ),
        );
        e.retry_after_s = Some(retry_after_s);
        e
    }
    /// 403 CSRF.
    pub fn csrf() -> Self {
        Self::new(
            StatusCode::FORBIDDEN,
            "csrf",
            "This request didn't come from your Familiar page. Reload the page and try again.",
        )
    }
    /// 403: the session must confirm the password first.
    pub fn reauth() -> Self {
        Self::new(
            StatusCode::FORBIDDEN,
            "reauth_required",
            "Confirm your password to continue.",
        )
    }
    /// 409: resources exhausted.
    pub fn budget(message: impl Into<String>) -> Self {
        Self::new(StatusCode::CONFLICT, "budget", message)
    }
    /// 423: an encrypted Mac needs its owner's key, which isn't unlocked since the
    /// server started. Confirming the password (`/api/account/confirm`) unlocks it.
    pub fn locked() -> Self {
        Self::new(StatusCode::LOCKED, "locked", crate::vm::LOCKED)
    }
    /// A supervisor's refusal: `locked` when it's [`crate::vm::LOCKED`], else 503.
    pub fn from_machine(message: String) -> Self {
        if message == crate::vm::LOCKED {
            Self::locked()
        } else {
            Self::unavailable(message)
        }
    }
    /// 503.
    pub fn unavailable(message: impl Into<String>) -> Self {
        Self::new(StatusCode::SERVICE_UNAVAILABLE, "unavailable", message)
    }
    /// 500. The cause is logged, never returned.
    pub fn internal(cause: impl std::fmt::Display) -> Self {
        tracing::error!(%cause, "internal error");
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            "Something went wrong on the server. Try again; if it keeps happening, check the server log.",
        )
    }
    /// Attach a top-level field.
    #[must_use]
    pub fn with(mut self, key: &'static str, value: String) -> Self {
        self.extra = Some((key, value));
        self
    }
}

impl From<crate::db::DbError> for ApiError {
    fn from(e: crate::db::DbError) -> Self {
        Self::internal(e)
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(e: anyhow::Error) -> Self {
        Self::internal(format!("{e:#}"))
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let mut body = json!({
            "error": { "code": self.code, "message": self.message }
        });
        if let Some(field) = self.field {
            body["error"]["field"] = json!(field);
        }
        if let Some(s) = self.retry_after_s {
            body["error"]["retry_after_s"] = json!(s);
        }
        if let Some((k, v)) = self.extra {
            body[k] = json!(v);
        }
        let mut response = (self.status, Json(body)).into_response();
        if let Some(s) = self.retry_after_s
            && let Ok(v) = HeaderValue::from_str(&s.to_string())
        {
            response.headers_mut().insert("retry-after", v);
        }
        response
    }
}
