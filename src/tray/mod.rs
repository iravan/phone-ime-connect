//! Tray icon integration. The two platform backends genuinely differ in
//! how they need to be driven, not just in which crate they call:
//!
//! - **Linux** (`linux.rs`): `ksni` implements the StatusNotifierItem
//!   D-Bus protocol in pure Rust and runs as an ordinary task on our
//!   existing Tokio runtime -- no separate event loop needed.
//! - **Windows/macOS** (`native.rs`): `tray-icon` needs a real native GUI
//!   event loop pumping on its owning thread (a Win32 message loop or a
//!   Cocoa run loop), which on macOS specifically must be the process's
//!   actual main thread. So on those platforms the GUI event loop *is*
//!   the main thread, and the Tokio runtime running the pairing server
//!   instead runs on a background OS thread.
//!
//! `main.rs` picks between them with `#[cfg(target_os = "linux")]` at the
//! top level rather than hiding the difference behind a fake shared
//! abstraction, since the actual control flow (who owns the main thread)
//! is genuinely different.

#[cfg(target_os = "linux")]
pub mod linux;

#[cfg(not(target_os = "linux"))]
pub mod native;

/// Actions the tray menu can trigger, shared by both backends.
pub struct TrayCallbacks {
    pub open_dashboard: Box<dyn Fn() + Send + Sync>,
    pub regenerate: Box<dyn Fn() + Send + Sync>,
    pub quit: Box<dyn Fn() + Send + Sync>,
}
