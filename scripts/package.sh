#!/usr/bin/env bash
# Build the release binaries and this platform's installer(s) into dist/.
#   Linux:   dist/Nexus-<version>-linux-<amd64|arm64>.deb                (needs cargo-deb; installed if missing)
#   macOS:   dist/Nexus-<version>-mac-<arm64|x64>.zip                    (both architectures)
#   Windows: dist/Nexus-Setup-<version>.exe                              (needs NSIS's makensis; run from Git Bash)
# Code signing is optional and switches on when the certificates are there: WINDOWS_CERT_PFX (path to a .pfx) with
# WINDOWS_CERT_PASSWORD signs the programs and the installer; see scripts/package-macos.sh for macOS.
set -euo pipefail
cd "$(dirname "$0")/.."
version="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n1)"
mkdir -p dist
case "$(uname -s)" in
  Linux)
    command -v cargo-deb >/dev/null || cargo install cargo-deb --locked
    cargo build --release --locked
    arch="$(dpkg --print-architecture)"
    cargo deb --no-build --output "dist/Nexus-$version-linux-$arch.deb"
    ;;
  Darwin)
    for pair in aarch64-apple-darwin:arm64 x86_64-apple-darwin:x64; do
      target="${pair%%:*}"; arch="${pair##*:}"
      rustup target add "$target"
      cargo build --release --locked --target "$target"
      scripts/package-macos.sh "$version" "$target" "$arch"
    done
    ;;
  MINGW* | MSYS* | CYGWIN*)
    cargo build --release --locked
    winsign() {
      [ -n "${WINDOWS_CERT_PFX:-}" ] || return 0
      local tool
      tool="$(command -v signtool || ls -d "/c/Program Files (x86)/Windows Kits/10/bin/"*/x64/signtool.exe 2>/dev/null | sort -V | tail -n1)"
      [ -n "$tool" ] || { echo "WINDOWS_CERT_PFX is set but signtool was not found" >&2; exit 1; }
      MSYS_NO_PATHCONV=1 MSYS2_ARG_CONV_EXCL='*' "$tool" sign /fd SHA256 /f "$(cygpath -w "$WINDOWS_CERT_PFX")" /p "${WINDOWS_CERT_PASSWORD:-}" /tr http://timestamp.digicert.com /td SHA256 "$@"
    }
    winsign 'target\release\nexus.exe' 'target\release\nexus-mcp.exe'
    makensis="$(command -v makensis || true)"
    [ -n "$makensis" ] || makensis="/c/Program Files (x86)/NSIS/makensis.exe"
    [ -x "$makensis" ] || { choco install nsis -y --no-progress; }
    # NSIS takes Windows paths and /D switches; MSYS would rewrite both.
    (cd packaging/windows && MSYS_NO_PATHCONV=1 MSYS2_ARG_CONV_EXCL='*' "$makensis" "/DVERSION=$version" '/DBIN=..\..\target\release' "/DOUT=..\\..\\dist\\Nexus-Setup-$version.exe" nexus.nsi)
    winsign "dist\\Nexus-Setup-$version.exe"
    ;;
  *) echo "unsupported platform: $(uname -s)" >&2; exit 1 ;;
esac
ls -la dist
