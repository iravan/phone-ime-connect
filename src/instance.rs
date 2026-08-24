//! Lets a second launch of PhoneChat (e.g. clicking its icon again in an
//! app launcher) just reopen the dashboard of whichever instance is
//! already running, instead of starting a competing second server --
//! useful as a way back to the QR code that doesn't depend on a tray icon
//! being available at all (stock GNOME Shell has none -- see the README's
//! "Platform notes").
//!
//! Tracked with a single small file recording the last-started instance's
//! dashboard URL, in the same per-user app data directory as the cached
//! TLS certificate (`tls.rs`). A stale file left behind by a crashed
//! instance is harmless: its recorded port is checked for a live listener
//! before being trusted, and gets silently overwritten either way once
//! this instance actually starts.

use std::io;
use std::path::PathBuf;
use std::time::Duration;

use directories::ProjectDirs;

fn instance_file_path() -> io::Result<PathBuf> {
    let dirs = ProjectDirs::from("", "", "phonechat")
        .ok_or_else(|| io::Error::other("could not determine a per-user app data directory"))?;
    let dir = dirs.data_local_dir().to_path_buf();
    std::fs::create_dir_all(&dir)?;
    Ok(dir.join("instance.url"))
}

/// `https://127.0.0.1:PORT/dashboard` -> `127.0.0.1:PORT`. Only ever needs
/// to parse a URL this process itself generated (`dashboard_url` in
/// `server.rs`), so a small manual split is enough -- no URL-parsing
/// dependency needed just for this.
fn host_port(dashboard_url: &str) -> Option<&str> {
    dashboard_url.strip_prefix("https://")?.split('/').next()
}

/// If another PhoneChat instance is already running, returns its
/// dashboard URL so the caller can just open a browser to it and exit,
/// instead of starting a second server.
pub async fn find_running_instance() -> Option<String> {
    let path = instance_file_path().ok()?;
    let url = std::fs::read_to_string(&path).ok()?;
    let url = url.trim();
    if url.is_empty() {
        return None;
    }
    let addr = host_port(url)?;
    let reachable = tokio::time::timeout(
        Duration::from_millis(500),
        tokio::net::TcpStream::connect(addr),
    )
    .await;
    matches!(reachable, Ok(Ok(_))).then(|| url.to_string())
}

/// Records this instance's dashboard URL for a later launch's
/// `find_running_instance` to discover. Best-effort: if it can't be
/// written, a later launch just won't find this instance and will start
/// its own instead, which is a reasonable fallback either way.
pub fn record_running_instance(dashboard_url: &str) {
    if let Ok(path) = instance_file_path() {
        let _ = std::fs::write(path, dashboard_url);
    }
}
