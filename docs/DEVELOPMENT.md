# Development notes

Small, hard-won gotchas that aren't obvious from the code. Add to this file
when you burn time on something a future contributor will hit too.

## macOS: getting typing to work while developing

Symptom: the app runs, the phone connects, messages show up in the app window's
history — but **nothing is typed into other apps**. This is almost always the
Accessibility permission not being in effect for the process you're running, not
a bug.

### Why the debug binary in the Accessibility list doesn't help

Delivering a message simulates a Cmd+V keystroke, which macOS gates behind
**Accessibility** trust (`AXIsProcessTrusted`). For a *bundled* app the grant
attaches to the app. But when you run the **bare binary** (`cargo run`, or
`./target/debug/phone-input-connect`), macOS attributes the input request to the
**responsible process** — the terminal (or IDE) that launched it — *not* to the
binary. So adding `phone-input-connect` to System Settings → Privacy & Security →
Accessibility does nothing; the toggle that actually matters is on your terminal.

On top of that, an unsigned/ad-hoc binary's code hash changes on **every
rebuild**, so any grant tied to the binary itself goes stale the next
`cargo build`.

### The fix (pick one)

**A. Grant your terminal (best for fast iteration).**

1. System Settings → Privacy & Security → **Accessibility**.
2. Add and enable the app you launch `cargo run` from:
   - plain **Terminal.app** or **iTerm** → add that
   - **VS Code** integrated terminal → add **Visual Studio Code**
   - **JetBrains** (RustRover/CLion) → add that IDE
3. **Fully quit and reopen** that terminal/IDE — macOS reads the grant only at
   process start, so an already-running terminal won't pick it up.
4. `cargo run` again. This survives rebuilds, since it's the terminal that's
   trusted, not the binary.

Whichever terminal you use, that exact app is what must be in the list. If you
switch terminals (Terminal → iTerm, or run from a different IDE), grant the new
one too.

**B. Run the real bundle.** It's its own responsible process and prompts for its
own grant:

```sh
./scripts/build-macos-app.sh && open target/release/PhoneInputConnect.app
```

Downside: the ad-hoc signature changes each build, so you re-grant after every
rebuild. Fine for a final check, tedious for iteration.

### Confirm before chasing settings

`env_logger` prints nothing by default, so run with logging and read the one
line printed at startup:

```sh
RUST_LOG=info cargo run
```

- `macOS Accessibility: trusted -- pasting into other apps is enabled.` → good.
- `macOS Accessibility: NOT trusted -- approve the permission prompt...` → the
  terminal (or bundle) still isn't granted, or wasn't restarted after granting.

Also make sure some **other** app has focus when you send — if PhoneInputConnect's
own window is focused, the paste has nowhere useful to go.

## Linux/Wayland

Synthetic input (both the clipboard write and the Ctrl+V) is blocked by many
Wayland compositors; the first real keystroke can trigger a one-time "allow
remote desktop interaction" portal prompt. See the README "Platform notes" —
same behavior applies to the phone page's Enter/⌫/Esc keys, which additionally
have **no clipboard fallback** if the compositor drops them.
