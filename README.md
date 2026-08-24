# PhoneChat

Type on your desktop by texting it from your phone.

PhoneChat runs as a small tray-icon app on your computer. It shows a QR
code; scan it with your phone (same Wi-Fi network, no app install), type a
message in the browser tab that opens, and it's instantly typed into
whatever window has focus on your desktop -- as if you'd typed it yourself.

## Why

Typing on a phone keyboard is often faster or more comfortable than reaching
for a desktop keyboard -- e.g. dictating a note, pasting a password from a
phone-based manager, or entering text one-handed. PhoneChat turns your
phone into an ad hoc keyboard for whatever you're focused on, with no
account, no cloud service, and no app to install.

## How it works

1. Launch PhoneChat. It opens a **dashboard** page in your default browser
   (`https://127.0.0.1:<port>/dashboard`) showing a QR code, and a tray/menu
   bar icon appears.
2. Scan the QR code with your phone's camera. It opens a chat-style page
   served directly from your computer over your LAN.
3. Type a message on your phone and hit send. It's echoed back to your
   phone as a chat bubble, and delivered into the desktop's currently
   focused window by placing it on the clipboard and simulating a paste
   (Ctrl+V, or Cmd+V on macOS) -- your previous clipboard contents are
   restored right after.
4. The dashboard updates live: it shows "Phone connected" and a rolling
   history of the last few messages sent.

Only one phone can be paired at a time. Scanning a new QR code (via the
tray menu's "New code") invalidates the old one.

If the phone's connection drops (screen lock, backgrounded browser tab,
brief Wi-Fi blip), the same QR code/URL keeps working for about 45 seconds
in case it reconnects on its own -- no need to re-scan for a short hiccup.
Past that window, the code is invalidated for good and needs a fresh scan.

Lost the dashboard tab and there's no tray icon to get it back? Just
launch PhoneChat again -- it detects the already-running instance and
reopens its dashboard instead of starting a second one.

## Building and running

Requires a recent Rust toolchain (`cargo build`/`cargo run`).

### Linux

The tray icon uses [`ksni`](https://docs.rs/ksni), a pure-Rust
implementation of the StatusNotifierItem D-Bus protocol -- no GTK or other
system tray development packages are needed to *build*. Typing uses
[`enigo`](https://docs.rs/enigo), which on X11 links against `libxdo`, so
that needs to be installed to build and run:

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

### Windows / macOS

Uses [`tray-icon`](https://docs.rs/tray-icon) and
[`winit`](https://docs.rs/winit) for the tray/menu-bar icon and its event
loop. No extra system packages are required. `cargo run` builds and runs
as normal.

## Security

PhoneChat is designed to be safe to run on an ordinary home or office Wi-Fi
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
- The dashboard (which shows the QR code and message history, and needs no
  token) is reachable only from `127.0.0.1` -- it's served from a second,
  separate listening socket bound to loopback only, and every request is
  additionally checked against the connecting socket's own remote address.
- Nothing is persisted to disk. Chat history is an in-memory ring buffer
  (the last 10 messages) that vanishes the moment the app stops.
- Each message is placed on the system clipboard just long enough to paste
  it into the focused window, then your previous clipboard contents are
  restored. Anything else on your machine that happens to poll the
  clipboard during that brief window could observe the message in transit.

## Platform notes

- **Wayland**: most compositors block synthetic input (both the clipboard
  write and the Ctrl+V keystroke) from arbitrary clients as a security
  measure. On Wayland, PhoneChat's typing may silently do nothing even
  though the phone shows the message as delivered. This is a compositor
  policy, not something PhoneChat can work around; X11 and Xwayland
  sessions are unaffected. If the paste keystroke doesn't land, the
  message is still sitting on the clipboard -- a manual Ctrl+V/Cmd+V works
  as a fallback.
- Delivery works by placing the message on the clipboard and simulating a
  paste, specifically *because* simulating individual keypresses (the
  previous approach, via `libxdo` on Linux) only has keycodes for the
  physical keyboard layout: non-Latin text (CJK, etc.) needed a synthetic
  Unicode keysym trick that most IMEs -- built to interpret real keystrokes
  as composition input, not to accept a pre-composed character outright --
  would silently drop or mangle. Pasting sidesteps that entirely.
- **GNOME Shell (Linux)**: stock GNOME Shell has no StatusNotifierWatcher
  running, so no tray icon will appear unless you install the "AppIndicator
  and KStatusNotifierItem Support" extension. The app still runs and opens
  the dashboard; only the tray icon (and its "New code"/"Quit" menu) is
  affected -- quit with Ctrl+C in the terminal, or send the process
  `SIGINT`, instead.
