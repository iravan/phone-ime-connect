//! Lets a second launch of PhoneInputConnect (e.g. clicking its icon again in an
//! app launcher) just leave whichever instance is already running alone,
//! instead of starting a competing second server.
//!
//! Tracked with a single small file recording the last-started instance's
//! LAN listener address (`host:port`, from `PairingServer::lan_socket_addr`),
//! in the same per-user app data directory as the cached TLS certificate
//! (`tls.rs`). A stale file left behind by a crashed instance is harmless:
//! its recorded address is checked for a live listener before being
//! trusted, and gets silently overwritten either way once this instance
//! actually starts.

use std::io;
use std::path::PathBuf;
use std::time::Duration;

use directories::ProjectDirs;

fn instance_file_path() -> io::Result<PathBuf> {
    let dirs = ProjectDirs::from("", "", "phone-input-connect")
        .ok_or_else(|| io::Error::other("could not determine a per-user app data directory"))?;
    let dir = dirs.data_local_dir().to_path_buf();
    std::fs::create_dir_all(&dir)?;
    Ok(dir.join("instance.addr"))
}

/// True if another PhoneInputConnect instance is already running and its
/// recorded LAN listener address is actually reachable -- the caller
/// should leave it alone and exit instead of starting a competing server.
pub async fn find_running_instance() -> bool {
    let Ok(path) = instance_file_path() else {
        return false;
    };
    let Ok(addr) = std::fs::read_to_string(&path) else {
        return false;
    };
    let addr = addr.trim();
    if addr.is_empty() {
        return false;
    }
    let reachable = tokio::time::timeout(
        Duration::from_millis(500),
        tokio::net::TcpStream::connect(addr),
    )
    .await;
    matches!(reachable, Ok(Ok(_)))
}

/// Records this instance's LAN listener address for a later launch's
/// `find_running_instance` to discover. Best-effort: if it can't be
/// written, a later launch just won't find this instance and will start
/// its own instead, which is a reasonable fallback either way.
pub fn record_running_instance(lan_addr: &str) {
    if let Ok(path) = instance_file_path() {
        let _ = std::fs::write(path, lan_addr);
    }
}
