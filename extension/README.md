# Fistbump — Browser Extension

A Chrome (MV3) extension that exposes `window.fistbump` to web pages and
bridges into the Fistbump desktop app via Chrome native messaging — no
HTTP, no localhost ports from the extension's side.

Supports connect, sendTx, and signMessage (with optional
sign-as-a-Fistbump-name). Every state-changing request pops a review
modal in the wallet before anything happens.

Works on **macOS, Linux, and Windows**. The wallet↔bridge IPC uses a
single cross-platform local-socket API (Unix domain sockets on Unix,
named pipes on Windows) via the `interprocess` crate, and the wallet
installs the native-messaging host registration using whatever
mechanism the OS's Chromium-family browsers expect:
`~/Library/Application Support/<browser>/NativeMessagingHosts/` on
macOS, `~/.config/<browser>/NativeMessagingHosts/` on Linux, and
per-user registry keys under
`HKCU\Software\<vendor>\<browser>\NativeMessagingHosts\` on Windows.

## Loading the extension in Chrome

1. Open `chrome://extensions`.
2. Toggle **Developer mode** on (top-right).
3. Click **Load unpacked** and pick this directory (`wallet/extension/`).
4. The Fistbump icon shows up in your toolbar. Clicking it opens a small
   popup showing the extension version and a live "wallet running" dot.

The extension's ID is locked at `epflhbnbnmhicfmiepfhbldfchjoojmb` via a
stable RSA public key in `manifest.json`, so the wallet's native messaging
host JSON allow-list always recognises us regardless of where the unpacked
extension lives on disk.

## Using it from a web page

Open any page (e.g. https://example.com) and in DevTools console:

```js
// ── connect ──────────────────────────────────────────────
await window.fistbump.connect();
// → first time from this origin: a modal pops up in the wallet desktop
//   app asking "Allow this site to connect to your wallet?"
// → after you click Allow, the promise resolves with:
//   { address: "fb1...", origin: "https://example.com" }
// → subsequent calls from the same origin skip the prompt.

await window.fistbump.isConnected();
// → boolean — is this origin already approved?


// ── sendTx ───────────────────────────────────────────────
// `to` accepts either a raw fb1… address or a Fistbump name — the
// wallet runs `resolveaddress` on names before showing the review modal.
await window.fistbump.sendTx({ to: 'eskimo', amount: 1.5 });
// → review modal shows the recipient, fee, and total; on approve the
//   wallet builds + signs + broadcasts and resolves with { txid }


// ── signMessage ──────────────────────────────────────────
// No name → signs with the active wallet's receive address.
await window.fistbump.signMessage('hello');
// → { signature, address }

// With a name → signs with the key that owns that Fistbump name via
// fbd's signmessagewithname RPC. The wallet first verifies ownership
// by calling getnameinfo + validateaddress.ismine; if the current
// wallet doesn't own the name, the request is denied before the
// review modal is shown.
await window.fistbump.signMessage('hello', { name: 'eskimo' });
// → { signature, name }
```

If the wallet isn't running when you call `connect()` (or any other
request), the extension launches it via the bundled `fistbump-bridge`
native messaging host: Chrome spawns the bridge over stdio, the bridge
`open`s the wallet's `.app`, waits for the Unix socket to come up, then
forwards the request through. No protocol-handler prompt, no "open in
another app?" dialog.

The popup's status probe is a special case: it sends a `type: 'info'`
frame that the bridge short-circuits without auto-launching the wallet,
so clicking the toolbar icon to check status never boots the desktop app
as a side-effect.

## How requests reach the wallet

```
dApp page
  └─ window.fistbump.connect() / sendTx() / signMessage()
     └─ injected.js (page main world)
         └─ postMessage → content-script.js (extension isolated world)
             └─ chrome.runtime.sendMessage → background.js (service worker)
                 └─ chrome.runtime.connectNative('org.fistbump.wallet')
                     └─ Chrome verifies our extension ID against the JSON
                        manifest's allowed_origins
                     └─ spawns Fistbump.app/Contents/Resources/fistbump-bridge
                         └─ bridge connects to ~/.fistbump/extension.sock
                             └─ if socket missing: `open Fistbump.app`,
                                wait for socket, retry
                             └─ forward bytes both directions
                                 └─ wallet handles the message
                                    (review modal, name resolution,
                                    ownership check, createtx/signtx/
                                    broadcasttx or signmessage)
                                 └─ wallet writes response back
                         └─ bridge forwards response to Chrome stdout
                     └─ Chrome delivers response on the port
                 └─ background.js resolves the promise
             └─ content-script.js postMessage → injected.js
         └─ injected.js resolves the dApp's await
```

