# PhoneInputConnect

Type on your desktop by texting it from your phone.

PhoneInputConnect runs as a small app on your computer. It shows a QR code; scan
it with your phone (same Wi-Fi network, no app install), type a message in
the page that opens, and it's instantly typed into whatever window has
focus on your desktop -- as if you'd typed it yourself.

**Platform status**: on Linux, the QR code/status/history live in a native
window (see `window.rs`) that opens on launch -- closing it quits the app
entirely, including the pairing server. Windows and macOS don't have that
yet and fall back to a tray icon plus a browser tab for the same
information (see `tray/native.rs`); giving them an equivalent native
window is follow-up work.

## Why

Typing on a phone keyboard is often faster or more comfortable than reaching
for a desktop keyboard -- e.g. dictating a note, pasting a password from a
phone-based manager, or entering text one-handed. PhoneInputConnect turns your
phone into an ad hoc keyboard for whatever you're focused on, with no
account, no cloud service, and no app to install.

## How it works

1. Launch PhoneInputConnect. On Linux, a window showing a QR code opens directly.
   On Windows/macOS (no native window yet), it instead opens a
   **dashboard** page in your default browser
   (`https://127.0.0.1:<port>/dashboard`) with the same QR code, plus a
   tray/menu bar icon.
2. Scan the QR code with your phone's camera. It opens a chat-style page
   served directly from your computer over your LAN.
3. Type a message on your phone and hit send. It's echoed back to your
   phone as a chat bubble, and delivered into the desktop's currently
   focused window by placing it on the clipboard and simulating a paste
   (Ctrl+V, or Cmd+V on macOS) -- your previous clipboard contents are
   restored right after.
4. The window (or dashboard page) updates live: it shows "Phone connected"
   and a rolling history of the last few messages sent.

Only one phone can be paired at a time. Scanning a new QR code (the
window's/dashboard's "New code" button) invalidates the old one.

If the phone's connection drops (screen lock, backgrounded browser tab,
brief Wi-Fi blip), the same QR code/URL keeps working for about 45 seconds
in case it reconnects on its own -- no need to re-scan for a short hiccup.
Past that window, the code is invalidated for good and needs a fresh scan.

