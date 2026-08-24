//! Types a received message into whatever window currently has focus, via
//! simulated keyboard input (`enigo`) rather than any IME framework --
//! this design never needed keystroke-level composition, since a whole
//! finished message arrives at once and is typed/committed in one shot.
//!
//! On X11 and Windows/macOS this is exactly equivalent to a user typing
//! the message by hand. On Wayland, most compositors block synthetic
//! input from arbitrary clients as a security measure, so this may
//! silently do nothing -- see the README's "Platform notes" section.

use std::sync::Mutex;

use enigo::{Enigo, Keyboard, Settings};

pub struct Injector {
    enigo: Mutex<Enigo>,
}

impl Injector {
    pub fn new() -> Result<Self, enigo::NewConError> {
        Ok(Self {
            enigo: Mutex::new(Enigo::new(&Settings::default())?),
        })
    }

    /// Types `text` into the currently focused window. Errors (e.g. no
    /// compositor permission on Wayland) are logged, not propagated --
    /// there is no sensible per-message recovery action, and the phone
    /// side has already shown the message as sent.
    pub fn type_text(&self, text: &str) {
        let mut enigo = self.enigo.lock().unwrap();
        if let Err(err) = enigo.text(text) {
            log::warn!("Failed to type text into the focused window: {err}");
        }
    }
}
