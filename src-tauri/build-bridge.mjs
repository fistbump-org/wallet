#!/usr/bin/env node
//
// Compile the fistbump-bridge native messaging host and stage it next to
// the wallet so Tauri's bundler picks it up via `bundle.resources` in
// tauri.conf.json (or tauri.windows.conf.json on Windows). Called from
// `beforeBuildCommand` so a normal `tauri build` always produces a fresh
// bridge binary.
//
// Written in Node so it runs identically on macOS, Linux, and Windows —
// Tauri already requires Node for its dev/build tooling, so we don't
// introduce a new dependency.
//
// Target selection:
//
// Tauri sets `TAURI_ENV_TARGET_TRIPLE` in the hook environment whenever
// `tauri build --target <triple>` is used (see tauri-cli's
// `rust.rs::env()`). We honour it so the bridge is built for the same
// target the wallet is being built for — critical for cross-arch cases
// like the Windows ARM build, where the Tauri build runs on an x86_64
// Windows host but targets `aarch64-pc-windows-msvc`.
//
// Special cases:
//
//   - `universal-apple-darwin`: Tauri's macOS meta-target that builds
//     both arm64 and x86_64 slices and lipos them. We mirror that for
//     the bridge so the universal wallet has a universal helper.
//   - Mobile targets (contains "android" or "ios"): the bridge is
//     desktop-only, so we leave an empty placeholder and exit — the
//     wallet still builds cleanly because bundle.resources on mobile
//     platforms is either ignored or packages the placeholder
//     harmlessly.
//   - No `TAURI_ENV_TARGET_TRIPLE` at all (e.g. `tauri build` without
//     `--target` on Linux/Windows): do a host-native cargo build.

import { spawnSync } from 'node:child_process';
import {
  chmodSync,
  copyFileSync,
  existsSync,
  mkdirSync,
  writeFileSync,
} from 'node:fs';
import { dirname } from 'node:path';
import { fileURLToPath } from 'node:url';

const __dirname = dirname(fileURLToPath(import.meta.url));
process.chdir(__dirname);

// Stage a placeholder for BOTH possible resource paths on first run
// so Tauri's build.rs doesn't choke on a missing `bundle.resources`
// entry before we've had a chance to write the real binary. These get
// overwritten below with the actual (possibly universal) binary.
mkdirSync('resources', { recursive: true });
for (const name of ['fistbump-bridge', 'fistbump-bridge.exe']) {
  const p = `resources/${name}`;
  if (!existsSync(p)) {
    writeFileSync(p, '');
  }
}

function run(cmd, args) {
  const result = spawnSync(cmd, args, { stdio: 'inherit', shell: false });
  if (result.error) {
    throw new Error(`failed to spawn ${cmd}: ${result.error.message}`);
  }
  if (result.status !== 0) {
    throw new Error(`${cmd} ${args.join(' ')} exited with ${result.status}`);
  }
}

const target = process.env.TAURI_ENV_TARGET_TRIPLE || '';

// Mobile targets: nothing to do. The bridge is only used by the browser
// extension, which is desktop-only. Exit with a success so the rest of
// the Tauri build continues.
if (target.includes('android') || target.includes('ios')) {
  console.log(`[fistbump] bridge skipped for mobile target ${target}`);
  process.exit(0);
}

// macOS universal meta-target: two cargo builds + lipo. Also the default
// path when we're on darwin with no explicit target (common for local
// `tauri dev` / `tauri build` invocations).
if (
  target === 'universal-apple-darwin' ||
  (process.platform === 'darwin' && !target)
) {
  buildUniversalDarwin();
} else {
  buildSingle(target);
}

// ── helpers ──────────────────────────────────────────────────────

// Resolve the directory cargo will write its build outputs to. Respects
// `CARGO_TARGET_DIR` if set (Tauri itself honours this, and the dev-repo
// build script sets it to `C:\build` on Windows to keep paths short and
// out of the OneDrive-synced repo dir). Falls back to the workspace's
// `target/` dir otherwise.
function cargoTargetBase() {
  return process.env.CARGO_TARGET_DIR || 'target';
}

function buildUniversalDarwin() {
  const arm = 'aarch64-apple-darwin';
  const intel = 'x86_64-apple-darwin';
  run('rustup', ['target', 'add', arm, intel]);
  run('cargo', ['build', '-p', 'fistbump-bridge', '--release', '--target', arm]);
  run('cargo', ['build', '-p', 'fistbump-bridge', '--release', '--target', intel]);
  const base = cargoTargetBase();
  const armBin = `${base}/${arm}/release/fistbump-bridge`;
  const intelBin = `${base}/${intel}/release/fistbump-bridge`;
  const out = 'resources/fistbump-bridge';
  run('lipo', ['-create', armBin, intelBin, '-output', out]);
  chmodSync(out, 0o755);
  const archs = spawnSync('lipo', ['-archs', out], { encoding: 'utf8' });
  console.log(
    `[fistbump] built fistbump-bridge (${(archs.stdout || 'unknown').trim()}) -> ${out}`
  );
}

function buildSingle(triple) {
  // If a concrete target was passed (e.g. cross-compile path for
  // Windows ARM), hand it straight to cargo. Otherwise let cargo pick
  // the host default.
  const args = ['build', '-p', 'fistbump-bridge', '--release'];
  if (triple) {
    run('rustup', ['target', 'add', triple]);
    args.push('--target', triple);
  }
  run('cargo', args);

  // Resolve where the compiled binary actually landed and what name
  // it should end up under inside resources/. The filename has .exe
  // on Windows (either because the host is win32 or because we're
  // building for a *-windows-* triple).
  const isWindows = triple
    ? triple.includes('windows')
    : process.platform === 'win32';

  const base = cargoTargetBase();
  const buildDir = triple ? `${base}/${triple}/release` : `${base}/release`;
  const srcName = isWindows ? 'fistbump-bridge.exe' : 'fistbump-bridge';
  const src = `${buildDir}/${srcName}`;
  const dst = `resources/${srcName}`;

  copyFileSync(src, dst);
  if (!isWindows) {
    chmodSync(dst, 0o755);
  }
  console.log(`[fistbump] built fistbump-bridge -> ${dst}`);
}
