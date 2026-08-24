//! Delivers a received message into whatever window currently has focus by
//! placing it on the clipboard and simulating a paste (Ctrl+V, or Cmd+V on
//! macOS), rather than simulating individual keypresses (`enigo`'s
//! `Keyboard::text`).
//!
//! Keypress simulation only has keycodes for whatever's on the physical
//! keyboard layout, so non-Latin text (CJK, etc.) has to be typed via a
//! synthetic Unicode keysym trick that most IMEs -- built to interpret real
//! keystrokes as composition input, not to accept a pre-composed character
//! outright -- silently drop or mangle. Paste sidesteps that entirely: the
//! target app receives already-decoded text with no composition involved,
//! at the cost of briefly overwriting the system clipboard (saved and
//! restored around each message).
//!
//! On Wayland, most compositors block synthetic input (both the clipboard
//! write and the paste keystroke) from arbitrary clients as a security
//! measure, so this may silently do nothing -- see the README's "Platform
//! notes" section.

use std::sync::Mutex;
use std::thread;
use std::time::Duration;

use arboard::Clipboard;
use enigo::{Direction, Enigo, Key, Keyboard, Settings};

/// How long to wait after sending the paste keystroke before restoring the
/// clipboard, so the target app has time to actually read it.
const PASTE_SETTLE_TIME: Duration = Duration::from_millis(200);

#[cfg(target_os = "macos")]
const PASTE_MODIFIER: Key = Key::Meta;
#[cfg(not(target_os = "macos"))]
const PASTE_MODIFIER: Key = Key::Control;

struct State {
    enigo: Enigo,
    clipboard: Clipboard,
}

pub struct Injector {
    state: Mutex<State>,
}

impl Injector {
    pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
        Ok(Self {
            state: Mutex::new(State {
                enigo: Enigo::new(&Settings::default())?,
                clipboard: Clipboard::new()?,
            }),
        })
    }

    /// Pastes `text` into the currently focused window. Errors (e.g. no
    /// compositor permission on Wayland) are logged, not propagated --
    /// there is no sensible per-message recovery action, and the phone
    /// side has already shown the message as sent.
    pub fn type_text(&self, text: &str) {
        let mut state = self.state.lock().unwrap();
        let State { enigo, clipboard } = &mut *state;

        let previous_clipboard = clipboard.get_text().ok();

        if let Err(err) = clipboard.set_text(text.to_string()) {
            log::warn!("Failed to set the clipboard to paste from: {err}");
            return;
        }

        if let Err(err) = paste(enigo) {
            log::warn!("Failed to paste text into the focused window: {err}");
        }

        // Give the target app time to read the clipboard before it's
        // overwritten again -- pasting is fire-and-forget from here, with
        // no signal for when the target has actually consumed it.
        thread::sleep(PASTE_SETTLE_TIME);

        let restore = match previous_clipboard {
            Some(prev) => clipboard.set_text(prev),
            None => clipboard.clear(),
        };
        if let Err(err) = restore {
            log::warn!("Failed to restore the previous clipboard contents: {err}");
        }
    }
}

fn paste(enigo: &mut Enigo) -> enigo::InputResult<()> {
    enigo.key(PASTE_MODIFIER, Direction::Press)?;
    enigo.key(Key::Unicode('v'), Direction::Click)?;
    enigo.key(PASTE_MODIFIER, Direction::Release)?;
    Ok(())
}
