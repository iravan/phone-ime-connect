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
    NSAutoresizingMaskOptions, NSBox, NSBoxType, NSButton, NSColor, NSFont, NSImage, NSImageView,
    NSLayoutAttribute, NSLineBreakMode, NSStackView, NSTextAlignment, NSTextField,
    NSUserInterfaceLayoutOrientation, NSView,
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
    // The whole "Received messages" section (separator + header + clear
    // button) is shown only once there's history; toggled from apply_snapshot.
    separator: Retained<NSBox>,
    messages_header: Retained<NSTextField>,
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

        // --- "Received messages" section --------------------------------
        // A rule separates the pairing area (QR/status) from the message
        // log so the two read as distinct sections.
        let separator = NSBox::new(mtm);
        separator.setBoxType(NSBoxType::Separator);
        separator.setHidden(true);

        // A small left-aligned section title above the messages.
        let messages_header = label(s.history_header, mtm);
        messages_header.setFont(Some(&NSFont::boldSystemFontOfSize(12.0)));
        messages_header.setTextColor(Some(&NSColor::secondaryLabelColor()));
        messages_header.setAlignment(NSTextAlignment::Left);
        messages_header.setHidden(true);

        // The message log itself: left-aligned and full-strength text so it's
        // easy to read, and selectable so a message can be copied (Cmd+C).
        let history = label("", mtm);
        history.setSelectable(true);
        history.setFont(Some(&NSFont::systemFontOfSize(13.0)));
        history.setAlignment(NSTextAlignment::Left);

        let stack = NSStackView::new(mtm);
        stack.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
        stack.setSpacing(16.0);
        stack.setAlignment(NSLayoutAttribute::CenterX);
        // Generous margins instead of edge-to-edge.
        stack.setEdgeInsets(NSEdgeInsets {
            top: 28.0,
            left: 28.0,
            bottom: 28.0,
            right: 28.0,
        });
        stack.addArrangedSubview(&status);
        stack.addArrangedSubview(&qr);
        stack.addArrangedSubview(&hint);
        stack.addArrangedSubview(&button);
        stack.addArrangedSubview(&separator);
        stack.addArrangedSubview(&messages_header);
        stack.addArrangedSubview(&history);
        stack.addArrangedSubview(&clear_button);

        // A fixed content column so the message text left-aligns into a tidy
        // block (rather than hugging its own width and reading as centered).
        const COL: f64 = 340.0;
        for v in [
            status.as_ref() as &NSView,
            hint.as_ref(),
            button.as_ref(),
            separator.as_ref(),
            messages_header.as_ref(),
            history.as_ref(),
            clear_button.as_ref(),
        ] {
            v.widthAnchor().constraintEqualToConstant(COL).setActive(true);
        }

        // Extra air around the separator so the two sections feel distinct.
        stack.setCustomSpacing_afterView(24.0, &button);
        stack.setCustomSpacing_afterView(6.0, &separator);

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
            separator,
            messages_header,
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
        // Show the whole "Received messages" section only once there's history.
        let has_history = !lines.is_empty();
        self.separator.setHidden(!has_history);
        self.messages_header.setHidden(!has_history);
        self.history.setHidden(!has_history);
        self.clear_button.setHidden(!has_history);
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

/// A non-editable, multi-line AppKit label. Wraps onto as many lines as
/// needed rather than truncating with an ellipsis, so long status/hint
/// text (e.g. the reconnecting message) is always shown in full.
fn label(text: &str, mtm: MainThreadMarker) -> Retained<NSTextField> {
    let label = NSTextField::labelWithString(&NSString::from_str(text), mtm);
    label.setSelectable(false);
    label.setPreferredMaxLayoutWidth(360.0);
    label.setUsesSingleLineMode(false);
    label.setMaximumNumberOfLines(0);
    label.setLineBreakMode(NSLineBreakMode::ByWordWrapping);
    label.setAlignment(objc2_app_kit::NSTextAlignment::Center);
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
