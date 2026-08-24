# PhoneInputConnect

Type on your desktop by texting it from your phone.

PhoneInputConnect runs as a small app on your computer. It shows a QR code; scan
it with your phone (same Wi-Fi network, no app install), type a message in
the page that opens, and it's instantly typed into whatever window has
focus on your desktop -- as if you'd typed it yourself.

**Platform status**: every platform opens a native window showing the QR
code/status/history on launch. On Linux it's a GTK4 window
(`window/linux.rs`); on Windows a `native-windows-gui`/Win32 window
(`window/windows.rs`); on macOS native AppKit widgets in a `winit` window
with a menu-bar icon (`tray/native.rs`, `tray/appkit_dashboard.rs`).
Closing the Linux/Windows window quits the app entirely, including the
pairing server; on macOS closing just hides the window (the menu-bar
icon's "Show window" brings it back, "Quit" exits). The Linux and macOS
windows have been built and tested on real machines; the Windows one is
unverified -- written from documented APIs with no Windows machine
available to build or run it against.

## Why

Typing on a phone keyboard is often faster or more comfortable than reaching
for a desktop keyboard -- e.g. dictating a note, pasting a password from a
phone-based manager, or entering text one-handed. PhoneInputConnect turns your
phone into an ad hoc keyboard for whatever you're focused on, with no
account, no cloud service, and no app to install.

## How it works

1. Launch PhoneInputConnect. A window showing a QR code opens directly --
   a GTK4 window on Linux, a Win32 window on Windows, native AppKit
   widgets on macOS (which also gets a menu-bar icon). The same QR
   code/status/history is still reachable as a browser **dashboard** page
   at `https://127.0.0.1:<port>/dashboard` if you want it in a real
   browser tab.
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

The window itself *is* the app on every platform, so there's nothing to
lose track of. Relaunching while an instance is already running just logs
that it's already up rather than starting a second one or bringing the
existing window forward. (On macOS, where closing hides the window rather
than quitting, the menu-bar icon's "Show window" brings it back.)

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

**`.deb` and `.rpm` packages**, with the icon/desktop entry handled
properly (an installed icon theme entry rather than an absolute path,
standard `/usr/bin` install), are built automatically for x86_64 on
every tagged release -- see [Releases](../../releases) for prebuilt
ones, or run `cargo install cargo-deb && cargo deb` /
`cargo install cargo-generate-rpm && cargo build --release && cargo generate-rpm`
yourself. That needs to run on an actual x86_64 machine: this project is
developed on aarch64, which can't reliably cross-compile a
GTK4/libxdo-linked binary for x86_64 (see
`.github/workflows/release.yml`, which builds the `.deb` on a real
x86_64 Ubuntu GitHub Actions runner and the `.rpm` inside an actual
Fedora container, so each package's automatic dependency detection
reflects its own ecosystem's real conventions).

### Windows

The window is a native [`native-windows-gui`](https://docs.rs/native-windows-gui)
(Win32) UI. No extra system packages are required -- `cargo run` builds
and runs as normal. **Unverified**: written and formatted, but never
actually compiled or run, since this has only ever been developed from
Linux/macOS machines with no Windows available to test against. If you
hit a build error or a runtime issue, that's expected until someone
actually tries it; please report back what broke.

History doesn't have the Linux window's per-row hover-to-copy button --
`native-windows-gui`'s plain controls don't support per-item interactive
widgets without a much larger owner-drawn-ListView undertaking. Instead
it's a read-only text box with a single "Copy last message" button.

### macOS

The window is drawn with native AppKit widgets via
[`objc2`](https://docs.rs/objc2) (`NSStackView` of
`NSTextField`/`NSImageView`/`NSButton`), hosted in a
[`winit`](https://docs.rs/winit) window with a
[`tray-icon`](https://docs.rs/tray-icon) menu-bar icon. No extra system
packages are required -- `cargo run` builds and runs as normal.

**Build a double-clickable app**: `cargo run` is fine for development,
but to get a normal app users launch from Finder/Dock (no terminal),
build the bundle:

```sh
./scripts/build-macos-app.sh   # -> target/release/PhoneInputConnect.app
```

Move `PhoneInputConnect.app` to `/Applications` (or run it in place).
Delivering messages into other apps simulates a Cmd+V keystroke, which
macOS gates behind **Accessibility** permission: the first launch pops a
"PhoneInputConnect would like to control this computer" prompt -- approve
it once (System Settings > Privacy & Security > Accessibility) and it
sticks to the app. Running the bare binary instead (e.g. `cargo run`)
can't hold that grant on its own; it borrows the *launching terminal's*
Accessibility permission, so the packaged app is the supported way to
run it. Prebuilt Apple Silicon (arm64) `.app` bundles are attached to
each tagged [release](../../releases) (see
`.github/workflows/release-macos.yml`).

**Picking the LAN address** (any platform): the QR must encode an address
the phone can actually reach. The app prefers a physical Wi-Fi/Ethernet
interface and skips VPN/tunnel interfaces automatically, but if it still
guesses wrong (unusual multi-interface setups), set
`PHONE_INPUT_CONNECT_LAN_IP` to the right IPv4 address to override
detection.

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
  remote address. Every platform's native window gets these same live
  updates directly in-process instead of over this socket (the GTK,
  Win32, and AppKit widgets are all fed straight from the snapshot
  stream, no network connection involved), but the page is still there at
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
- **macOS native window**: native AppKit widgets in a `winit` window with
  a menu-bar icon (`tray/native.rs`, `tray/appkit_dashboard.rs`). The app
  runs under the "Regular" activation policy (a Dock icon), so it behaves
  like an ordinary windowed app; closing the window hides it and leaves
  the app in the menu bar rather than quitting. The paste keystroke needs
  Accessibility permission (see the macOS build section); until it's
  granted, messages arrive but typing into other apps silently does
  nothing.
- **Windows is unverified**: `window/windows.rs` was written from
  `native-windows-gui`'s documented API with no Windows machine
  available to compile or run it against -- unlike the Linux and macOS
  windows, which have actually been built and tested. Treat it as a
  best-effort first pass, not something known to work yet.
