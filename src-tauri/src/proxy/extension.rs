//! Browser extension <-> wallet bridge.
//!
//! The wallet listens on a Unix domain socket at `~/.fistbump/extension.sock`
//! (mode 0600). The browser extension never opens the socket directly —
//! Chrome's native messaging API spawns the small `fistbump-bridge` binary
//! we ship in the .app, which forwards length-prefixed JSON frames between
//! Chrome's stdio and our Unix socket. Native messaging guarantees Chrome
//! only spawns the bridge for our specific extension ID (locked via the
//! public key in `wallet/extension/manifest.json`), so other browser
//! extensions can't impersonate us.
//!
//! ## Wire format
//!
//! Each direction sends a 4-byte little-endian length prefix followed by
//! a JSON body. This matches Chrome's native messaging frame format
//! exactly, so the bridge is a pure byte-forwarder — it never has to
//! parse anything.
//!
//! ## Endpoints
//!
//! - `{"type": "info", "origin": "..."}` — liveness/version probe, doesn't
//!   prompt the user. Reply has `version` and `connected` (whether the
//!   origin is already in the user's allow-list).
//! - `{"type": "connect", "origin": "..."}` — request approval and return
//!   the wallet's stable receive address. The wallet pops to the front and
//!   shows a modal for new origins; auto-approves for already-allowed ones.
//!
//! ## Approval flow
//!
//! `connect` is synchronous from the extension's point of view: the
//! response only comes back once the user has answered in the wallet UI.
//! Internally we:
//!
//! 1. If the origin is already on `approvedOrigins`, return the address
//!    immediately (after waiting briefly for an active wallet).
//! 2. Otherwise stash a `PendingRequest` keyed by a fresh request id and
//!    emit `ext://request` to the wallet frontend.
//! 3. Block the listener thread on a oneshot channel until the frontend
//!    calls `resolve_ext_request` with the user's decision.
//! 4. On approve, look up the address; on deny, return an error.
//!
//! Blocking the listener thread is fine because each Unix socket
//! connection runs in its own std::thread.

use serde::Serialize;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::mpsc;
use std::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};

use crate::settings_dir;

/// Reach into Tauri's window registry, unhide + unminimize + focus the main
/// wallet window. Called whenever an extension request needs the user's
/// attention so the wallet pops to the front instead of getting buried.
/// Errors are intentionally swallowed — best-effort UX, never blocks the
/// actual request handling.
fn bring_wallet_to_front(app: &AppHandle) {
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.unminimize();
        let _ = win.show();
        let _ = win.set_focus();
    }
}

/// Snapshot of the specific window that was frontmost before we pulled
/// focus. On non-macOS platforms this is always empty and
/// `restore_previous_focus` is a no-op.
///
/// We track the window ID (not just the owning app's PID) because users
/// routinely have multiple browser windows across multiple Spaces, and
/// activating "Chrome" by PID alone lets macOS pick whichever Chrome
/// window happens to be on the current Space — which after a modal
/// interaction is usually the wallet's Space, not the one that made
/// the request.
#[derive(Clone, Debug, Default)]
struct PreviousFocus {
    #[cfg(target_os = "macos")]
    pid: Option<i32>,
    #[cfg(target_os = "macos")]
    window_id: Option<u32>,
}

/// Capture the OS-level frontmost window right before we bring the
/// wallet up. On macOS we walk CGWindowList for the topmost layer-0
/// window and stash both its owning PID and its window number; on
/// restore we raise that specific window via SkyLight's
/// `_SLPSSetFrontProcessWithOptions`, which correctly switches Spaces
/// to wherever that window lives.
///
/// Skips recording if the frontmost window is ours — in that case the
/// user was already working in the wallet and we shouldn't hand focus
/// anywhere after we're done.
fn capture_previous_focus(_app: &AppHandle) -> PreviousFocus {
    #[cfg(target_os = "macos")]
    {
        unsafe {
            let Some((pid, wid)) = frontmost_window_info() else {
                return PreviousFocus::default();
            };
            let own = libc::getpid();
            if pid == own || pid <= 0 {
                return PreviousFocus::default();
            }
            return PreviousFocus {
                pid: Some(pid),
                window_id: Some(wid),
            };
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        PreviousFocus::default()
    }
}

/// Counterpart to `capture_previous_focus` — raise the exact window
/// that was frontmost before us. No-op if nothing was captured or if
/// we're not on macOS.
///
/// This is trickier than it sounds because macOS actively resists
/// cross-app focus stealing, and even when it lets you activate a
/// different app by PID it picks "some" window owned by that app —
/// usually the one on the current Space — rather than the one the user
/// originally came from. Neither `activateFromApplication:options:`
/// (the blessed macOS 14 API) nor hiding our own windows solves this:
/// the former is window-unaware, and the latter only works on a single
/// Space.
///
/// The reliable fix is to raise the specific window via SkyLight's
/// `_SLPSSetFrontProcessWithOptions(psn, wid, kCPSUserGenerated)`. This
/// is private API, but it's the same trick yabai/Rectangle/etc. all use
/// and it's stable across recent macOS versions. macOS switches Spaces
/// to wherever the window lives and brings it to front.
fn restore_previous_focus(_app: &AppHandle, prev: &PreviousFocus) {
    #[cfg(target_os = "macos")]
    {
        if let (Some(pid), Some(wid)) = (prev.pid, prev.window_id) {
            unsafe {
                raise_window(pid, wid);
            }
        }
    }
}

// ── macOS focus FFI ─────────────────────────────────────────────────
//
// Tiny shim over CoreGraphics (CGWindowList), ApplicationServices
// (GetProcessForPID) and SkyLight (_SLPSSetFrontProcessWithOptions).
// SkyLight is a private framework so we dlsym its one function instead
// of linking against it, which avoids having to muck with rustc's
// framework search path in build.rs.
#[cfg(target_os = "macos")]
#[repr(C)]
struct ProcessSerialNumber {
    high_long: u32,
    low_long: u32,
}

#[cfg(target_os = "macos")]
#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFArrayGetCount(array: *const std::ffi::c_void) -> isize;
    fn CFArrayGetValueAtIndex(
        array: *const std::ffi::c_void,
        idx: isize,
    ) -> *const std::ffi::c_void;
    fn CFDictionaryGetValue(
        dict: *const std::ffi::c_void,
        key: *const std::ffi::c_void,
    ) -> *const std::ffi::c_void;
    fn CFNumberGetValue(
        number: *const std::ffi::c_void,
        ty: i32,
        value_ptr: *mut std::ffi::c_void,
    ) -> bool;
    fn CFRelease(cf: *const std::ffi::c_void);
}

#[cfg(target_os = "macos")]
#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGWindowListCopyWindowInfo(
        option: u32,
        relative_to_window: u32,
    ) -> *const std::ffi::c_void;
    static kCGWindowOwnerPID: *const std::ffi::c_void;
    static kCGWindowNumber: *const std::ffi::c_void;
    static kCGWindowLayer: *const std::ffi::c_void;
}

#[cfg(target_os = "macos")]
#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn GetProcessForPID(pid: i32, psn: *mut ProcessSerialNumber) -> i32;
}

