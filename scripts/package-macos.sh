#!/usr/bin/env bash
# Build Nexus.app and zip it, from a finished release build (macOS only).
#   scripts/package-macos.sh <version> <rust target, e.g. aarch64-apple-darwin> <arch label: arm64 | x64>
# Output: dist/Nexus-<version>-mac-<arch>.zip (unzip, then drag Nexus into Applications). There is no disk image:
# antivirus engines flagged the unsigned .dmg of 2.0.0 while passing the same app in the .zip.
# Signing: with MACOS_SIGN_IDENTITY set (a "Developer ID Application: …" certificate in the keychain) the app is signed
# with the hardened runtime, and with APPLE_ID, APPLE_TEAM_ID and APPLE_APP_PASSWORD also set
# it is notarized and stapled, so Gatekeeper opens them without a warning. Without those the app is ad-hoc signed
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
name="Nexus-$version-mac-$arch"
rm -f "$root/dist/$name.dmg" "$root/dist/$name.zip"
if [ -n "${MACOS_SIGN_IDENTITY:-}" ] && [ -n "${APPLE_ID:-}" ] && [ -n "${APPLE_TEAM_ID:-}" ] && [ -n "${APPLE_APP_PASSWORD:-}" ]; then
  # Notarization takes an archive; the ticket is then stapled to the app itself, which is zipped again below.
  (cd "$stage" && ditto -c -k --keepParent Nexus.app "$stage/notarize.zip")
  xcrun notarytool submit "$stage/notarize.zip" --apple-id "$APPLE_ID" --team-id "$APPLE_TEAM_ID" --password "$APPLE_APP_PASSWORD" --wait
  xcrun stapler staple "$app"
  rm -f "$stage/notarize.zip"
fi
(cd "$stage" && ditto -c -k --keepParent Nexus.app "$root/dist/$name.zip")
rm -rf "$stage"
echo "dist/$name.zip"
