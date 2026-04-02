# wallet

A [Fistbump](https://fistbump.org) wallet and name manager built with [Tauri](https://tauri.app).

## Compatibility

macOS, Windows, Linux, iOS, Android.

## Prerequisites

- [Node.js](https://nodejs.org)
- [Rust](https://rustup.rs)
- [Tauri CLI](https://tauri.app/start/prerequisites/)
- A compiled [fbd](https://github.com/fistbump-org/fbd) binary placed in `src-tauri/binaries/`

## Clone

```
git clone https://github.com/fistbump-org/wallet.git
cd wallet
npm install
```

## Run (development)

```
npx tauri dev
```

## Build

### macOS

```
./build.sh
```

### Other platforms

```
npx tauri build
```

See the [Tauri documentation](https://tauri.app/distribute/) for platform-specific build options.

## License

MIT
