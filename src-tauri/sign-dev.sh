#!/bin/sh
# Re-sign the dev binary with Keychain entitlements.
# Run after `cargo build` or use as beforeDevCommand.
codesign --entitlements Entitlements.plist --force -s - target/debug/fistbump 2>/dev/null
