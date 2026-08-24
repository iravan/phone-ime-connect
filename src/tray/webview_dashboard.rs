//! Windows dashboard: an embedded WebView2 (via `wry`) rendering the
//! dashboard page, driven in-process via wry's IPC bridge -- the same
//! `Content` interface macOS implements with native AppKit widgets
//! (`appkit_dashboard.rs`). A native Win32 dashboard is follow-up work.

use std::sync::Arc;

use winit::window::Window;
use wry::WebViewBuilder;

use crate::server::PairingServer;

const WINDOW_HTML: &str = include_str!("../webapp/window.html");

pub struct Content {
    webview: wry::WebView,
}

impl Content {
    pub fn create(window: &Window, server: Arc<PairingServer>) -> Content {
        let webview = WebViewBuilder::new()
            .with_html(WINDOW_HTML)
            .with_ipc_handler(move |req| {
                if req.body().as_str() == "regenerate" {
                    server.regenerate_token();
                }
            })
            .build(window)
            .expect("failed to create the dashboard webview");
        Content { webview }
    }

    pub fn apply_snapshot(&self, json: &str) {
        let _ = self
            .webview
            .evaluate_script(&format!("window.__render({json});"));
    }
}
