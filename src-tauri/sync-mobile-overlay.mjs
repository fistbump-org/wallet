// Symlinks tracked native overlay files into the gitignored gen/ tree
// before each `tauri android|ios dev` or `tauri android|ios build`.
// Two-way edits: changing a file through either the overlay path or the
// gen/ IDE path modifies the same inode, so the canonical sources in
// android-overlay/ and ios-overlay/ always stay in sync.
import { readdirSync, unlinkSync, symlinkSync, existsSync, mkdirSync } from 'node:fs';
import { dirname, resolve, join, relative } from 'node:path';
import { fileURLToPath } from 'node:url';

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
