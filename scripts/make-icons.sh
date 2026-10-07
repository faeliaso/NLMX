#!/usr/bin/env bash
# Regenerates the app icons in apps/desktop/src-tauri/icons from icons/source.png (square art).
# macOS layout: the art is cut to a rounded square of 896 px (radius 200) centred on a
# transparent 1024 px canvas, then `cargo tauri icon` writes every size (incl. icon.icns).
# Needs ImageMagick (`magick`) — development only, nothing of it ships in the app.
set -euo pipefail

ICONS="$(cd "$(dirname "$0")/.." && pwd)/apps/desktop/src-tauri/icons"
SOURCE="${1:-$ICONS/source.png}"
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

magick "$SOURCE" -resize 896x896 "$WORK/art.png"
magick -size 896x896 xc:black -fill white -draw "roundrectangle 0,0,895,895,200,200" "$WORK/mask.png"
magick "$WORK/art.png" "$WORK/mask.png" -alpha off -compose CopyOpacity -composite "$WORK/rounded.png"
magick "$WORK/rounded.png" -background none -gravity center -extent 1024x1024 "PNG32:$WORK/master.png"

cargo tauri icon "$WORK/master.png" -o "$WORK/out" >/dev/null 2>&1
cp "$WORK/out"/{32x32,128x128,128x128@2x,icon}.png "$WORK/out/icon.icns" "$ICONS/"
echo "icons updated in $ICONS"
