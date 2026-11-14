#!/bin/bash
# Builds the Apple silicon app bundle and its ZIP:
#   dist/Talk風Live翻訳.app
#   dist/talk-style-live-translation-<version>-macos-arm64.zip (+ .sha256)
#
# Run on a Mac with Rust (1.88 or later) and the Xcode command line tools:
#   scripts/package-macos.sh [version]
# The bundle is signed ad hoc (no Apple Developer ID). See docs/MACOS.md for
# how to open it the first time.
set -euo pipefail

version="${1:-1.0.0}"
if ! [[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[A-Za-z0-9.-]+)?$ ]]; then
    echo "version must look like 1.0.0" >&2
    exit 2
fi
if [[ "$(uname -s)" != "Darwin" ]]; then
    echo "this script runs on macOS" >&2
    exit 2
fi

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"
target="aarch64-apple-darwin"
app_name="Talk風Live翻訳"
out="$root/dist"
bundle="$out/$app_name.app"
zip_name="talk-style-live-translation-$version-macos-arm64.zip"

# .cargo/config.toml makes Windows the default target; name the Mac one.
export MACOSX_DEPLOYMENT_TARGET=13.0
cargo build --release --locked --target "$target" --bin plamo-translate --bin jimoto-setup

rm -rf "$bundle" "$out/$zip_name" "$out/$zip_name.sha256"
mkdir -p "$bundle/Contents/MacOS" "$bundle/Contents/Resources"

install -m 755 "target/$target/release/plamo-translate" "$bundle/Contents/MacOS/plamo-translate"
install -m 755 "target/$target/release/jimoto-setup" "$bundle/Contents/MacOS/jimoto-setup"

# Resources the app reads through storage::app_root() (Contents/Resources).
resources="$bundle/Contents/Resources"
mkdir -p "$resources/extension/icons" "$resources/docs" "$resources/licenses"
for file in manifest.json content.js background.js options.html options.js README.md; do
    install -m 644 "extension/$file" "$resources/extension/$file"
done
for size in 16 32 48 128; do
    install -m 644 "extension/icons/icon-$size.png" "$resources/extension/icons/icon-$size.png"
done
install -m 644 README.md "$resources/README.md"
install -m 644 docs/MACOS.md "$resources/docs/MACOS.md"
install -m 644 docs/SPECIFICATIONS.md "$resources/docs/SPECIFICATIONS.md"
install -m 644 LICENSE "$resources/licenses/LICENSE"
install -m 644 assets/fonts/OFL.txt "$resources/licenses/NotoSansJP-OFL.txt"

# App icon from the PNGs the Windows build uses.
iconset="$(mktemp -d)/AppIcon.iconset"
mkdir -p "$iconset"
cp assets/icons/icon-16.png "$iconset/icon_16x16.png"
cp assets/icons/icon-32.png "$iconset/icon_16x16@2x.png"
cp assets/icons/icon-32.png "$iconset/icon_32x32.png"
cp assets/icons/icon-64.png "$iconset/icon_32x32@2x.png"
cp assets/icons/icon-128.png "$iconset/icon_128x128.png"
cp assets/icons/icon-256.png "$iconset/icon_128x128@2x.png"
cp assets/icons/icon-256.png "$iconset/icon_256x256.png"
cp assets/icons/icon-512.png "$iconset/icon_256x256@2x.png"
cp assets/icons/icon-512.png "$iconset/icon_512x512.png"
iconutil -c icns "$iconset" -o "$resources/AppIcon.icns"

cat > "$bundle/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleDevelopmentRegion</key>
    <string>ja</string>
    <key>CFBundleDisplayName</key>
    <string>$app_name</string>
    <key>CFBundleName</key>
    <string>$app_name</string>
    <key>CFBundleExecutable</key>
    <string>plamo-translate</string>
    <key>CFBundleIconFile</key>
    <string>AppIcon</string>
    <key>CFBundleIdentifier</key>
    <string>io.github.talk-style-live-translation</string>
    <key>CFBundleInfoDictionaryVersion</key>
    <string>6.0</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleShortVersionString</key>
    <string>$version</string>
    <key>CFBundleVersion</key>
    <string>$version</string>
    <key>LSMinimumSystemVersion</key>
    <string>13.0</string>
    <key>LSApplicationCategoryType</key>
    <string>public.app-category.productivity</string>
    <key>NSHighResolutionCapable</key>
    <true/>
    <key>NSSupportsAutomaticGraphicsSwitching</key>
    <true/>
    <key>NSScreenCaptureUsageDescription</key>
    <string>PCで再生中の音声を聞き取って翻訳するため、また画面の文字を翻訳するために使います。内容はこのMacの外へ送りません。</string>
    <key>NSAudioCaptureUsageDescription</key>
    <string>PCで再生中の音声を聞き取って翻訳するために使います。音声はこのMacの外へ送りません。</string>
    <key>NSHumanReadableCopyright</key>
    <string>MIT License. translated by PLaMo.</string>
</dict>
</plist>
PLIST

# Ad hoc signature: required to run on Apple silicon, and it gives the
# screen-recording permission a stable identity for this build.
codesign --force --deep --sign - --identifier io.github.talk-style-live-translation "$bundle"
codesign --verify --deep --strict "$bundle"

(cd "$out" && ditto -c -k --sequesterRsrc --keepParent "$app_name.app" "$zip_name")
(cd "$out" && shasum -a 256 "$zip_name" > "$zip_name.sha256")
echo "built $out/$zip_name"
cat "$out/$zip_name.sha256"
