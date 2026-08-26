//! The local HTTPS+WSS server a phone pairs with.
//!
//! There used to also be a desktop-local "dashboard" HTTP endpoint on
//! 127.0.0.1, for viewing the QR code/status/history from an ordinary
//! browser tab without needing a native GUI toolkit. It's gone now: every
//! supported platform has its own native window (`window/` for
//! Linux/Windows, `tray/appkit_dashboard.rs` for macOS), so that fallback
//! had no remaining use, and removing it means the app only ever listens
//! on the network once (the LAN-facing socket below), not twice. What's
//! left of it is purely in-process: `dashboard_tx`/`broadcast_dashboard`/
//! `subscribe_dashboard`/`dashboard_snapshot` are the live-status stream
//! every native window renders from -- "dashboard" here just names that
//! snapshot data, not a network endpoint.
//!
//! Security model (see README's "Security" section for the full
//! writeup):
//!
//! - The phone-facing side is bound only to this machine's detected LAN
//!   address, never `0.0.0.0` -- refuses to start at all if no such
//!   address can be found.
//! - TLS (self-signed, see `tls.rs`) encrypts the LAN hop.
//! - The QR URL embeds a 256-bit random **pairing secret** at `/r/:secret`.
//!   Any browser that opens the URL authenticates its WebSocket with that
//!   secret (`/r/:secret/ws`), and the desktop honors it repeatedly rather
//!   than burning it on first use. So the *same* URL connects every time,
//!   across a scan, an iOS "Open in Safari" hand-off (a different storage
//!   context with no shared `localStorage`), a reload, a tab discard, a
//!   Wi-Fi drop, or long backgrounding -- no rescan. The secret is minted at
//!   startup, rotated by "New code", and lost on desktop restart (never
//!   persisted); those are the only times a rescan is needed. The phone also
//!   stores the secret (re-sent on connect) so it reconnects even if it later
//!   lands on a URL that no longer carries it.
//! - Tradeoff vs. a burn-once QR: a photo of the code can pair until the next
//!   "New code"/restart. Still gated by LAN-only binding, TLS, one phone at a
//!   time, per-IP rate limiting, and 256 bits of unguessable entropy.
//! - The static chat page carries no secret and is served for any path;
//!   pairing is gated only at the WebSocket. A legacy burn-once `/:token`
//!   pairing path still exists (the token rotates on first use / `TOKEN_TTL`,
//!   with `RECONNECT_GRACE` after a drop), but it is no longer what the QR
//!   points at.
//! - Only one phone may be connected at a time.
//! - Failed-token requests are rate-limited per source IP (defense in
//!   depth against scanning/log-spam, not against brute-forcing 256 bits,
//!   which is already computationally infeasible).
//! - Nothing is persisted to disk: chat history is an in-memory ring
//!   buffer capped at `HISTORY_MAX` and vanishes when the server stops.
//!
//! ponytail: the burn-once token machinery (`token`, `paired`,
//! `grace_deadline`, `rotate_token`, the expiry task) is now bypassed by the
//! secret-in-QR flow above and only drives the legacy `/:token` route + the
//! "reconnecting" UI hint. Removable in a follow-up if `/:token` is dropped.

use std::collections::{HashMap, VecDeque};
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::body::Body;
use axum::extract::ws::{Message, WebSocket};
use axum::extract::{ConnectInfo, Path, State, WebSocketUpgrade};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use axum_server::Handle;
use base64::Engine;
use rand::RngCore;
use serde::Serialize;
use tokio::sync::broadcast;
use tokio::sync::Notify;
use tokio::task::JoinHandle;

use crate::injector::{InputEvent, SpecialKey};
use crate::{lan, qr, tls};

const MAX_MESSAGE_LEN: usize = 2000;
const HISTORY_MAX: usize = 10;
const TOKEN_TTL: Duration = Duration::from_secs(5 * 60);
const RECONNECT_GRACE: Duration = Duration::from_secs(45);
const RATE_LIMIT_MAX_ATTEMPTS: usize = 20;
const RATE_LIMIT_WINDOW: Duration = Duration::from_secs(60);
const RATE_LIMIT_BLOCK: Duration = Duration::from_secs(5 * 60);

const CHAT_HTML: &str = include_str!("webapp/chat.html");

#[derive(Clone, Serialize)]
struct HistoryItem {
    text: String,
    ts: u64,
}

#[derive(Default)]
struct RateLimiter {
    fails: HashMap<IpAddr, VecDeque<Instant>>,
    blocked_until: HashMap<IpAddr, Instant>,
}

