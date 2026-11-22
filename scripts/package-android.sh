#!/usr/bin/env bash
# Build the Android app into dist/Nexus-<version>-android.apk: one APK for phones and tablets (arm64-v8a and
# armeabi-v7a) and for emulators and Chromebooks (x86_64). Android 8.0 or newer.
#
# Needs the Android SDK (build-tools and one platform) and NDK, found through ANDROID_HOME / ANDROID_SDK_ROOT and
# ANDROID_NDK_HOME / ANDROID_NDK_LATEST_HOME (GitHub's Linux and macOS runners have all of it), plus a JDK for the
# signing tools. No Gradle: the app is a single shared library, packaged with aapt2, zipalign and apksigner.
#
# Signing: with ANDROID_KEYSTORE (path to a keystore), ANDROID_KEYSTORE_PASSWORD and ANDROID_KEY_ALIAS (and
# ANDROID_KEY_PASSWORD if the key has its own) the APK is signed with that key. Android only installs an update over
# an app signed with the same key, so keep it. Without them the APK is signed with a throwaway key made for this
# build: it installs, but the next build won't update it in place (uninstall first).
set -euo pipefail
cd "$(dirname "$0")/.."
version="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n1)"
# 2.1.3 -> 20103: Android compares this number, not the name, to tell an update from a downgrade.
IFS=. read -r major minor patch <<< "${version%%-*}"
version_code=$(( major * 10000 + minor * 100 + ${patch:-0} ))
min_sdk=26      # Android 8.0
target_sdk=34   # Android 14 behaviour: the system insets the window for its bars and the keyboard, which the app relies on

sdk="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-}}"
[ -d "$sdk" ] || { echo "Set ANDROID_HOME to the Android SDK" >&2; exit 1; }
ndk="${ANDROID_NDK_HOME:-${ANDROID_NDK_LATEST_HOME:-}}"
[ -d "$ndk" ] || ndk="$(ls -d "$sdk"/ndk/* 2>/dev/null | sort -V | tail -n1)"
[ -d "$ndk" ] || { echo "Set ANDROID_NDK_HOME to the Android NDK" >&2; exit 1; }
tools="$(ls -d "$sdk"/build-tools/* 2>/dev/null | sort -V | tail -n1)"
[ -x "$tools/aapt2" ] || { echo "No build-tools in $sdk (sdkmanager 'build-tools;35.0.0')" >&2; exit 1; }
platform_jar="$(ls -d "$sdk"/platforms/android-* 2>/dev/null | sort -V | tail -n1)/android.jar"
[ -f "$platform_jar" ] || { echo "No platform in $sdk (sdkmanager 'platforms;android-35')" >&2; exit 1; }
case "$(uname -s)" in Linux) host=linux-x86_64 ;; Darwin) host=darwin-x86_64 ;; *) echo "unsupported platform: $(uname -s)" >&2; exit 1 ;; esac
bin="$ndk/toolchains/llvm/prebuilt/$host/bin"
[ -x "$bin/llvm-ar" ] || { echo "No toolchain at $bin" >&2; exit 1; }

out=target/android
rm -rf "$out"; mkdir -p "$out/apk" dist
# Rust target : Android ABI folder : the NDK's compiler prefix
for triple in aarch64-linux-android:arm64-v8a:aarch64-linux-android armv7-linux-androideabi:armeabi-v7a:armv7a-linux-androideabi x86_64-linux-android:x86_64:x86_64-linux-android; do
  IFS=: read -r target abi clang <<< "$triple"
  rustup target add "$target" >/dev/null
  var="$(printf '%s' "$target" | tr 'a-z-' 'A-Z_')"
  export "CARGO_TARGET_${var}_LINKER=$bin/$clang$min_sdk-clang"
  export "CC_${target//-/_}=$bin/$clang$min_sdk-clang" "CXX_${target//-/_}=$bin/$clang$min_sdk-clang++" "AR_${target//-/_}=$bin/llvm-ar"
  # 16 KB pages: newer devices refuse a library aligned for 4 KB ones.
  export "CARGO_TARGET_${var}_RUSTFLAGS=-C link-arg=-Wl,-z,max-page-size=16384"
  # Only Android loads the app as a shared library, so that is asked for here rather than in Cargo.toml.
  cargo rustc --release --locked --lib --target "$target" --crate-type cdylib
  mkdir -p "$out/apk/lib/$abi"
  cp "target/$target/release/libnexus_app.so" "$out/apk/lib/$abi/"
done

"$tools/aapt2" compile --dir packaging/android/res -o "$out/res.zip"
"$tools/aapt2" link -o "$out/unaligned.apk" -I "$platform_jar" --manifest packaging/android/AndroidManifest.xml \
  --version-code "$version_code" --version-name "$version" --min-sdk-version "$min_sdk" --target-sdk-version "$target_sdk" "$out/res.zip"
(cd "$out/apk" && zip -q -r -9 ../unaligned.apk lib)
"$tools/zipalign" -f 4 "$out/unaligned.apk" "$out/aligned.apk"

apk="dist/Nexus-$version-android.apk"
if [ -n "${ANDROID_KEYSTORE:-}" ]; then
  [ -f "$ANDROID_KEYSTORE" ] || { echo "ANDROID_KEYSTORE is set but $ANDROID_KEYSTORE is not a file" >&2; exit 1; }
  [ -n "${ANDROID_KEYSTORE_PASSWORD:-}" ] && [ -n "${ANDROID_KEY_ALIAS:-}" ] || { echo "ANDROID_KEYSTORE needs ANDROID_KEYSTORE_PASSWORD and ANDROID_KEY_ALIAS" >&2; exit 1; }
  export ANDROID_KEY_PASSWORD="${ANDROID_KEY_PASSWORD:-$ANDROID_KEYSTORE_PASSWORD}"
  "$tools/apksigner" sign --ks "$ANDROID_KEYSTORE" --ks-key-alias "$ANDROID_KEY_ALIAS" --ks-pass env:ANDROID_KEYSTORE_PASSWORD --key-pass env:ANDROID_KEY_PASSWORD --out "$apk" "$out/aligned.apk"
  echo "Signed with $ANDROID_KEY_ALIAS from $ANDROID_KEYSTORE"
else
  keytool -genkeypair -keystore "$out/throwaway.jks" -storepass android -keypass android -alias nexus -keyalg RSA -keysize 2048 -validity 10000 \
    -dname "CN=Nexus (unsigned build), O=mirrorsedgepro-stack" >/dev/null 2>&1
  "$tools/apksigner" sign --ks "$out/throwaway.jks" --ks-key-alias nexus --ks-pass pass:android --key-pass pass:android --out "$apk" "$out/aligned.apk"
  rm -f "$out/throwaway.jks"
  echo "No ANDROID_KEYSTORE: signed with a throwaway key (installs, but a later build won't update it in place)"
fi
rm -f "$apk.idsig"
"$tools/apksigner" verify --min-sdk-version "$min_sdk" "$apk"
ls -la dist
