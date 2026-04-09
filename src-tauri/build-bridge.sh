#!/bin/sh
#
# Compile the fistbump-bridge native messaging host and stage it next to the
# wallet so Tauri's bundler picks it up via `bundle.resources` in
# tauri.conf.json. Called from `beforeBuildCommand` so a normal `tauri build`
# always produces a fresh bridge binary.
#
# Kept as a script (rather than inlined into beforeBuildCommand) so the
# command line stays readable and we can chmod the output without quoting
# nightmares.

set -e

# Run from src-tauri/ regardless of where we were invoked from.
cd "$(dirname "$0")"

# Tauri's build.rs validates `bundle.resources` paths during `cargo build`,
# so the destination has to exist *before* we kick the compile off. We stage
# an empty placeholder on first run; subsequent runs overwrite with the real
# binary below.
mkdir -p resources
[ -f resources/fistbump-bridge ] || touch resources/fistbump-bridge

# fistbump-bridge lives in its own workspace member (`bridge/`) so Tauri's
# bundler doesn't pick it up as an additional binary of the wallet package.
# `-p fistbump-bridge` builds just that crate, sharing the workspace target/.
#
# We compile separately for both macOS architectures and `lipo` the results
# into a single universal Mach-O — the wallet's main bundle is universal,
# so the bridge has to be too or Intel Macs can't run it.

ARM_TARGET="aarch64-apple-darwin"
INTEL_TARGET="x86_64-apple-darwin"

# Make sure both Rust targets are installed before building. `rustup target
# add` is idempotent, so this is a no-op when they already are.
rustup target add "$ARM_TARGET" "$INTEL_TARGET" >/dev/null

cargo build -p fistbump-bridge --release --target "$ARM_TARGET"
cargo build -p fistbump-bridge --release --target "$INTEL_TARGET"

ARM_BIN="target/${ARM_TARGET}/release/fistbump-bridge"
INTEL_BIN="target/${INTEL_TARGET}/release/fistbump-bridge"
OUT="resources/fistbump-bridge"

lipo -create "$ARM_BIN" "$INTEL_BIN" -output "$OUT"
chmod +x "$OUT"

# Sanity-check the result has both slices.
ARCHES="$(lipo -archs "$OUT" 2>/dev/null || echo unknown)"
echo "[fistbump] built fistbump-bridge ($ARCHES) -> $OUT"