## Files

- `manifest.json` — MV3 manifest with the stable `key`, `nativeMessaging`
  permission, host wordmark icons, and the popup definition.
- `injected.js` — runs in the page's main world, defines `window.fistbump`
  (connect, isConnected, sendTx, signMessage, and placeholder on/off for
  a future event API), posts messages to the content script.
- `content-script.js` — runs in the isolated world, injects `injected.js`
  into the page and relays messages to/from the background service worker.
- `background.js` — service worker. Receives messages from content scripts
  and translates them into native messaging exchanges with the wallet.
- `popup.html` / `popup.js` — branded popup with a live "wallet running"
  dot. The status check uses the `info` short-circuit frame so it never
  auto-launches the wallet.
- `icons/` — toolbar icons + the wordmark SVG used by the popup.

## Native messaging host

The wallet registers `fistbump-bridge` as a native messaging host on
every desktop launch, using whatever mechanism the current platform's
Chromium-family browsers read:

- **macOS** — JSON manifest at
  `~/Library/Application Support/<browser>/NativeMessagingHosts/org.fistbump.wallet.json`
  for each installed browser (Chrome, Chromium, Brave, Edge, Arc, Vivaldi).
- **Linux** — JSON manifest at
  `~/.config/<browser>/NativeMessagingHosts/org.fistbump.wallet.json`
  for each installed browser (google-chrome, chromium,
  BraveSoftware/Brave-Browser, microsoft-edge, vivaldi). Arc isn't on
  Linux, so it's absent from the list.
- **Windows** — the manifest goes to
  `%APPDATA%\Fistbump\org.fistbump.wallet.json`, and the wallet writes
  a per-user registry key at
  `HKCU\Software\<vendor>\<browser>\NativeMessagingHosts\org.fistbump.wallet`
  whose default value points at that JSON path. Chrome on Windows uses
  the registry to find native messaging manifests; it doesn't look at
  the `NativeMessagingHosts/` directory on this platform.

All three platforms use the same manifest content: it points at the
bundled `fistbump-bridge` binary (with `.exe` extension on Windows) and
lists our extension ID in `allowed_origins`. macOS and Linux also clean
up the older `org.fistbump.wallet.launcher.json` from the previous
architecture if it's still around.

If you `tauri dev` instead of building the bundled `.app`, native
messaging won't work because there's no bundled bridge binary. For dev
work, just keep the wallet running via `tauri dev` — the connect flow
won't need to launch anything.

## Security model

- Chrome verifies our stable extension ID against the host JSON's
  `allowed_origins` before spawning the bridge. Other extensions can't
  impersonate us.
- The wallet has **no HTTP listener** for the extension to reach. There's
  no localhost surface for a malicious page or other process to hit.
- The Unix socket lives at `~/.fistbump/extension.sock` with mode 0600.
  Other users on the same machine can't connect to it.
- Approved origins persist in `~/.fistbump/settings.json`. The wallet
  exposes `list_approved_origins` and `revoke_approved_origin` Tauri
  commands (no UI yet — see below).
- `sendTx` and `signMessage` both pop a review modal every time, even
  for already-approved origins, and require the wallet to be unlocked
  before signing. `signMessage(..., { name })` additionally verifies
  via `getnameinfo` + `validateaddress.ismine` that the current wallet
  actually owns the name — a hostile dApp can't trick the user into
  signing "as eskimo" if they don't control that name.
- Same-user processes can in principle still connect to the Unix socket
  directly — that's the same trust boundary the rest of `~/.fistbump/`
  has. The user-facing approval modal is the actual gate for any
  state-changing operation.

## Not implemented yet

- Real event API (`accountsChanged`, `disconnect`, etc.). `on`/`off`
  exist on `window.fistbump` but don't fire anything yet.
- Per-origin revocation UI in the wallet settings page. The backend
  commands (`list_approved_origins`, `revoke_approved_origin`) exist.
- Firefox and Safari support. Firefox's native messaging API works the
  same way as Chrome's but uses different host install locations, and
  Safari extensions use a different (non-WebExtensions) API entirely.
