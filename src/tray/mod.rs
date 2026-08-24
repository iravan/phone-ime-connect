//! Tray icon integration for platforms without a native window UI yet
//! (see `window.rs`'s module doc): `tray-icon` needs a real native GUI
//! event loop pumping on its owning thread (a Win32 message loop or a
//! Cocoa run loop), which on macOS specifically must be the process's
//! actual main thread. So on those platforms the GUI event loop *is* the
//! main thread, and the Tokio runtime running the pairing server instead
//! runs on a background OS thread.

#[cfg(not(target_os = "linux"))]
pub mod native;

/// Actions the tray menu can trigger.
pub struct TrayCallbacks {
    pub open_dashboard: Box<dyn Fn() + Send + Sync>,
    pub regenerate: Box<dyn Fn() + Send + Sync>,
    pub quit: Box<dyn Fn() + Send + Sync>,
}
