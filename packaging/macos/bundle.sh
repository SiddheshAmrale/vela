#!/usr/bin/env bash
# Build VELA.app and a .dmg on macOS.
#
#   brew install mpv dylibbundler
#   packaging/macos/bundle.sh
#
# Optional signing + notarization (otherwise the app is ad-hoc signed and
# Gatekeeper will warn on other Macs):
#   MACOS_SIGN_IDENTITY="Developer ID Application: Your Name (TEAMID)"
#   MACOS_NOTARY_PROFILE=<keychain profile created with `xcrun notarytool store-credentials`>
set -euo pipefail
cd "$(dirname "$0")/../.."
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
ARCH=$(uname -m)
APP=dist/VELA.app

cargo build --release

rm -rf "$APP" && mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources" "$APP/Contents/Frameworks"
cp target/release/vela "$APP/Contents/MacOS/vela"
sed "s/@VERSION@/$VERSION/g" packaging/macos/Info.plist > "$APP/Contents/Info.plist"

# Icon: build/icon.png -> vela.icns
ICONSET=$(mktemp -d)/vela.iconset
mkdir -p "$ICONSET"
for s in 16 32 128 256 512; do
  sips -z $s $s build/icon.png --out "$ICONSET/icon_${s}x${s}.png" >/dev/null
  sips -z $((s * 2)) $((s * 2)) build/icon.png --out "$ICONSET/icon_${s}x${s}@2x.png" >/dev/null
done
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/vela.icns"

# Bundle libmpv and everything it links against, rewriting install names.
dylibbundler -od -b -x "$APP/Contents/MacOS/vela" -d "$APP/Contents/Frameworks" -p @executable_path/../Frameworks/ >/dev/null

if [[ -n "${MACOS_SIGN_IDENTITY:-}" ]]; then
  codesign --force --deep --options runtime --timestamp --sign "$MACOS_SIGN_IDENTITY" "$APP"
else
  codesign --force --deep --sign - "$APP"
fi

DMG="dist/VELA-$VERSION-macos-$ARCH.dmg"
rm -f "$DMG"
hdiutil create -volname VELA -srcfolder "$APP" -ov -format UDZO "$DMG" >/dev/null

if [[ -n "${MACOS_SIGN_IDENTITY:-}" && -n "${MACOS_NOTARY_PROFILE:-}" ]]; then
  codesign --sign "$MACOS_SIGN_IDENTITY" --timestamp "$DMG"
  xcrun notarytool submit "$DMG" --keychain-profile "$MACOS_NOTARY_PROFILE" --wait
  xcrun stapler staple "$DMG"
fi
echo "built $DMG"
