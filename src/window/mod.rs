//! The primary UI: a native window built on each platform's own toolkit
//! (GTK4 on Linux, `native-windows-gui`/Win32 on Windows), rather than a
//! webview or a browser tab -- see each submodule's doc for specifics.
//! macOS doesn't have one yet and still falls back to a tray icon plus a
//! browser tab (`tray/native.rs`); giving it an equivalent native window
//! is follow-up work.

#[cfg(target_os = "linux")]
pub mod linux;

#[cfg(target_os = "windows")]
pub mod windows;
