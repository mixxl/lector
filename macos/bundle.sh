#!/usr/bin/env bash
# Crea Lector.app (universal: Apple Silicon + Intel) y Lector-macOS.zip
set -euo pipefail
cd "$(dirname "$0")/.."
APP=Lector.app
rm -rf "$APP" icon.iconset Lector-macOS.zip
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources" icon.iconset
lipo -create -output "$APP/Contents/MacOS/lector" \
  target/aarch64-apple-darwin/release/lector \
  target/x86_64-apple-darwin/release/lector
cp macos/Info.plist "$APP/Contents/"
for s in 16 32 128 256 512; do
  sips -z $s $s macos/icon.png --out icon.iconset/icon_${s}x${s}.png >/dev/null
  d=$((s*2))
  sips -z $d $d macos/icon.png --out icon.iconset/icon_${s}x${s}@2x.png >/dev/null
done
iconutil -c icns icon.iconset -o "$APP/Contents/Resources/AppIcon.icns"
rm -rf icon.iconset
codesign --force --deep -s - "$APP"
ditto -c -k --keepParent "$APP" Lector-macOS.zip
# imagen de disco con acceso directo a Aplicaciones
rm -rf dmg Lector.dmg
mkdir dmg
cp -R "$APP" dmg/
ln -s /Applications dmg/Aplicaciones
hdiutil create -volname "Lector" -srcfolder dmg -ov -format UDZO Lector.dmg
rm -rf dmg
echo "Listo: Lector-macOS.zip y Lector.dmg"
