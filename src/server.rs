//! The local HTTPS+WSS server a phone pairs with, plus a desktop-local
//! "dashboard" endpoint used to show the QR code/status/history without
//! any GUI toolkit dependency -- `main.rs` just opens the dashboard URL in
//! the default browser.
//!
//! Security model (see README's "Security" section for the full
//! writeup):
//!
//! - The phone-facing side is bound only to this machine's detected LAN
//!   address, never `0.0.0.0` -- refuses to start at all if no such
//!   address can be found.
//! - TLS (self-signed, see `tls.rs`) encrypts the LAN hop.
//! - A single 256-bit random token, embedded in the URL path, gates both
//!   the HTML page and the WebSocket upgrade. It is unguessable and
//!   immediately rotated the first time a phone successfully connects, so
//!   a QR code, once used, cannot be reused to open a second, competing
//!   session. It also expires on its own (`TOKEN_TTL`) if nothing ever
//!   connects. Once a phone has paired, a *disconnect* doesn't rotate the
//!   token immediately either -- it starts a short `RECONNECT_GRACE`
//!   window where the same token still works, so a phone browser tab
//!   surviving a brief network drop or backgrounding can reconnect
//!   without a fresh QR scan. The token only rotates for real once that
//!   window elapses with no reconnect.
//! - Only one phone may be connected at a time.
//! - Failed-token requests are rate-limited per source IP (defense in
//!   depth against scanning/log-spam, not against brute-forcing 256 bits,
//!   which is already computationally infeasible).
//! - The dashboard endpoints need no token, but are reachable only
//!   because the server also binds a second listening socket on
//!   127.0.0.1 -- and every dashboard request is additionally checked
//!   against the connecting socket's own remote address, rejecting
//!   anything that isn't loopback even if a LAN attacker somehow got a
//!   packet routed there.
//! - Nothing is persisted to disk: chat history is an in-memory ring
//!   buffer capped at `HISTORY_MAX` and vanishes when the server stops.

use std::collections::{HashMap, VecDeque};
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
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

use crate::{lan, qr, tls};

const MAX_MESSAGE_LEN: usize = 2000;
const HISTORY_MAX: usize = 10;
const TOKEN_TTL: Duration = Duration::from_secs(5 * 60);
const RECONNECT_GRACE: Duration = Duration::from_secs(45);
const RATE_LIMIT_MAX_ATTEMPTS: usize = 20;
const RATE_LIMIT_WINDOW: Duration = Duration::from_secs(60);
const RATE_LIMIT_BLOCK: Duration = Duration::from_secs(5 * 60);

const CHAT_HTML: &str = include_str!("webapp/chat.html");
const DASHBOARD_HTML: &str = include_str!("webapp/dashboard.html");

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
    dash_port: u16,
    token: Mutex<String>,
    token_created_at: Mutex<Instant>,
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
    on_message: Arc<dyn Fn(String) + Send + Sync>,
}

impl ServerState {
    fn current_token(&self) -> String {
        self.token.lock().unwrap().clone()
    }

    fn rotate_token(&self) {
        *self.token.lock().unwrap() = new_token();
        *self.token_created_at.lock().unwrap() = Instant::now();
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
        self.broadcast_dashboard();
    }