// CGWindowList options / CFNumber type constants.
#[cfg(target_os = "macos")]
const CG_WINDOW_LIST_OPTION_ON_SCREEN_ONLY: u32 = 1 << 0;
#[cfg(target_os = "macos")]
const CG_WINDOW_LIST_EXCLUDE_DESKTOP_ELEMENTS: u32 = 1 << 4;
#[cfg(target_os = "macos")]
const CG_NULL_WINDOW_ID: u32 = 0;
#[cfg(target_os = "macos")]
const CF_NUMBER_SINT32_TYPE: i32 = 3;

/// Walk CGWindowList for the topmost on-screen layer-0 window and
/// return `(owner_pid, window_id)`. Layer 0 is the normal user-window
/// layer; higher layers are menu bars, docks, status items, etc. The
/// list is already ordered front-to-back so the first layer-0 match is
/// the frontmost visible window on the current Space.
#[cfg(target_os = "macos")]
unsafe fn frontmost_window_info() -> Option<(i32, u32)> {
    let list = CGWindowListCopyWindowInfo(
        CG_WINDOW_LIST_OPTION_ON_SCREEN_ONLY | CG_WINDOW_LIST_EXCLUDE_DESKTOP_ELEMENTS,
        CG_NULL_WINDOW_ID,
    );
    if list.is_null() {
        return None;
    }

    let count = CFArrayGetCount(list);
    let mut result = None;
    for i in 0..count {
        let dict = CFArrayGetValueAtIndex(list, i);
        if dict.is_null() {
            continue;
        }

        // Only consider normal user windows (layer 0). Skip the menu
        // bar, dock, notification centre, etc.
        let layer_val = CFDictionaryGetValue(dict, kCGWindowLayer);
        if layer_val.is_null() {
            continue;
        }
        let mut layer: i32 = 0;
        if !CFNumberGetValue(
            layer_val,
            CF_NUMBER_SINT32_TYPE,
            &mut layer as *mut i32 as *mut std::ffi::c_void,
        ) {
            continue;
        }
        if layer != 0 {
            continue;
        }

        let pid_val = CFDictionaryGetValue(dict, kCGWindowOwnerPID);
        if pid_val.is_null() {
            continue;
        }
        let mut pid: i32 = 0;
        if !CFNumberGetValue(
            pid_val,
            CF_NUMBER_SINT32_TYPE,
            &mut pid as *mut i32 as *mut std::ffi::c_void,
        ) {
            continue;
        }

        let wid_val = CFDictionaryGetValue(dict, kCGWindowNumber);
        if wid_val.is_null() {
            continue;
        }
        let mut wid: i32 = 0;
        if !CFNumberGetValue(
            wid_val,
            CF_NUMBER_SINT32_TYPE,
            &mut wid as *mut i32 as *mut std::ffi::c_void,
        ) {
            continue;
        }

        result = Some((pid, wid as u32));
        break;
    }

    CFRelease(list);
    result
}

/// Raise the specific window identified by `(pid, wid)` — switching
/// Spaces if necessary and bringing it to front within its owning app.
///
/// This follows the yabai recipe:
///
///   1. `_SLPSSetFrontProcessWithOptions(psn, wid, kCPSUserGenerated)`
///      marks the target process as front on the window-server side
///      with a hint of which window we mean, bypassing focus-stealing
///      guards by claiming the activation is user-initiated.
///   2. Two `SLPSPostEventRecordTo` calls post synthetic window-order
///      events to the target app's event queue. Without these, the app
///      is "front" but AppKit keeps showing whichever window was already
///      key in that app — usually one on the current Space, not the
///      window we actually want. The event records are what make the
///      specific `wid` come to front *inside* the target app.
///
/// The event record byte layout is copied verbatim from yabai's
/// `window_manager_focus_window_with_raise` — these are private AppKit
/// event types, not documented anywhere I know of, but stable across
/// the last several macOS releases.
#[cfg(target_os = "macos")]
unsafe fn raise_window(pid: i32, wid: u32) {
    let mut psn = ProcessSerialNumber {
        high_long: 0,
        low_long: 0,
    };
    if GetProcessForPID(pid, &mut psn) != 0 {
        return;
    }

    // Step 1: set the front process with a window-id hint.
    if let Some(slps) = slps_fn::<SlpsSetFrontFn>(b"_SLPSSetFrontProcessWithOptions\0") {
        // kCPSUserGenerated = 0x200
        let _ = slps(&psn, wid, 0x200);
    }

    // Step 2: post the two window-raise event records so the target
    // app actually brings `wid` to front within itself (which is what
    // triggers macOS to switch to that window's Space).
    if let Some(post) = slps_fn::<SlpsPostEventRecordFn>(b"SLPSPostEventRecordTo\0") {
        let mut bytes1 = [0u8; 0xf8];
        bytes1[0x04] = 0xF8;
        bytes1[0x08] = 0x01;
        bytes1[0x3a] = 0x10;
        bytes1[0x3c..0x40].copy_from_slice(&wid.to_ne_bytes());

        let mut bytes2 = [0u8; 0xf8];
        bytes2[0x04] = 0xF8;
        bytes2[0x08] = 0x02;
        bytes2[0x3a] = 0x10;
        bytes2[0x3c..0x40].copy_from_slice(&wid.to_ne_bytes());

        let _ = post(&psn, bytes1.as_mut_ptr());
        let _ = post(&psn, bytes2.as_mut_ptr());
    }
}

#[cfg(target_os = "macos")]
type SlpsSetFrontFn = unsafe extern "C" fn(*const ProcessSerialNumber, u32, u32) -> i32;
#[cfg(target_os = "macos")]
type SlpsPostEventRecordFn = unsafe extern "C" fn(*const ProcessSerialNumber, *mut u8) -> i32;

/// Lazily resolve a private symbol from SkyLight.framework via dlsym.
/// We cache each (name → pointer) result in its own OnceLock keyed by
/// the symbol name, but since we only look up two symbols it's simpler
/// to just dlopen+dlsym each one and rely on the loader cache.
#[cfg(target_os = "macos")]
unsafe fn slps_fn<F: Copy>(symbol: &[u8]) -> Option<F> {
    let path = b"/System/Library/PrivateFrameworks/SkyLight.framework/SkyLight\0";
    let handle = libc::dlopen(path.as_ptr() as *const i8, libc::RTLD_NOW);
    if handle.is_null() {
        return None;
    }
    let sym = libc::dlsym(handle, symbol.as_ptr() as *const i8);
    if sym.is_null() {
        return None;
    }
    // Safety: F is required by the caller to match the symbol's ABI.
    debug_assert_eq!(
        std::mem::size_of::<F>(),
        std::mem::size_of::<*const std::ffi::c_void>()
    );
    Some(std::mem::transmute_copy::<*mut std::ffi::c_void, F>(&sym))
}

/// How long the listener thread waits for the frontend to resolve a request
/// before giving up and returning an error to the extension. Long enough for
/// a user to actually react to a modal, short enough that a stuck request
/// doesn't permanently leak a thread.
const APPROVAL_TIMEOUT: Duration = Duration::from_secs(120);

/// One pending approval request the listener thread is blocked on.
pub struct PendingRequest {
    pub origin: String,
    pub kind: &'static str, // "connect", later: "signTx", etc.
    /// Sender side of the rendezvous channel. The frontend's
    /// `resolve_ext_request` command pushes the user's decision here.
    pub responder: mpsc::SyncSender<Decision>,
}

