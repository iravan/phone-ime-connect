//! Tray icon integration for macOS, the one platform without a native
//! window UI yet (see `window/mod.rs`'s module doc): `tray-icon` needs a
//! real native GUI event loop pumping on the process's actual main
//! thread on macOS specifically, so the Tokio runtime running the
//! pairing server instead runs on a background OS thread.

#[cfg(target_os = "macos")]
pub mod native;

// The window's dashboard content, rendered with native AppKit widgets.
#[cfg(target_os = "macos")]
mod appkit_dashboard;