    fn pairing_url(&self) -> String {
        format!(
            "https://{}:{}/{}",
            self.lan_ip,
            self.lan_port,
            self.current_token()
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
    Path(token): Path<String>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
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
    ws.on_upgrade(move |socket| handle_phone_socket(socket, state))
}

async fn handle_phone_socket(mut socket: WebSocket, state: Arc<ServerState>) {
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
    if is_first_pairing {
        // Burn the QR the instant it's used, so a photo of it (or anyone
        // else who saw the screen) can't open a second, competing
        // session. A grace-window reconnect skips this: rotating here
        // too would invalidate the token this same tab already has,
        // forcing a fresh scan on the very next drop.
        state.rotate_token();
    }
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
        Err(_) => return send_error(socket, "Malformed message.").await,
    };
    if payload.get("type").and_then(|v| v.as_str()) != Some("message") {
        return send_error(socket, "Unknown message type.").await;
    }
    let text = match payload.get("text").and_then(|v| v.as_str()) {
        Some(t) if !t.is_empty() => t,
        _ => return send_error(socket, "Empty message.").await,
    };
    if text.chars().count() > MAX_MESSAGE_LEN {
        return send_error(socket, "Message too long.").await;
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
    tokio::task::spawn_blocking(move || (callback)(text_for_callback));

    state.broadcast_dashboard();

    socket
        .send(Message::Text(
            serde_json::json!({"type": "ack", "text": text}).to_string(),
        ))
        .await
        .is_ok()
}

async fn send_error(socket: &mut WebSocket, message: &str) -> bool {
    socket
        .send(Message::Text(
            serde_json::json!({"type": "error", "message": message}).to_string(),
        ))
        .await
        .is_ok()
}

async fn dashboard_page(ConnectInfo(addr): ConnectInfo<SocketAddr>) -> Response {
    if !addr.ip().is_loopback() {
        return StatusCode::NOT_FOUND.into_response();
    }
    html_response(DASHBOARD_HTML)
}

async fn dashboard_ws(
    State(state): State<Arc<ServerState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    ws: WebSocketUpgrade,
) -> Response {
    if !addr.ip().is_loopback() {
        return StatusCode::NOT_FOUND.into_response();
    }
    ws.on_upgrade(move |socket| handle_dashboard_socket(socket, state))
}

async fn handle_dashboard_socket(mut socket: WebSocket, state: Arc<ServerState>) {
    let mut rx = state.dashboard_tx.subscribe();
    if socket
        .send(Message::Text(state.dashboard_snapshot_json()))
        .await
        .is_err()
    {
        return;
    }
    loop {
        tokio::select! {
            incoming = socket.recv() => {
                match incoming {
                    Some(Ok(Message::Text(text))) => {
                        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
                            if v.get("type").and_then(|t| t.as_str()) == Some("regenerate") {
                                state.force_new_code();
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                    _ => {}
                }
            }
            update = rx.recv() => {
                match update {
                    Ok(payload) => {
                        if socket.send(Message::Text(payload)).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                }
            }
        }
    }
}

fn build_router(state: Arc<ServerState>) -> Router {
    Router::new()
        .route("/dashboard", get(dashboard_page))
        .route("/dashboard/ws", get(dashboard_ws))
        .route("/:token", get(chat_page))
        .route("/:token/ws", get(chat_ws))
        .with_state(state)
}

/// Owns the two listening sockets (LAN-facing and loopback-only
/// dashboard), the shared state, and the background token-expiry task.
pub struct PairingServer {
    state: Arc<ServerState>,
    lan_handle: Handle<SocketAddr>,
    dash_handle: Handle<SocketAddr>,
    lan_task: Mutex<Option<JoinHandle<()>>>,
    dash_task: Mutex<Option<JoinHandle<()>>>,
    expiry_notify: Arc<Notify>,
    expiry_task: Mutex<Option<JoinHandle<()>>>,
}

impl PairingServer {
    /// Starts both listeners. `on_message` is called (on a dedicated
    /// blocking thread, never the async runtime's own worker threads)
    /// every time a message arrives from the paired phone.
    pub async fn start(on_message: Arc<dyn Fn(String) + Send + Sync>) -> io::Result<Self> {
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
        let dash_listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        dash_listener.set_nonblocking(true)?;
        let dash_addr = dash_listener.local_addr()?;

        let tls_config = tls::load_or_create_config().await?;

        let (dashboard_tx, _rx) = broadcast::channel(16);
        let state = Arc::new(ServerState {
            lan_ip,
            lan_port: lan_addr.port(),
            dash_port: dash_addr.port(),
            token: Mutex::new(new_token()),
            token_created_at: Mutex::new(Instant::now()),
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
        let dash_handle = Handle::new();

        let lan_task = {
            let router = build_router(state.clone());
            let handle = lan_handle.clone();
            let server =
                axum_server::tls_rustls::from_tcp_rustls(lan_listener, tls_config.clone())?
                    .handle(handle);
            tokio::spawn(async move {
                let _ = server
                    .serve(router.into_make_service_with_connect_info::<SocketAddr>())
                    .await;
            })
        };

        let dash_task = {
            let router = build_router(state.clone());
            let handle = dash_handle.clone();
            let server =
                axum_server::tls_rustls::from_tcp_rustls(dash_listener, tls_config)?.handle(handle);
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
            dash_handle,
            lan_task: Mutex::new(Some(lan_task)),
            dash_task: Mutex::new(Some(dash_task)),
            expiry_notify,
            expiry_task: Mutex::new(Some(expiry_task)),
        })
    }

    /// Shuts both listeners and the background token-expiry task down.
    /// Takes `&self` (not owned `self`) so it can be called through an
    /// `Arc<PairingServer>` shared with tray-menu callbacks.
    pub async fn stop(&self) {
        self.lan_handle.shutdown();
        self.dash_handle.shutdown();
        self.expiry_notify.notify_waiters();
        let lan_task = self.lan_task.lock().unwrap().take();
        if let Some(h) = lan_task {
            let _ = h.await;
        }
        let dash_task = self.dash_task.lock().unwrap().take();
        if let Some(h) = dash_task {
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

    pub fn dashboard_url(&self) -> String {
        format!("https://127.0.0.1:{}/dashboard", self.state.dash_port)
    }

    /// The same live-updating status the browser-based dashboard's
    /// WebSocket streams, for an in-process UI (e.g. the native window on
    /// Linux) to render without going through HTTP at all. Pair with
    /// [`Self::dashboard_snapshot`] for the state as of right now.
    pub fn subscribe_dashboard(&self) -> broadcast::Receiver<String> {
        self.state.dashboard_tx.subscribe()
    }

    pub fn dashboard_snapshot(&self) -> String {
        self.state.dashboard_snapshot_json()
    }
}