/// What the frontend can answer to a pending request.
///
/// `Approve` is used by request types where the Rust side produces the
/// response itself (connect — we look up the address after the user agrees).
/// `ApproveWith` is used when the frontend has already done the work and is
/// handing us a finished JSON payload (sendTx → {txid}, signMessage →
/// {signature, address}). `Deny` carries a human-readable error message.
#[derive(Clone, Debug)]
pub enum Decision {
    Approve,
    ApproveWith(serde_json::Value),
    Deny(String),
}

/// Process-wide registry of pending approval requests, keyed by request id.
static EXT_REQUESTS: Mutex<Option<HashMap<String, PendingRequest>>> = Mutex::new(None);

fn registry<R, F: FnOnce(&mut HashMap<String, PendingRequest>) -> R>(f: F) -> R {
    let mut guard = EXT_REQUESTS.lock().unwrap();
    if guard.is_none() {
        *guard = Some(HashMap::new());
    }
    f(guard.as_mut().unwrap())
}

/// Look up a pending request by id and remove it from the registry. The
/// `resolve_ext_request` command calls this and then sends the decision via
/// the returned channel.
pub fn take_pending(id: &str) -> Option<PendingRequest> {
    registry(|m| m.remove(id))
}

/// Generate a short, opaque, URL-safe id. We don't need cryptographic
/// uniqueness here — these ids only need to be unique among in-flight
/// requests, which is at most a handful at a time.
fn fresh_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    format!("ext-{:x}-{:x}", nanos, n)
}

/// Payload sent to the frontend when a new approval is needed.
#[derive(Serialize, Clone)]
pub struct ExtensionRequestEvent {
    pub id: String,
    pub origin: String,
    pub kind: &'static str,
}

/// Read the wallet's persisted approved-origins list from settings.json.
/// Done on each request so revocations from the UI take effect immediately
/// without restarting the listener.
fn approved_origins() -> Vec<String> {
    let path = settings_dir().join("settings.json");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Vec::new();
    };
    json.get("approvedOrigins")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default()
}

/// Outcome of looking up the address for the active wallet — distinct from
/// `Option<String>` so the caller can give the user a clear error message
/// when no wallet is selected versus when the RPC failed.
enum AddressLookup {
    Address(String),
    NoActiveWallet,
    RpcFailed(String),
}

/// How long the connect handler will wait for the user to select a wallet
/// before giving up. Returns immediately on the first check if a wallet is
/// already active, so this only matters when the user lands on the login
/// screen and needs a moment to pick (or unlock) a wallet.
const WAIT_FOR_WALLET: Duration = Duration::from_secs(60);
const WAIT_POLL_INTERVAL: Duration = Duration::from_millis(200);

/// Block until `AppState.active_wallet` becomes `Some`, or until `timeout`
/// elapses. The first iteration checks without sleeping so the common case
/// (wallet already open) has zero added latency. We use a busy poll instead
/// of a Condvar to keep the surface area small — selecting a wallet is a
/// rare, user-driven event, so a 200 ms tick is fine.
fn wait_for_active_wallet(app: &AppHandle, timeout: Duration) -> Option<String> {
    use tauri::Manager;

    let start = std::time::Instant::now();
    loop {
        let active = app
            .state::<crate::AppState>()
            .active_wallet
            .lock()
            .unwrap()
            .clone();
        if active.is_some() {
            return active;
        }
        if start.elapsed() >= timeout {
            return None;
        }
        std::thread::sleep(WAIT_POLL_INTERVAL);
    }
}

/// Fetch the stable receive address of the wallet the user currently has open
/// in the UI. We deliberately use `getwalletinfo` rather than `getnewaddress`
/// because dApps want a stable identifier — minting a fresh address every
/// connect would surprise both the user and any dApp tracking their address.
fn current_address(app: &AppHandle) -> AddressLookup {
    let Some(wallet) = wait_for_active_wallet(app, WAIT_FOR_WALLET) else {
        return AddressLookup::NoActiveWallet;
    };

    // Reuse the same RPC bridge config as the rest of the app.
    let cookie_path = crate::cookie_path_for(crate::DEFAULT_NETWORK);
    let api_key = std::fs::read_to_string(&cookie_path)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let port = crate::network_ports(crate::DEFAULT_NETWORK).rpc;
    let url = format!("http://127.0.0.1:{}/", port);
    let client = reqwest::blocking::Client::new();

    // fbd's wallet RPC takes the wallet name in a top-level `wallet` field
    // alongside `method`/`params`/`id`. The desktop wallet's `rpc_call`
    // command does the same thing.
    let body = serde_json::json!({
        "method": "getwalletinfo",
        "params": [],
        "id": 1,
        "wallet": wallet,
    })
    .to_string();

    let mut builder = client
        .post(&url)
        .header("Content-Type", "application/json")
        .timeout(Duration::from_secs(5))
        .body(body);
    if let Some(key) = api_key {
        builder = builder.basic_auth("x", Some(key));
    }
    let res = match builder.send() {
        Ok(r) => r,
        Err(e) => return AddressLookup::RpcFailed(format!("send: {}", e)),
    };
    let json: serde_json::Value = match res.json() {
        Ok(j) => j,
        Err(e) => return AddressLookup::RpcFailed(format!("decode: {}", e)),
    };
    if let Some(err) = json.get("error").and_then(|e| e.get("message")).and_then(|m| m.as_str()) {
        return AddressLookup::RpcFailed(err.to_string());
    }
    json.get("result")
        .and_then(|v| v.get("address"))
        .and_then(|v| v.as_str())
        .map(|s| AddressLookup::Address(s.to_string()))
        .unwrap_or_else(|| AddressLookup::RpcFailed("getwalletinfo: no address field".to_string()))
}

// ── Native messaging host installer ──

/// Stable extension ID, shared between the Chrome Web Store listing and
/// local unpacked dev builds. The `key` field in
/// `wallet/extension/manifest.json` is the Web Store's assigned public
/// key — Chrome hashes it to derive this ID on local loads, and the
/// Web Store already publishes under it. We put it in the native
/// messaging host JSON's `allowed_origins` so Chrome only spawns the
/// bridge when the request comes from our extension, regardless of
/// whether it was installed from the Web Store or loaded unpacked.
const EXTENSION_ID: &str = "gdmmlkmiogkhboacejgemhghamolgaol";

/// Firefox extension ID used in `browser_specific_settings.gecko.id`
/// in the Firefox build of the extension manifest, and in the native
/// messaging host's `allowed_extensions` list.
const FIREFOX_EXTENSION_ID: &str = "extension@fistbump.org";

/// Native messaging host name. The extension's `chrome.runtime.connectNative`
/// call uses this exact string; the JSON file we write must match.
const NM_HOST_NAME: &str = "org.fistbump.wallet";

