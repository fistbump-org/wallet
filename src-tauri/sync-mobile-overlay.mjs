// Symlinks tracked native overlay files into the gitignored gen/ tree
// before each `tauri android|ios dev` or `tauri android|ios build`.
// Two-way edits: changing a file through either the overlay path or the
// gen/ IDE path modifies the same inode, so the canonical sources in
// android-overlay/ and ios-overlay/ always stay in sync.
import { readdirSync, unlinkSync, symlinkSync, existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { dirname, resolve, join, relative } from 'node:path';
import { fileURLToPath } from 'node:url';
import { execFileSync } from 'node:child_process';

const here = dirname(fileURLToPath(import.meta.url));

const overlays = [
  { src: resolve(here, 'android-overlay'), dst: resolve(here, 'gen/android') },
  { src: resolve(here, 'ios-overlay'),     dst: resolve(here, 'gen/apple')   },
];

function walk(srcDir, dstDir, rel = '') {
  for (const entry of readdirSync(srcDir, { withFileTypes: true })) {
    const full = join(srcDir, entry.name);
    const entryRel = join(rel, entry.name);
    if (entry.isDirectory()) {
      walk(full, dstDir, entryRel);
    } else if (entry.isFile()) {
      const dst = join(dstDir, entryRel);
      mkdirSync(dirname(dst), { recursive: true });
      try { unlinkSync(dst); } catch {}
      symlinkSync(relative(dirname(dst), full), dst);
    }
  }
}

for (const { src, dst } of overlays) {
  if (existsSync(src) && existsSync(dst)) walk(src, dst);
}

// XcodeGen assigns any source file it can't compile to the Resources build
// phase. `Externals/` holds libapp.a — the Rust staticlib — so a bare
// `- path: Externals` copies 300+ MB of already-linked object code into the
// .app, on top of linking it. That inflates the IPA roughly tenfold and can
// draw an App Store rejection for shipping a .a inside the bundle.
//
// `buildPhase: none` keeps Externals as a link input only. gen/apple is
// gitignored and `tauri ios init` recreates project.yml from scratch, so the
// setting has to be re-applied on every build rather than fixed once by hand.
function pinExternalsBuildPhase(projectYml) {
  if (!existsSync(projectYml)) return;
  const lines = readFileSync(projectYml, 'utf8').split('\n');
  const i = lines.findIndex((l) => /^\s*-\s*path:\s*Externals\s*$/.test(l));
  if (i === -1) return;
  if (/^\s*buildPhase:/.test(lines[i + 1] ?? '')) return; // already pinned
  const indent = `${lines[i].match(/^\s*/)[0]}  `;
  lines.splice(i + 1, 0, `${indent}buildPhase: none`);
  writeFileSync(projectYml, lines.join('\n'));
  console.log('[overlay] pinned Externals to buildPhase: none (keeps libapp.a out of the .app)');
}

// App Store Connect requires CFBundleVersion — the "Build" number — to
// increase with every upload within a marketing version. Tauri stamps it from
// the app version, so every 0.4.1 build was literally "0.4.1" and the second
// upload of a release was refused as a duplicate; the only escape was bumping
// the marketing version.
//
// The number lives in the tracked ios-build-number file rather than being
// derived from the commit count, so it can be bumped for a re-upload without
// inventing a commit. build-all.sh increments it on an iOS release; bump it
// by hand for a one-off. Falls back to the commit count if the file is
// missing, which is what earlier releases used (0.2.0 → 43, 0.4.0 → 53).
function stampBuildNumber(projectYml, numberFile) {
  if (!existsSync(projectYml)) return;
  let build = '';
  if (existsSync(numberFile)) build = readFileSync(numberFile, 'utf8').trim();
  if (!/^\d+$/.test(build)) {
    try {
      build = execFileSync('git', ['rev-list', '--count', 'HEAD'], { cwd: here, encoding: 'utf8' }).trim();
    } catch {
      return; // no file, no git — leave whatever Tauri wrote
    }
  }
  if (!/^\d+$/.test(build)) return;
  const src = readFileSync(projectYml, 'utf8');
  const next = src.replace(/^(\s*CFBundleVersion:\s*).*$/m, `$1"${build}"`);
  if (next === src) return;
  writeFileSync(projectYml, next);
  console.log(`[overlay] stamped CFBundleVersion ${build}`);
}

pinExternalsBuildPhase(resolve(here, 'gen/apple/project.yml'));
stampBuildNumber(resolve(here, 'gen/apple/project.yml'), resolve(here, 'ios-build-number'));
