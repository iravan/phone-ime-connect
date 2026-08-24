#!/usr/bin/env bash
# Installs a .desktop entry pointing at this checkout's release binary, so
# PhoneChat gets a proper icon in the app launcher/dock/Alt-Tab -- GTK4
# under Wayland has no in-process way to set a window icon; it's resolved
# entirely from a desktop file matched by application ID. Re-run this
# after moving the checkout, since the binary path is baked in as-is.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
binary="$repo_root/target/release/phonechat"
icon="$repo_root/assets/icon-256.png"
template="$repo_root/packaging/linux/org.phonechat.PhoneChat.desktop.in"
dest_dir="$HOME/.local/share/applications"
dest="$dest_dir/org.phonechat.PhoneChat.desktop"

if [ ! -x "$binary" ]; then
    echo "error: $binary not found -- run 'cargo build --release' first" >&2
    exit 1
fi

mkdir -p "$dest_dir"
sed -e "s|@EXEC@|$binary|" -e "s|@ICON@|$icon|" "$template" > "$dest"

echo "Installed $dest"
echo "PhoneChat should now show up in your app launcher with its icon; look for it there instead of running the binary directly, so the desktop environment can match the window to it."
