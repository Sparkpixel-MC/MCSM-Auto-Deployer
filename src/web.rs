//! Embedded single-page Material Design 3 dashboard.

use axum::body::Body;
use axum::http::{header, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "src/web"]
struct WebAssets;

pub async fn static_handler(uri: Uri) -> Response {
    let requested = uri.path().trim_start_matches('/');
    if requested.starts_with("api/") {
        return (StatusCode::NOT_FOUND, "Not Found").into_response();
    }
    let requested = if requested.is_empty() {
        "index.html"
    } else {
        requested
    };
    let (served, content) = match WebAssets::get(requested) {
        Some(content) => (requested, content),
        None => match WebAssets::get("index.html") {
            Some(content) => ("index.html", content),
            None => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "index.html is not embedded",
                )
                    .into_response()
            }
        },
    };
    let mime = mime_guess::from_path(served).first_or_octet_stream();
    let cache = "no-cache, must-revalidate";
    match Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, mime.as_ref())
        .header(header::CACHE_CONTROL, cache)
        .header("X-Content-Type-Options", "nosniff")
        .body(Body::from(content.data.into_owned()))
    {
        Ok(response) => response,
        Err(error) => {
            tracing::error!(error = %error, "无法构建静态资源响应");
            (StatusCode::INTERNAL_SERVER_ERROR, "Internal Server Error").into_response()
        }
    }
}
