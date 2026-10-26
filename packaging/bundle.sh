#!/bin/zsh
# Builds a release binary and wraps it in "Space Hog Inspector.app".
# Usage: packaging/bundle.sh            -> target/release/Space Hog Inspector.app
set -euo pipefail

cd "${0:A:h}/.."
APP_NAME="Space Hog Inspector"
BIN="space-hog-inspector"
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
OUT="target/release/$APP_NAME.app"

cargo build --release

# App icon: render once, then cache in packaging/AppIcon.icns.
if [[ ! -f packaging/AppIcon.icns ]]; then
    work=$(mktemp -d)
    swift packaging/make_icon.swift "$work/icon_1024.png"
    set_dir="$work/AppIcon.iconset"
    mkdir -p "$set_dir"
    for s in 16 32 128 256 512; do
        sips -z $s $s "$work/icon_1024.png" --out "$set_dir/icon_${s}x${s}.png" >/dev/null
        sips -z $((s * 2)) $((s * 2)) "$work/icon_1024.png" --out "$set_dir/icon_${s}x${s}@2x.png" >/dev/null
    done
    iconutil -c icns "$set_dir" -o packaging/AppIcon.icns
    rm -rf "$work"
fi

rm -rf "$OUT"
mkdir -p "$OUT/Contents/MacOS" "$OUT/Contents/Resources"
cp "target/release/$BIN" "$OUT/Contents/MacOS/$BIN"
cp packaging/AppIcon.icns "$OUT/Contents/Resources/AppIcon.icns"
sed "s/__VERSION__/$VERSION/g" packaging/Info.plist > "$OUT/Contents/Info.plist"

# Sign with a real certificate when there is one: macOS ties privacy grants
# such as Full Disk Access to the signature, and an ad-hoc signature changes
# on every build, which would silently drop the grant after each rebuild.
# Override with SIGN_IDENTITY="<name or SHA-1>" or SIGN_IDENTITY=- (ad-hoc).
IDENTITY="${SIGN_IDENTITY:-$(security find-identity -p codesigning -v 2>/dev/null \
    | sed -n 's/.*) \([0-9A-F]\{40\}\) "Apple Development.*/\1/p' | head -1)}"
IDENTITY="${IDENTITY:--}"
codesign --force --deep --sign "$IDENTITY" "$OUT"
if [[ "$IDENTITY" == "-" ]]; then
    echo "Signed ad-hoc: Full Disk Access must be granted again after each rebuild."
else
    echo "Signed with: $(codesign -dvv "$OUT" 2>&1 | sed -n 's/^Authority=//p' | head -1)"
fi

echo "Built $OUT"
