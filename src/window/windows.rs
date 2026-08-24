//! The primary UI on Windows: a native `native-windows-gui` (Win32)
//! window showing the QR code, connection status, and message history,
//! with a "New code" button -- the Windows equivalent of `window/linux.rs`'s
//! GTK4 window (see that module's doc for the overall intent; the same
//! rationale applies here).
//!
//! Per-row hover-to-copy buttons (like the Linux window has) aren't
//! implemented here: nwg's plain controls don't support per-item
//! interactive widgets without an owner-drawn ListView, which is a much
//! larger undertaking. Instead, history is a read-only multi-line text
//! box, with a single "Copy last message" button as the clipboard
//! convenience.
//!
//! Closing this window quits the whole application, including the
//! pairing server, same as on Linux.
//!
//! Unlike the Linux window (built and smoke-tested on a real machine),
//! this module is unverified: written from `native-windows-gui`'s
//! documented API with no Windows machine available to actually compile
//! or run it against.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use base64::Engine;
use native_windows_gui as nwg;
use tokio::sync::broadcast;

use crate::server::PairingServer;

struct App {
    window: nwg::Window,
    status_label: nwg::Label,
    qr_frame: nwg::ImageFrame,
    qr_bitmap: RefCell<Option<nwg::Bitmap>>,
    hint_label: nwg::Label,
    regenerate_button: nwg::Button,
    copy_last_button: nwg::Button,
    clear_history_button: nwg::Button,
    history_box: nwg::TextBox,
    notice: nwg::Notice,
    layout: nwg::GridLayout,
    server: Arc<PairingServer>,
    // Written from the Tokio runtime's worker threads (see
    // `forward_updates`), read only from the UI thread in response to
    // `notice` firing -- the one piece of this struct that's ever
    // touched off the UI thread, so it alone needs real synchronization
    // rather than a `RefCell`.
    latest_snapshot: Arc<Mutex<String>>,
}

