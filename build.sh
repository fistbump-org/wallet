#!/bin/sh
set -e

DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$DIR"

# Version — read from Tauri config so `bump.sh` only has to touch the canonical file
VERSION=$(jq -r .version src-tauri/tauri.conf.json)

# Signing identity
APPLE_SIGNING_IDENTITY="Developer ID Application: Eskimo Software (5JAGPKCDD7)"

# Notarization — uses keychain profile "fistbump"
# Set up with: xcrun notarytool store-credentials "fistbump" ...
APPLE_KEYCHAIN_PROFILE="fistbump"

# Build without signing — we re-sign manually after patching the bundle
echo "Building Fistbump wallet..."
npx tauri build --target universal-apple-darwin --bundles app --config '{"bundle":{"externalBin":["binaries/fbd"]}}'

# Compile Liquid Glass icon and patch macOS bundle (macOS 26+).
# Tauri's bundler doesn't support .icon assets yet, so we compile
# with actool, copy into the bundle, and update the plist.
ICON_SRC="src-tauri/icons/fistbump.icon"
APP="src-tauri/target/universal-apple-darwin/release/bundle/macos/Fistbump.app"
if [ -d "$APP" ] && [ -d "$ICON_SRC" ]; then
  echo "Compiling Liquid Glass icon..."
  ICON_TMP="$(mktemp -d)"
  xcrun actool "$ICON_SRC" \
    --compile "$ICON_TMP" \
    --output-format human-readable-text --notices --warnings --errors \
    --output-partial-info-plist "$ICON_TMP/info.plist" \
    --app-icon fistbump --include-all-app-icons \
    --enable-on-demand-resources NO \
    --development-region en \
    --target-device mac \
    --minimum-deployment-target 26.0 \
    --platform macosx

  echo "Patching app bundle..."
  cp "$ICON_TMP/Assets.car" "$APP/Contents/Resources/Assets.car"
  [ -f "$ICON_TMP/fistbump.icns" ] && cp "$ICON_TMP/fistbump.icns" "$APP/Contents/Resources/fistbump.icns"
  rm -f "$APP/Contents/Resources/icon.icns"
  /usr/libexec/PlistBuddy -c "Set :CFBundleIconName fistbump" "$APP/Contents/Info.plist"
  /usr/libexec/PlistBuddy -c "Set :CFBundleIconFile fistbump" "$APP/Contents/Info.plist"
  rm -rf "$ICON_TMP"

  # Ensure the latest fbd binary is in the bundle (Tauri's incremental build
  # may cache the old one if only the external binary changed).
  cp src-tauri/binaries/fbd-aarch64-apple-darwin "$APP/Contents/MacOS/fbd"

  # Strip extended attributes and re-sign after modifying the bundle.
  # Inside-out: every Mach-O nested in the bundle gets signed individually
  # before we re-seal the container. Apple's notary rejects bundles with any
  # unsigned executables, and Tauri's bundler doesn't sign files copied via
  # `bundle.resources` (only the main binary and `externalBin` entries).
  # That's why fistbump-bridge needs an explicit codesign here even though
  # fbd is signed for a different reason (force-overwrite cache invalidation).
  echo "Re-signing app..."
  xattr -cr "$APP"
  codesign --force --sign "$APPLE_SIGNING_IDENTITY" --options runtime --timestamp "$APP/Contents/MacOS/fbd"
  if [ -f "$APP/Contents/Resources/fistbump-bridge" ]; then
    codesign --force --sign "$APPLE_SIGNING_IDENTITY" --options runtime --timestamp "$APP/Contents/Resources/fistbump-bridge"
  fi
  codesign --force --sign "$APPLE_SIGNING_IDENTITY" --options runtime --timestamp "$APP"

  # Rebuild DMG with the patched app
  DMG="src-tauri/target/universal-apple-darwin/release/bundle/dmg/Fistbump_${VERSION}_universal.dmg"
  echo "Rebuilding DMG..."
  mkdir -p "$(dirname "$DMG")"
  rm -f "$DMG"
  DMG_TMP="$(mktemp -d)"
  DMG_RW="$DMG_TMP/rw.dmg"
  # Create writable DMG, mount, copy contents, set volume icon, unmount
  hdiutil create -size 200m -volname "Fistbump Install" -fs HFS+ \
    -type UDIF "$DMG_RW"
  MOUNT_DIR="$(hdiutil attach -readwrite -noverify -noautoopen "$DMG_RW" \
    | grep '/Volumes/' | sed 's/.*\/Volumes/\/Volumes/')"
  cp -R "$APP" "$MOUNT_DIR/"
  ln -s /Applications "$MOUNT_DIR/Applications"
  cp "$APP/Contents/Resources/fistbump.icns" "$MOUNT_DIR/.VolumeIcon.icns"
  SetFile -a C "$MOUNT_DIR"
  hdiutil detach "$MOUNT_DIR"
  # Convert to compressed read-only DMG
  hdiutil convert "$DMG_RW" -format UDZO -o "$DMG"
  rm -rf "$DMG_TMP"
  codesign --force --sign "$APPLE_SIGNING_IDENTITY" --timestamp "$DMG"

  # Notarize the DMG
  echo "Submitting for notarization..."
  xcrun notarytool submit "$DMG" --keychain-profile "$APPLE_KEYCHAIN_PROFILE" --wait
  xcrun stapler staple "$DMG"
fi

echo ""
echo "Done! Output:"
ls -lh src-tauri/target/universal-apple-darwin/release/bundle/macos/Fistbump.app 2>/dev/null
ls -lh src-tauri/target/universal-apple-darwin/release/bundle/dmg/*.dmg 2>/dev/null
