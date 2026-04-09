#!/usr/bin/env bash
#
# build-icons.sh — rasterize icon.svg into the PNG sizes the Chrome
# extension manifest references (16, 32, 48, 128) plus a 1024 master
# for high-DPI uses / Chrome Web Store listing assets.
#
# Each size is rendered directly from the SVG (not downscaled from
# the 1024 PNG) so fine details like the squircle corner stay sharp
# at small sizes.
#
# Usage:
#   cd wallet/extension/icons
#   ./build-icons.sh
#
# Requires: rsvg-convert.

set -euo pipefail
cd "$(dirname "$0")"

if ! command -v rsvg-convert >/dev/null 2>&1; then
    echo "error: rsvg-convert not found — install with 'brew install librsvg'" >&2
    exit 1
fi

SIZES=(16 32 48 128 1024)

for size in "${SIZES[@]}"; do
    out="icon-${size}.png"
    rsvg-convert -w "$size" -h "$size" icon.svg -o "$out"
    echo "  rendered ${out} (${size}x${size})"
done
