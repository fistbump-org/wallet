#!/usr/bin/env bash
#
# build-promo-tile.sh — generate the 440x280 Chrome Web Store small
# promo tile. Composes the 1024x1024 squircle icon with the "Fistbump"
# wordmark in SF Pro Display Bold (the same font stack the wallet UI
# uses via `--fb-font-sans`). Output: promo-tile.png.
#
# Text is rendered via ImageMagick + the actual SF Pro Display OTF
# file instead of SVG + rsvg-convert, because rsvg-convert falls back
# to a generic sans-serif when -apple-system is requested and the
# result looks nothing like the app.
#
# Usage:
#   cd wallet/extension/icons
#   ./build-promo-tile.sh
#
# Requires: ImageMagick, SF Pro Display Bold at /Library/Fonts/
#           (ships with the "SF Pro" download from Apple).

set -euo pipefail
cd "$(dirname "$0")"

FONT="/Library/Fonts/SF-Pro-Display-Bold.otf"

if ! command -v magick >/dev/null 2>&1; then
    echo "error: ImageMagick not found — install with 'brew install imagemagick'" >&2
    exit 1
fi

if [ ! -f "$FONT" ]; then
    echo "error: SF Pro Display Bold not found at $FONT" >&2
    echo "download from https://developer.apple.com/fonts/ and install." >&2
    exit 1
fi

magick -size 440x280 xc:"#09090b" \
    \( icon-1024.png -resize 150x150 \) \
    -gravity northwest -geometry +40+65 -composite \
    -font "$FONT" \
    -pointsize 44 \
    -fill "#fafafa" \
    -kerning -1.2 \
    -gravity west \
    -annotate +210+0 "Fistbump" \
    promo-tile.png

echo "  rendered promo-tile.png (440x280)"
