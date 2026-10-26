//! The embedded portal and the security headers on every response.
use axum::{
    body::Body,
    extract::Request,
    http::{HeaderValue, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "web-dist/"]
struct Dist;

const CSP: &str = "default-src 'self'; script-src 'self'; style-src 'self'; style-src-attr 'unsafe-inline'; \
img-src 'self' data: blob:; font-src 'self'; connect-src 'self'; worker-src 'self' blob:; \
media-src 'self' blob:; frame-src 'self' blob:; frame-ancestors 'none'; base-uri 'none'; form-action 'self'; object-src 'none'";

/// Serve a file from the bundle, or `index.html` for client-side routes.
pub async fn serve(request: Request) -> Response {
    let path = request.uri().path().trim_start_matches('/');
    let (file, path) = match Dist::get(path) {
        Some(f) if !path.is_empty() => (f, path.to_owned()),
        _ => {
            // Missing assets are real 404s; anything else is a portal route.
            if path.starts_with("assets/") || path.contains('.') && !path.ends_with(".html") {
                return StatusCode::NOT_FOUND.into_response();
            }
            match Dist::get("index.html") {
                Some(f) => (f, "index.html".to_owned()),
                None => return StatusCode::NOT_FOUND.into_response(),
            }
        }
    };
    let mime = mime_guess::from_path(&path).first_or_octet_stream();
    // Hashed assets never change; the HTML shell must always be fresh.
    let cache = if path.starts_with("assets/") {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    let mut response = Response::new(Body::from(file.data.into_owned()));
    let h = response.headers_mut();
    if let Ok(v) = HeaderValue::from_str(mime.as_ref()) {
        h.insert(header::CONTENT_TYPE, v);
    }
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
    response
}

/// Security headers for everything the server returns.
pub async fn security_headers(https: bool, request: Request, next: Next) -> Response {
    let is_api =
        request.uri().path().starts_with("/api") || request.uri().path().starts_with("/oauth");
    let mut response = next.run(request).await;
    let h = response.headers_mut();
    h.insert("content-security-policy", HeaderValue::from_static(CSP));
    h.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    h.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    h.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    h.insert(
        "cross-origin-opener-policy",
        HeaderValue::from_static("same-origin"),
    );
    h.insert(
        "cross-origin-resource-policy",
        HeaderValue::from_static("same-origin"),
    );
    h.insert(
        "permissions-policy",
        HeaderValue::from_static(
            "camera=(), microphone=(), geolocation=(), payment=(), usb=(), interest-cohort=()",
        ),
    );
    if https {
        h.insert(
            header::STRICT_TRANSPORT_SECURITY,
            HeaderValue::from_static("max-age=63072000; includeSubDomains"),
        );
    }
    if is_api && !h.contains_key(header::CACHE_CONTROL) {
        h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    response
}
