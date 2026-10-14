//! `subweave dash`: the web dashboard. Another client of the daemon's files, like the
//! CLI: it reads `services/`, writes only `inbox/` (through `service::put`), and serves
//! the React app built from `dash/` and embedded by `build.rs`.

mod assets;
mod auth;
pub mod config;
mod live;
mod read;

use std::future::IntoFuture;
use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::{Context, Result};
use axum::extract::{FromRequest, Request};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{any, get, post};
use axum::{Json, Router, middleware};
use serde::de::DeserializeOwned;
use serde_json::json;

pub use config::Config;

/// What every request shares.
pub(crate) struct App {
    pub config: Config,
    pub sessions: auth::Sessions,
    pub live: live::Live,
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
        match Json::<T>::from_request(req, state).await {
            Ok(Json(v)) => Ok(Body(v)),
            Err(e) => Err(ApiError::bad(e.body_text())),
        }
    }
}

/// Runs the dashboard in the foreground until SIGINT or SIGTERM.
pub async fn serve(config: Config) -> Result<()> {
    let ip = config.addr()?;
    let port = config.port;
    let app = Arc::new(App {
        config,
        sessions: auth::Sessions::default(),
        live: live::Live::new(),
    });
    tokio::spawn(live::tail(app.clone()));
    let api = Router::new()
        .route("/session", get(auth::session))
        .route("/login", post(auth::login))
        .route("/logout", post(auth::logout))
        .route("/services", get(read::services))
        .route("/services/{s}", get(read::service))
        .route("/services/{s}/journal", get(read::journal))
        .route("/events", get(live::events))
        .fallback(any(|| async { ApiError::not_found("no such API route") }));
    let router = Router::new()
        .nest("/api", api)
        .fallback(assets::serve)
        .layer(middleware::from_fn_with_state(app.clone(), auth::guard))
        .with_state(app);
    let listener = tokio::net::TcpListener::bind((ip, port))
        .await
        .with_context(|| format!("listening on {ip}:{port}"))?;
    eprintln!(
        "subweave dash: http://{} (state {})",
        SocketAddr::new(ip, port),
        crate::paths::state().display()
    );
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .context("listening for SIGTERM")?;
    // Not a graceful shutdown: open event streams never end on their own.
    let server = axum::serve(
        listener,
        router.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .into_future();
    tokio::select! {
        r = server => r.context("serving the dashboard"),
        _ = tokio::signal::ctrl_c() => Ok(()),
        _ = term.recv() => Ok(()),
    }
}
