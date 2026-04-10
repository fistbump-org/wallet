# Fistbump Wallet — Privacy Policy

Last updated: April 10, 2026

## Summary

**The Fistbump wallet app collects no personal data. It has no
accounts, no analytics, no telemetry, no crash reporting, and no
backend servers. Your keys and wallet data never leave your device
unless you explicitly broadcast a transaction to the Fistbump
peer-to-peer network.**

This policy applies to the Fistbump wallet app on iOS and Android.

## What the app does

Fistbump is a self-custody cryptocurrency wallet and full node for
the Fistbump network. The app runs a node directly on your device
that connects to other nodes in the peer-to-peer network, downloads
and verifies the blockchain, and lets you send and receive
transactions.

Your private keys are generated and stored entirely on your device.
They are never transmitted to us or any third party.

## What data is stored on your device

All wallet data is stored locally on your device:

- **Private keys** — generated on your device, optionally encrypted
  with a passphrase you choose. We never have access to your keys.
- **Blockchain data** — the node downloads and verifies blocks from
  the peer-to-peer network. This is public data shared by all
  participants in the network.
- **Transaction history** — derived locally from the blockchain data
  on your device.
- **Biometric reference** — if you enable biometric unlock, your
  encrypted passphrase is stored in the platform's secure enclave
  (iOS Keychain / Android Keystore). The app never sees your
  biometric data directly — it only receives a yes/no authentication
  result from the operating system.

If you delete the app, all local data is removed.

## What network connections the app makes

The app makes two types of network connections:

1. **Fistbump peer-to-peer network** — Your node connects to other
   Fistbump nodes to download blocks and broadcast transactions.
   This is the same mechanism used by Bitcoin and other blockchain
   networks. Your device's IP address is visible to the peers you
   connect to, as is standard for any peer-to-peer protocol.

2. **Localhost only** — The wallet UI communicates with the node
   over a local-only connection on your device (127.0.0.1). This
   traffic never leaves your device.

The app does not connect to any Fistbump-operated servers, CDNs,
or cloud services. There is no "phone home" behavior of any kind.

## What data we collect

**None.** We operate no servers that receive data from the app. We
do not know who you are, what your wallet address is, what
transactions you make, or even that you have the app installed.

## Device permissions

- **Camera** (optional) — Used solely to scan QR codes when
  importing a wallet. No images are stored or transmitted. You can
  deny this permission and enter wallet data manually instead.
- **Face ID / Touch ID / Biometrics** (optional) — Used to unlock
  your wallet without typing your passphrase. Authentication is
  handled entirely by the operating system. The app never accesses
  your biometric data — it only receives a success or failure
  result. You can deny this permission and use your passphrase
  instead.
- **Internet access** — Required for the node to connect to the
  Fistbump peer-to-peer network.

## Third parties

The app does not use any third-party analytics, advertising,
tracking, or crash-reporting services. It does not embed any
third-party SDKs that collect data. It does not load remote
scripts, fonts, or resources.

## Children

The app does not knowingly collect any information from anyone,
including children. Since we collect no data at all, there is
nothing to distinguish or protect — but we want to be explicit:
the app is not directed at children under 13.

## Changes to this policy

If this policy changes, we will update it at this same URL and
update the date at the top.

## Contact

If you have questions about this policy or the app, open an issue
at https://github.com/fistbump-org/wallet/issues.
