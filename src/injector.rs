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
//! On Linux, `enigo` is built with both its `xdo` (legacy X11/XTest, via
//! Xwayland) and `libei` (the `xdg-desktop-portal` RemoteDesktop session)
//! backends, and tries both on every keystroke. `xdo` alone is silently
//! blocked by Wayland compositors -- Mutter, for instance, only allows XTest
//! from whichever client currently holds keyboard focus, which is never
//! this app -- so `libei` is what actually delivers the paste there, at the
//! cost of a one-time GNOME "allow remote desktop interaction" consent
//! dialog the first time a message arrives (see the README's "Platform
//! notes" section). That handshake is why `Enigo` is created lazily on the
//! first message rather than at startup: doing it eagerly would block the
//! window from appearing until that first dialog (which the user has no
//! context for yet, since the window isn't even up) is answered.

#[cfg(not(target_os = "macos"))]
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

// macOS's `Enigo` wraps a `CGEventSource`, which is neither `Send` nor
// `Sync`, so it can't live inside the `Arc<Injector>` that's shared across
// the server's async tasks. There it's built fresh per message on the
// blocking thread that delivers it -- cheap enough for human-paced input,
// and NSPasteboard stores data in the OS pasteboard server, so a
// short-lived `Clipboard` still restores correctly. Everywhere else the
// instances are persistent: on X11 in particular, `arboard`'s clipboard
// must stay alive to keep serving the contents it restored.
#[cfg(not(target_os = "macos"))]
struct State {
    /// Lazily created on the first message -- see the module docs.
    enigo: Option<Enigo>,
    clipboard: Clipboard,
}

#[cfg(not(target_os = "macos"))]
pub struct Injector {
    state: Mutex<State>,
}

#[cfg(not(target_os = "macos"))]
impl Injector {
    pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
        Ok(Self {
            state: Mutex::new(State {
                enigo: None,
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

        if enigo.is_none() {
            log::info!(
                "Setting up keyboard input for the first message -- on Wayland this may \
                 show a one-time \"allow remote desktop interaction\" permission prompt."
            );
            match Enigo::new(&Settings::default()) {
                Ok(e) => *enigo = Some(e),
                // Left as `None` -- retried on the next message rather
                // than treated as permanent, since the failure may be
                // transient (e.g. the portal wasn't up yet).
                Err(err) => {
                    log::warn!("Failed to initialize keyboard input: {err}");
                    return;
                }
            }
        }

        deliver(enigo.as_mut().unwrap(), clipboard, text);
    }
}

#[cfg(target_os = "macos")]
pub struct Injector {
    _private: (),
}

#[cfg(target_os = "macos")]
impl Injector {
    pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
        // Fail fast if input/clipboard access can't be set up at all,
        // matching the other platforms' construction-time check.
        Enigo::new(&Settings::default())?;
        Clipboard::new()?;
        Ok(Self { _private: () })
    }

    /// See the non-macOS impl. `Enigo`/`Clipboard` are built per call here
    /// because they're `!Send` on macOS.
    pub fn type_text(&self, text: &str) {
        let mut enigo = match Enigo::new(&Settings::default()) {
            Ok(enigo) => enigo,
            Err(err) => {
                log::warn!("Failed to initialize keyboard input injector: {err}");
                return;
            }
        };
        let mut clipboard = match Clipboard::new() {
            Ok(clipboard) => clipboard,
            Err(err) => {
                log::warn!("Failed to access the clipboard: {err}");
                return;
            }
        };
        deliver(&mut enigo, &mut clipboard, text);
    }
}

/// Places `text` on the clipboard, pastes it into the focused window, then
/// restores the previous clipboard contents.
fn deliver(enigo: &mut Enigo, clipboard: &mut Clipboard, text: &str) {
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

fn paste(enigo: &mut Enigo) -> enigo::InputResult<()> {
    enigo.key(PASTE_MODIFIER, Direction::Press)?;
    enigo.key(Key::Unicode('v'), Direction::Click)?;
    enigo.key(PASTE_MODIFIER, Direction::Release)?;
    Ok(())
}