/// Strip Rust's `\\?\` verbatim path prefix on Windows.
///
/// `resource_dir()` can return paths with the extended-length `\\?\` prefix
/// (either because Tauri canonicalizes internally or the OS hands it back
/// that way). Chrome's native messaging host launcher rejects such paths:
/// the extension gets "Error when communicating with the native messaging
/// host" and the bridge never spawns. Normalize to a plain drive-letter
/// path before serializing into the manifest JSON.
#[cfg(target_os = "windows")]
fn strip_verbatim_prefix(p: &std::path::Path) -> std::path::PathBuf {
    let s = p.as_os_str().to_string_lossy();
    // \\?\UNC\server\share\... -> \\server\share\...
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        let mut out = String::from(r"\\");
        out.push_str(rest);
        return std::path::PathBuf::from(out);
    }
    // \\?\C:\... -> C:\...
    if let Some(rest) = s.strip_prefix(r"\\?\") {
        return std::path::PathBuf::from(rest);
    }
    p.to_path_buf()
}

/// On desktop, register this wallet's `fistbump-bridge` binary as the
/// native messaging host for the browser extension in every supported
/// browser (Chromium-family and Firefox).
///
/// This is platform-specific:
///   - **macOS + Linux**: drop a JSON manifest into each browser's
///     per-user `NativeMessagingHosts/` directory under
///     `~/Library/Application Support/…` (macOS) or `~/.config/…` (Linux).
///     The file's name is `org.fistbump.wallet.json` and its contents
///     point at the bundled bridge binary's absolute path.
///   - **Windows**: Chrome doesn't look at `NativeMessagingHosts/`
///     directories on Windows. Instead it reads a per-user registry key
///     under `HKCU\Software\<vendor>\<browser>\NativeMessagingHosts\<name>`
///     whose default value is the absolute path to a manifest JSON
///     somewhere on disk. We write the JSON once to `%APPDATA%\Fistbump\`
///     and then create the registry entries for each known browser.
///
/// Chromium and Firefox use different native messaging manifest formats:
///   - Chromium: `allowed_origins` with `chrome-extension://ID/`
///   - Firefox:  `allowed_extensions` with the addon ID string
///
/// Idempotent and best-effort: if a browser isn't installed we skip it,
/// and any individual write failure is logged but doesn't fail startup.
/// In dev mode (`tauri dev`) the bundled bridge binary doesn't exist,
/// so we no-op silently.
pub fn install_native_messaging_host(app: &AppHandle) {
    let Some(resource_dir) = app.path().resource_dir().ok() else {
        return;
    };
    #[cfg(target_os = "windows")]
    let bridge = strip_verbatim_prefix(&resource_dir.join("fistbump-bridge.exe"));
    #[cfg(not(target_os = "windows"))]
    let bridge = resource_dir.join("fistbump-bridge");

    if !bridge.exists() {
        return;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&bridge, std::fs::Permissions::from_mode(0o755));
    }

    let bridge_path = bridge.display().to_string();

    // Chromium-family manifest (allowed_origins).
    let chromium_manifest = serde_json::json!({
        "name": NM_HOST_NAME,
        "description": "Fistbump Wallet bridge for the browser extension",
        "path": bridge_path,
        "type": "stdio",
        "allowed_origins": [format!("chrome-extension://{}/", EXTENSION_ID)],
    });

    // Firefox manifest (allowed_extensions).
    let firefox_manifest = serde_json::json!({
        "name": NM_HOST_NAME,
        "description": "Fistbump Wallet bridge for the browser extension",
        "path": bridge_path,
        "type": "stdio",
        "allowed_extensions": [FIREFOX_EXTENSION_ID],
    });

    let chromium_str = match serde_json::to_string_pretty(&chromium_manifest) {
        Ok(s) => s,
        Err(e) => {
            println!("[fistbump] failed to serialize chromium host manifest: {}", e);
            return;
        }
    };
    let firefox_str = match serde_json::to_string_pretty(&firefox_manifest) {
        Ok(s) => s,
        Err(e) => {
            println!("[fistbump] failed to serialize firefox host manifest: {}", e);
            return;
        }
    };

    #[cfg(target_os = "macos")]
    {
        install_host_json_unix(&chromium_str, &macos_browser_dirs());
        install_host_json_unix(&firefox_str, &macos_firefox_dirs());
    }

    #[cfg(target_os = "linux")]
    {
        install_host_json_unix(&chromium_str, &linux_browser_dirs());
        install_host_json_unix(&firefox_str, &linux_firefox_dirs());
    }

    #[cfg(target_os = "windows")]
    {
        install_host_windows(&chromium_str, &windows_chromium_keys());
        install_host_windows(&firefox_str, &windows_firefox_keys());
    }
}

// Browser directory tuples: (browser_detect_dir, nm_subdir_name, display_name).
//
// `browser_detect_dir` is checked for existence to skip browsers that
// aren't installed. `nm_subdir_name` is appended to it to form the
// directory where the manifest JSON is written. All paths are relative
// to $HOME.
//
// Chromium browsers always use `NativeMessagingHosts` as the subdir.
// Firefox on macOS also uses `NativeMessagingHosts`, but Firefox on
// Linux uses the lowercase-hyphenated `native-messaging-hosts`.

/// Per-user Chromium browser data dirs on macOS.
#[cfg(target_os = "macos")]
fn macos_browser_dirs() -> Vec<(&'static str, &'static str, &'static str)> {
    vec![
        ("Library/Application Support/Google/Chrome", "NativeMessagingHosts", "Chrome"),
        ("Library/Application Support/Chromium", "NativeMessagingHosts", "Chromium"),
        ("Library/Application Support/BraveSoftware/Brave-Browser", "NativeMessagingHosts", "Brave"),
        ("Library/Application Support/Microsoft Edge", "NativeMessagingHosts", "Edge"),
        ("Library/Application Support/Arc/User Data", "NativeMessagingHosts", "Arc"),
        ("Library/Application Support/Vivaldi", "NativeMessagingHosts", "Vivaldi"),
    ]
}

/// Firefox on macOS.
#[cfg(target_os = "macos")]
fn macos_firefox_dirs() -> Vec<(&'static str, &'static str, &'static str)> {
    vec![
        ("Library/Application Support/Mozilla", "NativeMessagingHosts", "Firefox"),
    ]
}

/// Per-user Chromium browser data dirs on Linux.
#[cfg(target_os = "linux")]
fn linux_browser_dirs() -> Vec<(&'static str, &'static str, &'static str)> {
    vec![
        (".config/google-chrome", "NativeMessagingHosts", "Chrome"),
        (".config/chromium", "NativeMessagingHosts", "Chromium"),
        (".config/BraveSoftware/Brave-Browser", "NativeMessagingHosts", "Brave"),
        (".config/microsoft-edge", "NativeMessagingHosts", "Edge"),
        (".config/vivaldi", "NativeMessagingHosts", "Vivaldi"),
    ]
}

/// Firefox on Linux uses `~/.mozilla/native-messaging-hosts/` (lowercase,
/// hyphens) rather than the CamelCase `NativeMessagingHosts` that
/// Chromium and macOS Firefox use.
#[cfg(target_os = "linux")]
fn linux_firefox_dirs() -> Vec<(&'static str, &'static str, &'static str)> {
    vec![
        (".mozilla", "native-messaging-hosts", "Firefox"),
    ]
}

