//! Tray icon integration for macOS, the one platform without a native
//! window UI yet (see `window/mod.rs`'s module doc): `tray-icon` needs a
//! real native GUI event loop pumping on the process's actual main
//! thread on macOS specifically, so the Tokio runtime running the
//! pairing server instead runs on a background OS thread.

#[cfg(target_os = "macos")]
pub mod native;

/// Actions the tray menu can trigger.
pub struct TrayCallbacks {
    pub open_dashboard: Box<dyn Fn() + Send + Sync>,
    pub regenerate: Box<dyn Fn() + Send + Sync>,
    pub quit: Box<dyn Fn() + Send + Sync>,
}
