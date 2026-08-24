//! Tray icon integration for platforms without a native window UI yet
//! (see `window.rs`'s module doc): `tray-icon` needs a real native GUI
//! event loop pumping on its owning thread (a Win32 message loop or a
//! Cocoa run loop), which on macOS specifically must be the process's
//! actual main thread. So on those platforms the GUI event loop *is* the
//! main thread, and the Tokio runtime running the pairing server instead
//! runs on a background OS thread.

#[cfg(not(target_os = "linux"))]
pub mod native;

// The window's dashboard content: native AppKit widgets on macOS, an
// embedded webview elsewhere (Windows). Both expose the same `Content` API
// that `native.rs` drives.
#[cfg(target_os = "macos")]
mod appkit_dashboard;
#[cfg(all(not(target_os = "linux"), not(target_os = "macos")))]
mod webview_dashboard;