/// Drop the manifest JSON into each browser's native-messaging-hosts
/// directory. Shared between macOS and Linux — they only differ in paths.
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn install_host_json_unix(manifest_str: &str, browsers: &[(&str, &str, &str)]) {
    let Some(home) = dirs::home_dir() else {
        return;
    };

    let filename = format!("{}.json", NM_HOST_NAME);
    let legacy_filename = "org.fistbump.wallet.launcher.json";

    for (subpath, nm_subdir, name) in browsers {
        let browser_dir = home.join(subpath);
        if !browser_dir.exists() {
            continue;
        }
        let nm_dir = browser_dir.join(nm_subdir);
        if let Err(e) = std::fs::create_dir_all(&nm_dir) {
            println!(
                "[fistbump] native host install: mkdir {} failed: {}",
                name, e
            );
            continue;
        }

        let legacy_target = nm_dir.join(legacy_filename);
        if legacy_target.exists() {
            let _ = std::fs::remove_file(&legacy_target);
        }

        let target = nm_dir.join(&filename);
        if let Ok(existing) = std::fs::read_to_string(&target) {
            if existing == manifest_str {
                continue;
            }
        }
        match std::fs::write(&target, manifest_str) {
            Ok(()) => {
                println!("[fistbump] installed native messaging host for {}", name);
            }
            Err(e) => {
                println!(
                    "[fistbump] native host install: write {} failed: {}",
                    name, e
                );
            }
        }
    }
}

/// Chromium registry keys on Windows.
#[cfg(target_os = "windows")]
fn windows_chromium_keys() -> Vec<(&'static str, &'static str)> {
    vec![
        ("Software\\Google\\Chrome", "Chrome"),
        ("Software\\Chromium", "Chromium"),
        ("Software\\BraveSoftware\\Brave-Browser", "Brave"),
        ("Software\\Microsoft\\Edge", "Edge"),
        ("Software\\Vivaldi", "Vivaldi"),
    ]
}

/// Firefox registry key on Windows.
#[cfg(target_os = "windows")]
fn windows_firefox_keys() -> Vec<(&'static str, &'static str)> {
    vec![
        ("Software\\Mozilla", "Firefox"),
    ]
}

/// Install a native messaging host on Windows. Two-step:
///
///   1. Write the manifest JSON to `%APPDATA%\Fistbump\org.fistbump.wallet.json`
///      (a stable per-user location the wallet owns). Chromium and Firefox
///      manifests are written to separate files (suffixed `-chromium` and
///      `-firefox`) since they have different `allowed_*` fields.
///   2. For each browser, create
///      `HKCU\Software\<vendor>\<browser>\NativeMessagingHosts\org.fistbump.wallet`
///      with the default value set to the manifest path.
///
/// Idempotent: we rewrite the JSON only if it changed, and `create_subkey`
/// is a no-op if the key already exists.
#[cfg(target_os = "windows")]
fn install_host_windows(manifest_str: &str, browsers: &[(&str, &str)]) {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;

    let Some(appdata) = dirs::config_dir() else {
        println!("[fistbump] native host install: no APPDATA available");
        return;
    };
    let wallet_cfg = appdata.join("Fistbump");
    if let Err(e) = std::fs::create_dir_all(&wallet_cfg) {
        println!("[fistbump] native host install: mkdir {} failed: {}",
                 wallet_cfg.display(), e);
        return;
    }

    // Use browser-specific filename so Chromium and Firefox manifests
    // (which differ in allowed_origins vs allowed_extensions) don't
    // overwrite each other.
    let suffix = if manifest_str.contains("allowed_extensions") { "firefox" } else { "chromium" };
    let manifest_path = wallet_cfg.join(format!("{}-{}.json", NM_HOST_NAME, suffix));
    let write_needed = match std::fs::read_to_string(&manifest_path) {
        Ok(existing) => existing != manifest_str,
        Err(_) => true,
    };
    if write_needed {
        if let Err(e) = std::fs::write(&manifest_path, manifest_str) {
            println!(
                "[fistbump] native host install: write {} failed: {}",
                manifest_path.display(),
                e
            );
            return;
        }
    }
    let manifest_path_str = manifest_path.display().to_string();

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    for (browser_key, name) in browsers {
        let host_key_path =
            format!("{}\\NativeMessagingHosts\\{}", browser_key, NM_HOST_NAME);
        let (host_key, _disposition) = match hkcu.create_subkey(&host_key_path) {
            Ok(k) => k,
            Err(e) => {
                println!(
                    "[fistbump] native host install: create key {} failed: {}",
                    host_key_path, e
                );
                continue;
            }
        };
        if let Err(e) = host_key.set_value("", &manifest_path_str) {
            println!(
                "[fistbump] native host install: set {} value failed: {}",
                name, e
            );
            continue;
        }
        println!("[fistbump] installed native messaging host for {}", name);
    }
}

// ── Cross-platform local-socket transport ──
//
// The browser extension talks to us through a small `fistbump-bridge`
// binary that Chrome spawns via native messaging. The bridge does
// stdio↔local-socket forwarding, so all the actual request handling
// lives here on the wallet side. Wire format on both sides of the
// bridge is identical to Chrome's native messaging frame format: a
// 4-byte little-endian length prefix followed by a JSON body.
//
// On Unix we use a Unix domain socket at `~/.fistbump/extension.sock`
// (0600); on Windows we use a named pipe at
// `\\.\pipe\org.fistbump.wallet.extension`. Both are exposed through
// `interprocess::local_socket` as a single blocking Read+Write
// `Stream` API so the transport code doesn't have to fork. The bridge
// binary uses the same `interprocess` name derivation so both ends
// agree on where to rendezvous without any config plumbing.

/// On Unix, where the Unix domain socket lives on disk. Callers need
/// this to clean up stale socket files before binding, and to chmod
/// after binding. The bridge side derives the same path from $HOME.
#[cfg(unix)]
fn ipc_socket_path() -> std::path::PathBuf {
    settings_dir().join("extension.sock")
}

/// Build the cross-platform `Name` we use for both bind and connect.
///
/// On Unix we *always* use `GenericFilePath` pointing at the existing
/// `~/.fistbump/extension.sock` path. On Windows we use
/// `GenericNamespaced` which resolves to the named pipe
/// `\\.\pipe\org.fistbump.wallet.extension`.
///
/// We deliberately *don't* feature-test via
/// `GenericNamespaced::is_supported()` like the interprocess docs
/// suggest — its `SpecialDirUdSocket` impl on macOS returns `true` from
/// `is_supported()` and then resolves the name to a hardcoded
/// `/tmp/<name>` path (see interprocess 2.4
/// `os/unix/uds_local_socket.rs::tmpdir`), which (a) is in a
/// world-writable directory rather than the per-user `~/.fistbump/`
/// we want, (b) leaks across crashes because nothing cleans it up,
/// and (c) silently disagrees with the path our `start_ipc_listener`
/// stale-socket cleanup actually targets, so EADDRINUSE on every
/// restart. Hard-cfgging the branch instead of trusting the runtime
/// check sidesteps all of that.
fn ipc_socket_name() -> std::io::Result<interprocess::local_socket::Name<'static>> {
    use interprocess::local_socket::prelude::*;

    #[cfg(unix)]
    {
        use interprocess::local_socket::GenericFilePath;
        ipc_socket_path()
            .into_os_string()
            .to_fs_name::<GenericFilePath>()
    }
    #[cfg(windows)]
    {
        use interprocess::local_socket::GenericNamespaced;
        "org.fistbump.wallet.extension".to_ns_name::<GenericNamespaced>()
    }
    #[cfg(not(any(unix, windows)))]
    {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "no supported local socket transport on this target",
        ))
    }
}

