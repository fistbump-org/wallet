#!/usr/bin/env bash
#
# build-extension.sh — package the Fistbump browser extension for upload.
#
# Produces two zips in dist/:
#
#   fistbump-extension-<version>-webstore.zip
#       For uploading to the Chrome Web Store. The `key` field is
#       stripped from manifest.json — the Web Store assigns its own
#       public key. NOTE: the first Web Store upload will give you
#       an extension ID *different* from the local-dev ID. You'll
#       need to update the wallet's `EXTENSION_ID` constant in
#       src-tauri/src/proxy/extension.rs to match, and also copy
#       the Web Store's assigned public key back into manifest.json
#       so local dev and Web Store share the same ID.
#
#   fistbump-extension-<version>-unpacked.zip
#       For internal distribution / side-loading. The `key` field
#       is kept, so side-loaded installs have the same ID the wallet
#       currently allowlists (`epflhbnbnmhicfmiepfhbldfchjoojmb`).
#
# Both zips have manifest.json at the root (no wallet/extension/
# wrapper directory) and exclude README / design assets that aren't
# part of the runtime extension.
#
# Usage:
#   cd wallet/extension
#   ./build-extension.sh
#
# Requires: jq, zip.

set -euo pipefail

cd "$(dirname "$0")"

if ! command -v jq >/dev/null 2>&1; then
    echo "error: jq not found — install with 'brew install jq'" >&2
    exit 1
fi

VERSION=$(jq -r .version manifest.json)
if [ -z "$VERSION" ] || [ "$VERSION" = "null" ]; then
    echo "error: could not read version from manifest.json" >&2
    exit 1
fi

DIST="../dist/extension"
mkdir -p "$DIST"

# ── Stage the runtime files in a temp dir ──
# Only the files that are actually loaded by Chrome at runtime. Notably
# excludes README.md (docs), icons/wordmark.svg (unused branding asset),
# this build script itself, and the build output dir.
STAGE="$(mktemp -d -t fistbump-ext.XXXXXX)"
trap 'rm -rf "$STAGE"' EXIT

mkdir -p "$STAGE/icons"
cp manifest.json "$STAGE/manifest.json"
cp background.js "$STAGE/background.js"
cp content-script.js "$STAGE/content-script.js"
cp injected.js "$STAGE/injected.js"
cp popup.html "$STAGE/popup.html"
cp popup.js "$STAGE/popup.js"
cp icons/icon-16.png "$STAGE/icons/icon-16.png"
cp icons/icon-32.png "$STAGE/icons/icon-32.png"
cp icons/icon-48.png "$STAGE/icons/icon-48.png"
cp icons/icon-128.png "$STAGE/icons/icon-128.png"

# ── unpacked variant (keeps `key`) ──
UNPACKED="${DIST}/fistbump-extension-${VERSION}-unpacked.zip"
rm -f "$UNPACKED"
(cd "$STAGE" && zip -rq "${PWD}/.tmp-unpacked.zip" .)
mv "$STAGE/.tmp-unpacked.zip" "$UNPACKED"
echo "  unpacked  → $UNPACKED"

# ── Web Store variant (strips `key`) ──
# Chrome Web Store ignores the `key` field and uses its own on upload,
# but stripping it here keeps the uploaded zip clean and avoids
# accidentally relying on the dev key in production.
jq 'del(.key)' "$STAGE/manifest.json" > "$STAGE/manifest.webstore.json"
mv "$STAGE/manifest.webstore.json" "$STAGE/manifest.json"

WEBSTORE="${DIST}/fistbump-extension-${VERSION}-webstore.zip"
rm -f "$WEBSTORE"
(cd "$STAGE" && zip -rq "${PWD}/.tmp-webstore.zip" .)
mv "$STAGE/.tmp-webstore.zip" "$WEBSTORE"
echo "  webstore  → $WEBSTORE"

# ── Summary ──
echo ""
echo "Packaged Fistbump extension v${VERSION}:"
ls -lh "$UNPACKED" "$WEBSTORE" | awk '{print "  " $NF " (" $5 ")"}'
