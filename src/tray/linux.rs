//! Linux tray icon via `ksni`, the StatusNotifierItem D-Bus protocol
//! implemented in pure Rust. Runs as a plain task on our existing Tokio
//! runtime; no GTK, no separate event loop, no system dev packages
//! needed to build (see the module-level note in `tray/mod.rs`).
//!
//! Whether an icon actually appears depends on the desktop environment
//! having a StatusNotifierWatcher running -- stock GNOME Shell doesn't,
//! unless the user installs the "AppIndicator and KStatusNotifierItem
//! Support" extension. That's a desktop-environment limitation, not
//! something this code can work around; see the README's "Platform
//! notes".

use ksni::menu::{MenuItem, StandardItem};
use ksni::{Handle, Tray, TrayMethods};

use super::TrayCallbacks;

pub struct PhoneChatTray {
    callbacks: TrayCallbacks,
}

impl Tray for PhoneChatTray {
    fn id(&self) -> String {
        "phonechat".into()
    }

    fn title(&self) -> String {
        "PhoneChat".into()
    }

    fn icon_name(&self) -> String {
        "network-transmit-receive".into()
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        vec![
            StandardItem {
                label: "Open dashboard".into(),
                activate: Box::new(|this: &mut Self| (this.callbacks.open_dashboard)()),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "New code".into(),
                activate: Box::new(|this: &mut Self| (this.callbacks.regenerate)()),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: "Quit".into(),
                activate: Box::new(|this: &mut Self| (this.callbacks.quit)()),
                ..Default::default()
            }
            .into(),
        ]
    }
}

/// Starts the tray icon on the current Tokio runtime. The returned
/// `Handle` can be dropped or explicitly `.shutdown()`'d to remove the
/// icon; there is no event loop to separately drive.
pub async fn spawn(callbacks: TrayCallbacks) -> Result<Handle<PhoneChatTray>, ksni::Error> {
    PhoneChatTray { callbacks }.spawn().await
}