/// Bind the local socket and start accepting connections in a
/// background thread. Each client gets its own thread; blocking inside
/// a request (e.g. waiting for the user to approve a modal) is fine
/// because no other clients are stuck behind it.
pub fn start_ipc_listener(app: AppHandle) {
    use interprocess::local_socket::{prelude::*, ListenerOptions};

    // On Unix, make sure the parent dir exists and clean up any
    // leftover socket file from a previous (or crashed) run — Unix
    // domain sockets don't auto-cleanup on process exit, so a stale
    // file would otherwise cause bind to fail with AddrInUse.
    #[cfg(unix)]
    {
        let socket_path = ipc_socket_path();
        if let Some(parent) = socket_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::remove_file(&socket_path);

        // Migration cleanup: an earlier build of the wallet (briefly
        // shipped) called interprocess with `GenericNamespaced` on
        // macOS, which silently resolved to `/tmp/<name>` via
        // SpecialDirUdSocket → tmpdir(). The stale socket file from
        // that build can outlive every wallet restart and prevent
        // legitimate filesystem-path binds elsewhere. Remove it so
        // upgraders aren't stuck. Best-effort: any error here is
        // benign because we're not using that path anymore anyway.
        #[cfg(target_os = "macos")]
        {
            let _ = std::fs::remove_file("/tmp/org.fistbump.wallet.extension");
        }
    }

    let name = match ipc_socket_name() {
        Ok(n) => n,
        Err(e) => {
            println!("[fistbump] extension IPC name build failed: {}", e);
            return;
        }
    };

    let listener = match ListenerOptions::new().name(name).create_sync() {
        Ok(l) => l,
        Err(e) => {
            println!("[fistbump] extension IPC bind failed: {}", e);
            return;
        }
    };

    // On Unix, tighten the socket file to 0600 so other users on the
    // same machine can't connect. No-op on Windows — named pipe ACLs
    // are handled separately, and by default are only accessible to
    // the creating user's session.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let socket_path = ipc_socket_path();
        let _ =
            std::fs::set_permissions(&socket_path, std::fs::Permissions::from_mode(0o600));
        println!(
            "[fistbump] extension IPC listening on {}",
            socket_path.display()
        );
    }
    #[cfg(not(unix))]
    {
        println!("[fistbump] extension IPC listening on named pipe org.fistbump.wallet.extension");
    }

    std::thread::spawn(move || {
        for stream in listener.incoming() {
            match stream {
                Ok(s) => {
                    let app = app.clone();
                    std::thread::spawn(move || {
                        if let Err(e) = handle_ipc_connection(s, &app) {
                            let kind = e.kind();
                            // Clean disconnects are normal; only complain about
                            // anything else.
                            if kind != std::io::ErrorKind::UnexpectedEof
                                && kind != std::io::ErrorKind::BrokenPipe
                            {
                                println!("[fistbump] extension IPC client error: {}", e);
                            }
                        }
                    });
                }
                Err(e) => {
                    println!("[fistbump] extension IPC accept error: {}", e);
                }
            }
        }
    });
}

/// Read length-prefixed JSON frames from `stream`, dispatch them, and
/// write responses back. Returns when the client disconnects.
fn handle_ipc_connection(
    mut stream: interprocess::local_socket::Stream,
    app: &AppHandle,
) -> std::io::Result<()> {
    loop {
        let mut len_buf = [0u8; 4];
        if let Err(e) = stream.read_exact(&mut len_buf) {
            if e.kind() == std::io::ErrorKind::UnexpectedEof {
                return Ok(());
            }
            return Err(e);
        }
        let len = u32::from_le_bytes(len_buf) as usize;
        if len == 0 || len > 1024 * 1024 {
            // Malformed frame — bail rather than try to recover.
            return Ok(());
        }
        let mut msg_buf = vec![0u8; len];
        stream.read_exact(&mut msg_buf)?;

        let response = process_ipc_message(&msg_buf, app);
        let resp_str = response.to_string();
        let resp_len = (resp_str.len() as u32).to_le_bytes();
        stream.write_all(&resp_len)?;
        stream.write_all(resp_str.as_bytes())?;
        stream.flush()?;
    }
}

/// Parse a single JSON-encoded request and route it to the appropriate
/// handler. Errors come back as `{"error": "..."}` so the bridge can
/// forward them to Chrome unchanged.
fn process_ipc_message(bytes: &[u8], app: &AppHandle) -> serde_json::Value {
    let msg: serde_json::Value = match serde_json::from_slice(bytes) {
        Ok(v) => v,
        Err(_) => return serde_json::json!({ "error": "invalid json frame" }),
    };
    let kind = msg.get("type").and_then(|v| v.as_str()).unwrap_or("");
    let origin = msg.get("origin").and_then(|v| v.as_str()).unwrap_or("");

    match kind {
        "info" => build_info_value(origin),
        "connect" => process_connect_request(app, origin),
        "sendTx" => process_sendtx_request(app, origin, &msg),
        "signMessage" => process_sign_message_request(app, origin, &msg),
        "getPublicKey" => process_get_pubkey_request(app, origin),
        "fundHtlc" => process_fund_htlc_request(app, origin, &msg),
        "signHtlcSpend" => process_sign_htlc_spend_request(app, origin, &msg),
        other => serde_json::json!({ "error": format!("unknown request type: {}", other) }),
    }
}

/// Liveness/version probe. Doesn't prompt the user, doesn't touch state.
/// `running: true` here is tautological — if we're serving this response
/// the wallet is obviously running — but having the field lets the bridge
/// use a matching `running: false` shape when it short-circuits an info
/// request against a wallet that isn't up, so the extension popup has a
/// single boolean to switch on.
fn build_info_value(origin: &str) -> serde_json::Value {
    let connected = !origin.is_empty() && approved_origins().contains(&origin.to_string());
    serde_json::json!({
        "ok": true,
        "running": true,
        "version": env!("CARGO_PKG_VERSION"),
        "connected": connected,
    })
}

/// Core connect-handling logic, transport-independent. Returns a JSON
/// value to be sent back over whatever channel the caller is using.
fn process_connect_request(app: &AppHandle, origin: &str) -> serde_json::Value {
    if origin.is_empty() {
        return serde_json::json!({ "error": "missing origin" });
    }

    // Snapshot whichever app is currently frontmost BEFORE we pull
    // ourselves forward, so we can explicitly re-activate it (the browser
    // tab, typically) once the user has dealt with the request.
    let prev_focus = capture_previous_focus(app);

    // Pop the wallet to the front for every connect request — both for new
    // origins (so the user sees the approval modal) and for already-approved
    // ones (so the user has visual confirmation that something happened on
    // the dApp side).
    bring_wallet_to_front(app);

    // Auto-approved path: return the address immediately, then hand focus
    // back since there was no user interaction needed.
    if approved_origins().contains(&origin.to_string()) {
        let result = address_value(app, origin);
        restore_previous_focus(app, &prev_focus);
        return result;
    }

    // Set up a rendezvous channel and register the request before emitting
    // the event, so the frontend's resolve call can never race with us.
    let (tx, rx) = mpsc::sync_channel::<Decision>(1);
    let id = fresh_id();
    registry(|m| {
        m.insert(
            id.clone(),
            PendingRequest {
                origin: origin.to_string(),
                kind: "connect",
                responder: tx,
            },
        );
    });

    let event = ExtensionRequestEvent {
        id: id.clone(),
        origin: origin.to_string(),
        kind: "connect",
    };
    if let Err(e) = app.emit("ext://request", event) {
        registry(|m| {
            m.remove(&id);
        });
        restore_previous_focus(app, &prev_focus);
        return serde_json::json!({ "error": format!("emit failed: {}", e) });
    }

    let result = match rx.recv_timeout(APPROVAL_TIMEOUT) {
        Ok(Decision::Approve) => address_value(app, origin),
        Ok(Decision::ApproveWith(value)) => value,
        Ok(Decision::Deny(msg)) => serde_json::json!({ "error": msg }),
        Err(_) => {
            registry(|m| {
                m.remove(&id);
            });
            serde_json::json!({ "error": "approval request timed out" })
        }
    };

    restore_previous_focus(app, &prev_focus);
    result
}