impl RateLimiter {
    fn is_blocked(&self, ip: IpAddr) -> bool {
        self.blocked_until
            .get(&ip)
            .is_some_and(|until| Instant::now() < *until)
    }

    fn record_failure(&mut self, ip: IpAddr) {
        let now = Instant::now();
        let fails = self.fails.entry(ip).or_default();
        fails.push_back(now);
        while let Some(&front) = fails.front() {
            if now.duration_since(front) > RATE_LIMIT_WINDOW {
                fails.pop_front();
            } else {
                break;
            }
        }
        if fails.len() >= RATE_LIMIT_MAX_ATTEMPTS {
            self.blocked_until.insert(ip, now + RATE_LIMIT_BLOCK);
            fails.clear();
        }
    }
}

fn new_token() -> String {
    let mut bytes = [0u8; 32]; // 256 bits of entropy
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

struct ServerState {
    lan_ip: Ipv4Addr,
    lan_port: u16,
    token: Mutex<String>,
    token_created_at: Mutex<Instant>,
    // The 256-bit pairing secret embedded in the QR URL (`/r/:secret`) and
    // honored idempotently at `/r/:secret/ws` -- see the module doc.
    // In-memory only: minted at startup, rotated by "New code", gone on
    // restart. Always set, so the QR always carries a working credential.
    resume_secret: Mutex<String>,
    active: Mutex<bool>,
    // Whether the current token has ever been used to successfully pair.
    // Distinguishes "fresh QR, never scanned" (subject to `TOKEN_TTL`)
    // from "a phone paired and is now within its reconnect grace window"
    // (subject to `RECONNECT_GRACE` instead) -- see the module doc.
    paired: Mutex<bool>,
    grace_deadline: Mutex<Option<Instant>>,
    // Set by `PairingServer::regenerate_token` just before waking
    // `kick_notify`, so `handle_phone_socket`'s post-loop cleanup can
    // tell a deliberate kick (skip the reconnect grace window -- the
    // token's already moved on) apart from an ordinary drop.
    kicked: Mutex<bool>,
    kick_notify: Notify,
    history: Mutex<VecDeque<HistoryItem>>,
    rate_limiter: Mutex<RateLimiter>,
    dashboard_tx: broadcast::Sender<String>,
    on_message: Arc<dyn Fn(InputEvent) + Send + Sync>,
}

impl ServerState {
    fn current_token(&self) -> String {
        self.token.lock().unwrap().clone()
    }

    fn rotate_token(&self) {
        *self.token.lock().unwrap() = new_token();
        *self.token_created_at.lock().unwrap() = Instant::now();
    }

    /// The current pairing secret (the one embedded in the QR URL). Always
    /// set; handed to the phone on every connect so it can also reconnect
    /// from storage if it later lands on a URL without the secret in it.
    fn current_resume_secret(&self) -> String {
        self.resume_secret.lock().unwrap().clone()
    }

    /// Immediately invalidates the current code and issues a fresh one,
    /// kicking whichever phone is connected (if any) so the new code is
    /// actually usable right away rather than sitting inert behind "only
    /// one phone may be connected at a time." Shared by the dashboard's
    /// own "regenerate" WebSocket message and `PairingServer::regenerate_token`.
    fn force_new_code(&self) {
        let was_active = *self.active.lock().unwrap();
        if was_active {
            *self.kicked.lock().unwrap() = true;
            self.kick_notify.notify_waiters();
        }
        self.rotate_token();
        *self.grace_deadline.lock().unwrap() = None;
        *self.paired.lock().unwrap() = false;
        // "New code" is an explicit re-pair: mint a fresh secret (and thus a
        // fresh QR) so the old URL -- or a photo of the old QR -- can no
        // longer reconnect.
        *self.resume_secret.lock().unwrap() = new_token();
        self.broadcast_dashboard();
    }

    fn pairing_url(&self) -> String {
        // The QR points straight at the resume endpoint's page so any browser
        // opening it pairs with the (idempotent) secret -- see the module doc.
        format!(
            "https://{}:{}/r/{}",
            self.lan_ip,
            self.lan_port,
            self.current_resume_secret()
        )
    }

    fn dashboard_snapshot_json(&self) -> String {
        let connected = *self.active.lock().unwrap();
        let reconnecting = self.grace_deadline.lock().unwrap().is_some();
        let history: Vec<HistoryItem> = self.history.lock().unwrap().iter().cloned().collect();
        let pairing_url = self.pairing_url();
        let qr_data_uri = qr::build_data_uri(&pairing_url);
        serde_json::json!({
            "type": "snapshot",
            "connected": connected,
            "reconnecting": reconnecting,
            "pairing_url": pairing_url,
            "qr_data_uri": qr_data_uri,
            "history": history,
        })
        .to_string()
    }

    fn broadcast_dashboard(&self) {
        // No receivers yet (dashboard never opened) is a normal, common
        // case, not an error -- `send` returning Err just means that.
        let _ = self.dashboard_tx.send(self.dashboard_snapshot_json());
    }
}

fn html_response(body: &'static str) -> Response {
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        .header(
            header::CONTENT_SECURITY_POLICY,
            "default-src 'self'; script-src 'unsafe-inline'; \
             style-src 'unsafe-inline'; connect-src 'self'; img-src 'self' data:",
        )
        .header("X-Content-Type-Options", "nosniff")
        .header(header::CACHE_CONTROL, "no-store")
        .body(Body::from(body))
        .expect("building a static HTML response should never fail")
}

async fn chat_page(
    State(state): State<Arc<ServerState>>,
    Path(_token): Path<String>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> Response {
    if state.rate_limiter.lock().unwrap().is_blocked(addr.ip()) {
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    }
    // Served for any path token, valid or not: the page holds no secret, and
    // pairing is gated at the WebSocket (current token, or the resume secret).
    // A phone whose tab the OS discarded reloads its now-stale `/<token>` URL
    // and re-pairs from its stored resume secret -- which only works if the
    // page still loads here.
    html_response(CHAT_HTML)
}

async fn chat_ws(
    State(state): State<Arc<ServerState>>,
    Path(token): Path<String>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    ws: WebSocketUpgrade,
) -> Response {
    let ip = addr.ip();
    {
        let rl = state.rate_limiter.lock().unwrap();
        if rl.is_blocked(ip) {
            return StatusCode::TOO_MANY_REQUESTS.into_response();
        }
    }
    if token != state.current_token() {
        state.rate_limiter.lock().unwrap().record_failure(ip);
        return StatusCode::NOT_FOUND.into_response();
    }
    ws.on_upgrade(move |socket| handle_phone_socket(socket, state, false))
}

/// Reconnect endpoint authenticated by the in-memory resume secret rather
/// than the (by now rotated) QR token -- see the module doc. Same rate limit
/// and single-phone rules as the token path.
async fn resume_ws(
    State(state): State<Arc<ServerState>>,
    Path(secret): Path<String>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    ws: WebSocketUpgrade,
) -> Response {
    let ip = addr.ip();
    {
        let rl = state.rate_limiter.lock().unwrap();
        if rl.is_blocked(ip) {
            return StatusCode::TOO_MANY_REQUESTS.into_response();
        }
    }
    let matches = *state.resume_secret.lock().unwrap() == secret;
    if !matches {
        // A stale secret (rotated by "New code", or from before a restart).
        // Deliberately NOT counted toward the rate-limit block: the
        // secret is 256-bit (brute force is infeasible, so nothing to defend
        // against here), and the common case is a legit phone retrying an
        // invalidated secret every few seconds -- blocking its IP would then
        // 429 its own fresh QR-scan page load and prevent recovery.
        return StatusCode::NOT_FOUND.into_response();
    }
    ws.on_upgrade(move |socket| handle_phone_socket(socket, state, true))
}

async fn handle_phone_socket(mut socket: WebSocket, state: Arc<ServerState>, via_resume: bool) {
    let already_active = {
        let mut active = state.active.lock().unwrap();
        let was_active = *active;
        if !was_active {
            *active = true;
        }
        was_active
    };
    if already_active {
        let _ = socket
            .send(Message::Text(
                serde_json::json!({
                    "type": "error",
                    "code": "already_connected",
                    "message": "Another phone is already connected.",
                })
                .to_string(),
            ))
            .await;
        let _ = socket
            .send(Message::Close(Some(axum::extract::ws::CloseFrame {
                code: 1008,
                reason: "session already active".into(),
            })))
            .await;
        return;
    }

    // This connection means the phone is back, whether it's the very
    // first pairing or a reconnect within the grace window -- either way,
    // there's no more pending expiry to apply.
    *state.grace_deadline.lock().unwrap() = None;
    let is_first_pairing = {
        let mut paired = state.paired.lock().unwrap();
        let was_paired = *paired;
        *paired = true;
        !was_paired
    };
    if is_first_pairing && !via_resume {
        // Burn the QR the instant it's used, so a photo of it (or anyone
        // else who saw the screen) can't open a second, competing
        // session. A grace-window reconnect skips this: rotating here
        // too would invalidate the token this same tab already has,
        // forcing a fresh scan on the very next drop. A resume connect
        // authenticates with the secret, not the token, so it never rotates
        // the token either.
        state.rotate_token();
    }

    // Hand the phone the resume secret (minted on the first pairing, then the
    // same value every time) so it can reconnect after a drop, tab discard, or
    // long backgrounding without a fresh scan. In-memory only -- see the
    // module doc.
    let resume_secret = state.current_resume_secret();
    let _ = socket
        .send(Message::Text(
            serde_json::json!({"type": "resume", "secret": resume_secret}).to_string(),
        ))
        .await;

    state.broadcast_dashboard();

    let history: Vec<HistoryItem> = state.history.lock().unwrap().iter().cloned().collect();
    let _ = socket
        .send(Message::Text(
            serde_json::json!({"type": "history", "items": history}).to_string(),
        ))
        .await;

    loop {
        tokio::select! {
            incoming = socket.recv() => {
                match incoming {
                    Some(Ok(Message::Text(text))) => {
                        if !handle_incoming(&mut socket, &state, text).await {
                            break;
                        }
                    }
                    Some(Ok(Message::Binary(_))) => {
                        let _ = socket
                            .send(Message::Close(Some(axum::extract::ws::CloseFrame {
                                code: 1003,
                                reason: "binary frames unsupported".into(),
                            })))
                            .await;
                        break;
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(_)) => {} // ping/pong: handled automatically by axum
                    Some(Err(_)) => break,
                }
            }
            _ = state.kick_notify.notified() => {
                let _ = socket
                    .send(Message::Text(
                        serde_json::json!({
                            "type": "error",
                            "code": "new_code_generated",
                            "message": "Disconnected: a new code was generated.",
                        })
                        .to_string(),
                    ))
                    .await;
                let _ = socket
                    .send(Message::Close(Some(axum::extract::ws::CloseFrame {
                        code: 1001,
                        reason: "a new code was generated".into(),
                    })))
                    .await;
                break;
            }
        }
    }

    *state.active.lock().unwrap() = false;
    // A deliberate kick (via `PairingServer::regenerate_token`) already
    // moved the token on -- no grace window to reconnect with a token
    // that's intentionally dead.
    let was_kicked = std::mem::take(&mut *state.kicked.lock().unwrap());
    if !was_kicked {
        *state.grace_deadline.lock().unwrap() = Some(Instant::now() + RECONNECT_GRACE);
    }
    state.broadcast_dashboard();
}

/// Returns `false` if the connection should be closed.
async fn handle_incoming(socket: &mut WebSocket, state: &Arc<ServerState>, raw: String) -> bool {
    let payload: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(_) => return send_error(socket, "malformed", "Malformed message.").await,
    };
    if payload.get("type").and_then(|v| v.as_str()) == Some("clear") {
        state.history.lock().unwrap().clear();
        state.broadcast_dashboard();
        return true;
    }
    if payload.get("type").and_then(|v| v.as_str()) == Some("key") {
        let name = payload.get("key").and_then(|v| v.as_str()).unwrap_or_default();
        let key = match SpecialKey::from_name(name) {
            Some(k) => k,
            None => return send_error(socket, "unknown_key", "Unknown key.").await,
        };
        // Same blocking-thread dispatch as text (see below); keys aren't
        // added to the message history -- they're actions, not messages.
        let callback = state.on_message.clone();
        tokio::task::spawn_blocking(move || (callback)(InputEvent::Key(key)));
        return socket
            .send(Message::Text(
                serde_json::json!({"type": "ack", "key": name}).to_string(),
            ))
            .await
            .is_ok();
    }
    if payload.get("type").and_then(|v| v.as_str()) != Some("message") {
        return send_error(socket, "unknown_type", "Unknown message type.").await;
    }
    let text = match payload.get("text").and_then(|v| v.as_str()) {
        Some(t) if !t.is_empty() => t,
        _ => return send_error(socket, "empty_message", "Empty message.").await,
    };
    if text.chars().count() > MAX_MESSAGE_LEN {
        return send_error(socket, "too_long", "Message too long.").await;
    }
    let text = text.to_string();

    {
        let mut history = state.history.lock().unwrap();
        if history.len() == HISTORY_MAX {
            history.pop_front();
        }
        history.push_back(HistoryItem {
            text: text.clone(),
            ts: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
        });
    }

    let callback = state.on_message.clone();
    let text_for_callback = text.clone();
    // Injecting synthetic keystrokes is a blocking OS call; running it on
    // a dedicated blocking thread keeps it from stalling the async
    // runtime's worker threads.
    tokio::task::spawn_blocking(move || (callback)(InputEvent::Text(text_for_callback)));

    state.broadcast_dashboard();

    socket
        .send(Message::Text(
            serde_json::json!({"type": "ack", "text": text}).to_string(),
        ))
        .await
        .is_ok()
}

/// `code` is a stable machine-readable identifier the client can map to a
/// localized string in its own language (the phone's browser language,
/// independent of the desktop's); `message` is the English fallback for
/// any client that doesn't recognize the code.
async fn send_error(socket: &mut WebSocket, code: &str, message: &str) -> bool {
    socket
        .send(Message::Text(
            serde_json::json!({"type": "error", "code": code, "message": message}).to_string(),
        ))
        .await
        .is_ok()
}

fn build_router(state: Arc<ServerState>) -> Router {
    Router::new()
        .route("/:token", get(chat_page))
        .route("/:token/ws", get(chat_ws))
        .route("/r/:secret", get(chat_page))
        .route("/r/:secret/ws", get(resume_ws))
        .with_state(state)
}

/// Owns the LAN-facing listening socket, the shared state, and the
/// background token-expiry task.
pub struct PairingServer {
    state: Arc<ServerState>,
    lan_handle: Handle<SocketAddr>,
    lan_task: Mutex<Option<JoinHandle<()>>>,
    expiry_notify: Arc<Notify>,
    expiry_task: Mutex<Option<JoinHandle<()>>>,
}

impl PairingServer {
    /// Starts the listener. `on_message` is called (on a dedicated
    /// blocking thread, never the async runtime's own worker threads)
    /// every time a message arrives from the paired phone.
    pub async fn start(on_message: Arc<dyn Fn(InputEvent) + Send + Sync>) -> io::Result<Self> {
        let lan_ip = lan::detect_lan_ipv4().ok_or_else(|| {
            io::Error::other(
                "No LAN network interface found -- refusing to start a pairing \
                 server with no safe address to bind to.",
            )
        })?;
        log::info!("Phone-facing listener will bind to LAN address {lan_ip}");

        let lan_listener = std::net::TcpListener::bind((lan_ip, 0))?;
        lan_listener.set_nonblocking(true)?;
        let lan_addr = lan_listener.local_addr()?;

        let tls_config = tls::load_or_create_config().await?;

        let (dashboard_tx, _rx) = broadcast::channel(16);
        let state = Arc::new(ServerState {
            lan_ip,
            lan_port: lan_addr.port(),
            token: Mutex::new(new_token()),
            token_created_at: Mutex::new(Instant::now()),
            resume_secret: Mutex::new(new_token()),
            active: Mutex::new(false),
            paired: Mutex::new(false),
            grace_deadline: Mutex::new(None),
            kicked: Mutex::new(false),
            kick_notify: Notify::new(),
            history: Mutex::new(VecDeque::with_capacity(HISTORY_MAX)),
            rate_limiter: Mutex::new(RateLimiter::default()),
            dashboard_tx,
            on_message,
        });

        let lan_handle = Handle::new();

        let lan_task = {
            let router = build_router(state.clone());
            let handle = lan_handle.clone();
            let server = axum_server::tls_rustls::from_tcp_rustls(lan_listener, tls_config)?
                .handle(handle);
            tokio::spawn(async move {
                let _ = server
                    .serve(router.into_make_service_with_connect_info::<SocketAddr>())
                    .await;
            })
        };

        let expiry_notify = Arc::new(Notify::new());
        let expiry_task = {
            let state = state.clone();
            let notify = expiry_notify.clone();
            tokio::spawn(async move {
                loop {
                    tokio::select! {
                        // Polled fairly often (rather than e.g. every 30s)
                        // so RECONNECT_GRACE's expiry stays reasonably
                        // tight -- TOKEN_TTL is five minutes either way,
                        // so checking it this often costs nothing.
                        _ = tokio::time::sleep(Duration::from_secs(5)) => {
                            let should_rotate = {
                                let active = *state.active.lock().unwrap();
                                if active {
                                    false
                                } else if *state.paired.lock().unwrap() {
                                    // Paired, then dropped: only expire once
                                    // the reconnect grace window has passed.
                                    let deadline = *state.grace_deadline.lock().unwrap();
                                    matches!(deadline, Some(d) if Instant::now() >= d)
                                } else {
                                    // Never paired: expire an idle, unused QR.
                                    state.token_created_at.lock().unwrap().elapsed() >= TOKEN_TTL
                                }
                            };
                            if should_rotate {
                                *state.paired.lock().unwrap() = false;
                                *state.grace_deadline.lock().unwrap() = None;
                                state.rotate_token();
                                state.broadcast_dashboard();
                            }
                        }
                        _ = notify.notified() => break,
                    }
                }
            })
        };

        Ok(Self {
            state,
            lan_handle,
            lan_task: Mutex::new(Some(lan_task)),
            expiry_notify,
            expiry_task: Mutex::new(Some(expiry_task)),
        })
    }

    /// Shuts the listener and the background token-expiry task down.
    /// Takes `&self` (not owned `self`) so it can be called through an
    /// `Arc<PairingServer>` shared with tray-menu callbacks.
    pub async fn stop(&self) {
        self.lan_handle.shutdown();
        self.expiry_notify.notify_waiters();
        let lan_task = self.lan_task.lock().unwrap().take();
        if let Some(h) = lan_task {
            let _ = h.await;
        }
        let expiry_task = self.expiry_task.lock().unwrap().take();
        if let Some(h) = expiry_task {
            let _ = h.await;
        }
    }

    pub fn regenerate_token(&self) {
        self.state.force_new_code();
    }

    /// Wipes the in-memory message history (it was already never persisted
    /// to disk -- this just lets the user clear it from the current
    /// session without waiting for it to age out on its own). Doesn't
    /// touch the pairing token or connection state, unlike
    /// [`Self::regenerate_token`]. All three native windows expose a
    /// "Clear history" button that calls this.
    pub fn clear_history(&self) {
        self.state.history.lock().unwrap().clear();
        self.state.broadcast_dashboard();
    }

    /// The LAN-facing listener's own address. Not meant to be user-facing
    /// (there's no browsable page here without a valid pairing token) --
    /// its only purpose is letting a later launch's
    /// `instance::find_running_instance` check whether this instance is
    /// still alive and reachable.
    pub fn lan_socket_addr(&self) -> SocketAddr {
        SocketAddr::V4(SocketAddrV4::new(self.state.lan_ip, self.state.lan_port))
    }

    /// The live-updating status every native window renders from, for an
    /// in-process UI to pull without any network involved at all. Pair
    /// with [`Self::dashboard_snapshot`] for the state as of right now.
    pub fn subscribe_dashboard(&self) -> broadcast::Receiver<String> {
        self.state.dashboard_tx.subscribe()
    }

    pub fn dashboard_snapshot(&self) -> String {
        self.state.dashboard_snapshot_json()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_state() -> Arc<ServerState> {
        let (dashboard_tx, _rx) = broadcast::channel(1);
        Arc::new(ServerState {
            lan_ip: Ipv4Addr::LOCALHOST,
            lan_port: 0,
            token: Mutex::new(new_token()),
            token_created_at: Mutex::new(Instant::now()),
            resume_secret: Mutex::new(new_token()),
            active: Mutex::new(false),
            paired: Mutex::new(false),
            grace_deadline: Mutex::new(None),
            kicked: Mutex::new(false),
            kick_notify: Notify::new(),
            history: Mutex::new(VecDeque::new()),
            rate_limiter: Mutex::new(RateLimiter::default()),
            dashboard_tx,
            on_message: Arc::new(|_| {}),
        })
    }

    #[test]
    fn router_builds_without_route_conflict() {
        // axum's Router panics at construction on conflicting paths -- this
        // proves the static `/r/:secret/ws` resume route coexists with the
        // `/:token` param route rather than aborting at startup.
        let _ = build_router(test_state());
    }

    #[test]
    fn resume_secret_lifecycle() {
        let state = test_state();
        // Minted eagerly at startup (so the QR always carries it) and stable
        // across reconnects.
        let first = state.current_resume_secret();
        assert!(!first.is_empty());
        assert_eq!(first, state.current_resume_secret());
        // The QR URL embeds that same secret, at the resume endpoint's path.
        assert!(state.pairing_url().ends_with(&format!("/r/{first}")));
        // "New code" rotates it to a fresh value, so the old URL (or a photo
        // of the old QR) can no longer reconnect.
        state.force_new_code();
        assert_ne!(first, state.current_resume_secret());
    }
}
