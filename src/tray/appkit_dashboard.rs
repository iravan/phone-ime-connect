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
    NSAutoresizingMaskOptions, NSButton, NSColor, NSFont, NSImage, NSImageView, NSLayoutAttribute,
    NSStackView, NSTextField, NSUserInterfaceLayoutOrientation, NSView,
};
use objc2_foundation::{NSData, NSEdgeInsets, NSSize, NSString};
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::Window;

use crate::server::PairingServer;

/// The AppKit target object whose action methods the dashboard buttons fire
/// ("New code" and "Clear history"). NSButton only weakly references its
/// target, so the owning [`Content`] keeps this alive.
struct ActionIvars {
    server: Arc<PairingServer>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "PICActionTarget"]
    #[ivars = ActionIvars]
    struct ActionTarget;

    impl ActionTarget {
        #[unsafe(method(regenerate:))]
        fn regenerate(&self, _sender: Option<&AnyObject>) {
            self.ivars().server.regenerate_token();
        }

        #[unsafe(method(clearHistory:))]
        fn clear_history(&self, _sender: Option<&AnyObject>) {
            self.ivars().server.clear_history();
        }
    }
);

impl ActionTarget {
    fn new(server: Arc<PairingServer>) -> Retained<Self> {
        let this = Self::alloc().set_ivars(ActionIvars { server });
        unsafe { msg_send![super(this), init] }
    }
}

/// The live widgets, retained so snapshots can update them in place.
pub struct Content {
    status: Retained<NSTextField>,
    hint: Retained<NSTextField>,
    qr: Retained<NSImageView>,
    history: Retained<NSTextField>,
    // Hidden while there's no history; touched from apply_snapshot.
    clear_button: Retained<NSButton>,
    // Kept alive but not otherwise touched: the button weakly references the
    // target, and the stack/button are also retained by the view hierarchy.
    _action_target: Retained<ActionTarget>,
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

        // Typographic hierarchy: status is the headline, hint is secondary.
        let status = label(s.connecting, mtm);
        status.setFont(Some(&NSFont::boldSystemFontOfSize(16.0)));
        let qr = NSImageView::new(mtm);
        let hint = label(s.hint_scan, mtm);
        hint.setFont(Some(&NSFont::systemFontOfSize(12.0)));
        hint.setTextColor(Some(&NSColor::secondaryLabelColor()));

        let action_target = ActionTarget::new(server);
        // Unsafe only because the action selectors must exist on the target
        // (they do: `ActionTarget`'s `regenerate:` and `clearHistory:`).
        let button = unsafe {
            NSButton::buttonWithTitle_target_action(
                &NSString::from_str(s.button_new_code),
                Some(&action_target),
                Some(sel!(regenerate:)),
                mtm,
            )
        };
        let clear_button = unsafe {
            NSButton::buttonWithTitle_target_action(
                &NSString::from_str(s.button_clear_history),
                Some(&action_target),
                Some(sel!(clearHistory:)),
                mtm,
            )
        };
        clear_button.setHidden(true); // shown only once there's history

        // Selectable so the user can highlight a received message and copy it
        // (Cmd+C) -- the whole point is getting text off the phone. Slightly
        // muted so the QR/status stay the focus until messages arrive.
        let history = label("", mtm);
        history.setSelectable(true);
        history.setFont(Some(&NSFont::systemFontOfSize(13.0)));
        history.setTextColor(Some(&NSColor::secondaryLabelColor()));

        let stack = NSStackView::new(mtm);
        stack.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
        stack.setSpacing(14.0);
        stack.setAlignment(NSLayoutAttribute::CenterX);
        // Breathing room around the whole dashboard instead of edge-to-edge.
        stack.setEdgeInsets(NSEdgeInsets {
            top: 24.0,
            left: 24.0,
            bottom: 24.0,
            right: 24.0,
        });
        stack.addArrangedSubview(&status);
        stack.addArrangedSubview(&qr);
        stack.addArrangedSubview(&hint);
        stack.addArrangedSubview(&button);
        stack.addArrangedSubview(&history);
        stack.addArrangedSubview(&clear_button);

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
            clear_button,
            _action_target: action_target,
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
        self.clear_button.setHidden(lines.is_empty());
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
