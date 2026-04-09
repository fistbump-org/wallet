# Fistbump Browser Extension — Privacy Policy

Last updated: April 9, 2026

## Summary

**The Fistbump browser extension collects no data. It does not
communicate with any remote server. Every request made through the
extension is forwarded to the locally installed Fistbump desktop
wallet on your own computer via Chrome's native messaging API, and
nothing ever leaves your machine through the extension.**

If you're looking for a one-line answer: we don't know who you are,
we don't know what dApps you use, and we couldn't track you if we
wanted to — the extension has no analytics, no telemetry, no crash
reporting, and no network access to any origin other than the local
Fistbump desktop app.

## What the extension does

The Fistbump extension exposes a JavaScript provider object
(`window.fistbump`) to every web page you visit. Web apps
("dApps") can use this object to:

- Request your Fistbump wallet address (`window.fistbump.connect()`)
- Ask you to sign a message (`window.fistbump.signMessage(...)`)
- Build, sign, and broadcast a transaction
  (`window.fistbump.sendTx({ to, amount })`)

Every request from a dApp triggers a confirmation modal in the
Fistbump desktop wallet. Nothing happens without your explicit
approval each time. The desktop wallet, not the extension, is what
holds your keys and what actually signs or broadcasts anything.

## How it talks to the desktop wallet

The extension talks to the desktop wallet through **Chrome native
messaging** — a built-in browser API for communicating with a
trusted local process. Specifically:

1. When a dApp calls `window.fistbump.*`, the extension serializes
   the request and hands it to Chrome.
2. Chrome verifies the extension's ID against a manifest file the
   Fistbump desktop wallet registers with your browser.
3. Chrome then spawns a small bridge helper that ships inside the
   Fistbump desktop app. The bridge connects to the wallet over a
   Unix domain socket (macOS / Linux) or named pipe (Windows), both
   scoped to your user account with per-user permissions.
4. The desktop wallet handles the request, shows you the approval
   modal, and sends the response back through the bridge.

At no point does the extension open any HTTP connection, fetch any
remote resource, or contact any server outside your own computer.

## What data the extension accesses

The extension's `content_scripts` entry in the manifest uses
`<all_urls>`, which lets it inject the `window.fistbump` provider
into every page you visit. This is the same mechanism other wallet
extensions (MetaMask, Phantom, etc.) use to expose `window.ethereum`
/ `window.solana` to every page.

The content script only injects a small JavaScript file that
defines the provider object and forwards method calls to the
extension's service worker. It **does not** read page content,
form inputs, cookies, local storage, browsing history, or anything
else about the page. It does not take screenshots or record
interactions. Its only job is to make `window.fistbump` available.

The only information sent to the desktop wallet when a dApp makes
a request is:

- The `window.location.origin` of the calling page (so the wallet
  can show you which site is making the request)
- The request type (`connect`, `sendTx`, `signMessage`)
- For `sendTx`: the recipient address or Fistbump name and the
  amount you typed
- For `signMessage`: the message string and optionally a Fistbump
  name to sign with

That information is used only for the approval modal shown in the
desktop wallet on your own computer. It is not sent anywhere else.

## What data we collect

**None.** We have no servers, no analytics, no telemetry, no crash
reporting, no error logging, and no backend of any kind. The
extension does not make any network requests except through Chrome
native messaging to the local Fistbump desktop app.

If you uninstall the extension, there is nothing to delete on our
side — we never had anything.

## Third parties

The extension does not use any third-party services. It does not
load any third-party scripts, fonts, analytics libraries, or any
other remote resources.

## Permissions we ask for

- **nativeMessaging** — required to talk to the Fistbump desktop
  wallet's bridge binary. This is the only thing the extension does,
  so this permission is the whole point. Without it, the extension
  cannot function.
- **content_scripts on `<all_urls>`** — required to expose
  `window.fistbump` on every dApp. Same mechanism MetaMask etc. use.

No other permissions are requested.

## Changes to this policy

If anything about this policy ever needs to change, we'll post the
updated version at this same URL and bump the "Last updated" date
at the top of the page.

## Contact

If you have questions about this policy or the extension, open an
issue at https://github.com/fistbump-org/wallet/issues.
