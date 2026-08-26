//! macOS UI: a winit window plus a menu-bar tray icon (`tray-icon`), which
//! needs a real native GUI event loop pumping on the process's actual main
//! thread on macOS specifically, so the Tokio runtime running the
//! pairing server instead runs on a background OS thread. (Linux/Windows
//! have their own native windows and, on Windows, a native tray -- see
//! `window/mod.rs`.)

#[cfg(target_os = "macos")]
pub mod native;

// The window's dashboard content, rendered with native AppKit widgets.
#[cfg(target_os = "macos")]
mod appkit_dashboard;
