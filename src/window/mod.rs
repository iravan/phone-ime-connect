//! The primary UI: a native window built on each platform's own toolkit
//! (GTK4 on Linux, `native-windows-gui`/Win32 on Windows), rather than a
//! webview or a browser tab -- see each submodule's doc for specifics.
//! macOS has its own native equivalent too, drawn with AppKit widgets in
//! a tray/menu-bar app instead of a plain window (`tray/native.rs`,
//! `tray/appkit_dashboard.rs`).

#[cfg(target_os = "linux")]
pub mod linux;

#[cfg(target_os = "windows")]
pub mod windows;
