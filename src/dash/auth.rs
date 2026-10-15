//! Password login, sessions in memory, and the guard every request passes: a `Host`
//! this dashboard answers to (no DNS rebinding), JSON and a same-origin `Origin` on
//! POSTs (with SameSite=Strict, the CSRF guard), and a session cookie for `/api/*`.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::Json;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{HeaderMap, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use tokio::sync::Semaphore;

use super::{ApiError, App, Body};

const COOKIE: &str = "sw_session";
const SESSION_AGE: Duration = Duration::from_secs(7 * 24 * 3600);
const WRONG_DELAY: Duration = Duration::from_secs(1);
const FAIL_WINDOW: Duration = Duration::from_secs(600);
/// Wrong passwords plus logins in flight, per client, before it waits out the window.
const FAIL_LIMIT: usize = 10;
/// argon2 checks at once, and logins that may wait for one; past that, 429.
const VERIFY_AT_ONCE: usize = 2;
const VERIFY_QUEUE: usize = 16;
/// Log streams one session may hold open.
const STREAMS: usize = 8;

#[derive(Default)]
struct Tries {
    /// Wrong passwords within the window.
    wrong: Vec<Instant>,
    blocked_until: Option<Instant>,
    in_flight: usize,
}

pub(crate) struct Sessions {
    /// Token → when it ends.
    live: Mutex<HashMap<String, Instant>>,
    tries: Mutex<HashMap<IpAddr, Tries>>,
    verify: Semaphore,
    queued: AtomicUsize,
    /// Token → its open log streams.
    streams: Mutex<HashMap<String, usize>>,
    /// SHA-256 of the password a `/metrics` scraper sent, once argon2 passed it.
    scrape: Mutex<Option<[u8; 32]>>,
}

impl Default for Sessions {
    fn default() -> Self {
        Sessions {
            live: Mutex::default(),
            tries: Mutex::default(),
            verify: Semaphore::new(VERIFY_AT_ONCE),
            queued: AtomicUsize::new(0),
            streams: Mutex::default(),
            scrape: Mutex::default(),
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// A login in flight for a client; it stops counting when dropped.
struct Attempt<'a> {
    sessions: &'a Sessions,
    ip: IpAddr,
}

impl Drop for Attempt<'_> {
    fn drop(&mut self) {
        if let Some(t) = lock(&self.sessions.tries).get_mut(&self.ip) {
            t.in_flight -= 1;
        }
    }
}

/// A login waiting for an argon2 check; it leaves the queue when dropped.
struct Queued<'a>(&'a AtomicUsize);

impl Drop for Queued<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

/// One open log stream of a session; it closes when dropped.
pub(crate) struct StreamSlot {
    app: Arc<App>,
    token: String,
}

impl Drop for StreamSlot {
    fn drop(&mut self) {
        let mut streams = lock(&self.app.sessions.streams);
        if let Some(n) = streams.get_mut(&self.token) {
            *n -= 1;
            if *n == 0 {
                streams.remove(&self.token);
            }
        }
    }
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

    /// Counts a login before its password is checked: a client with `FAIL_LIMIT` wrong
    /// passwords and logins in flight waits.
    fn begin(&self, ip: IpAddr) -> Option<Attempt<'_>> {
        let mut tries = lock(&self.tries);
        let now = Instant::now();
        tries.retain(|_, t| {
            t.wrong.retain(|w| now.duration_since(*w) < FAIL_WINDOW);
            !t.wrong.is_empty() || t.in_flight > 0 || t.blocked_until.is_some_and(|u| u > now)
        });
        let t = tries.entry(ip).or_default();
        if t.blocked_until.is_some_and(|u| u > now) || t.wrong.len() + t.in_flight >= FAIL_LIMIT {
            return None;
        }
        t.in_flight += 1;
        Some(Attempt { sessions: self, ip })
    }

    fn failed(&self, ip: IpAddr) {
        let mut tries = lock(&self.tries);
        let now = Instant::now();
        let t = tries.entry(ip).or_default();
        t.wrong.push(now);
        if t.wrong.len() >= FAIL_LIMIT {
            t.blocked_until = Some(now + FAIL_WINDOW);
            t.wrong.clear();
        }
    }
}

/// A log stream for the request's session, or 429 past `STREAMS`.
pub(crate) fn stream_slot(app: &Arc<App>, headers: &HeaderMap) -> Result<StreamSlot, ApiError> {
    let token = cookie(headers).unwrap_or_default().to_string();
    let mut streams = lock(&app.sessions.streams);
    let n = streams.entry(token.clone()).or_default();
    if *n >= STREAMS {
        return Err(ApiError(
            StatusCode::TOO_MANY_REQUESTS,
            format!("{STREAMS} log streams are open: close one"),
        ));
    }
    *n += 1;
    Ok(StreamSlot {
        app: app.clone(),
        token,
    })
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
    cookie(headers).is_some_and(|t| app.sessions.valid(t))
}

