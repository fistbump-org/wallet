# Chrome Web Store listing copy

Everything you need to paste into the Developer Dashboard to unblock
submission. Numbers in parentheses are the field limits Google enforces.

## Name (45)

Fistbump

## Summary / short description (132)

Connects dApps to the Fistbump desktop wallet. Exposes window.fistbump for connecting, signing, and sending transactions.

## Detailed description (16000)

Fistbump is the official browser extension for the Fistbump desktop
wallet. It exposes a `window.fistbump` provider to any web page, so
dApps can request the user's wallet address, ask the user to sign
messages, and build transactions — all gated behind per-request
approval modals in the desktop wallet.

Features
• Connect — dApps request access with `window.fistbump.connect()`. The
  first time a site connects, the desktop wallet pops a confirmation
  modal showing the site's origin; subsequent connects from the same
  site are silent.
• Send transactions — `window.fistbump.sendTx({ to, amount })` accepts
  either a raw fb1… address or a Fistbump name (resolved on the wallet
  side). The wallet shows a review modal with the recipient, fee, and
  total, and requires the wallet to be unlocked before signing.
• Sign messages — `window.fistbump.signMessage("hello")` signs with the
  active wallet's receive address. `window.fistbump.signMessage("hello",
  { name: "eskimo" })` signs with the key that owns a specific Fistbump
  name, after verifying ownership on the wallet side.
• Zero data collection — the extension does not talk to any remote
  server. Every request is forwarded to your locally installed Fistbump
  desktop wallet via Chrome's native messaging API.

How it works
The extension uses Chrome's native messaging API to talk to a small
bridge binary shipped inside the Fistbump desktop app. Chrome
pre-verifies the extension's ID before spawning the bridge, so no
other extension can impersonate it. The bridge forwards requests to
the wallet over a local Unix domain socket (on macOS and Linux) or
named pipe (on Windows), both of which live in your user directory
with per-user permissions.

Requirements
• The Fistbump desktop wallet must be installed. Download at
  https://fistbump.org/download.
• First-time use: launch the desktop wallet at least once so it
  registers the extension's native messaging host with your browser.

The desktop wallet, fbd node, and this extension are all open source.

## Category

Developer Tools

## Language

English (United States)

## Single purpose (200)

Exposes window.fistbump to web pages so dApps can connect to, send transactions through, and sign messages with the Fistbump desktop wallet via Chrome native messaging.

## Permission justifications

### nativeMessaging

The extension forwards dApp requests (connect, sendTx, signMessage) to the locally installed Fistbump desktop wallet via Chrome's native messaging API. This is the entire purpose of the extension — it does not communicate with any remote server. Chrome pre-verifies the extension ID against the native messaging host manifest's allowed_origins list, so only this extension can spawn the bridge.

### Host permission justification

The extension does not request any host permissions. The manifest's
`content_scripts` uses `<all_urls>` to inject the `window.fistbump`
provider into every page, which is the only way to make a global
JavaScript API available to dApps across arbitrary origins.

### <all_urls> content script justification

The `window.fistbump` provider needs to be available to dApps on any
origin, the same way wallet providers like MetaMask expose
`window.ethereum` on every page. The content script's only job is to
inject a small script that defines `window.fistbump` and relays its
method calls to the extension background service worker, which in
turn forwards them to the desktop wallet. No page content is read
or modified.

## Data usage disclosures

Check:
• **Personally identifiable information**: No
• **Health information**: No
• **Financial and payment information**: No (wallet addresses and
  transaction details are only sent to the locally installed Fistbump
  desktop app, never to any remote server or to us)
• **Authentication information**: No
• **Personal communications**: No
• **Location**: No
• **Web history**: No
• **User activity**: No
• **Website content**: No

Certification checkboxes:
• ☑ I do not sell or transfer user data to third parties, outside
  of the approved use cases.
• ☑ I do not use or transfer user data for purposes that are
  unrelated to my item's single purpose.
• ☑ I do not use or transfer user data to determine creditworthiness
  or for lending purposes.

## Privacy policy URL

You need to host the contents of `privacy-policy.md` (in this
directory) at a public URL — a page on fistbump.org is ideal. The
dashboard will ask for the URL of that hosted page.
