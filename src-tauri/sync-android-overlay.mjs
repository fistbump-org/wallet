// Copies tracked Kotlin overlay files into the gitignored gen/android tree
// before each `tauri android dev` or `tauri android build`. Keeps our custom
// MainActivity/BrowserBridge/BiometricBridge in version control even though
// Tauri regenerates the surrounding Android project from templates.
import { cpSync, existsSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const src = resolve(here, 'android-overlay');
const dst = resolve(here, 'gen/android');

if (!existsSync(dst)) process.exit(0);
cpSync(src, dst, { recursive: true, force: true });
