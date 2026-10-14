//! Die Weboberfläche: drei statische Dateien, ins Programm eingebaut.

use axum::{
    http::{header, HeaderName, HeaderValue},
    response::IntoResponse,
    routing::get,
    Router,
};

use crate::AppState;

const INDEX: &str = include_str!("../web/index.html");
const APP_JS: &str = include_str!("../web/app.js");
const STYLE: &str = include_str!("../web/style.css");

const CSP: &str = "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self' data:; \
                   connect-src 'self'; form-action 'self'; base-uri 'none'; frame-ancestors 'none'";

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(|| asset(INDEX, "text/html; charset=utf-8")))
        .route(
            "/app.js",
            get(|| asset(APP_JS, "text/javascript; charset=utf-8")),
        )
        .route(
            "/style.css",
            get(|| asset(STYLE, "text/css; charset=utf-8")),
        )
}

async fn asset(body: &'static str, content_type: &'static str) -> impl IntoResponse {
    let headers: [(HeaderName, HeaderValue); 6] = [
        (header::CONTENT_TYPE, HeaderValue::from_static(content_type)),
        (
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(CSP),
        ),
        (
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        ),
        (header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY")),
        (
            header::REFERRER_POLICY,
            HeaderValue::from_static("no-referrer"),
        ),
        (header::CACHE_CONTROL, HeaderValue::from_static("no-cache")),
    ];
    (headers, body)
}