On Windows/macOS, if you lose the dashboard tab and there's no tray icon
to get it back, just launch PhoneInputConnect again -- it detects the
already-running instance and reopens its dashboard instead of starting a
second one. (On Linux the window itself *is* the app, so there's nothing
to lose track of; relaunching while it's already running just logs that
it's already up rather than trying to bring the existing window forward.)

## Building and running

Requires a recent Rust toolchain (`cargo build`/`cargo run`).

### Linux

The window is a native [GTK4](https://docs.rs/gtk4) UI (GTK 4.6 or newer),
which needs GTK4's development packages to build:

```sh
# Fedora
sudo dnf install gtk4-devel
# Debian/Ubuntu (including derivatives like Zorin OS)
sudo apt install libgtk-4-dev
```

Targeting Zorin OS 17 (Ubuntu 22.04 base, GNOME-based Core/Pro edition)
specifically: its system GTK4 is version 4.6, so the code deliberately
stays within GTK 4.6 APIs rather than something newer, even though this
repo is otherwise built and tested against a much newer GTK4 -- not yet
confirmed working on actual Zorin hardware, just built to be compatible
with what it ships. On an *older* base than that (Zorin OS 16 / Ubuntu
20.04 or earlier), `libgtk-4-dev` may not be in the default repos at all,
since GTK4 was still quite new then; you'd need a PPA or a newer release.

Typing uses [`enigo`](https://docs.rs/enigo) (for the paste keystroke),
which on X11 links against `libxdo`, so that needs to be installed too:

```sh
# Fedora
sudo dnf install libxdo-devel
# Debian/Ubuntu
sudo apt install libxdo-dev
```

Then:

```sh
cargo run
```

**Optional -- a proper icon in the dock/taskbar/Alt-Tab**: GTK4 under
Wayland has no in-process way to set a window icon; it's resolved
entirely from a `.desktop` file matched by application ID, not anything
the app itself can set at runtime. Without one installed, the window
just gets a generic icon. To fix that:

```sh
cargo build --release
./scripts/install-linux-desktop-entry.sh
```

The icon then applies whether you launch from your app launcher/dock or
just run the binary directly -- GNOME matches by application ID either
way. Re-run the script if you move the checkout, since the binary's path
is baked into the installed entry as-is.

### Windows / macOS

Uses [`tray-icon`](https://docs.rs/tray-icon) and
[`winit`](https://docs.rs/winit) for the tray/menu-bar icon and its event
loop. No extra system packages are required. `cargo run` builds and runs
as normal.

## Security

PhoneInputConnect is designed to be safe to run on an ordinary home or office Wi-Fi
network, where other devices are untrusted:

- The phone-facing listener binds only to this machine's detected LAN
  address -- never `0.0.0.0` -- and refuses to start at all if no such
  address can be found, so it's never accidentally more exposed than
  intended.
- The LAN hop is TLS-encrypted with a self-signed certificate generated on
  first run and cached per-user. Since there's no certificate authority
  behind it, your phone's browser will show a one-time "connection is not
  private" warning -- click through it once per phone.
- Pairing is gated by a single 256-bit random token embedded in the QR
  code's URL. It's unguessable, and is rotated the instant a phone *first*
  successfully connects, so a QR code can't be reused to open a second,
  competing session. It also expires on its own after 5 minutes if nothing
  ever connects. A *disconnect*, by contrast, doesn't rotate the token
  immediately -- it opens a ~45-second grace window where the same token
  still works, so a phone tab surviving a brief drop can reconnect without
  a new scan; the token only rotates for real once that window elapses.
- Only one phone may be connected at a time.
- Repeated failed-token requests from a given source IP are rate-limited
  (defense in depth against scanning or log-spam, not against
  brute-forcing 256 bits of entropy, which is already computationally
  infeasible).
- The same QR code/status/history is also always served as an HTML
  dashboard page (needs no token), reachable only from `127.0.0.1` -- a
  second, separate listening socket bound to loopback only, with every
  request additionally checked against the connecting socket's own
  remote address. Windows/macOS currently rely on this (opened in your
  browser); on Linux, the native window gets the same live updates
  directly in-process instead, but the page is still there at
  `https://127.0.0.1:<port>/dashboard` if you ever want it -- e.g. to
  check status from another browser tab on the same machine.
- Nothing is persisted to disk. Chat history is an in-memory ring buffer
  (the last 10 messages) that vanishes the moment the app stops.
- Each message is placed on the system clipboard just long enough to paste
  it into the focused window, then your previous clipboard contents are
  restored. Anything else on your machine that happens to poll the
  clipboard during that brief window could observe the message in transit.

## Platform notes

- **Wayland**: most compositors block synthetic input (both the clipboard
  write and the Ctrl+V keystroke) from arbitrary clients as a security
  measure. On Wayland, PhoneInputConnect's typing may silently do nothing even
  though the phone shows the message as delivered. This is a compositor
  policy, not something PhoneInputConnect can work around. If the paste keystroke
  doesn't land, the message is still sitting on the clipboard -- a manual
  Ctrl+V/Cmd+V works as a fallback.
- **First keystroke on GNOME/Mutter (Wayland sessions)**: newer Mutter
  versions gate synthetic input -- including the legacy
  XTest-via-XWayland path `enigo` uses here -- behind the
  `RemoteDesktop` portal's consent dialog, even in an otherwise-X11/
  Xwayland session. Expect a one-time "allow remote desktop
  interaction"-style prompt the first time PhoneInputConnect actually sends a
  keystroke; typing works normally after it's granted. Distributions
  that default to a plain Xorg session (no Wayland/Mutter involved at
  all) shouldn't see this prompt in the first place -- Zorin OS has
  historically defaulted to Xorg for broader hardware/driver
  compatibility, for instance, though this isn't independently
  confirmed for the exact version you're running; check with
  `echo $XDG_SESSION_TYPE`.
- Delivery works by placing the message on the clipboard and simulating a
  paste, specifically *because* simulating individual keypresses (the
  previous approach, via `libxdo` on Linux) only has keycodes for the
  physical keyboard layout: non-Latin text (CJK, etc.) needed a synthetic
  Unicode keysym trick that most IMEs -- built to interpret real keystrokes
  as composition input, not to accept a pre-composed character outright --
  would silently drop or mangle. Pasting sidesteps that entirely.
- **Windows/macOS don't have the native window yet**: they still use the
  tray icon + browser dashboard from before (`tray/native.rs`), including
  whatever tray-icon-visibility quirks that platform has. This is a
  temporary gap, not an intended long-term difference.
