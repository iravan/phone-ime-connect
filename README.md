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
   phone as a chat bubble, and typed into the desktop's currently focused
   window via simulated keystrokes.
4. The dashboard updates live: it shows "Phone connected" and a rolling
   history of the last few messages sent.

Only one phone can be paired at a time. Scanning a new QR code (via the
tray menu's "New code") invalidates the old one.

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
  code's URL. It's unguessable, and is rotated the instant a phone
  successfully connects, so a QR code can't be reused to open a second,
  competing session. It also expires on its own after 5 minutes if nothing
  ever connects.
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

## Platform notes

- **Wayland**: most compositors block synthetic keyboard input from
  arbitrary clients as a security measure. On Wayland, PhoneChat's typing
  may silently do nothing even though the phone shows the message as
  delivered. This is a compositor policy, not something PhoneChat can work
  around; X11 and Xwayland sessions are unaffected.
- **Non-Latin text (CJK, etc.) on Linux/X11**: typing works by simulating
  individual keypresses (via `libxdo`), which types Latin text reliably but
  can silently drop or mangle Chinese/Japanese/Korean and other non-Latin
  characters -- most IMEs only recognize real keyboard scancodes or proper
  IME composition events, not synthetic Unicode keypresses. This fails
  quietly, with the phone still showing the message as delivered. There's
  no code-level fix for this short of switching the injection mechanism
  entirely (e.g. clipboard + simulated paste instead of keystrokes).
- **GNOME Shell (Linux)**: stock GNOME Shell has no StatusNotifierWatcher
  running, so no tray icon will appear unless you install the "AppIndicator
  and KStatusNotifierItem Support" extension. The app still runs and opens
  the dashboard; only the tray icon (and its "New code"/"Quit" menu) is
  affected -- quit with Ctrl+C in the terminal, or send the process
  `SIGINT`, instead.
