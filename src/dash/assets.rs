//! The React app from `dash/dist`, embedded by `build.rs`. Every path that is no file
//! gets `index.html`: the app routes by the History API.

use axum::http::{Method, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};

include!(concat!(env!("OUT_DIR"), "/assets.rs"));

pub(crate) async fn serve(method: Method, uri: Uri) -> Response {
    if method != Method::GET && method != Method::HEAD {
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    }
    let path = uri.path().trim_start_matches('/');
    let (name, bytes) = ASSETS
        .iter()
        .find(|(p, _)| *p == path)
        .or_else(|| ASSETS.iter().find(|(p, _)| *p == "index.html"))
        .copied()
        .expect("build.rs embeds index.html");
    // Vite names built files by their hash: they never change under one name.
    let cache = if name.starts_with("assets/") {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    (
        [
            (header::CONTENT_TYPE, mime(name)),
            (header::CACHE_CONTROL, cache),
        ],
        bytes,
    )
        .into_response()
}

fn mime(name: &str) -> &'static str {
    match name.rsplit('.').next().unwrap_or_default() {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" | "map" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "ttf" => "font/ttf",
        "txt" => "text/plain; charset=utf-8",
        "webmanifest" => "application/manifest+json",
        _ => "application/octet-stream",
    }
}