/// Run `current_address` and translate the outcome into a JSON value.
fn address_value(app: &AppHandle, origin: &str) -> serde_json::Value {
    match current_address(app) {
        AddressLookup::Address(addr) => {
            serde_json::json!({ "address": addr, "origin": origin })
        }
        AddressLookup::NoActiveWallet => serde_json::json!({
            "error": "no wallet selected — open a wallet in the Fistbump app and try again"
        }),
        AddressLookup::RpcFailed(msg) => {
            serde_json::json!({ "error": format!("wallet RPC failed: {}", msg) })
        }
    }
}

/// Emit a Tauri event for the given pending request and block until the
/// frontend pushes a decision back through the channel. Shared between
/// sendTx and signMessage — both of them do the actual work in JS (using
/// rpc_call into fbd) and then call `resolve_ext_request` with either
/// an approved result value or an error string.
///
/// Returns the JSON value to send back to the extension, regardless of
/// whether the request was approved, denied, timed out, or failed.
fn dispatch_frontend_request(
    app: &AppHandle,
    origin: &str,
    kind: &'static str,
    event_name: &'static str,
    extra: serde_json::Map<String, serde_json::Value>,
) -> serde_json::Value {
    // Non-connect requests must come from an already-approved origin.
    // dApps have to connect() before they can ask us to sign or send.
    if !approved_origins().contains(&origin.to_string()) {
        return serde_json::json!({
            "error": "origin not connected — call window.fistbump.connect() first"
        });
    }

    // Snapshot the frontmost app before we pull ourselves forward so we
    // can explicitly re-activate it after the user resolves.
    let prev_focus = capture_previous_focus(app);

    bring_wallet_to_front(app);

    let (tx, rx) = mpsc::sync_channel::<Decision>(1);
    let id = fresh_id();
    registry(|m| {
        m.insert(
            id.clone(),
            PendingRequest {
                origin: origin.to_string(),
                kind,
                responder: tx,
            },
        );
    });

    // Build the event payload: id, origin, kind, plus whatever the specific
    // request brought with it (amount/to for sendTx, message for signMessage).
    let mut payload = serde_json::Map::new();
    payload.insert("id".into(), serde_json::json!(id));
    payload.insert("origin".into(), serde_json::json!(origin));
    payload.insert("kind".into(), serde_json::json!(kind));
    for (k, v) in extra {
        payload.insert(k, v);
    }

    if let Err(e) = app.emit(event_name, serde_json::Value::Object(payload)) {
        registry(|m| {
            m.remove(&id);
        });
        restore_previous_focus(app, &prev_focus);
        return serde_json::json!({ "error": format!("emit failed: {}", e) });
    }

    let result = match rx.recv_timeout(APPROVAL_TIMEOUT) {
        Ok(Decision::Approve) => {
            // Shouldn't happen for these request types — but don't hang.
            serde_json::json!({ "error": "internal: frontend sent bare approve for non-connect request" })
        }
        Ok(Decision::ApproveWith(value)) => value,
        Ok(Decision::Deny(msg)) => serde_json::json!({ "error": msg }),
        Err(_) => {
            registry(|m| {
                m.remove(&id);
            });
            serde_json::json!({ "error": "approval request timed out" })
        }
    };

    restore_previous_focus(app, &prev_focus);
    result
}

/// Handler for `{"type": "sendTx", "origin": "...", "to": "...", "amount": 1.5}`.
/// Validates shape, defers the actual createtx/signtx/broadcasttx to the
/// frontend (which already has the RPC plumbing and the approval UI).
fn process_sendtx_request(
    app: &AppHandle,
    origin: &str,
    msg: &serde_json::Value,
) -> serde_json::Value {
    if origin.is_empty() {
        return serde_json::json!({ "error": "missing origin" });
    }

    let Some(to) = msg.get("to").and_then(|v| v.as_str()) else {
        return serde_json::json!({ "error": "missing `to` field" });
    };
    if to.is_empty() {
        return serde_json::json!({ "error": "`to` is empty" });
    }

    let amount = match msg.get("amount") {
        Some(v) if v.is_number() => v.as_f64().unwrap_or(0.0),
        _ => {
            return serde_json::json!({ "error": "`amount` must be a positive number (FBC)" });
        }
    };
    if !(amount > 0.0 && amount.is_finite()) {
        return serde_json::json!({ "error": "`amount` must be a positive number (FBC)" });
    }

    let mut extra = serde_json::Map::new();
    extra.insert("to".into(), serde_json::json!(to));
    extra.insert("amount".into(), serde_json::json!(amount));

    dispatch_frontend_request(app, origin, "sendTx", "ext://tx-request", extra)
}

/// Handler for
/// `{"type": "signMessage", "origin": "...", "message": "...", "name"?: "..."}`.
/// The frontend pops a confirm modal, then calls either `signmessagewithname`
/// (if the dApp specified a Fistbump name) or `signmessage` against the
/// current wallet's active address, and resolves with the signature.
fn process_sign_message_request(
    app: &AppHandle,
    origin: &str,
    msg: &serde_json::Value,
) -> serde_json::Value {
    if origin.is_empty() {
        return serde_json::json!({ "error": "missing origin" });
    }

    let Some(message) = msg.get("message").and_then(|v| v.as_str()) else {
        return serde_json::json!({ "error": "missing `message` field" });
    };
    // Keep an upper bound so a pathological dApp can't ask us to display a
    // novel in the confirm modal. fbd's signmessage doesn't care about
    // length but macOS dialog rendering does.
    if message.len() > 8 * 1024 {
        return serde_json::json!({ "error": "message too long (max 8 KB)" });
    }

    // Optional name — when present, the wallet-side listener will call
    // `signmessagewithname` instead of the address-based `signmessage`.
    // Rust doesn't need to validate the shape; fbd will reject anything
    // that isn't a valid owned name.
    let name = msg
        .get("name")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());

    let mut extra = serde_json::Map::new();
    extra.insert("message".into(), serde_json::json!(message));
    if let Some(n) = name {
        extra.insert("name".into(), serde_json::json!(n));
    }

    dispatch_frontend_request(app, origin, "signMessage", "ext://sign-request", extra)
}