/// Runs the Win32 message loop on the calling thread until the window is
/// closed.
pub fn run(runtime: Arc<tokio::runtime::Runtime>, server: Arc<PairingServer>) {
    let s = crate::i18n::strings();

    nwg::init().expect("failed to initialize native-windows-gui");
    let _ = nwg::Font::set_global_family("Segoe UI");

    let mut app = App {
        window: Default::default(),
        status_label: Default::default(),
        qr_frame: Default::default(),
        qr_bitmap: RefCell::new(None),
        hint_label: Default::default(),
        regenerate_button: Default::default(),
        copy_last_button: Default::default(),
        clear_history_button: Default::default(),
        history_box: Default::default(),
        notice: Default::default(),
        layout: Default::default(),
        server: server.clone(),
        latest_snapshot: Arc::new(Mutex::new(String::new())),
    };

    nwg::Window::builder()
        .flags(nwg::WindowFlags::WINDOW | nwg::WindowFlags::VISIBLE)
        // Tall enough that `qr_frame`'s row-span below actually gets
        // enough of the grid's vertical space -- see the comment there.
        .size((360, 870))
        .title("PhoneInputConnect")
        .build(&mut app.window)
        .expect("failed to build the main window");

    nwg::Label::builder()
        .text(s.connecting)
        .parent(&app.window)
        .build(&mut app.status_label)
        .expect("failed to build the status label");

    nwg::ImageFrame::builder()
        // `qr::build_data_uri`'s output isn't scaled to the frame --
        // for this app's pairing URLs (fixed-length 256-bit token, so a
        // fixed QR version) it renders at ~246x246px. `ImageFrame` uses
        // SS_CENTERIMAGE, which draws the bitmap at its native size
        // centered in the control and *clips* it if the control ends
        // up smaller, rather than scaling it down to fit. This builder
        // size is only GridLayout's starting hint (it stretches the
        // control to its actual grid cell below), so what matters most
        // is that cell being comfortably bigger than ~246px -- this is
        // just kept in the same ballpark for a sane initial paint.
        .size((300, 300))
        .parent(&app.window)
        .build(&mut app.qr_frame)
        .expect("failed to build the QR image frame");

    nwg::Label::builder()
        .text(s.hint_scan)
        .parent(&app.window)
        .build(&mut app.hint_label)
        .expect("failed to build the hint label");

    nwg::Button::builder()
        .text(s.button_new_code)
        .parent(&app.window)
        .build(&mut app.regenerate_button)
        .expect("failed to build the regenerate button");

    nwg::Button::builder()
        .text(s.button_copy_last_message)
        .parent(&app.window)
        .build(&mut app.copy_last_button)
        .expect("failed to build the copy-last-message button");

    nwg::Button::builder()
        .text(s.button_clear_history)
        .parent(&app.window)
        .build(&mut app.clear_history_button)
        .expect("failed to build the clear-history button");

    nwg::TextBox::builder()
        .readonly(true)
        .parent(&app.window)
        .build(&mut app.history_box)
        .expect("failed to build the history text box");

    nwg::Notice::builder()
        .parent(&app.window)
        .build(&mut app.notice)
        .expect("failed to build the update notice");

    nwg::GridLayout::builder()
        .parent(&app.window)
        .max_column(Some(2))
        .child_item(nwg::GridLayoutItem::new(&app.status_label, 0, 0, 2, 1))
        // Given an 8-row span (out of 18 total rows) in an 870px-tall
        // window, this cell comes out well over 300px tall -- comfortable
        // room for the ~246px QR bitmap `set_qr_image` draws into it
        // (see the `ImageFrame` comment above for why it needs to be
        // this generous: the control clips an oversized bitmap instead
        // of scaling it down).
        .child_item(nwg::GridLayoutItem::new(&app.qr_frame, 0, 1, 2, 8))
        .child_item(nwg::GridLayoutItem::new(&app.hint_label, 0, 9, 2, 1))
        .child(0, 10, &app.regenerate_button)
        .child(1, 10, &app.copy_last_button)
        .child_item(nwg::GridLayoutItem::new(&app.clear_history_button, 0, 11, 2, 1))
        .child_item(nwg::GridLayoutItem::new(&app.history_box, 0, 12, 2, 6))
        .build(&app.layout)
        .expect("failed to build the window layout");

    let app = Rc::new(app);

    // Bridge the server's dashboard broadcast (the same live-status
    // stream the Linux window also renders from) onto the Win32 message
    // loop: the background task just overwrites
    // the shared latest-snapshot string and pokes `notice` to wake the
    // UI thread, which re-reads it from the `OnNotice` handler below.
    {
        let latest = app.latest_snapshot.clone();
        *latest.lock().unwrap() = server.dashboard_snapshot();
        let mut updates = server.subscribe_dashboard();
        let notice_sender = app.notice.sender();
        runtime.spawn(async move {
            loop {
                match updates.recv().await {
                    Ok(json) => {
                        *latest.lock().unwrap() = json;
                        notice_sender.notice();
                    }
                    // A slow-to-render UI missing a few intermediate
                    // snapshots is harmless -- the next one received
                    // still reflects current state.
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });
    }

    // Paint the current (already-fetched-above) state right away, via a
    // *direct* call rather than through `notice()`: `Notice::sender().
    // notice()` sends via `SendNotifyMessageW`, which for a same-thread
    // target -- this is still the UI thread, before `dispatch_thread_events`
    // below ever starts pumping messages -- dispatches synchronously
    // through the window procedure immediately, not queued for later. But
    // `full_bind_event_handler` (which is what actually routes a dispatched
    // notice to `render_snapshot` via the `OnNotice` match arm) hasn't run
    // yet at this point in the function, so that first notice had nowhere
    // to go and was silently dropped -- the window was then stuck showing
    // its build-time placeholder text forever, since nothing else ever
    // triggers a first render. Calling `render_snapshot` directly instead
    // sidesteps the notice/message-loop machinery entirely for this one
    // render, which needs to happen unconditionally rather than depend on
    // message-loop timing. Every later update still goes through `notice()`
    // above, correctly: those really do need to cross from the Tokio
    // runtime's worker thread onto this one, which `notice()` marshals via
    // the window's message queue (a genuine cross-thread `SendNotifyMessageW`
    // there, unlike this same-thread call).
    render_snapshot(&app);

    let weak_app = Rc::downgrade(&app);
    let handler =
        nwg::full_bind_event_handler(&app.window.handle, move |evt, _evt_data, handle| {
            let Some(app) = weak_app.upgrade() else {
                return;
            };
            match evt {
                nwg::Event::OnWindowClose => {
                    if &handle == &app.window {
                        nwg::stop_thread_dispatch();
                    }
                }
                nwg::Event::OnButtonClick => {
                    if &handle == &app.regenerate_button {
                        app.server.regenerate_token();
                    } else if &handle == &app.copy_last_button {
                        copy_last_message(&app);
                    } else if &handle == &app.clear_history_button {
                        app.server.clear_history();
                    }
                }
                nwg::Event::OnNotice => {
                    if &handle == &app.notice {
                        render_snapshot(&app);
                    }
                }
                _ => {}
            }
        });

    nwg::dispatch_thread_events();
    nwg::unbind_event_handler(&handler);
}

fn render_snapshot(app: &App) {
    let s = crate::i18n::strings();
    let json = app.latest_snapshot.lock().unwrap().clone();
    let snapshot: serde_json::Value = match serde_json::from_str(&json) {
        Ok(v) => v,
        Err(_) => return,
    };

    let connected = snapshot
        .get("connected")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let reconnecting = snapshot
        .get("reconnecting")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    if connected {
        app.status_label.set_text(s.status_connected);
        app.hint_label.set_text(s.hint_connected);
        app.qr_frame.set_visible(false);
    } else {
        app.qr_frame.set_visible(true);
        if reconnecting {
            app.status_label.set_text(s.status_reconnecting);
            app.hint_label.set_text(s.hint_reconnecting);
        } else {
            app.status_label.set_text(s.status_waiting);
            app.hint_label.set_text(s.hint_scan);
        }
        set_qr_image(app, &snapshot);
    }

    let mut history_text = String::new();
    if let Some(items) = snapshot.get("history").and_then(|v| v.as_array()) {
        for item in items {
            if let Some(text) = item.get("text").and_then(|v| v.as_str()) {
                history_text.push_str(text);
                history_text.push_str("\r\n");
            }
        }
    }
    app.history_box.set_text(&history_text);
}

fn set_qr_image(app: &App, snapshot: &serde_json::Value) {
    let Some(data_uri) = snapshot.get("qr_data_uri").and_then(|v| v.as_str()) else {
        return;
    };
    let Some(b64) = data_uri.strip_prefix("data:image/png;base64,") else {
        return;
    };
    let Ok(png_bytes) = base64::engine::general_purpose::STANDARD.decode(b64) else {
        return;
    };

    let mut bitmap = nwg::Bitmap::default();
    let built = nwg::Bitmap::builder()
        .source_bin(Some(&png_bytes))
        .build(&mut bitmap);
    match built {
        Ok(()) => {
            app.qr_frame.set_bitmap(Some(&bitmap));
            // The frame only borrows the bitmap -- it has to be kept
            // alive for as long as it's displayed, hence stashing it
            // here rather than letting it drop at the end of this
            // function.
            *app.qr_bitmap.borrow_mut() = Some(bitmap);
        }
        Err(err) => log::warn!("Failed to decode the QR code image: {err}"),
    }
}

fn copy_last_message(app: &App) {
    let json = app.latest_snapshot.lock().unwrap().clone();
    let snapshot: serde_json::Value = match serde_json::from_str(&json) {
        Ok(v) => v,
        Err(_) => return,
    };
    let Some(text) = snapshot
        .get("history")
        .and_then(|v| v.as_array())
        .and_then(|items| items.last())
        .and_then(|item| item.get("text"))
        .and_then(|v| v.as_str())
    else {
        return;
    };
    match arboard::Clipboard::new() {
        Ok(mut clipboard) => {
            if let Err(err) = clipboard.set_text(text.to_string()) {
                log::warn!("Failed to copy the message to the clipboard: {err}");
            }
        }
        Err(err) => log::warn!("Failed to access the clipboard: {err}"),
    }
}
