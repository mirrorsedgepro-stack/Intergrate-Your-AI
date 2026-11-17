#!/usr/bin/env bash
# Build Nexus.app and a .dmg from a finished release build (macOS only).
#   scripts/package-macos.sh <version> <rust target, e.g. aarch64-apple-darwin> <arch label: arm64 | x64>
# Output: dist/Nexus-<version>-mac-<arch>.dmg and .zip.
# Signing: with MACOS_SIGN_IDENTITY set (a "Developer ID Application: …" certificate in the keychain) the app and
# disk image are signed with the hardened runtime, and with APPLE_ID, APPLE_TEAM_ID and APPLE_APP_PASSWORD also set
# they are notarized and stapled, so Gatekeeper opens them without a warning. Without those the app is ad-hoc signed
# (it runs, but the first time you right-click Nexus in Applications -> Open).
set -euo pipefail
version="$1"; target="$2"; arch="$3"
root="$(cd "$(dirname "$0")/.." && pwd)"
bin="$root/target/$target/release"
stage="$(mktemp -d)"
app="$stage/Nexus.app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources" "$root/dist"
cp "$bin/nexus" "$bin/nexus-mcp" "$app/Contents/MacOS/"
sed "s/__VERSION__/$version/g" "$root/packaging/macos/Info.plist" > "$app/Contents/Info.plist"

# icon.icns from the 1024px PNG.
iconset="$stage/icon.iconset"
mkdir -p "$iconset"
for s in 16 32 128 256 512; do
  sips -z "$s" "$s" "$root/assets/icon-1024.png" --out "$iconset/icon_${s}x${s}.png" >/dev/null
  sips -z "$((s * 2))" "$((s * 2))" "$root/assets/icon-1024.png" --out "$iconset/icon_${s}x${s}@2x.png" >/dev/null
done
iconutil -c icns "$iconset" -o "$app/Contents/Resources/icon.icns"
rm -rf "$iconset"

if [ -n "${MACOS_SIGN_IDENTITY:-}" ]; then
  for f in "$app/Contents/MacOS/nexus-mcp" "$app/Contents/MacOS/nexus" "$app"; do
    codesign --force --options runtime --timestamp --sign "$MACOS_SIGN_IDENTITY" "$f"
  done
  codesign --verify --strict "$app"
else
  codesign --force --deep --sign - "$app"
fi
ln -s /Applications "$stage/Applications"
name="Nexus-$version-mac-$arch"
rm -f "$root/dist/$name.dmg" "$root/dist/$name.zip"
hdiutil create -volname "Nexus $version" -srcfolder "$stage" -ov -format UDZO "$root/dist/$name.dmg" >/dev/null
if [ -n "${MACOS_SIGN_IDENTITY:-}" ]; then
  codesign --force --timestamp --sign "$MACOS_SIGN_IDENTITY" "$root/dist/$name.dmg"
  if [ -n "${APPLE_ID:-}" ] && [ -n "${APPLE_TEAM_ID:-}" ] && [ -n "${APPLE_APP_PASSWORD:-}" ]; then
    xcrun notarytool submit "$root/dist/$name.dmg" --apple-id "$APPLE_ID" --team-id "$APPLE_TEAM_ID" --password "$APPLE_APP_PASSWORD" --wait
    xcrun stapler staple "$root/dist/$name.dmg"
    xcrun stapler staple "$app"
  fi
fi
(cd "$stage" && ditto -c -k --keepParent Nexus.app "$root/dist/$name.zip")
rm -rf "$stage"
echo "dist/$name.dmg"