/// Handler for `{"type": "getPublicKey", "origin": "..."}`.
///
/// Returns the wallet's secp256k1 compressed pubkey used for atomic swaps.
/// The frontend calls fbd's `getswappubkey` RPC and returns the result —
/// no modal, because exposing a pubkey is not a privacy change beyond what
/// a prior `connect()` already granted (the origin already knows an address
/// that commits to the same key). A future hardening could add a first-use
/// confirmation per origin; left out of v1 for UX parity with `connect`.
fn process_get_pubkey_request(app: &AppHandle, origin: &str) -> serde_json::Value {
    if origin.is_empty() {
        return serde_json::json!({ "error": "missing origin" });
    }
    dispatch_frontend_request(
        app,
        origin,
        "getPublicKey",
        "ext://swap-pubkey-request",
        serde_json::Map::new(),
    )
}

/// Handler for
/// `{"type": "fundHtlc", "origin": "...", "witnessScriptHex": "...", "amount": 1.5, "memo": "..."}`.
///
/// Shape-checks the payload and forwards to the wallet frontend, which
/// verifies the script matches `Script.htlc(...)` before showing a
/// "Fund Swap" review modal with the deposit amount, HTLC address, and fee.
fn process_fund_htlc_request(
    app: &AppHandle,
    origin: &str,
    msg: &serde_json::Value,
) -> serde_json::Value {
    if origin.is_empty() {
        return serde_json::json!({ "error": "missing origin" });
    }

    let Some(script_hex) = msg.get("witnessScriptHex").and_then(|v| v.as_str()) else {
        return serde_json::json!({ "error": "missing `witnessScriptHex` field" });
    };
    if script_hex.is_empty() || !is_hex_string(script_hex) {
        return serde_json::json!({ "error": "`witnessScriptHex` must be a hex string" });
    }
    // Upper bound matches fbd's consensus script size cap (10_000 bytes =
    // 20_000 hex chars). We're stricter than necessary — a well-formed HTLC
    // is about 103 bytes — but allow some headroom for future templates.
    if script_hex.len() > 2 * 10_000 {
        return serde_json::json!({ "error": "`witnessScriptHex` is too large" });
    }

    let amount = match msg.get("amount") {
        Some(v) if v.is_number() => v.as_f64().unwrap_or(0.0),
        _ => return serde_json::json!({ "error": "`amount` must be a positive number (FBC)" }),
    };
    if !(amount > 0.0 && amount.is_finite()) {
        return serde_json::json!({ "error": "`amount` must be a positive number (FBC)" });
    }

    let memo = msg
        .get("memo")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());

    let mut extra = serde_json::Map::new();
    extra.insert("witnessScriptHex".into(), serde_json::json!(script_hex));
    extra.insert("amount".into(), serde_json::json!(amount));
    if let Some(m) = memo {
        extra.insert("memo".into(), serde_json::json!(m));
    }

    dispatch_frontend_request(app, origin, "fundHtlc", "ext://htlc-fund-request", extra)
}

/// Handler for
/// `{"type": "signHtlcSpend", "origin": "...", ...}`. See `injected.js` for
/// the full parameter list. The wallet frontend shows a branch-specific
/// review modal ("Claim swap" vs "Refund swap") before signing.
fn process_sign_htlc_spend_request(
    app: &AppHandle,
    origin: &str,
    msg: &serde_json::Value,
) -> serde_json::Value {
    if origin.is_empty() {
        return serde_json::json!({ "error": "missing origin" });
    }

    let Some(txid) = msg.get("fundingTxid").and_then(|v| v.as_str()) else {
        return serde_json::json!({ "error": "missing `fundingTxid`" });
    };
    if txid.len() != 64 || !is_hex_string(txid) {
        return serde_json::json!({ "error": "`fundingTxid` must be 64 hex chars" });
    }

    let Some(vout_f) = msg.get("fundingVout").and_then(|v| v.as_f64()) else {
        return serde_json::json!({ "error": "`fundingVout` must be a non-negative integer" });
    };
    if vout_f < 0.0 || vout_f > u32::MAX as f64 || vout_f.fract() != 0.0 {
        return serde_json::json!({ "error": "`fundingVout` must be a non-negative integer" });
    }

    let Some(amount_f) = msg.get("fundingAmount").and_then(|v| v.as_f64()) else {
        return serde_json::json!({ "error": "`fundingAmount` must be a positive number (bumps)" });
    };
    if amount_f <= 0.0 || amount_f.fract() != 0.0 {
        return serde_json::json!({ "error": "`fundingAmount` must be a positive integer" });
    }

    let Some(script_hex) = msg.get("witnessScriptHex").and_then(|v| v.as_str()) else {
        return serde_json::json!({ "error": "missing `witnessScriptHex`" });
    };
    if script_hex.is_empty() || !is_hex_string(script_hex) {
        return serde_json::json!({ "error": "`witnessScriptHex` must be a hex string" });
    }

    let Some(branch) = msg.get("branch").and_then(|v| v.as_str()) else {
        return serde_json::json!({ "error": "missing `branch`" });
    };
    if branch != "claim" && branch != "refund" {
        return serde_json::json!({ "error": "`branch` must be \"claim\" or \"refund\"" });
    }

    let preimage = msg
        .get("preimageHex")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());

    if branch == "claim" {
        match preimage.as_deref() {
            Some(p) if p.len() == 64 && is_hex_string(p) => {}
            _ => {
                return serde_json::json!({
                    "error": "claim branch requires `preimageHex` (64 hex chars)"
                });
            }
        }
    }

    let Some(dest) = msg.get("destinationAddress").and_then(|v| v.as_str()) else {
        return serde_json::json!({ "error": "missing `destinationAddress`" });
    };
    if dest.is_empty() {
        return serde_json::json!({ "error": "`destinationAddress` is empty" });
    }

    let Some(fee_f) = msg.get("feeRate").and_then(|v| v.as_f64()) else {
        return serde_json::json!({ "error": "`feeRate` must be a positive integer" });
    };
    if fee_f <= 0.0 || fee_f.fract() != 0.0 {
        return serde_json::json!({ "error": "`feeRate` must be a positive integer" });
    }

    let mut extra = serde_json::Map::new();
    extra.insert("fundingTxid".into(), serde_json::json!(txid));
    extra.insert("fundingVout".into(), serde_json::json!(vout_f as u64));
    extra.insert("fundingAmount".into(), serde_json::json!(amount_f as u64));
    extra.insert("witnessScriptHex".into(), serde_json::json!(script_hex));
    extra.insert("branch".into(), serde_json::json!(branch));
    if let Some(p) = preimage {
        extra.insert("preimageHex".into(), serde_json::json!(p));
    }
    extra.insert("destinationAddress".into(), serde_json::json!(dest));
    extra.insert("feeRate".into(), serde_json::json!(fee_f as u64));

    dispatch_frontend_request(
        app,
        origin,
        "signHtlcSpend",
        "ext://htlc-spend-request",
        extra,
    )
}

/// Cheap ASCII hex validator for the extension proxy's shape checks. Full
/// decoding happens downstream in fbd — we just reject obvious garbage here
/// so a hostile dApp can't flood a review modal with non-hex nonsense.
fn is_hex_string(s: &str) -> bool {
    !s.is_empty()
        && s.len() % 2 == 0
        && s.bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f' | b'A'..=b'F'))
}

