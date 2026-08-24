//! The primary UI on Linux: a native GTK4 window showing the QR code,
//! connection status, and message history, with a "New code" button.
//! There's no browser-based fallback any more (`server.rs` used to also
//! serve a `/dashboard` page on 127.0.0.1 for that; it's gone now that
//! every platform has its own native window).
//!
//! Closing this window quits the whole application, including the
//! pairing server -- it's the only UI now, not a tray-minimizable
//! convenience, so there's nothing meaningful left running without it.
//!
//! Windows has its own equivalent in `window/windows.rs`; macOS has its
//! own native window too, drawn with AppKit widgets in a tray/menu-bar
//! app instead of a plain window (`tray/native.rs`,
//! `tray/appkit_dashboard.rs`).

use std::sync::Arc;

use base64::Engine;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{
    Application, ApplicationWindow, Box as GtkBox, Button, CssProvider, Label, Orientation,
    Picture, ScrolledWindow, Separator,
};
use tokio::sync::broadcast;

use crate::server::PairingServer;

const APP_ID: &str = "org.phoneinputconnect.PhoneInputConnect";

/// Custom styling layered on top of whatever GTK theme is active. Kept to
/// properties supported since GTK 4.6 (this project's floor -- see the
/// README) and to theme-provided named colors (`@theme_*`, `@borders`)
/// rather than hardcoded hex values, so it still looks native across
/// different GTK themes and light/dark variants. The QR card is the one
/// deliberate exception: it's pinned to white regardless of theme, since
/// the QR PNG itself assumes a light quiet zone for reliable scanning.
const STYLE: &str = "
    .status-dot {
        min-width: 10px; min-height: 10px; border-radius: 50%;
        background-color: @theme_fg_color; opacity: 0.35;
    }
    .status-dot.connected { background-color: #2fa84f; opacity: 1; }
    .status-dot.waiting { background-color: @theme_fg_color; opacity: 0.35; }
    .status-dot.reconnecting { background-color: #d99a2b; opacity: 1; }
    .status-text { font-weight: 600; font-size: 1.05em; }
    .qr-card {
        background-color: #ffffff; border-radius: 12px; padding: 16px;
        box-shadow: 0 1px 4px rgba(0, 0, 0, 0.25);
    }
    .section-header { font-weight: 600; opacity: 0.7; }
    .msg-bubble {
        background-color: @theme_base_color; border: 1px solid @borders;
        border-radius: 10px; padding: 6px 10px;
    }
";

/// Runs the GTK main loop on the calling thread until the window is
/// closed. `runtime` is shared (not consumed) so the caller can keep
/// using the same one afterwards, e.g. to run the server's own async
/// shutdown -- GTK's `connect_activate` needs a `'static` closure, which
/// an owned `Arc` clone satisfies without giving up the caller's own.
pub fn run(runtime: Arc<tokio::runtime::Runtime>, server: Arc<PairingServer>) {
    let app = Application::builder().application_id(APP_ID).build();

    app.connect_activate(move |app| {
        build_window(app, &runtime, &server);
    });

    app.run();
}

fn build_window(app: &Application, runtime: &tokio::runtime::Runtime, server: &Arc<PairingServer>) {
    let s = crate::i18n::strings();

    let provider = CssProvider::new();
    provider.load_from_data(STYLE);
    gtk4::style_context_add_provider_for_display(
        &gtk4::gdk::Display::default().expect("no GDK display available"),
        &provider,
        gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );

    let status_dot = GtkBox::new(Orientation::Horizontal, 0);
    status_dot.add_css_class("status-dot");
    status_dot.add_css_class("waiting");
    // Without a fixed size, a horizontal box's default "fill" valign
    // stretches it to the row's full height (set by the status label's
    // font), turning the circle into a pill/oval.
    status_dot.set_size_request(10, 10);
    status_dot.set_valign(gtk4::Align::Center);
    status_dot.set_halign(gtk4::Align::Center);

    let status_label = Label::new(Some(s.connecting));
    status_label.set_wrap(true);
    status_label.set_xalign(0.0);
    status_label.add_css_class("status-text");

    let status_row = GtkBox::new(Orientation::Horizontal, 8);
    status_row.set_valign(gtk4::Align::Center);
    status_row.append(&status_dot);
    status_row.append(&status_label);

    let qr_picture = Picture::new();
    qr_picture.set_can_shrink(true);
    // `ContentFit`/`set_content_fit` (the non-deprecated replacement) needs
    // GTK 4.8; staying on this deprecated-since-4.8-but-still-functional
    // call keeps the build working against GTK 4.6, the system version on
    // e.g. Ubuntu 22.04/Zorin OS 17.
    #[allow(deprecated)]
    qr_picture.set_keep_aspect_ratio(true);
    qr_picture.set_size_request(220, 220);

    let qr_card = GtkBox::new(Orientation::Vertical, 0);
    qr_card.set_halign(gtk4::Align::Center);
    qr_card.add_css_class("qr-card");
    qr_card.append(&qr_picture);

    let hint_label = Label::new(Some(s.hint_scan));
    hint_label.set_wrap(true);
    hint_label.set_justify(gtk4::Justification::Center);
    hint_label.add_css_class("dim-label");

    let regenerate_button = Button::with_label(s.button_new_code);
    regenerate_button.add_css_class("suggested-action");
    {
        let server = server.clone();
        regenerate_button.connect_clicked(move |_| {
            server.regenerate_token();
        });
    }

    let button_row = GtkBox::new(Orientation::Horizontal, 6);
    button_row.set_halign(gtk4::Align::Center);
    button_row.append(&regenerate_button);

    let clear_history_button = Button::with_label(s.button_clear_history);
    clear_history_button.add_css_class("destructive-action");
    {
        let server = server.clone();
        clear_history_button.connect_clicked(move |_| {
            server.clear_history();
        });
    }
    button_row.append(&clear_history_button);

    let history_header = Label::new(Some(s.history_header));
    history_header.set_xalign(0.0);
    history_header.add_css_class("section-header");

    let history_box = GtkBox::new(Orientation::Vertical, 6);
    let history_scroller = ScrolledWindow::builder()
        .child(&history_box)
        .vexpand(true)
        .build();

    let root = GtkBox::new(Orientation::Vertical, 12);
    root.set_margin_top(16);
    root.set_margin_bottom(16);
    root.set_margin_start(16);
    root.set_margin_end(16);
    root.append(&status_row);
    root.append(&qr_card);
    root.append(&hint_label);
    root.append(&button_row);
    root.append(&Separator::new(Orientation::Horizontal));
    root.append(&history_header);
    root.append(&history_scroller);

    let window = ApplicationWindow::builder()
        .application(app)
        .title("PhoneInputConnect")
        .default_width(380)
        .default_height(580)
        .child(&root)
        .build();

    // This window is the entire UI now (no tray icon to fall back to),
    // so closing it ends the process -- including the pairing server.
    {
        let app = app.clone();
        window.connect_close_request(move |_| {
            app.quit();
            glib::Propagation::Proceed
        });
    }

    window.present();

    // The server's dashboard broadcast (the same live-status stream every
    // native window renders from) is bridged onto the GTK main loop
    // via an async_channel -- GTK objects may only be touched from the
    // thread running the main loop, and that broadcast is driven from
    // the Tokio runtime's own worker threads.
    let (tx, rx) = async_channel::unbounded::<String>();
    {
        let updates = server.subscribe_dashboard();
        let initial = server.dashboard_snapshot();
        runtime.spawn(forward_updates(initial, updates, tx));
    }
    glib::spawn_future_local(async move {
        while let Ok(json) = rx.recv().await {
            apply_snapshot(
                &status_dot,
                &status_label,
                &qr_card,
                &qr_picture,
                &hint_label,
                &history_box,
                &json,
            );
        }
    });
}

async fn forward_updates(
    initial: String,
    mut updates: broadcast::Receiver<String>,
    tx: async_channel::Sender<String>,
) {
    if tx.send(initial).await.is_err() {
        return;
    }
    loop {
        match updates.recv().await {
            Ok(json) => {
                if tx.send(json).await.is_err() {
                    break;
                }
            }
            // A slow-to-render UI missing a few intermediate snapshots
            // is harmless -- the next one received still reflects
            // current state.
            Err(broadcast::error::RecvError::Lagged(_)) => continue,
            Err(broadcast::error::RecvError::Closed) => break,
        }
    }
}

/// Swaps `dot`'s state class (`connected`/`waiting`/`reconnecting`) for the
/// one named -- the CSS in `STYLE` gives each its own dot color.
fn set_status_dot_state(dot: &GtkBox, state: &str) {
    for other in ["connected", "waiting", "reconnecting"] {
        if other == state {
            dot.add_css_class(other);
        } else {
            dot.remove_css_class(other);
        }
    }
}

fn apply_snapshot(
    status_dot: &GtkBox,
    status_label: &Label,
    qr_card: &GtkBox,
    qr_picture: &Picture,
    hint_label: &Label,
    history_box: &GtkBox,
    json: &str,
) {
    let s = crate::i18n::strings();
    let snapshot: serde_json::Value = match serde_json::from_str(json) {
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
        set_status_dot_state(status_dot, "connected");
        status_label.set_label(s.status_connected);
        hint_label.set_label(s.hint_connected);
        qr_card.set_visible(false);
    } else {
        qr_card.set_visible(true);
        if reconnecting {
            set_status_dot_state(status_dot, "reconnecting");
            status_label.set_label(s.status_reconnecting);
            hint_label.set_label(s.hint_reconnecting);
        } else {
            set_status_dot_state(status_dot, "waiting");
            status_label.set_label(s.status_waiting);
            hint_label.set_label(s.hint_scan);
        }
        set_qr_image(qr_picture, &snapshot);
    }

    while let Some(child) = history_box.first_child() {
        history_box.remove(&child);
    }
    let items = snapshot
        .get("history")
        .and_then(|v| v.as_array())
        .filter(|items| !items.is_empty());
    match items {
        Some(items) => {
            for item in items {
                if let Some(text) = item.get("text").and_then(|v| v.as_str()) {
                    history_box.append(&build_history_row(text));
                }
            }
        }
        None => {
            let placeholder = Label::new(Some(s.history_empty));
            placeholder.add_css_class("dim-label");
            placeholder.set_margin_top(12);
            history_box.append(&placeholder);
        }
    }
}

/// One history entry, styled like a chat bubble, plus a copy-to-clipboard
/// button that's only shown while the pointer is over the row -- kept out
/// of the way otherwise, since it's a rarely-needed convenience, not a
/// primary action.
fn build_history_row(text: &str) -> GtkBox {
    let row = GtkBox::new(Orientation::Horizontal, 6);
    row.add_css_class("msg-bubble");

    let label = Label::new(Some(text));
    label.set_xalign(0.0);
    label.set_wrap(true);
    label.set_hexpand(true);

    let copy_button = Button::from_icon_name("edit-copy-symbolic");
    copy_button.set_tooltip_text(Some(crate::i18n::strings().copy_to_clipboard_tooltip));
    copy_button.add_css_class("flat");
    copy_button.set_visible(false);
    {
        let text = text.to_string();
        copy_button.connect_clicked(move |button| {
            button.clipboard().set_text(&text);
        });
    }

    row.append(&label);
    row.append(&copy_button);

    let hover = gtk4::EventControllerMotion::new();
    {
        let copy_button = copy_button.clone();
        hover.connect_enter(move |_, _, _| copy_button.set_visible(true));
    }
    {
        let copy_button = copy_button.clone();
        hover.connect_leave(move |_| copy_button.set_visible(false));
    }
    row.add_controller(hover);

    row
}

fn set_qr_image(qr_picture: &Picture, snapshot: &serde_json::Value) {
    let Some(data_uri) = snapshot.get("qr_data_uri").and_then(|v| v.as_str()) else {
        return;
    };
    let Some(b64) = data_uri.strip_prefix("data:image/png;base64,") else {
        return;
    };
    let Ok(png_bytes) = base64::engine::general_purpose::STANDARD.decode(b64) else {
        return;
    };
    let bytes = glib::Bytes::from_owned(png_bytes);
    match gtk4::gdk::Texture::from_bytes(&bytes) {
        Ok(texture) => qr_picture.set_paintable(Some(&texture)),
        Err(err) => log::warn!("Failed to decode the QR code image: {err}"),
    }
}
