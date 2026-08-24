//! The primary UI on Linux: a native GTK4 window showing the QR code,
//! connection status, and message history, with a "New code" button --
//! replacing the browser-tab-based dashboard (`server.rs`'s
//! `/dashboard` route still exists and still works if opened by hand, but
//! nothing points at it by default anymore).
//!
//! Closing this window quits the whole application, including the
//! pairing server -- it's the only UI now, not a tray-minimizable
//! convenience, so there's nothing meaningful left running without it.
//!
//! Windows/macOS don't have this yet (see `tray/native.rs`); giving them
//! an equivalent native window, in each platform's own toolkit, is
//! follow-up work.

use std::sync::Arc;

use base64::Engine;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{
    Application, ApplicationWindow, Box as GtkBox, Button, ContentFit, Label, Orientation, Picture,
    ScrolledWindow,
};
use tokio::sync::broadcast;

use crate::server::PairingServer;

const APP_ID: &str = "org.phonechat.PhoneChat";

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
    let status_label = Label::new(Some("Connecting…"));
    status_label.set_wrap(true);

    let qr_picture = Picture::new();
    qr_picture.set_can_shrink(true);
    qr_picture.set_content_fit(ContentFit::Contain);
    qr_picture.set_size_request(240, 240);

    let hint_label = Label::new(Some("Scan with a phone on the same network."));
    hint_label.set_wrap(true);

    let regenerate_button = Button::with_label("New code");
    {
        let server = server.clone();
        regenerate_button.connect_clicked(move |_| {
            server.regenerate_token();
        });
    }

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
    root.append(&status_label);
    root.append(&qr_picture);
    root.append(&hint_label);
    root.append(&regenerate_button);
    root.append(&history_scroller);

    let window = ApplicationWindow::builder()
        .application(app)
        .title("PhoneChat")
        .default_width(360)
        .default_height(560)
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

    // The server's dashboard broadcast (also what the browser-based
    // dashboard's WebSocket streams) is bridged onto the GTK main loop
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
            apply_snapshot(&status_label, &qr_picture, &hint_label, &history_box, &json);
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

fn apply_snapshot(
    status_label: &Label,
    qr_picture: &Picture,
    hint_label: &Label,
    history_box: &GtkBox,
    json: &str,
) {
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
        status_label.set_label("Phone connected");
        hint_label.set_label("Messages you send from the phone appear below.");
        qr_picture.set_visible(false);
    } else {
        qr_picture.set_visible(true);
        if reconnecting {
            status_label.set_label("Phone disconnected — waiting to reconnect…");
            hint_label.set_label(
                "The same code still works for a bit in case it reconnects on its own; \
                 scan again if it doesn't.",
            );
        } else {
            status_label.set_label("Waiting for a phone to scan the code below");
            hint_label.set_label("Scan with a phone on the same network.");
        }
        set_qr_image(qr_picture, &snapshot);
    }

    while let Some(child) = history_box.first_child() {
        history_box.remove(&child);
    }
    if let Some(items) = snapshot.get("history").and_then(|v| v.as_array()) {
        for item in items {
            if let Some(text) = item.get("text").and_then(|v| v.as_str()) {
                let label = Label::new(Some(text));
                label.set_xalign(0.0);
                label.set_wrap(true);
                history_box.append(&label);
            }
        }
    }
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
