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

// App icons live under gen/, which `tauri android|ios init` recreates from
// Tauri's own templates — so an init silently replaces them with the Tauri
// logo. That is exactly what happened to Android: gen/android's icons date
// from an init on 2026-06-28, 0.4.0 shipped before it, and 0.4.1 was the
// first release to go out wearing the default mark.
//
// The real icons are tracked under icons/, so mirror them in on every build
// the same way the native overlays are mirrored. Android also needs
// mipmap-anydpi-v26/ic_launcher.xml, the adaptive-icon definition that
// Tauri's template does not emit.
const icons = [
  { src: resolve(here, 'icons/android'), dst: resolve(here, 'gen/android/app/src/main/res') },
  { src: resolve(here, 'icons/ios'),     dst: resolve(here, 'gen/apple/Assets.xcassets/AppIcon.appiconset') },
];
for (const { src, dst } of icons) {
  if (existsSync(src) && existsSync(dst)) walk(src, dst);
}

// XcodeGen assigns any source file it can't compile to the Resources build
// phase. `Externals/` holds libapp.a — the Rust staticlib — so a bare
// `- path: Externals` copies 300+ MB of already-linked object code into the
// .app, on top of linking it. That inflates the IPA roughly tenfold and can
// draw an App Store rejection for shipping a .a inside the bundle.
// `buildPhase: none` keeps Externals as a link input only.
//
// `tauri ios build` does NOT run XcodeGen — only `tauri ios init` does — so
// editing project.yml alone never reaches the .xcodeproj that actually gets
// built. Re-run XcodeGen ourselves when the pin was missing, which is exactly
// the case after an init has recreated project.yml from Tauri's template.
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
  try {
    execFileSync('xcodegen', ['generate', '--quiet'], { cwd: dirname(projectYml), stdio: 'inherit' });
    console.log('[overlay] regenerated the Xcode project so the pin takes effect');
  } catch {
    console.warn('[overlay] WARNING: xcodegen not available — run it in gen/apple or libapp.a will ship inside the .app');
  }
}

// NOTE: the App Store build number (CFBundleVersion) is NOT set here.
// cargo-mobile2 runs `agvtool new-version -all <v>` after the build and before
// archiving, making it the last writer to Info.plist — so anything stamped
// earlier is overwritten. The value it uses comes from bundle.iOS.bundleVersion
// in tauri.conf.json, which is therefore the only place that works.

pinExternalsBuildPhase(resolve(here, 'gen/apple/project.yml'));
