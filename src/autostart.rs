//! Launch-at-login toggle, backed by the `auto-launch` crate: on macOS a
//! per-user LaunchAgent plist under `~/Library/LaunchAgents/`, on Windows
//! the HKCU `...\CurrentVersion\Run` registry value. Both are per-user and
//! need no elevation, so toggling from the tray menu never prompts.
//!
//! ponytail: the login entry points at `current_exe()`. For a macOS `.app`
//! that's the binary inside `Contents/MacOS`, which launches fine on its
//! own; if the bundle is later moved, re-toggle to refresh the recorded
//! path. Upgrade path if that bites: resolve the enclosing `.app` and
//! register that instead.

use auto_launch::{AutoLaunch, AutoLaunchBuilder};

const APP_NAME: &str = "PhoneInputConnect";

fn build() -> Option<AutoLaunch> {
    let exe = std::env::current_exe().ok()?;
    // `MacOSLaunchMode::LaunchAgent` is auto-launch's default, so it needs
    // no explicit mode call here.
    AutoLaunchBuilder::new()
        .set_app_name(APP_NAME)
        .set_app_path(exe.to_str()?)
        .build()
        .ok()
}

/// Whether launch-at-login is currently on. Any lookup error reads as off.
pub fn is_enabled() -> bool {
    build().and_then(|a| a.is_enabled().ok()).unwrap_or(false)
}

/// Turns launch-at-login on or off, then reports the state that actually
/// took effect (re-read rather than assumed, so a failed write is reflected
/// back to the caller's checkmark instead of silently diverging).
pub fn set_enabled(enabled: bool) -> bool {
    let Some(auto) = build() else {
        log::warn!("launch-at-login: could not resolve this executable's path");
        return false;
    };
    let res = if enabled { auto.enable() } else { auto.disable() };
    if let Err(err) = res {
        log::warn!(
            "launch-at-login: failed to {}: {err}",
            if enabled { "enable" } else { "disable" }
        );
    }
    auto.is_enabled().unwrap_or(false)
}
