#!/bin/zsh
# Builds PhoneInputConnect.app -- a double-clickable macOS bundle, so users
# never need a terminal. Running it as a real .app also means macOS attributes
# the Accessibility permission prompt to the app itself (not to Terminal), so
# the "control this computer" grant sticks to the app.
#
# Usage: scripts/build-macos-app.sh   ->   target/release/PhoneInputConnect.app
set -euo pipefail

cd "$(dirname "$0")/.."
ROOT="$(pwd)"
APP="$ROOT/target/release/PhoneInputConnect.app"
BIN_NAME="phone-input-connect"

echo "==> Building release binary"
cargo build --release

echo "==> Assembling $APP"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$ROOT/packaging/macos/Info.plist" "$APP/Contents/Info.plist"
cp "$ROOT/target/release/$BIN_NAME" "$APP/Contents/MacOS/$BIN_NAME"

echo "==> Generating AppIcon.icns from assets/icon-256.png"
ICONSET="$(mktemp -d)/AppIcon.iconset"
mkdir -p "$ICONSET"
# icon_<pt>x<pt>[@2x].png = the pixel size to render at that slot.
for spec in "16:16" "16:32:@2x" "32:32" "32:64:@2x" "128:128" "128:256:@2x" \
            "256:256" "256:512:@2x" "512:512" "512:1024:@2x"; do
  pt="${spec%%:*}"; rest="${spec#*:}"; px="${rest%%:*}"; suffix="${rest#*:}"
  [ "$suffix" = "$rest" ] && suffix=""   # no @2x on this slot
  sips -z "$px" "$px" "$ROOT/assets/icon-256.png" \
    --out "$ICONSET/icon_${pt}x${pt}${suffix}.png" >/dev/null
done
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/AppIcon.icns"

echo "==> Ad-hoc code-signing (so macOS treats it as a stable app identity)"
codesign --force --deep --sign - "$APP"

echo
echo "==> Done: $APP"
echo "    Move it to /Applications (or double-click in place)."
echo "    On first message it asks for Accessibility -- approve once and it sticks."
