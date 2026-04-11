// Symlinks tracked Kotlin overlay files into the gitignored gen/android
// tree before each `tauri android dev` or `tauri android build`. Symlinks
// (vs copies) make edits two-way: changing a file through either the
// overlay path or the Android Studio gen/ path modifies the same inode,
// so the canonical source in android-overlay/ always stays in sync.
import { readdirSync, lstatSync, unlinkSync, symlinkSync, existsSync, mkdirSync } from 'node:fs';
import { dirname, resolve, join, relative } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const srcRoot = resolve(here, 'android-overlay');
const dstRoot = resolve(here, 'gen/android');

if (!existsSync(dstRoot)) process.exit(0);

function walk(dir, rel = '') {
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const full = join(dir, entry.name);
    const entryRel = join(rel, entry.name);
    if (entry.isDirectory()) {
      walk(full, entryRel);
    } else if (entry.isFile()) {
      const dst = join(dstRoot, entryRel);
      mkdirSync(dirname(dst), { recursive: true });
      try { unlinkSync(dst); } catch {}
      symlinkSync(relative(dirname(dst), full), dst);
    }
  }
}

walk(srcRoot);
