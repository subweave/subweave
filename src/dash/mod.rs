//! `subweave dash`: the web dashboard. Another client of the daemon's files, like the
//! CLI: it reads `services/`, writes only `inbox/` (through `service::put`), and serves
//! the React app built from `dash/` and embedded by `build.rs`.

mod act;
mod assets;
mod auth;
pub mod config;
mod live;
mod logs;
mod metrics;
mod read;
mod vitals;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use axum::extract::{ConnectInfo, FromRequest, FromRequestParts, RawPathParams, Request};
use axum::http::StatusCode;
use axum::http::request::Parts;
use axum::response::{IntoResponse, Response};
use axum::routing::{any, get, post};
use axum::{Json, Router, middleware};
use hyper_util::rt::{TokioIo, TokioTimer};
use serde::de::DeserializeOwned;
use serde_json::json;
use tokio::sync::Semaphore;
use tower::ServiceExt;

use crate::service;

/// A client must send its request headers, and a JSON body, within these.
const HEADER_TIMEOUT: Duration = Duration::from_secs(10);
const BODY_TIMEOUT: Duration = Duration::from_secs(10);
/// Connections open at once; the next waits in the kernel's queue.
const CONNECTIONS: usize = 256;

pub use config::Config;

/// What every request shares.
pub(crate) struct App {
    pub config: Config,
    pub sessions: auth::Sessions,
    pub live: live::Live,
    pub vitals: vitals::Vitals,
    pub metrics: metrics::Metrics,
}

/// `{error}` with a status.
#[derive(Debug)]
pub(crate) struct ApiError(pub StatusCode, pub String);

impl ApiError {
    pub fn bad(text: impl Into<String>) -> ApiError {
        ApiError(StatusCode::BAD_REQUEST, text.into())
    }

    pub fn not_found(text: impl Into<String>) -> ApiError {
        ApiError(StatusCode::NOT_FOUND, text.into())
    }

    pub fn conflict(text: impl Into<String>) -> ApiError {
        ApiError(StatusCode::CONFLICT, text.into())
    }

    /// A failure reading or writing the daemon's files.
    pub fn internal(e: anyhow::Error) -> ApiError {
        ApiError(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}"))
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({"error": self.1}))).into_response()
    }
}

/// A JSON body whose rejection is an `{error}` too.
pub(crate) struct Body<T>(pub T);

impl<S: Send + Sync, T: DeserializeOwned> FromRequest<S> for Body<T> {
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &S) -> Result<Self, ApiError> {
        match tokio::time::timeout(BODY_TIMEOUT, Json::<T>::from_request(req, state)).await {
            Ok(Ok(Json(v))) => Ok(Body(v)),
            Ok(Err(e)) => Err(ApiError::bad(e.body_text())),
            Err(_) => Err(ApiError(
                StatusCode::REQUEST_TIMEOUT,
                "the request body took too long".into(),
            )),
        }
    }
}

/// The `{s}` of a path: a service the daemon keeps. Every route reads it this way, so no
/// path is built from a name that is not one.
pub(crate) struct Svc(pub String);

impl<S: Send + Sync> FromRequestParts<S> for Svc {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, ApiError> {
        let params = RawPathParams::from_request_parts(parts, state)
            .await
            .map_err(|e| ApiError::bad(e.body_text()))?;
        let s = params
            .iter()
            .find(|(k, _)| *k == "s")
            .map(|(_, v)| v.to_string())
            .unwrap_or_default();
        let known = crate::daemon::valid_name(&s)
            && service::names().map_err(ApiError::internal)?.contains(&s);
        if !known {
            return Err(ApiError::not_found(format!("no service {s} here")));
        }
        Ok(Svc(s))
    }
}

/// Runs the dashboard in the foreground until SIGINT or SIGTERM.
pub async fn serve(config: Config) -> Result<()> {
    config.need_password()?;
    crate::logs::init();
    let ip = config.addr()?;
    let port = config.port;
    let app = Arc::new(App {
        config,
        sessions: auth::Sessions::default(),
        live: live::Live::new(),
        vitals: vitals::Vitals::default(),
        metrics: metrics::Metrics::default(),
    });
    tokio::spawn(live::tail(app.clone()));
    tokio::spawn(vitals::sample(app.clone()));
    let api = Router::new()
        .route("/session", get(auth::session))
        .route("/login", post(auth::login))
        .route("/logout", post(auth::logout))
        .route("/box", get(vitals::box_info))
        .route("/vitals", get(vitals::get))
        .route("/events", get(live::events))
        .route("/services", get(read::services))
        .route("/services/{s}", get(read::service))
        .route("/services/{s}/journal", get(read::journal))
        .route("/services/{s}/metrics", get(read::metrics))
        .route("/services/{s}/logs", get(logs::list))
        .route("/services/{s}/logs/{name}", get(logs::stream))
        .route("/services/{s}/answer", post(act::answer))
        .route("/services/{s}/do", post(act::do_action))
        .route("/services/{s}/task", post(act::task))
        .route("/services/{s}/tell", post(act::tell))
        .route("/services/{s}/stop", post(act::stop))
        .route("/services/{s}/start", post(act::start))
        .route("/services/{s}/forget", post(act::forget))
        .fallback(any(|| async { ApiError::not_found("no such API route") }));
    let router = Router::new()
        .nest("/api", api)
        .route("/metrics", get(metrics::get))
        .fallback(assets::serve)
        .layer(middleware::from_fn_with_state(app.clone(), auth::guard))
        .with_state(app);
    let listener = tokio::net::TcpListener::bind((ip, port))
        .await
        .with_context(|| format!("listening on {ip}:{port}"))?;
    tracing::info!(
        url = %format!("http://{}", SocketAddr::new(ip, port)),
        state = %crate::paths::state().display(),
        "subweave dash started"
    );
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .context("listening for SIGTERM")?;
    let conns = Arc::new(Semaphore::new(CONNECTIONS));
    // Not a graceful shutdown: open event streams never end on their own.
    let accept = async {
        loop {
            let Ok(permit) = conns.clone().acquire_owned().await else {
                return;
            };
            let (stream, peer) = match listener.accept().await {
                Ok(c) => c,
                Err(e) => {
                    // Out of file descriptors, most likely: let some close.
                    tracing::warn!(error = %e, "accept failed");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    continue;
                }
            };
            let router = router.clone();
            tokio::spawn(async move {
                let svc = hyper::service::service_fn(move |mut req: Request<hyper::body::Incoming>| {
                    req.extensions_mut().insert(ConnectInfo(peer));
                    router.clone().oneshot(req)
                });
                let mut http = hyper::server::conn::http1::Builder::new();
                http.timer(TokioTimer::new())
                    .header_read_timeout(HEADER_TIMEOUT);
                let _ = http.serve_connection(TokioIo::new(stream), svc).await;
                drop(permit);
            });
        }
    };
    tokio::select! {
        () = accept => Ok(()),
        _ = tokio::signal::ctrl_c() => Ok(()),
        _ = term.recv() => Ok(()),
    }
}
