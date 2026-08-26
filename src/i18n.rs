//! Minimal translation support for the native windows (Linux/Windows/
//! macOS): English and Traditional Chinese, picked once at startup from
//! the OS's own UI language. Only these two are supported so far -- an
//! unrecognized locale falls back to English rather than guessing.
//!
//! The phone-facing web page (`webapp/chat.html`) does its own,
//! independent translation from the *phone's* browser language
//! (`navigator.language`) instead of anything in this module: the phone
//! and desktop are different devices, so there's no reason their UI
//! languages should be tied together.

use std::sync::OnceLock;

// Every field here is read by *some* platform's window module, but no
// single platform build uses all of them (e.g. `history_empty` and
// `copy_to_clipboard_tooltip` are only read by the Linux GTK window's
// per-row history list). That makes an any-platform build legitimately
// warn about the fields the *other* platforms use -- not actual dead
// code, just cross-platform sharing of one struct.
#[allow(dead_code)]
pub struct Strings {
    pub connecting: &'static str,
    pub hint_scan: &'static str,
    pub button_new_code: &'static str,
    pub button_clear_history: &'static str,
    pub button_copy_last_message: &'static str,
    pub accessibility_needed: &'static str,
    pub button_open_accessibility: &'static str,
    pub status_connected: &'static str,
    pub hint_connected: &'static str,
    pub status_reconnecting: &'static str,
    pub hint_reconnecting: &'static str,
    pub status_waiting: &'static str,
    pub history_header: &'static str,
    pub history_empty: &'static str,
    pub copy_to_clipboard_tooltip: &'static str,
    pub menu_show: &'static str,
    pub menu_launch_at_login: &'static str,
    pub menu_quit: &'static str,
}

const EN: Strings = Strings {
    connecting: "Connecting…",
    hint_scan: "Scan with a phone on the same network.",
    button_new_code: "New code",
    button_clear_history: "Clear history",
    button_copy_last_message: "Copy last message",
    accessibility_needed: "Typing is off — grant Accessibility so messages can be typed into \
                           the focused app.",
    button_open_accessibility: "Open Accessibility settings",
    status_connected: "Phone connected",
    hint_connected: "Messages you send from the phone appear below.",
    status_reconnecting: "Phone disconnected — waiting to reconnect…",
    hint_reconnecting: "The same code still works for a bit in case it reconnects on its \
                         own; scan again if it doesn't.",
    status_waiting: "Waiting for a phone to scan the code below",
    history_header: "Received messages",
    history_empty: "No messages yet",
    copy_to_clipboard_tooltip: "Copy to clipboard",
    menu_show: "Show PhoneInputConnect",
    menu_launch_at_login: "Launch at login",
    menu_quit: "Quit",
};

const ZH_HANT: Strings = Strings {
    connecting: "連線中…",
    hint_scan: "請使用同一網路下的手機掃描。",
    button_new_code: "產生新代碼",
    button_clear_history: "清除紀錄",
    button_copy_last_message: "複製最後訊息",
    accessibility_needed: "尚未開啟輸入功能 — 請授予「輔助使用」權限，訊息才能輸入到目前的 App。",
    button_open_accessibility: "開啟輔助使用設定",
    status_connected: "手機已連線",
    hint_connected: "手機傳來的訊息會顯示在下方。",
    status_reconnecting: "手機已中斷連線 — 正在等待重新連線…",
    hint_reconnecting: "同一組代碼仍可短暫使用，以便自動重新連線；若無法自動重連，請重新掃描。",
    status_waiting: "請使用手機掃描下方的代碼",
    history_header: "已接收的訊息",
    history_empty: "尚無訊息",
    copy_to_clipboard_tooltip: "複製到剪貼簿",
    menu_show: "顯示 PhoneInputConnect",
    menu_launch_at_login: "開機時啟動",
    menu_quit: "結束",
};

/// The OS UI language can't change over the life of the process, and
/// every window-construction site needs this, so it's detected once and
/// cached rather than re-resolved per call.
pub fn strings() -> &'static Strings {
    static CACHE: OnceLock<&'static Strings> = OnceLock::new();
    *CACHE.get_or_init(|| if is_traditional_chinese() { &ZH_HANT } else { &EN })
}

/// Traditional Chinese locales: Taiwan, Hong Kong, Macau, or an explicit
/// "Hant" script subtag. Simplified Chinese (`zh-CN`/`zh-Hans`/etc.) and
/// everything else falls back to English -- only these two languages are
/// supported so far.
fn is_traditional_chinese() -> bool {
    let Some(locale) = sys_locale::get_locale() else {
        return false;
    };
    let locale = locale.to_ascii_lowercase();
    locale.starts_with("zh-tw")
        || locale.starts_with("zh_tw")
        || locale.starts_with("zh-hk")
        || locale.starts_with("zh_hk")
        || locale.starts_with("zh-mo")
        || locale.starts_with("zh_mo")
        || locale.contains("hant")
}
