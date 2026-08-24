//! macOS native dashboard: the QR code, connection status, hint, "New code"
//! button, and message history rendered with AppKit widgets (an NSStackView
//! of NSTextField / NSImageView / NSButton) added into the winit window's
//! content view. Fed by the same in-process JSON snapshots the Linux GTK
//! window uses (see `window.rs`) -- no embedded webview, no HTTP.

use std::ptr::NonNull;
use std::sync::Arc;

use base64::Engine;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject};
use objc2::{define_class, msg_send, sel, AllocAnyThread, DefinedClass, MainThreadMarker};
use objc2_app_kit::{
    NSAutoresizingMaskOptions, NSButton, NSImage, NSImageView, NSLayoutAttribute, NSStackView,
    NSTextField, NSUserInterfaceLayoutOrientation, NSView,
};
use objc2_foundation::{NSData, NSSize, NSString};
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::Window;

use crate::server::PairingServer;

/// The AppKit target object whose action method the "New code" button fires.
/// NSButton only weakly references its target, so the owning [`Content`]
/// keeps this alive.
struct RegenerateIvars {
    server: Arc<PairingServer>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "PICRegenerateTarget"]
    #[ivars = RegenerateIvars]
    struct RegenerateTarget;

    impl RegenerateTarget {
        #[unsafe(method(regenerate:))]
        fn regenerate(&self, _sender: Option<&AnyObject>) {
            self.ivars().server.regenerate_token();
        }
    }
);

impl RegenerateTarget {
    fn new(server: Arc<PairingServer>) -> Retained<Self> {
        let this = Self::alloc().set_ivars(RegenerateIvars { server });
        unsafe { msg_send![super(this), init] }
    }
}

/// The live widgets, retained so snapshots can update them in place.
pub struct Content {
    status: Retained<NSTextField>,
    hint: Retained<NSTextField>,
    qr: Retained<NSImageView>,
    history: Retained<NSTextField>,
    // Kept alive but not otherwise touched: the button weakly references the
    // target, and the stack/button are also retained by the view hierarchy.
    _regenerate_target: Retained<RegenerateTarget>,
    _button: Retained<NSButton>,
    _stack: Retained<NSStackView>,
}

impl Content {
    /// Builds the dashboard widgets into `window`'s content view. Must be
    /// called on the main thread (AppKit's only UI thread).
    pub fn create(window: &Window, server: Arc<PairingServer>) -> Content {
        let s = crate::i18n::strings();
        let mtm =
            MainThreadMarker::new().expect("the dashboard UI must be built on the main thread");
        let content = content_view(window);

        let status = label(s.connecting, mtm);
        let qr = NSImageView::new(mtm);
        let hint = label(s.hint_scan, mtm);

        let regenerate_target = RegenerateTarget::new(server);
        // Unsafe only because the action selector must exist on the target
        // (it does: `RegenerateTarget`'s `regenerate:`).
        let button = unsafe {
            NSButton::buttonWithTitle_target_action(
                &NSString::from_str(s.button_new_code),
                Some(&regenerate_target),
                Some(sel!(regenerate:)),
                mtm,
            )
        };

        let history = label("", mtm);

        let stack = NSStackView::new(mtm);
        stack.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
        stack.setSpacing(12.0);
        stack.setAlignment(NSLayoutAttribute::CenterX);
        stack.addArrangedSubview(&status);
        stack.addArrangedSubview(&qr);
        stack.addArrangedSubview(&hint);
        stack.addArrangedSubview(&button);
        stack.addArrangedSubview(&history);

        // Fill the content view and track its resizes.
        stack.setFrame(content.bounds());
        stack.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        content.addSubview(&stack);

        Content {
            status,
            hint,
            qr,
            history,
            _regenerate_target: regenerate_target,
            _button: button,
            _stack: stack,
        }
    }

    /// Applies a dashboard-snapshot JSON object (the same shape the browser
    /// dashboard's WebSocket streams) to the widgets. Mirrors
    /// `window.rs::apply_snapshot`.
    pub fn apply_snapshot(&self, json: &str) {
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
            set_text(&self.status, s.status_connected);
            set_text(&self.hint, s.hint_connected);
            self.qr.setHidden(true);
        } else {
            self.qr.setHidden(false);
            if reconnecting {
                set_text(&self.status, s.status_reconnecting);
                set_text(&self.hint, s.hint_reconnecting);
            } else {
                set_text(&self.status, s.status_waiting);
                set_text(&self.hint, s.hint_scan);
            }
            self.set_qr(&snapshot);
        }

        let mut lines = String::new();
        if let Some(items) = snapshot.get("history").and_then(|v| v.as_array()) {
            for item in items {
                if let Some(text) = item.get("text").and_then(|v| v.as_str()) {
                    if !lines.is_empty() {
                        lines.push('\n');
                    }
                    lines.push_str(text);
                }
            }
        }
        set_text(&self.history, &lines);
    }

    fn set_qr(&self, snapshot: &serde_json::Value) {
        let Some(data_uri) = snapshot.get("qr_data_uri").and_then(|v| v.as_str()) else {
            return;
        };
        let Some(b64) = data_uri.strip_prefix("data:image/png;base64,") else {
            return;
        };
        let Ok(png_bytes) = base64::engine::general_purpose::STANDARD.decode(b64) else {
            return;
        };
        let data = NSData::with_bytes(&png_bytes);
        let Some(image) = NSImage::initWithData(NSImage::alloc(), &data) else {
            log::warn!("Failed to decode the QR code image");
            return;
        };
        image.setSize(NSSize::new(220.0, 220.0));
        self.qr.setImage(Some(&image));
    }
}

/// A non-editable, multi-line AppKit label.
fn label(text: &str, mtm: MainThreadMarker) -> Retained<NSTextField> {
    let label = NSTextField::labelWithString(&NSString::from_str(text), mtm);
    label.setSelectable(false);
    label.setPreferredMaxLayoutWidth(320.0);
    label
}

fn set_text(field: &NSTextField, text: &str) {
    field.setStringValue(&NSString::from_str(text));
}

/// Retains the winit window's NSView (its content view). The raw pointer
/// comes from `raw-window-handle`, which is agnostic to objc2's version.
fn content_view(window: &Window) -> Retained<NSView> {
    let handle = window
        .window_handle()
        .expect("window handle unavailable")
        .as_raw();
    let RawWindowHandle::AppKit(handle) = handle else {
        panic!("expected an AppKit window handle on macOS");
    };
    let ns_view: NonNull<NSView> = handle.ns_view.cast();
    unsafe { Retained::retain(ns_view.as_ptr()).expect("NSView should be retainable") }
}
