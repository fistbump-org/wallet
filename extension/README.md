# Fistbump — Browser Extension

A Chrome (MV3) extension that exposes `window.fistbump` to web pages and
bridges into the Fistbump desktop app via Chrome native messaging — no
HTTP, no localhost ports from the extension's side.

This is a scaffold — only `connect` (and a static popup) is wired up so
far. Tx signing and rosenbridge-specific helpers come next.

## Loading the extension in Chrome

1. Open `chrome://extensions`.
2. Toggle **Developer mode** on (top-right).
3. Click **Load unpacked** and pick this directory (`wallet/extension/`).
4. The Fistbump icon shows up in your toolbar. Clicking it opens a small
   popup with the brand and the extension version.

The extension's ID is locked at `epflhbnbnmhicfmiepfhbldfchjoojmb` via a
stable RSA public key in `manifest.json`, so the wallet's native messaging
host JSON allow-list always recognises us regardless of where the unpacked
extension lives on disk.

## Using it from a web page

Open any page (e.g. https://example.com) and in DevTools console:

```js
await window.fistbump.connect();
// → first time from this origin: a modal pops up in the wallet desktop
//   app asking "Allow this site to connect to your wallet?"
// → after you click Allow, the promise resolves with:
//   { address: "fb1...", origin: "https://example.com" }
// → subsequent calls from the same origin skip the prompt.
```

```js
await window.fistbump.isConnected();
// → boolean — is this origin already approved?
```

If the wallet isn't running when you call `connect()`, the extension
launches it via the bundled `fistbump-bridge` native messaging host:
Chrome spawns the bridge over stdio, the bridge `open`s the wallet's
`.app`, waits for the Unix socket to come up, then forwards the request
through. No protocol-handler prompt, no "open in another app?" dialog.

## How requests reach the wallet

```
dApp page
  └─ window.fistbump.connect()
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
                                    (popup approval modal, address lookup)
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
- `injected.js` — runs in the page's main world, defines `window.fistbump`,
  posts messages to the content script.
- `content-script.js` — runs in the isolated world, injects `injected.js`
  into the page and relays messages to/from the background service worker.
- `background.js` — service worker. Receives messages from content scripts
  and translates them into native messaging exchanges with the wallet.
- `popup.html` / `popup.js` — static branded popup. No live status check
  on purpose (a check would launch the wallet via the bridge, which is
  bad UX for clicking a status icon).
- `icons/` — toolbar icons + the wordmark SVG used by the popup.

## Native messaging host

The wallet writes a small JSON manifest at:

```
~/Library/Application Support/<browser>/NativeMessagingHosts/org.fistbump.wallet.json
```

(for every Chromium-family browser the wallet finds installed). The
manifest points at the bundled `fistbump-bridge` binary inside
`Fistbump.app/Contents/Resources/` and lists our extension ID in
`allowed_origins`. The wallet writes this on every desktop launch and
cleans up the older `org.fistbump.wallet.launcher.json` from the
previous architecture if it's still around.

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
  exposes a `revoke_approved_origin` Tauri command (no UI yet).
- Same-user processes can in principle still connect to the Unix socket
  directly — that's the same trust boundary the rest of `~/.fistbump/`
  has. The user-facing approval modal is the actual gate for any
  state-changing operation.

## Not implemented yet

- `signTx` / `sendTx` (transaction signing flow)
- `signMessage`
- Real event API (`accountsChanged`, etc.)
- Per-origin revocation UI in the wallet settings page
- Rosenbridge-specific tx construction helpers
- Firefox / Safari / Linux / Windows ports (the wallet currently only
  installs the host JSON on macOS Chromium-family browsers)