/// The box's own addresses, from its interfaces.
fn local_ips() -> Vec<IpAddr> {
    let mut out = Vec::new();
    let mut head: *mut libc::ifaddrs = std::ptr::null_mut();
    // SAFETY: getifaddrs fills `head` with a list that freeifaddrs frees below.
    if unsafe { libc::getifaddrs(&mut head) } != 0 {
        return out;
    }
    let mut p = head;
    while !p.is_null() {
        // SAFETY: `p` is a node of the list getifaddrs returned, not yet freed.
        let ifa = unsafe { &*p };
        if !ifa.ifa_addr.is_null() {
            // SAFETY: ifa_addr points to a sockaddr whose family says its real type.
            match i32::from(unsafe { (*ifa.ifa_addr).sa_family }) {
                libc::AF_INET => {
                    let sa = unsafe { &*ifa.ifa_addr.cast::<libc::sockaddr_in>() };
                    out.push(IpAddr::V4(Ipv4Addr::from(u32::from_be(sa.sin_addr.s_addr))));
                }
                libc::AF_INET6 => {
                    let sa = unsafe { &*ifa.ifa_addr.cast::<libc::sockaddr_in6>() };
                    out.push(IpAddr::V6(Ipv6Addr::from(sa.sin6_addr.s6_addr)));
                }
                _ => {}
            }
        }
        p = ifa.ifa_next;
    }
    // SAFETY: `head` came from getifaddrs and is freed once.
    unsafe { libc::freeifaddrs(head) };
    out
}

/// `Host` without its port, lowercase, no trailing dot.
fn host_name(host: &str) -> String {
    let name = match host.strip_prefix('[') {
        Some(v6) => v6.split(']').next().unwrap_or_default(),
        None => match host.rsplit_once(':') {
            Some((name, port)) if port.bytes().all(|b| b.is_ascii_digit()) => name,
            _ => host,
        },
    };
    name.trim_end_matches('.').to_ascii_lowercase()
}

/// The `Host` names this dashboard: the bind address, a loopback name, a name in
/// `hosts`, or one of the box's own IP addresses. A page on another name that resolves
/// here (DNS rebinding) is refused.
fn host_ok(app: &App, headers: &HeaderMap) -> bool {
    let Some(host) = headers.get(header::HOST).and_then(|h| h.to_str().ok()) else {
        return false;
    };
    let name = host_name(host);
    if name == "localhost" || app.config.hosts.contains(&name) {
        return true;
    }
    let Ok(ip) = name.parse::<IpAddr>() else {
        return false;
    };
    ip.is_loopback() || app.config.addr().is_ok_and(|b| b == ip) || local_ips().contains(&ip)
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
    let headers = req.headers();
    if !host_ok(&app, headers) {
        return ApiError(
            StatusCode::MISDIRECTED_REQUEST,
            "this dashboard does not answer to that host name: subweave dash --host <name>".into(),
        )
        .into_response();
    }
    let path = req.uri().path();
    if !path.starts_with("/api/") {
        return next.run(req).await;
    }
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
    Json(json!({"ok": signed_in(&app, &headers)}))
}

#[derive(Deserialize)]
pub(crate) struct Login {
    password: String,
}

/// The dashboard's password, checked under the login limits: wrong tries per client, and
/// argon2 checks at once.
async fn check(app: &App, ip: IpAddr, password: String) -> Result<(), ApiError> {
    let Some(phc) = app.config.password.clone() else {
        return Err(ApiError::internal(anyhow::anyhow!("no password set")));
    };
    let too_many = || {
        ApiError(
            StatusCode::TOO_MANY_REQUESTS,
            "too many wrong passwords: try again in 10 minutes".into(),
        )
    };
    let Some(_attempt) = app.sessions.begin(ip) else {
        return Err(too_many());
    };
    let s = &app.sessions;
    let permit = match s.verify.try_acquire() {
        Ok(p) => p,
        Err(_) => {
            if s.queued.fetch_add(1, Ordering::Relaxed) >= VERIFY_QUEUE {
                s.queued.fetch_sub(1, Ordering::Relaxed);
                return Err(ApiError(
                    StatusCode::TOO_MANY_REQUESTS,
                    "too many logins at once: try again".into(),
                ));
            }
            let _queued = Queued(&s.queued);
            s.verify
                .acquire()
                .await
                .map_err(|e| ApiError::internal(e.into()))?
        }
    };
    let ok = tokio::task::spawn_blocking(move || super::config::verify(&password, &phc))
        .await
        .unwrap_or(false);
    drop(permit);
    if !ok {
        app.sessions.failed(ip);
        tokio::time::sleep(WRONG_DELAY).await;
        return Err(ApiError(StatusCode::UNAUTHORIZED, "wrong password".into()));
    }
    Ok(())
}

/// A `/metrics` scraper: on this box (a loopback peer no proxy forwarded for), or sending
/// the dashboard password as `Authorization: Bearer`. The first good password is kept as
/// its SHA-256, so later scrapes skip argon2.
pub(crate) async fn scraper(app: &App, headers: &HeaderMap, peer: SocketAddr) -> Result<(), ApiError> {
    let ip = client_ip(headers, peer);
    // A proxy on this box reaches the dashboard over loopback for clients anywhere.
    let forwarded = headers.contains_key("x-forwarded-for");
    if ip.is_loopback() && !forwarded {
        return Ok(());
    }
    let Some(password) = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
    else {
        return Err(ApiError(
            StatusCode::UNAUTHORIZED,
            "send the dashboard password: Authorization: Bearer <password>".into(),
        ));
    };
    let digest: [u8; 32] = Sha256::digest(password.as_bytes()).into();
    let kept: Option<[u8; 32]> = *lock(&app.sessions.scrape);
    if kept.is_some_and(|k| same(&k, &digest)) {
        return Ok(());
    }
    check(app, ip, password.to_string()).await?;
    *lock(&app.sessions.scrape) = Some(digest);
    Ok(())
}

/// Equal in time that does not depend on where they differ.
fn same(a: &[u8; 32], b: &[u8; 32]) -> bool {
    a.iter().zip(b).fold(0u8, |d, (x, y)| d | (x ^ y)) == 0
}

pub(crate) async fn login(
    State(app): State<Arc<App>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Body(body): Body<Login>,
) -> Result<Response, ApiError> {
    check(&app, client_ip(&headers, peer), body.password).await?;
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
