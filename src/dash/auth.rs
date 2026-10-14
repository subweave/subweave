//! Password login, sessions in memory, and the guard every `/api/*` request passes:
//! JSON and a same-origin `Origin` on POSTs (with SameSite=Strict, the CSRF guard), a
//! session cookie when a password is set.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::Json;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{HeaderMap, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::json;

use super::{ApiError, App, Body};

const COOKIE: &str = "sw_session";
const SESSION_AGE: Duration = Duration::from_secs(7 * 24 * 3600);
const WRONG_DELAY: Duration = Duration::from_secs(1);
const FAIL_WINDOW: Duration = Duration::from_secs(600);
const FAIL_LIMIT: usize = 10;

#[derive(Default)]
pub(crate) struct Sessions {
    /// Token → when it ends.
    live: Mutex<HashMap<String, Instant>>,
    /// Per client, its wrong passwords within the window, and the end of its block.
    fails: Mutex<HashMap<IpAddr, (Vec<Instant>, Option<Instant>)>>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Sessions {
    fn valid(&self, token: &str) -> bool {
        let mut live = lock(&self.live);
        let now = Instant::now();
        live.retain(|_, end| *end > now);
        live.contains_key(token)
    }

    fn new_session(&self) -> String {
        use argon2::password_hash::rand_core::{OsRng, RngCore};
        let mut bytes = [0u8; 32];
        OsRng.fill_bytes(&mut bytes);
        let token = hex::encode(bytes);
        lock(&self.live).insert(token.clone(), Instant::now() + SESSION_AGE);
        token
    }

    fn blocked(&self, ip: IpAddr) -> bool {
        let mut fails = lock(&self.fails);
        let now = Instant::now();
        fails.retain(|_, (tries, until)| {
            tries.retain(|t| now.duration_since(*t) < FAIL_WINDOW);
            !tries.is_empty() || until.is_some_and(|u| u > now)
        });
        fails
            .get(&ip)
            .and_then(|(_, until)| *until)
            .is_some_and(|u| u > now)
    }

    fn failed(&self, ip: IpAddr) {
        let mut fails = lock(&self.fails);
        let now = Instant::now();
        let (tries, until) = fails.entry(ip).or_default();
        tries.push(now);
        if tries.len() >= FAIL_LIMIT {
            *until = Some(now + FAIL_WINDOW);
            tries.clear();
        }
    }
}

/// The client: the peer, or the address a proxy on this machine forwarded for.
pub(crate) fn client_ip(headers: &HeaderMap, peer: SocketAddr) -> IpAddr {
    if peer.ip().is_loopback() {
        let forwarded = headers
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.rsplit(',').next())
            .and_then(|v| v.trim().parse().ok());
        if let Some(ip) = forwarded {
            return ip;
        }
    }
    peer.ip()
}

/// `dash@<client ip>`: who acted, on the lines and Commands the dashboard causes.
pub(crate) fn by(headers: &HeaderMap, peer: SocketAddr) -> String {
    format!("dash@{}", client_ip(headers, peer))
}

fn cookie(headers: &HeaderMap) -> Option<&str> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .find_map(|c| c.trim().strip_prefix(COOKIE)?.strip_prefix('='))
}

fn signed_in(app: &App, headers: &HeaderMap) -> bool {
    app.config.password.is_none() || cookie(headers).is_some_and(|t| app.sessions.valid(t))
}

/// `Origin`, when sent, names the host the request came to.
fn same_origin(headers: &HeaderMap) -> bool {
    let Some(origin) = headers.get(header::ORIGIN) else {
        return true;
    };
    let host = headers.get(header::HOST).and_then(|h| h.to_str().ok());
    let origin_host = origin
        .to_str()
        .ok()
        .and_then(|o| o.split_once("://"))
        .map(|(_, h)| h.trim_end_matches('/'));
    host.is_some() && origin_host == host
}

pub(crate) async fn guard(State(app): State<Arc<App>>, req: Request, next: Next) -> Response {
    let path = req.uri().path();
    if !path.starts_with("/api/") {
        return next.run(req).await;
    }
    let headers = req.headers();
    if req.method() == Method::POST {
        let json = headers
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.starts_with("application/json"));
        if !json {
            return ApiError::bad("send JSON, with Content-Type: application/json").into_response();
        }
        if !same_origin(headers) {
            return ApiError::bad("the request's Origin is not this dashboard").into_response();
        }
    }
    let open = matches!(path, "/api/session" | "/api/login");
    if !open && !signed_in(&app, headers) {
        return ApiError(StatusCode::UNAUTHORIZED, "log in first".into()).into_response();
    }
    next.run(req).await
}

pub(crate) async fn session(State(app): State<Arc<App>>, headers: HeaderMap) -> Json<serde_json::Value> {
    let auth = if app.config.password.is_some() {
        "password"
    } else {
        "none"
    };
    Json(json!({"auth": auth, "ok": signed_in(&app, &headers)}))
}

#[derive(Deserialize)]
pub(crate) struct Login {
    password: String,
}

pub(crate) async fn login(
    State(app): State<Arc<App>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Body(body): Body<Login>,
) -> Result<Response, ApiError> {
    let Some(phc) = app.config.password.clone() else {
        return Err(ApiError::bad("this dashboard has no password"));
    };
    let ip = client_ip(&headers, peer);
    if app.sessions.blocked(ip) {
        return Err(ApiError(
            StatusCode::TOO_MANY_REQUESTS,
            "too many wrong passwords: try again in 10 minutes".into(),
        ));
    }
    let ok = tokio::task::spawn_blocking(move || super::config::verify(&body.password, &phc))
        .await
        .unwrap_or(false);
    if !ok {
        app.sessions.failed(ip);
        tokio::time::sleep(WRONG_DELAY).await;
        return Err(ApiError(StatusCode::UNAUTHORIZED, "wrong password".into()));
    }
    let token = app.sessions.new_session();
    let https = headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.eq_ignore_ascii_case("https"));
    let mut cookie = format!(
        "{COOKIE}={token}; HttpOnly; SameSite=Strict; Path=/; Max-Age={}",
        SESSION_AGE.as_secs()
    );
    if https {
        cookie.push_str("; Secure");
    }
    Ok((StatusCode::NO_CONTENT, [(header::SET_COOKIE, cookie)]).into_response())
}

pub(crate) async fn logout(State(app): State<Arc<App>>, headers: HeaderMap) -> Response {
    if let Some(t) = cookie(&headers) {
        lock(&app.sessions.live).remove(t);
    }
    let gone = format!("{COOKIE}=; HttpOnly; SameSite=Strict; Path=/; Max-Age=0");
    (StatusCode::NO_CONTENT, [(header::SET_COOKIE, gone)]).into_response()
}
