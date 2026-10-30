#!/usr/bin/env bash
# Builds "Orca Profile Studio.app" into target/release/bundle/ from a release build.
#   scripts/bundle-macos.sh            build the bundle
#   scripts/bundle-macos.sh --install  also copy it to ~/Applications/
# Only target/ is written (and ~/Applications/ with --install). The bundle is signed ad hoc for
# local use; it is not notarized for distribution.
set -euo pipefail

install=false
for arg in "$@"; do
    case "$arg" in
        --install) install=true ;;
        -h | --help) sed -n '2,6p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) echo "unknown argument: $arg" >&2; exit 2 ;;
    esac
done
[[ "$(uname -s)" == Darwin ]] || { echo "bundle-macos.sh runs on macOS only" >&2; exit 1; }

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"
name="Orca Profile Studio"
identifier="org.orca-profile-studio.app"
executable="profile-studio"
icon_source="crates/profile-studio/resources/icon.png"
version="$(cargo pkgid -p profile-studio | sed 's/.*[@#]//')"
bundle_dir="target/release/bundle"
app="$bundle_dir/$name.app"

cargo build --release --locked -p profile-studio

rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "target/release/$executable" "$app/Contents/MacOS/$executable"

# The icon set needs every size from 16 to 512 points at 1× and 2×.
iconset="$bundle_dir/AppIcon.iconset"
rm -rf "$iconset"
mkdir -p "$iconset"
if [[ ! -f "$icon_source" ]]; then
    icon_source="$bundle_dir/icon.png"
    echo "no icon source; drawing the placeholder with scripts/icon.swift" >&2
    swift scripts/icon.swift "$icon_source"
fi
for points in 16 32 128 256 512; do
    sips -z "$points" "$points" "$icon_source" --out "$iconset/icon_${points}x${points}.png" >/dev/null
    pixels=$((points * 2))
    sips -z "$pixels" "$pixels" "$icon_source" --out "$iconset/icon_${points}x${points}@2x.png" >/dev/null
done
iconutil -c icns "$iconset" -o "$app/Contents/Resources/AppIcon.icns"
rm -rf "$iconset"

cat >"$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleDevelopmentRegion</key>
    <string>en</string>
    <key>CFBundleLocalizations</key>
    <array>
        <string>en</string>
        <string>zh-Hans</string>
    </array>
    <key>CFBundleDisplayName</key>
    <string>$name</string>
    <key>CFBundleName</key>
    <string>$name</string>
    <key>CFBundleExecutable</key>
    <string>$executable</string>
    <key>CFBundleIconFile</key>
    <string>AppIcon</string>
    <key>CFBundleIdentifier</key>
    <string>$identifier</string>
    <key>CFBundleInfoDictionaryVersion</key>
    <string>6.0</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleShortVersionString</key>
    <string>$version</string>
    <key>CFBundleVersion</key>
    <string>$version</string>
    <key>LSApplicationCategoryType</key>
    <string>public.app-category.developer-tools</string>
    <key>LSMinimumSystemVersion</key>
    <string>11.0</string>
    <key>NSHighResolutionCapable</key>
    <true/>
</dict>
</plist>
PLIST
plutil -lint "$app/Contents/Info.plist" >/dev/null

# Ad-hoc signing binds Info.plist to the bundle, which notifications need for a stable identity.
codesign --force --sign - "$app"
codesign --verify --strict "$app"
echo "built $root/$app ($version)"

if $install; then
    target="$HOME/Applications/$name.app"
    mkdir -p "$HOME/Applications"
    rm -rf "$target"
    ditto "$app" "$target"
    echo "installed $target"
fi
