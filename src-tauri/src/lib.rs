use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs;
#[cfg(any(desktop, target_os = "android"))]
use std::io::{BufRead, BufReader};
#[cfg(desktop)]
use std::path::Path;
use std::path::PathBuf;
#[cfg(any(desktop, target_os = "android"))]
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use tauri::Manager;

mod proxy;
#[cfg(desktop)]
mod ledger;

const MAX_LOG_LINES: usize = 500;

// ── Network Defaults ──

pub(crate) const DEFAULT_NETWORK: &str = env!("FISTBUMP_NETWORK");

pub(crate) struct NetworkPorts {
    pub rpc: u16,
    #[allow(dead_code)]
    pub dns: u16,
}

pub(crate) fn network_ports(network: &str) -> NetworkPorts {
    match network {
        "main"    => NetworkPorts { rpc: 32869, dns: 32870 },
        "testnet" => NetworkPorts { rpc: 42869, dns: 42870 },
        "regtest" => NetworkPorts { rpc: 52869, dns: 52870 },
        "simnet"  => NetworkPorts { rpc: 62869, dns: 62870 },
        _         => network_ports(DEFAULT_NETWORK),
    }
}

// ── Settings ──

#[derive(Serialize, Deserialize, Clone, Default)]
struct Settings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    port: Option<u16>,
    #[serde(default, rename = "apiKey", skip_serializing_if = "Option::is_none")]
    api_key: Option<String>,
    #[serde(default, rename = "proxySetupDone")]
    proxy_setup_done: bool,
    #[serde(default, rename = "proxyEnabled")]
    proxy_enabled: bool,
    #[serde(default, rename = "minerAddress", skip_serializing_if = "Option::is_none")]
    miner_address: Option<String>,
    #[serde(default, rename = "miningEnabled")]
    mining_enabled: bool,
    #[serde(default, rename = "minerThreads")]
    miner_threads: u32,
    /// Web origins that have been granted permission to talk to this wallet
    /// via the browser extension. Persisted so we don't re-prompt the same
    /// dApp on every connect.
    #[serde(default, rename = "approvedOrigins")]
    approved_origins: Vec<String>,
}

/// Description of a legacy fbd data directory that has wallet files we could
/// copy into the wallet's isolated data dir on first launch. Only populated on
/// desktop; mobile is already sandbox-isolated from any standalone fbd.
#[derive(Clone, Debug, serde::Serialize)]
pub struct MigrationCandidate {
    /// Absolute path to the source `<old>/wallets` directory.
    source: String,
    /// Number of wallet subdirectories found in `source`.
    wallet_count: usize,
}

pub struct AppState {
    settings: Mutex<Settings>,
    log_lines: Mutex<Vec<String>>,
    fbd_pid: Mutex<Option<u32>>,
    is_quitting: AtomicBool,
    /// Some(candidate) while the frontend is being asked whether to copy
    /// wallets from an old `~/.fbd` directory. start_node is deferred until
    /// `resolve_migration` is called.
    #[cfg(desktop)]
    pending_migration: Mutex<Option<MigrationCandidate>>,
    /// Name of the wallet the user currently has open in the UI. Synced
    /// from JS via `set_active_wallet`. The browser-extension bridge needs
    /// this to know which wallet to query for the receive address.
    #[cfg(desktop)]
    pub(crate) active_wallet: Mutex<Option<String>>,
}

fn settings_base_dir() -> PathBuf {
    if cfg!(target_os = "android") {
        android_files_dir().join(".fistbump")
    } else if cfg!(target_os = "ios") {
        ios_documents_dir().join(".fistbump")
    } else {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".fistbump")
    }
}

pub(crate) fn settings_dir() -> PathBuf {
    let base = settings_base_dir();
    if DEFAULT_NETWORK == "main" { base } else { base.join(DEFAULT_NETWORK) }
}

fn ios_documents_dir() -> PathBuf {
    // Try HOME env first (set by iOS runtime), then fall back to /var/mobile
    std::env::var("HOME")
        .map(|h| PathBuf::from(h).join("Documents"))
        .unwrap_or_else(|_| PathBuf::from("/var/mobile/Documents"))
}

fn settings_path() -> PathBuf {
    settings_dir().join("settings.json")
}

#[cfg(any(desktop, target_os = "android"))]
fn pid_path() -> PathBuf {
    settings_base_dir().join("fbd.pid")
}

/// Data directory for the embedded fbd subprocess. Intentionally **not** fbd's
/// default `~/.fbd` on desktop, so a standalone fbd install (possibly run with
/// flags like `--index-tx` that change db schema) can't block the wallet from
/// starting. Android and iOS already isolate themselves via their app sandboxes.
fn fbd_data_dir() -> PathBuf {
    if cfg!(target_os = "android") {
        android_files_dir().join("fbd-data")
    } else if cfg!(target_os = "ios") {
        ios_documents_dir().join("fbd")
    } else {
        settings_base_dir().join("fbd-data")
    }
}

pub(crate) fn cookie_path_for(network: &str) -> PathBuf {
    let base = fbd_data_dir();
    if network == "main" {
        base.join(".cookie")
    } else {
        base.join(network).join(".cookie")
    }
}

/// Location fbd uses by default (not the wallet's isolated copy). We check
/// here on first launch to offer the user a chance to bring their wallets
/// over. Desktop only — mobile is sandboxed so there's nothing to migrate.
#[cfg(desktop)]
fn legacy_fbd_wallets_dir() -> PathBuf {
    let base = if cfg!(target_os = "windows") {
        let local_app_data = std::env::var("LOCALAPPDATA").unwrap_or_else(|_| "C:\\".to_string());
        PathBuf::from(local_app_data).join("fbd")
    } else {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".fbd")
    };
    if DEFAULT_NETWORK == "main" {
        base.join("wallets")
    } else {
        base.join(DEFAULT_NETWORK).join("wallets")
    }
}

/// Wallet directory inside the wallet's isolated fbd data dir.
#[cfg(desktop)]
fn wallet_wallets_dir() -> PathBuf {
    let base = fbd_data_dir();
    if DEFAULT_NETWORK == "main" {
        base.join("wallets")
    } else {
        base.join(DEFAULT_NETWORK).join("wallets")
    }
}

/// Marker written next to the wallets dir after the migration prompt has been
/// answered (either "copy" or "skip"), so we never pester the user twice.
#[cfg(desktop)]
fn migration_marker_path() -> PathBuf {
    fbd_data_dir().join(".fistbump-migration-done")
}

/// Counts direct subdirectories of `dir`. Used to decide whether the legacy
/// wallets dir is worth migrating (empty = nothing to do).
#[cfg(desktop)]
fn count_wallet_subdirs(dir: &Path) -> usize {
    fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
                .count()
        })
        .unwrap_or(0)
}

/// Check for a legacy `~/.fbd/wallets` directory we can offer to copy. Returns
/// `None` if the marker exists, the legacy dir is missing/empty, or the new
/// wallets dir already has content (meaning the user has already been using
/// the isolated location).
#[cfg(desktop)]
fn detect_migration_candidate() -> Option<MigrationCandidate> {
    if migration_marker_path().exists() {
        return None;
    }
    if count_wallet_subdirs(&wallet_wallets_dir()) > 0 {
        return None;
    }
    let legacy = legacy_fbd_wallets_dir();
    let count = count_wallet_subdirs(&legacy);
    if count == 0 {
        return None;
    }
    Some(MigrationCandidate {
        source: legacy.display().to_string(),
        wallet_count: count,
    })
}

/// Recursively copy `src` into `dst`, creating `dst` and any missing parents.
/// Symlinks are skipped intentionally — fbd wallet files are plain files in a
/// flat per-wallet subdir, so there's no need to chase links.
#[cfg(desktop)]
fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else if file_type.is_file() {
            fs::copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
}

// Make android_files_dir available on all platforms (used by cookie_path at compile time)
#[cfg(not(target_os = "android"))]
fn android_files_dir() -> PathBuf {
    PathBuf::from("/data/data/org.fistbump.wallet/files")
}

fn load_settings() -> Settings {
    let _ = fs::create_dir_all(settings_dir());
    match fs::read_to_string(settings_path()) {
        Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
        Err(_) => Settings::default(),
    }
}

fn read_cookie(network: &str) -> Option<String> {
    fs::read_to_string(cookie_path_for(network))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn get_api_key(settings: &Settings) -> Option<String> {
    settings.api_key.clone().or_else(|| read_cookie(DEFAULT_NETWORK))
}

fn rpc_host(settings: &Settings) -> String {
    settings
        .host
        .clone()
        .unwrap_or_else(|| "127.0.0.1".to_string())
}

fn rpc_port(settings: &Settings) -> u16 {
    settings.port.unwrap_or_else(|| network_ports(DEFAULT_NETWORK).rpc)
}

// ── RPC ──

#[tauri::command]
async fn rpc_call(
    method: String,
    params: Value,
    wallet: Option<String>,
    state: tauri::State<'_, AppState>,
) -> Result<Value, String> {
    let settings = state.settings.lock().unwrap().clone();
    let host = rpc_host(&settings);
    let port = rpc_port(&settings);

    let mut payload = json!({
        "method": method,
        "params": params,
        "id": 1
    });
    if let Some(ref w) = wallet {
        payload["wallet"] = json!(w);
    }

    let client = reqwest::Client::new();
    let url = format!("http://{}:{}/", host, port);
    let api_key = get_api_key(&settings);

    // Retry on transport failures (connection refused, timeout). The
    // in-process node on iOS can be briefly unreachable while it's
    // initializing, restarting on foreground, or rebinding listeners —
    // a short retry lets the next call succeed instead of surfacing a
    // raw reqwest error to the user.
    let mut last_error = String::new();
    for attempt in 0..3u32 {
        if attempt > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
        let mut builder = client
            .post(&url)
            .header("Content-Type", "application/json")
            .json(&payload)
            .timeout(std::time::Duration::from_secs(120));

        if let Some(ref key) = api_key {
            builder = builder.basic_auth("x", Some(key));
        }

        match builder.send().await {
            Ok(res) => {
                return match res.json::<Value>().await {
                    Ok(json_val) => {
                        if let Some(err) = json_val.get("error") {
                            if !err.is_null() {
                                let msg = err
                                    .get("message")
                                    .and_then(|m| m.as_str())
                                    .unwrap_or("RPC error");
                                let code = err
                                    .get("code")
                                    .and_then(|c| c.as_i64())
                                    .unwrap_or(-1);
                                return Ok(json!({ "error": msg, "errorCode": code }));
                            }
                        }
                        Ok(json!({ "result": json_val.get("result").cloned().unwrap_or(Value::Null) }))
                    }
                    Err(_) => Ok(json!({ "error": "Invalid JSON response" })),
                };
            }
            Err(e) => {
                last_error = e.to_string();
            }
        }
    }
    Ok(json!({ "error": last_error }))
}

// ── Log ──

// Shared log buffer for FFI-based log capture (used on iOS; empty on desktop)
static MOBILE_LOG: Mutex<Vec<String>> = Mutex::new(Vec::new());

#[no_mangle]
pub extern "C" fn push_log_line(ptr: *const std::ffi::c_char) {
    if ptr.is_null() {
        return;
    }
    let c_str = unsafe { std::ffi::CStr::from_ptr(ptr) };
    if let Ok(s) = c_str.to_str() {
        let mut lines = MOBILE_LOG.lock().unwrap();
        for line in s.lines().filter(|l| !l.is_empty()) {
            lines.push(line.to_string());
        }
        if lines.len() > MAX_LOG_LINES {
            let drain = lines.len() - MAX_LOG_LINES;
            lines.drain(..drain);
        }
    }
}

#[tauri::command]
fn get_log(state: tauri::State<'_, AppState>) -> Vec<String> {
    // On mobile, push_log_line populates MOBILE_LOG via FFI.
    // On desktop, append_log populates state.log_lines from child process.
    // Check MOBILE_LOG first; if it has data, use it.
    let mobile = MOBILE_LOG.lock().unwrap();
    if !mobile.is_empty() {
        return mobile.clone();
    }
    drop(mobile);
    state.log_lines.lock().unwrap().clone()
}

// ── External Links ──

type OpenUrlFn = unsafe extern "C" fn(*const std::ffi::c_char);

static OPEN_URL_HANDLER: std::sync::OnceLock<OpenUrlFn> = std::sync::OnceLock::new();

#[no_mangle]
pub extern "C" fn register_url_handler(f: OpenUrlFn) {
    let _ = OPEN_URL_HANDLER.set(f);
}

#[tauri::command]
fn get_wallet_build_hash() -> String {
    env!("WALLET_BUILD_HASH").to_string()
}

#[tauri::command]
fn get_fbd_bundled_hash() -> String {
    env!("FBD_BUNDLED_HASH").to_string()
}

#[tauri::command]
fn get_api_key_cmd(state: tauri::State<'_, AppState>) -> Option<String> {
    let settings = state.settings.lock().unwrap();
    get_api_key(&settings)
}

// ── Ledger Stax (desktop only) ──

#[cfg(desktop)]
#[tauri::command]
async fn ledger_get_account_xpub(account: u32) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || ledger::cmd_get_account_xpub(account))
        .await
        .map_err(|e| format!("ledger task panicked: {e}"))?
}

#[cfg(desktop)]
#[tauri::command]
async fn ledger_sign_pstx(
    pstx_hex: String,
    network: String,
    address_to_path: std::collections::HashMap<String, String>,
) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        ledger::cmd_sign_pstx(&pstx_hex, &network, &address_to_path)
    })
    .await
    .map_err(|e| format!("ledger task panicked: {e}"))?
}

#[tauri::command]
fn open_external(url: String) -> Result<(), String> {
    if let Some(f) = OPEN_URL_HANDLER.get() {
        let c_str = std::ffi::CString::new(url).map_err(|e| e.to_string())?;
        unsafe { f(c_str.as_ptr()); }
        Ok(())
    } else {
        open::that(&url).map_err(|e| e.to_string())
    }
}

// ── Browser ──

/// Start the DANE proxy on mobile. Called from Swift/Kotlin at startup.
/// No AppHandle here — the extension bridge is desktop-only.
#[no_mangle]
pub extern "C" fn start_dane_proxy() {
    proxy::dns::set_port(network_ports(DEFAULT_NETWORK).dns);
    let base_dir = settings_base_dir();
    let ca = match proxy::ca::CertAuthority::load_or_create(&base_dir) {
        Ok(ca) => std::sync::Arc::new(ca),
        Err(e) => {
            println!("[fistbump] failed to init proxy CA: {}", e);
            return;
        }
    };
    proxy::start_proxy(ca, None);
}

type BrowseFn = unsafe extern "C" fn(*const std::ffi::c_char, u8);

static BROWSE_HANDLER: std::sync::OnceLock<BrowseFn> = std::sync::OnceLock::new();

#[no_mangle]
pub extern "C" fn register_browse_handler(show: BrowseFn) {
    let _ = BROWSE_HANDLER.set(show);
}

#[tauri::command]
fn browse(app: tauri::AppHandle, url: String, dark: bool) -> Result<(), String> {
    // iOS: present a modal WKWebView via FFI
    if let Some(f) = BROWSE_HANDLER.get() {
        let c_str = std::ffi::CString::new(url).map_err(|e| e.to_string())?;
        unsafe { f(c_str.as_ptr(), if dark { 1 } else { 0 }); }
        return Ok(());
    }

    // Android: launch the full-screen BrowserActivity
    #[cfg(target_os = "android")]
    {
        browse_android::browse(&url, dark);
        return Ok(());
    }

    // Desktop: open a new Tauri webview window
    #[cfg(desktop)]
    {
        use tauri::WebviewWindowBuilder;
        let parsed: url::Url = url.parse().map_err(|e: url::ParseError| e.to_string())?;

        if let Some(win) = app.get_webview_window("browser") {
            let _ = win.navigate(parsed);
            return Ok(());
        }

        WebviewWindowBuilder::new(
            &app, "browser", tauri::WebviewUrl::External(parsed),
        )
        .title("Fistbump Browser")
        .inner_size(1000.0, 700.0)
        .min_inner_size(400.0, 300.0)
        .build()
        .map_err(|e: tauri::Error| e.to_string())?;
    }

    let _ = app;
    Ok(())
}

// ── QR Scanner ──

// Native QR scanner callback: scan_fn starts the scanner, result comes via qr_scan_result FFI.
type QRScanFn = unsafe extern "C" fn();
type QRScanStopFn = unsafe extern "C" fn();

static QR_SCAN_HANDLER: std::sync::OnceLock<QRScanFn> = std::sync::OnceLock::new();
static QR_SCAN_STOP_HANDLER: std::sync::OnceLock<QRScanStopFn> = std::sync::OnceLock::new();
static QR_SCAN_TX: Mutex<Option<std::sync::mpsc::Sender<String>>> = Mutex::new(None);

#[no_mangle]
pub extern "C" fn register_qr_scan_handler(start: QRScanFn, stop: QRScanStopFn) {
    let _ = QR_SCAN_HANDLER.set(start);
    let _ = QR_SCAN_STOP_HANDLER.set(stop);
}

/// Called from Swift when a QR code is scanned.
#[no_mangle]
pub extern "C" fn qr_scan_result(ptr: *const std::ffi::c_char) {
    let text = if ptr.is_null() {
        String::new()
    } else {
        unsafe { std::ffi::CStr::from_ptr(ptr) }
            .to_string_lossy()
            .to_string()
    };
    if let Some(tx) = QR_SCAN_TX.lock().unwrap().take() {
        let _ = tx.send(text);
    }
}

#[tauri::command]
async fn scan_qr() -> Result<String, String> {
    // iOS: native AVFoundation scanner via FFI
    if let Some(f) = QR_SCAN_HANDLER.get() {
        let (tx, rx) = std::sync::mpsc::channel();
        *QR_SCAN_TX.lock().unwrap() = Some(tx);
        unsafe { f(); }
        let result = tokio::task::spawn_blocking(move || {
            rx.recv_timeout(std::time::Duration::from_secs(120))
                .unwrap_or_default()
        })
        .await
        .map_err(|e| e.to_string())?;
        if result.is_empty() {
            Err("Scan cancelled".into())
        } else {
            Ok(result)
        }
    }
    else {
        Err("QR scanning not available on this platform".into())
    }
}

#[tauri::command]
fn scan_qr_stop() {
    // Cancel any pending scan
    if let Some(tx) = QR_SCAN_TX.lock().unwrap().take() {
        let _ = tx.send(String::new());
    }
    if let Some(f) = QR_SCAN_STOP_HANDLER.get() {
        unsafe { f(); }
    }
}

// ── Node Manager ──

#[cfg(any(desktop, target_os = "android"))]
fn append_log(state: &AppState, text: &str) {
    let mut lines = state.log_lines.lock().unwrap();
    for line in text.lines().filter(|l| !l.is_empty()) {
        lines.push(line.to_string());
    }
    if lines.len() > MAX_LOG_LINES {
        let drain = lines.len() - MAX_LOG_LINES;
        lines.drain(..drain);
    }
}

#[cfg(any(desktop, target_os = "android"))]
fn find_binary() -> Option<PathBuf> {
    #[cfg(target_os = "android")]
    {
        // On Android, fbd is extracted from assets to the app's files dir
        let files_dir = android_files_dir();
        let bin_path = files_dir.join("fbd");
        if bin_path.exists() {
            return Some(bin_path);
        }
        println!("[fistbump] fbd not found at {:?}, needs extraction", bin_path);
        return None;
    }

    #[cfg(not(target_os = "android"))]
    {
        let exe_name = if cfg!(target_os = "windows") {
            "fbd.exe"
        } else {
            "fbd"
        };

        let mut candidates = Vec::new();

        // Prefer bundled binary next to the executable (inside .app bundle)
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                candidates.push(dir.join(exe_name));
            }
        }

        // Fallback: development paths
        let project_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");

        if cfg!(target_os = "windows") {
            candidates.push(project_root.join("bin").join("windows").join(exe_name));
        } else if cfg!(target_os = "linux") {
            let arch = if cfg!(target_arch = "aarch64") { "aarch64" } else { "x86_64" };
            candidates.push(project_root.join("bin").join("linux").join(arch).join(exe_name));
            candidates.push(project_root.join("bin").join("linux").join(exe_name));
        } else {
            candidates.push(project_root.join("bin").join("darwin").join(exe_name));
        }

        candidates.push(
            project_root
                .join("..")
                .join("fbd")
                .join(".build")
                .join("debug")
                .join(exe_name),
        );

        for p in &candidates {
            if p.as_os_str().is_empty() {
                continue;
            }
            if p.exists() {
                println!("[fistbump] found fbd at {:?}", p);
                return Some(p.clone());
            }
        }
        println!("[fistbump] fbd not found, searched: {:?}", candidates);
        None
    }
}

#[cfg(target_os = "android")]
fn android_files_dir() -> PathBuf {
    PathBuf::from("/data/data/org.fistbump.wallet/files")
}

/// Find the native library directory by locating where libfistbump.so was loaded from.
#[cfg(target_os = "android")]
fn find_native_lib_dir() -> Option<PathBuf> {
    if let Ok(maps) = fs::read_to_string("/proc/self/maps") {
        for line in maps.lines() {
            if line.contains("libfistbump.so") {
                if let Some(full_path) = line.split_whitespace().last() {
                    if let Some(parent) = PathBuf::from(full_path).parent() {
                        return Some(parent.to_path_buf());
                    }
                }
            }
        }
    }
    None
}

/// Find fbd in the native libs directory — run it directly from there
/// (Android SELinux allows executing from the native lib dir but not app data).
#[cfg(target_os = "android")]
fn setup_android_binary() -> Option<PathBuf> {
    let native_lib_dir = find_native_lib_dir()?;
    let bin_path = native_lib_dir.join("libfbd.so");
    println!("[fistbump] native lib dir: {:?}", native_lib_dir);

    if bin_path.exists() {
        println!("[fistbump] found fbd at {:?}", bin_path);
        return Some(bin_path);
    }

    println!("[fistbump] libfbd.so not found at {:?}", bin_path);
    None
}

#[cfg(any(desktop, target_os = "android"))]
fn kill_pid(pid: u32) {
    #[cfg(unix)]
    unsafe {
        libc::kill(pid as i32, libc::SIGTERM);
    }
    #[cfg(windows)]
    {
        let _ = Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/F"])
            .output();
    }
}

#[cfg(any(desktop, target_os = "android"))]
fn cleanup_stale(state: &AppState) {
    if let Ok(pid_str) = fs::read_to_string(pid_path()) {
        if let Ok(pid) = pid_str.trim().parse::<u32>() {
            if pid > 0 {
                #[cfg(unix)]
                {
                    let running = unsafe { libc::kill(pid as i32, 0) == 0 };
                    if running {
                        append_log(
                            state,
                            &format!("[fistbump] killing stale fbd (pid {})", pid),
                        );
                        kill_pid(pid);
                    }
                }
            }
        }
    }
    let _ = fs::remove_file(pid_path());
}

#[cfg(any(desktop, target_os = "android"))]
fn start_node(app_handle: tauri::AppHandle) {
    let state = app_handle.state::<AppState>();

    #[cfg(target_os = "android")]
    let binary = match setup_android_binary() {
        Some(b) => b,
        None => {
            append_log(&state, "[fistbump] error: fbd binary not found on Android");
            return;
        }
    };
    #[cfg(not(target_os = "android"))]
    let binary = match find_binary() {
        Some(b) => b,
        None => {
            append_log(&state, "[fistbump] error: bundled fbd binary not found");
            return;
        }
    };

    cleanup_stale(&state);

    println!("[fistbump] starting fbd: {:?}", binary);

    let data_dir = fbd_data_dir();
    let _ = fs::create_dir_all(&data_dir);

    let platform = if cfg!(target_os = "android") {
        "android"
    } else if cfg!(target_os = "macos") {
        "mac"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else {
        "linux"
    };
    let agent = format!("fbw:{}({})", env!("CARGO_PKG_VERSION"), platform);

    let mut cmd = Command::new(&binary);
    cmd.args(["--log-level", "debug", "--network", DEFAULT_NETWORK])
        .arg("--datadir")
        .arg(&data_dir)
        .args(["--agent", &agent])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    // Pass mining args if mining is enabled
    {
        let settings = state.settings.lock().unwrap();
        if settings.mining_enabled {
            if let Some(ref addr) = settings.miner_address {
                if !addr.is_empty() {
                    cmd.args(["--miner-address", addr]);
                }
            }
            if settings.miner_threads > 0 {
                cmd.args(["--miner-threads", &settings.miner_threads.to_string()]);
            }
        }
    }

    #[cfg(target_os = "android")]
    {
        let files_dir = android_files_dir();
        let lib_dir = find_native_lib_dir()
            .unwrap_or_else(|| PathBuf::from("/data/data/org.fistbump.wallet/lib"));
        cmd.env("HOME", &files_dir)
            .env("TMPDIR", files_dir.join("tmp"))
            .env("LD_LIBRARY_PATH", &lib_dir);
        let _ = fs::create_dir_all(files_dir.join("tmp"));
    }

    #[cfg(not(target_os = "android"))]
    cmd.env("NSUnbufferedIO", "YES");

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    let result = cmd.spawn();

    let mut child = match result {
        Ok(c) => c,
        Err(e) => {
            append_log(
                &state,
                &format!("[fistbump] error: failed to start fbd: {}", e),
            );
            return;
        }
    };

    let pid = child.id();
    *state.fbd_pid.lock().unwrap() = Some(pid);
    let _ = fs::create_dir_all(settings_dir());
    let _ = fs::write(pid_path(), pid.to_string());
    append_log(&state, &format!("[fistbump] started fbd (pid {})", pid));

    // On Android, start the DANE proxy once (desktop starts it in setup, iOS in Swift)
    #[cfg(target_os = "android")]
    {
        static PROXY_STARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        if !PROXY_STARTED.swap(true, std::sync::atomic::Ordering::SeqCst) {
            let base_dir = settings_base_dir();
            let ca = match proxy::ca::CertAuthority::load_or_create(&base_dir) {
                Ok(ca) => std::sync::Arc::new(ca),
                Err(e) => {
                    append_log(&state, &format!("[fistbump] proxy CA init failed: {}", e));
                    std::sync::Arc::new(proxy::ca::CertAuthority::load_or_create(&base_dir).expect("CA"))
                }
            };
            proxy::start_proxy(ca, None);
            append_log(&state, "[fistbump] DANE proxy started");
        }
    }

    // Read stdout in background
    if let Some(stdout) = child.stdout.take() {
        let handle = app_handle.clone();
        std::thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines().map_while(Result::ok) {
                let st = handle.state::<AppState>();
                append_log(&st, &line);
            }
        });
    }

    // Read stderr in background
    if let Some(stderr) = child.stderr.take() {
        let handle = app_handle.clone();
        std::thread::spawn(move || {
            let reader = BufReader::new(stderr);
            for line in reader.lines().map_while(Result::ok) {
                let st = handle.state::<AppState>();
                append_log(&st, &line);
            }
        });
    }

    // Wait for exit and auto-restart
    let handle = app_handle.clone();
    std::thread::spawn(move || {
        let status = child.wait();
        let code = status.map(|s| s.code().unwrap_or(-1)).unwrap_or(-1);
        let st = handle.state::<AppState>();
        append_log(&st, &format!("[fistbump] fbd exited (code {})", code));
        println!("[fistbump] fbd exited with code {}", code);
        *st.fbd_pid.lock().unwrap() = None;

        if !st.is_quitting.load(Ordering::Relaxed) {
            std::thread::sleep(std::time::Duration::from_secs(3));
            if !st.is_quitting.load(Ordering::Relaxed) && st.fbd_pid.lock().unwrap().is_none() {
                append_log(&st, "[fistbump] restarting fbd...");
                start_node(handle);
            }
        }
    });
}

#[cfg(any(desktop, target_os = "android"))]
fn stop_node(state: &AppState) {
    state.is_quitting.store(true, Ordering::Relaxed);
    if let Some(pid) = state.fbd_pid.lock().unwrap().take() {
        kill_pid(pid);
    }
    let _ = fs::remove_file(pid_path());
}

/// Registered iOS node restart callback (set from Swift via FFI).
static RESTART_NODE_HANDLER: std::sync::OnceLock<unsafe extern "C" fn()> = std::sync::OnceLock::new();

#[no_mangle]
pub extern "C" fn register_restart_handler(f: unsafe extern "C" fn()) {
    let _ = RESTART_NODE_HANDLER.set(f);
}

fn restart_if_managed(_state: &AppState, _app_handle: tauri::AppHandle) {
    // Clear logs so JS can detect the new "P2P listening" line
    _state.log_lines.lock().unwrap().clear();
    MOBILE_LOG.lock().unwrap().clear();

    #[cfg(any(desktop, target_os = "android"))]
    {
        if let Some(pid) = _state.fbd_pid.lock().unwrap().take() {
            append_log(_state, "[fistbump] restarting fbd for settings change...");
            kill_pid(pid);
            // Wait for old process to fully exit before starting new one
            #[cfg(unix)]
            {
                for _ in 0..100 {
                    let alive = unsafe { libc::kill(pid as i32, 0) == 0 };
                    if !alive { break; }
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
            }
            #[cfg(not(unix))]
            std::thread::sleep(std::time::Duration::from_secs(2));
        }
        start_node(_app_handle);
    }
    // iOS: fire-and-forget — JS watches logs for startup completion
    if let Some(f) = RESTART_NODE_HANDLER.get() {
        unsafe { f(); }
    }
}

// ── Biometric Keychain ──

// iOS: uses FFI callbacks registered from Swift (FBDNode.swift).
// macOS: calls Security.framework + LocalAuthentication.framework directly.
// Android: calls Kotlin BiometricBridge via JNI.

type BiometricAvailableFn = extern "C" fn() -> bool;
type BiometricSaveFn = extern "C" fn(*const std::ffi::c_char, *const std::ffi::c_char) -> bool;
type BiometricLoadFn = extern "C" fn(*const std::ffi::c_char) -> *const std::ffi::c_char;
type BiometricDeleteFn = extern "C" fn(*const std::ffi::c_char);

static BIOMETRIC_AVAILABLE: std::sync::OnceLock<BiometricAvailableFn> = std::sync::OnceLock::new();
static BIOMETRIC_SAVE: std::sync::OnceLock<BiometricSaveFn> = std::sync::OnceLock::new();
static BIOMETRIC_LOAD: std::sync::OnceLock<BiometricLoadFn> = std::sync::OnceLock::new();
static BIOMETRIC_DELETE: std::sync::OnceLock<BiometricDeleteFn> = std::sync::OnceLock::new();

#[no_mangle]
pub extern "C" fn register_biometric_handlers(
    available: BiometricAvailableFn,
    save: BiometricSaveFn,
    load: BiometricLoadFn,
    delete: BiometricDeleteFn,
) {
    let _ = BIOMETRIC_AVAILABLE.set(available);
    let _ = BIOMETRIC_SAVE.set(save);
    let _ = BIOMETRIC_LOAD.set(load);
    let _ = BIOMETRIC_DELETE.set(delete);
}

// macOS: per-wallet encrypted file under ~/.fistbump/biometric/, gated by a
// Touch ID prompt via LAContext.evaluatePolicy.
//
// Why not the Keychain? SecItemAdd with kSecAttrAccessControl requires a
// `keychain-access-groups` entitlement; dev builds and ad-hoc-signed builds
// fail with errSecMissingEntitlement (-34018). File storage works in every
// build configuration and sidesteps the entitlement entirely.
//
// Security boundary: 0600 file perms on each wallet file + the Touch ID
// gate in load(). The in-file random key means the encryption is really
// obfuscation — anyone who can read the file can decrypt it. But the same
// attacker could already read the wallet's own on-disk state in ~/.fistbump,
// so this isn't the weak link.
#[cfg(target_os = "macos")]
mod biometric_macos {
    use std::ffi::c_void;
    use std::path::PathBuf;
    use std::ptr;

    #[link(name = "LocalAuthentication", kind = "framework")]
    extern "C" {
        fn objc_getClass(name: *const u8) -> *mut c_void;
        fn sel_registerName(name: *const u8) -> *mut c_void;
        fn objc_msgSend();

        // libdispatch — turns LAContext's async callback into a sync call.
        fn dispatch_semaphore_create(value: isize) -> *mut c_void;
        fn dispatch_semaphore_signal(dsema: *mut c_void) -> isize;
        fn dispatch_semaphore_wait(dsema: *mut c_void, timeout: u64) -> isize;

        // Block runtime — LAContext.evaluatePolicy takes an Objective-C block.
        static _NSConcreteStackBlock: c_void;
    }

    const DISPATCH_TIME_FOREVER: u64 = !0;

    fn bio_dir() -> PathBuf {
        crate::settings_dir().join("biometric")
    }

    fn bio_path(wallet: &str) -> PathBuf {
        bio_dir().join(wallet)
    }

    pub fn is_available() -> bool {
        unsafe {
            let cls = objc_getClass(b"LAContext\0".as_ptr());
            if cls.is_null() { return false; }
            type AllocFn = unsafe extern "C" fn(*mut c_void, *mut c_void) -> *mut c_void;
            type InitFn = unsafe extern "C" fn(*mut c_void, *mut c_void) -> *mut c_void;
            type CanEvalFn = unsafe extern "C" fn(*mut c_void, *mut c_void, isize, *mut c_void) -> bool;
            type ReleaseFn = unsafe extern "C" fn(*mut c_void, *mut c_void);
            let alloc: AllocFn = std::mem::transmute(objc_msgSend as *const ());
            let init: InitFn = std::mem::transmute(objc_msgSend as *const ());
            let can_eval: CanEvalFn = std::mem::transmute(objc_msgSend as *const ());
            let release: ReleaseFn = std::mem::transmute(objc_msgSend as *const ());
            let obj = alloc(cls, sel_registerName(b"alloc\0".as_ptr()));
            let obj = init(obj, sel_registerName(b"init\0".as_ptr()));
            let result = can_eval(obj, sel_registerName(b"canEvaluatePolicy:error:\0".as_ptr()),
                                  1, ptr::null_mut());
            release(obj, sel_registerName(b"release\0".as_ptr()));
            result
        }
    }

    /// Encrypt `passphrase` under a freshly-generated 32-byte key and write
    /// `[key || nonce || ciphertext+tag]` to `~/.fistbump/biometric/<wallet>`.
    /// The key is embedded in the file because storing it elsewhere under the
    /// same user account gains nothing against the relevant attacker.
    pub fn save(wallet: &str, passphrase: &str) -> Result<(), String> {
        use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, AES_256_GCM};
        use ring::rand::{SecureRandom, SystemRandom};

        let dir = bio_dir();
        std::fs::create_dir_all(&dir).map_err(|e| format!("create biometric dir: {}", e))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
        }

        let rng = SystemRandom::new();
        let mut key_bytes = [0u8; 32];
        let mut nonce_bytes = [0u8; 12];
        rng.fill(&mut key_bytes).map_err(|_| "rng failed".to_string())?;
        rng.fill(&mut nonce_bytes).map_err(|_| "rng failed".to_string())?;

        let key = LessSafeKey::new(
            UnboundKey::new(&AES_256_GCM, &key_bytes).map_err(|_| "key init failed".to_string())?,
        );
        let nonce = Nonce::try_assume_unique_for_key(&nonce_bytes)
            .map_err(|_| "nonce init failed".to_string())?;

        let mut data = passphrase.as_bytes().to_vec();
        key.seal_in_place_append_tag(nonce, Aad::empty(), &mut data)
            .map_err(|_| "encryption failed".to_string())?;

        // File layout: [32 key][12 nonce][ciphertext + 16-byte GCM tag]
        let mut out = Vec::with_capacity(32 + 12 + data.len());
        out.extend_from_slice(&key_bytes);
        out.extend_from_slice(&nonce_bytes);
        out.extend_from_slice(&data);

        let path = bio_path(wallet);
        std::fs::write(&path, &out).map_err(|e| format!("write biometric file: {}", e))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
        }
        Ok(())
    }

    /// Block on a Touch ID prompt, then decrypt `~/.fistbump/biometric/<wallet>`
    /// and return the stored passphrase. Any failure in either step returns None.
    pub fn load(wallet: &str) -> Option<String> {
        use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, AES_256_GCM};

        let path = bio_path(wallet);
        if !path.exists() {
            return None;
        }

        // Touch ID happens BEFORE decryption so a failed/cancelled biometric
        // prompt can't be bypassed by anyone who couldn't have triggered it.
        if !prompt_biometric("Unlock wallet") {
            return None;
        }

        let data = std::fs::read(&path).ok()?;
        if data.len() < 32 + 12 + 16 {
            return None;
        }
        let key_bytes: [u8; 32] = data[..32].try_into().ok()?;
        let nonce_bytes: [u8; 12] = data[32..44].try_into().ok()?;
        let mut ciphertext = data[44..].to_vec();

        let key = LessSafeKey::new(UnboundKey::new(&AES_256_GCM, &key_bytes).ok()?);
        let nonce = Nonce::try_assume_unique_for_key(&nonce_bytes).ok()?;
        let plaintext = key
            .open_in_place(nonce, Aad::empty(), &mut ciphertext)
            .ok()?;
        String::from_utf8(plaintext.to_vec()).ok()
    }

    pub fn delete(wallet: &str) {
        let _ = std::fs::remove_file(bio_path(wallet));
    }

    /// Present a Touch ID prompt (LAPolicyDeviceOwnerAuthenticationWithBiometrics)
    /// and block the caller until the user finishes or cancels.
    fn prompt_biometric(reason: &str) -> bool {
        // Objective-C block layout for `^(BOOL success, NSError *error)`.
        #[repr(C)]
        struct ReplyBlock {
            isa: *const c_void,
            flags: i32,
            reserved: i32,
            invoke: unsafe extern "C" fn(*mut ReplyBlock, bool, *mut c_void),
            descriptor: *const BlockDesc,
            semaphore: *mut c_void,
            success: *mut bool,
        }
        #[repr(C)]
        struct BlockDesc {
            reserved: usize,
            size: usize,
        }
        static BLOCK_DESC: BlockDesc = BlockDesc {
            reserved: 0,
            size: std::mem::size_of::<ReplyBlock>(),
        };

        unsafe extern "C" fn reply_invoke(
            block: *mut ReplyBlock,
            success: bool,
            _err: *mut c_void,
        ) {
            *(*block).success = success;
            dispatch_semaphore_signal((*block).semaphore);
        }

        unsafe {
            let cls = objc_getClass(b"LAContext\0".as_ptr());
            if cls.is_null() {
                return false;
            }

            type AllocFn = unsafe extern "C" fn(*mut c_void, *mut c_void) -> *mut c_void;
            type InitFn = unsafe extern "C" fn(*mut c_void, *mut c_void) -> *mut c_void;
            type EvalPolicyFn = unsafe extern "C" fn(
                *mut c_void,
                *mut c_void,
                isize,
                *mut c_void,
                *mut ReplyBlock,
            );
            type ReleaseFn = unsafe extern "C" fn(*mut c_void, *mut c_void);

            let alloc: AllocFn = std::mem::transmute(objc_msgSend as *const ());
            let init: InitFn = std::mem::transmute(objc_msgSend as *const ());
            let evaluate_policy: EvalPolicyFn = std::mem::transmute(objc_msgSend as *const ());
            let release: ReleaseFn = std::mem::transmute(objc_msgSend as *const ());

            let ctx = alloc(cls, sel_registerName(b"alloc\0".as_ptr()));
            let ctx = init(ctx, sel_registerName(b"init\0".as_ptr()));

            // NSString *reason = [[NSString alloc] initWithBytes:... length:... encoding:NSUTF8StringEncoding]
            let ns_cls = objc_getClass(b"NSString\0".as_ptr());
            type StrInitFn = unsafe extern "C" fn(
                *mut c_void,
                *mut c_void,
                *const u8,
                usize,
                u64,
            ) -> *mut c_void;
            let str_init: StrInitFn = std::mem::transmute(objc_msgSend as *const ());
            let reason_obj = alloc(ns_cls, sel_registerName(b"alloc\0".as_ptr()));
            let reason_obj = str_init(
                reason_obj,
                sel_registerName(b"initWithBytes:length:encoding:\0".as_ptr()),
                reason.as_ptr(),
                reason.len(),
                4, // NSUTF8StringEncoding
            );

            let sem = dispatch_semaphore_create(0);
            let mut success = false;

            let mut block = ReplyBlock {
                isa: &_NSConcreteStackBlock as *const c_void,
                flags: 0,
                reserved: 0,
                invoke: reply_invoke,
                descriptor: &BLOCK_DESC,
                semaphore: sem,
                success: &mut success,
            };

            evaluate_policy(
                ctx,
                sel_registerName(b"evaluatePolicy:localizedReason:reply:\0".as_ptr()),
                1, // LAPolicyDeviceOwnerAuthenticationWithBiometrics
                reason_obj as *mut c_void,
                &mut block,
            );

            dispatch_semaphore_wait(sem, DISPATCH_TIME_FOREVER);

            release(reason_obj as *mut c_void, sel_registerName(b"release\0".as_ptr()));
            release(ctx, sel_registerName(b"release\0".as_ptr()));

            success
        }
    }
}

// Android: call Kotlin BiometricBridge via JNI
#[cfg(target_os = "android")]
mod biometric_android {
    use jni::objects::{JString, JValue};
    use jni::JavaVM;
    use std::sync::OnceLock;

    pub(crate) static JAVA_VM: OnceLock<JavaVM> = OnceLock::new();

    /// Called from Kotlin BiometricBridge.nativeRegisterVm() to provide the JavaVM.
    #[no_mangle]
    pub extern "system" fn Java_org_fistbump_wallet_BiometricBridge_nativeRegisterVm(
        env: jni::JNIEnv,
        _class: jni::objects::JClass,
    ) {
        if let Ok(vm) = env.get_java_vm() {
            let _ = JAVA_VM.set(vm);
        }
    }

    fn get_vm() -> Option<&'static JavaVM> {
        JAVA_VM.get()
    }

    pub fn is_available() -> bool {
        let vm = match get_vm() { Some(v) => v, None => return false };
        let mut env = match vm.attach_current_thread() { Ok(e) => e, Err(_) => return false };
        let cls = match env.find_class("org/fistbump/wallet/BiometricBridge") {
            Ok(c) => c, Err(_) => return false,
        };
        env.call_static_method(cls, "isAvailable", "()Z", &[])
            .ok().and_then(|v| v.z().ok()).unwrap_or(false)
    }

    pub fn save(wallet: &str, passphrase: &str) -> bool {
        let vm = match get_vm() { Some(v) => v, None => return false };
        let mut env = match vm.attach_current_thread() { Ok(e) => e, Err(_) => return false };
        let cls = match env.find_class("org/fistbump/wallet/BiometricBridge") {
            Ok(c) => c, Err(_) => return false,
        };
        let jw = match env.new_string(wallet) { Ok(s) => s, Err(_) => return false };
        let jp = match env.new_string(passphrase) { Ok(s) => s, Err(_) => return false };
        env.call_static_method(
            cls, "save", "(Ljava/lang/String;Ljava/lang/String;)Z",
            &[JValue::Object(&jw), JValue::Object(&jp)],
        ).ok().and_then(|v| v.z().ok()).unwrap_or(false)
    }

    pub fn load(wallet: &str) -> Option<String> {
        let vm = match get_vm() { Some(v) => v, None => return None };
        let mut env = match vm.attach_current_thread() { Ok(e) => e, Err(_) => return None };
        let cls = match env.find_class("org/fistbump/wallet/BiometricBridge") {
            Ok(c) => c, Err(_) => return None,
        };
        let jw = match env.new_string(wallet) { Ok(s) => s, Err(_) => return None };
        let result = env.call_static_method(
            cls, "load", "(Ljava/lang/String;)Ljava/lang/String;",
            &[JValue::Object(&jw)],
        ).ok()?;
        let obj = result.l().ok()?;
        if obj.is_null() { return None; }
        let jstr = JString::from(obj);
        env.get_string(&jstr).ok().map(|s| s.into())
    }

    pub fn delete(wallet: &str) {
        let vm = match get_vm() { Some(v) => v, None => return };
        let mut env = match vm.attach_current_thread() { Ok(e) => e, Err(_) => return };
        let cls = match env.find_class("org/fistbump/wallet/BiometricBridge") {
            Ok(c) => c, Err(_) => return,
        };
        let jw = match env.new_string(wallet) { Ok(s) => s, Err(_) => return };
        let _ = env.call_static_method(
            cls, "delete", "(Ljava/lang/String;)V",
            &[JValue::Object(&jw)],
        );
    }
}

// Android: call Kotlin BrowserBridge via JNI
#[cfg(target_os = "android")]
mod browse_android {
    use jni::objects::JValue;

    fn get_vm() -> Option<&'static jni::JavaVM> {
        super::biometric_android::JAVA_VM.get()
    }

    pub fn browse(url: &str, dark: bool) {
        let vm = match get_vm() { Some(v) => v, None => return };
        let mut env = match vm.attach_current_thread() { Ok(e) => e, Err(_) => return };
        let cls = match env.find_class("org/fistbump/wallet/BrowserBridge") {
            Ok(c) => c, Err(_) => return,
        };
        let jurl = match env.new_string(url) { Ok(s) => s, Err(_) => return };
        let _ = env.call_static_method(
            cls, "browse", "(Ljava/lang/String;Z)V",
            &[JValue::Object(&jurl), JValue::Bool(dark as u8)],
        );
    }
}


#[tauri::command]
fn biometric_available() -> bool {
    // iOS: registered FFI callback
    if let Some(f) = BIOMETRIC_AVAILABLE.get() { return f(); }
    // macOS: native Security + LocalAuthentication
    #[cfg(target_os = "macos")]
    { return biometric_macos::is_available(); }
    // Android: JNI to Kotlin
    #[cfg(target_os = "android")]
    { return biometric_android::is_available(); }
    #[allow(unreachable_code)]
    false
}

#[tauri::command]
fn biometric_save(wallet: String, passphrase: String, _state: tauri::State<'_, AppState>) -> Result<bool, String> {

    if let Some(f) = BIOMETRIC_SAVE.get() {
        let w = std::ffi::CString::new(wallet).map_err(|e| e.to_string())?;
        let p = std::ffi::CString::new(passphrase).map_err(|e| e.to_string())?;
        return Ok(f(w.as_ptr(), p.as_ptr()));
    }
    #[cfg(target_os = "macos")]
    {
        return match biometric_macos::save(&wallet, &passphrase) {
            Ok(()) => Ok(true),
            Err(e) => Err(e),
        };
    }
    #[cfg(target_os = "android")]
    { return Ok(biometric_android::save(&wallet, &passphrase)); }
    #[allow(unreachable_code)]
    Err("Biometric not available on this platform.".into())
}

#[tauri::command]
fn biometric_load(wallet: String, _state: tauri::State<'_, AppState>) -> Result<Option<String>, String> {

    if let Some(f) = BIOMETRIC_LOAD.get() {
        let w = std::ffi::CString::new(wallet).map_err(|e| e.to_string())?;
        let ptr = f(w.as_ptr());
        if ptr.is_null() { return Ok(None); }
        let c_str = unsafe { std::ffi::CStr::from_ptr(ptr) };
        return Ok(Some(c_str.to_string_lossy().to_string()));
    }
    #[cfg(target_os = "macos")]
    { return Ok(biometric_macos::load(&wallet)); }
    #[cfg(target_os = "android")]
    { return Ok(biometric_android::load(&wallet)); }
    #[allow(unreachable_code)]
    Err("Biometric not available on this platform.".into())
}

#[tauri::command]
fn biometric_delete(wallet: String, _state: tauri::State<'_, AppState>) -> Result<(), String> {

    if let Some(f) = BIOMETRIC_DELETE.get() {
        let w = std::ffi::CString::new(wallet).map_err(|e| e.to_string())?;
        f(w.as_ptr());
        return Ok(());
    }
    #[cfg(target_os = "macos")]
    { biometric_macos::delete(&wallet); return Ok(()); }
    #[cfg(target_os = "android")]
    { biometric_android::delete(&wallet); return Ok(()); }
    #[allow(unreachable_code)]
    Err("Biometric not available on this platform.".into())
}

// ── Settings Commands ──

#[tauri::command]
fn get_settings(state: tauri::State<'_, AppState>) -> Value {
    let settings = state.settings.lock().unwrap();
    json!({
        "minerAddress": settings.miner_address.clone().unwrap_or_default(),
        "miningEnabled": settings.mining_enabled,
        "minerThreads": settings.miner_threads,
        "network": DEFAULT_NETWORK,
    })
}

#[tauri::command]
fn set_miner_address(
    address: String,
    threads: Option<u32>,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    let addr = if address.trim().is_empty() {
        None
    } else {
        Some(address.trim().to_string())
    };

    {
        let mut settings = state.settings.lock().unwrap();
        settings.miner_address = addr;
        if let Some(t) = threads {
            settings.miner_threads = t;
        }
        let _ = fs::create_dir_all(settings_dir());
        if let Err(e) = fs::write(
            settings_path(),
            serde_json::to_string_pretty(&*settings).unwrap_or_default(),
        ) {
            println!("[fistbump] failed to save settings: {}", e);
        } else {
            println!("[fistbump] saved settings to {:?}", settings_path());
        }
    }

    Ok(())
}

#[tauri::command]
fn toggle_mining(
    enabled: bool,
    state: tauri::State<'_, AppState>,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    {
        let mut settings = state.settings.lock().unwrap();
        settings.mining_enabled = enabled;
        let _ = fs::create_dir_all(settings_dir());
        if let Err(e) = fs::write(
            settings_path(),
            serde_json::to_string_pretty(&*settings).unwrap_or_default(),
        ) {
            println!("[fistbump] failed to save settings: {}", e);
        } else {
            println!("[fistbump] saved settings to {:?}", settings_path());
        }
    }

    restart_if_managed(&state, app_handle);

    Ok(())
}

/// Tear down and restart fbd. Called by the frontend when it detects
/// that we've been backgrounded long enough for iOS/Android to have
/// broken the node's sockets or killed the child process — at which
/// point a full restart is more reliable than trying to limp along
/// with whatever half-dead state survived.
///
/// Delegates to `restart_if_managed`, which on desktop/Android kills
/// and respawns the fbd child, and on iOS fires the Swift
/// `FBDNode.shared.restart()` path via the registered FFI callback.
#[tauri::command]
fn restart_node(
    state: tauri::State<'_, AppState>,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    println!("[fistbump] restart_node: restarting for foreground");
    restart_if_managed(&state, app_handle);
    Ok(())
}

// ── Proxy Setup ──

#[cfg(desktop)]
#[tauri::command]
fn get_proxy_status() -> serde_json::Value {
    let settings = load_settings();
    let ca_path = settings_base_dir().join("proxy-ca.crt");
    let ca_exists = ca_path.exists();
    let ca_trusted = ca_exists && proxy::pac::is_ca_installed(&ca_path);
    let pac_installed = proxy::pac::is_pac_installed();

    // Read CA cert expiry date
    let ca_expires: Option<String> = if ca_exists {
        std::fs::read_to_string(&ca_path).ok().and_then(|pem| {
            let der_b64: String = pem.lines()
                .filter(|l| !l.starts_with("-----"))
                .collect();
            let der = base64_decode(&der_b64)?;
            let (_, cert) = x509_parser::parse_x509_certificate(&der).ok()?;
            Some(format!("{}", cert.validity().not_after))
        })
    } else {
        None
    };

    serde_json::json!({
        "caExists": ca_exists,
        "caTrusted": ca_trusted,
        "pacInstalled": pac_installed,
        "setupComplete": ca_trusted && pac_installed,
        "setupDone": settings.proxy_setup_done,
        "proxyEnabled": settings.proxy_enabled,
        "caExpires": ca_expires,
    })
}

#[cfg(desktop)]
fn base64_decode(input: &str) -> Option<Vec<u8>> {
    let table = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::new();
    let mut buf: u32 = 0;
    let mut bits = 0;
    for &b in input.as_bytes() {
        if b == b'=' || b == b'\n' || b == b'\r' { continue; }
        let val = table.iter().position(|&c| c == b)? as u32;
        buf = (buf << 6) | val;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
        }
    }
    Some(out)
}

#[cfg(desktop)]
#[tauri::command]
fn toggle_proxy(enabled: bool) -> Result<(), String> {
    let mut settings = load_settings();
    settings.proxy_enabled = enabled;
    let _ = fs::create_dir_all(settings_dir());
    let _ = fs::write(settings_path(), serde_json::to_string_pretty(&settings).unwrap_or_default());

    if enabled {
        // Install CA if needed
        let ca_path = settings_base_dir().join("proxy-ca.crt");
        if ca_path.exists() && !proxy::pac::is_ca_installed(&ca_path) {
            proxy::pac::install_ca_cert(&ca_path);
        }
        // Install PAC
        if !proxy::pac::is_pac_installed() {
            proxy::install_pac();
        }
    } else {
        // Remove PAC from all network services
        proxy::remove_pac();
    }
    Ok(())
}

#[cfg(desktop)]
#[tauri::command]
fn renew_ca() -> Result<String, String> {
    let dir = settings_base_dir();
    // Delete old CA files to force regeneration
    let _ = std::fs::remove_file(dir.join("proxy-ca.crt"));
    let _ = std::fs::remove_file(dir.join("proxy-ca.key"));
    // Regenerate
    let ca = proxy::ca::CertAuthority::load_or_create(&dir)
        .map_err(|e| format!("Failed to create CA: {}", e))?;
    // Re-install to system trust store
    proxy::pac::install_ca_cert(&ca.cert_path);
    Ok("CA certificate renewed".to_string())
}

#[cfg(desktop)]
#[tauri::command]
fn setup_proxy() -> Result<serde_json::Value, String> {
    let ca_path = settings_base_dir().join("proxy-ca.crt");
    if !ca_path.exists() {
        return Err("Root CA not generated yet".to_string());
    }

    // Install CA cert (will prompt Touch ID / password)
    if !proxy::pac::is_ca_installed(&ca_path) {
        proxy::pac::install_ca_cert(&ca_path);
    }

    // Install PAC
    if !proxy::pac::is_pac_installed() {
        proxy::install_pac();
    }

    // Check results and save flag
    let ca_trusted = proxy::pac::is_ca_installed(&ca_path);
    let pac_installed = proxy::pac::is_pac_installed();
    let complete = ca_trusted && pac_installed;

    if complete {
        // Mark setup as done and enable proxy
        let mut settings = load_settings();
        settings.proxy_setup_done = true;
        settings.proxy_enabled = true;
        let _ = fs::create_dir_all(settings_dir());
        let _ = fs::write(settings_path(), serde_json::to_string_pretty(&settings).unwrap_or_default());
    }

    Ok(serde_json::json!({
        "caTrusted": ca_trusted,
        "pacInstalled": pac_installed,
        "setupComplete": complete,
    }))
}

#[cfg(desktop)]
#[tauri::command]
fn get_pending_migration(state: tauri::State<'_, AppState>) -> Option<MigrationCandidate> {
    state.pending_migration.lock().unwrap().clone()
}

#[cfg(desktop)]
#[tauri::command]
fn resolve_migration(
    accept: bool,
    state: tauri::State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    // Clear pending state up front so a repeat invocation from a double-click
    // won't run the copy twice.
    let candidate = state.pending_migration.lock().unwrap().take();

    if accept {
        let Some(candidate) = candidate else {
            return Err("no pending migration".to_string());
        };
        let src = PathBuf::from(&candidate.source);
        let dst = wallet_wallets_dir();
        // Make sure parents exist before copying.
        if let Some(parent) = dst.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("create dest dir: {}", e))?;
        }
        copy_dir_recursive(&src, &dst).map_err(|e| format!("copy wallets: {}", e))?;
        println!(
            "[fistbump] migrated {} wallet(s) from {} to {}",
            candidate.wallet_count,
            src.display(),
            dst.display()
        );
    }

    // Mark as resolved so we never prompt again, regardless of the answer.
    let marker = migration_marker_path();
    if let Some(parent) = marker.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(&marker, b"1");

    // Now that the wallets (if any) are in place, start fbd.
    start_node(app);
    Ok(())
}

// ── Browser extension bridge ──

/// Resolve a pending browser-extension request after the user clicks
/// approve/deny in the wallet UI (or after the frontend has finished doing
/// the real work for a sendTx / signMessage request). The corresponding
/// extension IPC listener thread is blocked on the channel inside the
/// registered PendingRequest; pushing a Decision unblocks it and lets it
/// write the response back to the extension.
///
/// Three shapes to think about:
///   - `{id, approve: true}` — simple yes (used by connect). The Rust
///     handler builds the response (looks up the address).
///   - `{id, approve: true, result: {...}}` — frontend has already done
///     the work (sendTx produced a txid, signMessage produced a signature).
///     We pass the value through unchanged.
///   - `{id, approve: false, error?: "msg"}` — user denied, or frontend
///     hit an error (wallet RPC failed, user cancelled unlock, etc.). The
///     error message is what the dApp eventually sees.
#[cfg(desktop)]
#[tauri::command]
fn resolve_ext_request(
    id: String,
    approve: bool,
    result: Option<serde_json::Value>,
    error: Option<String>,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    let pending = proxy::extension::take_pending(&id)
        .ok_or_else(|| "no such pending request".to_string())?;

    // Only persist the origin allowlist entry for the "connect" request
    // type — for sendTx / signMessage the origin is already approved
    // (must have been, since dApps have to connect first), so there's
    // nothing to add.
    if approve && pending.kind == "connect" {
        let mut settings = state.settings.lock().unwrap();
        if !settings.approved_origins.contains(&pending.origin) {
            settings.approved_origins.push(pending.origin.clone());
            let _ = fs::create_dir_all(settings_dir());
            let _ = fs::write(
                settings_path(),
                serde_json::to_string_pretty(&*settings).unwrap_or_default(),
            );
        }
    }

    let decision = if approve {
        match result {
            Some(value) => proxy::extension::Decision::ApproveWith(value),
            None => proxy::extension::Decision::Approve,
        }
    } else {
        proxy::extension::Decision::Deny(error.unwrap_or_else(|| "user denied".to_string()))
    };
    pending
        .responder
        .send(decision)
        .map_err(|e| format!("listener thread is gone: {}", e))?;
    Ok(())
}

/// Read the user's currently-approved origins. Surfaced so the wallet
/// settings UI (later) can show them and offer per-origin revocation.
#[cfg(desktop)]
#[tauri::command]
fn list_approved_origins(state: tauri::State<'_, AppState>) -> Vec<String> {
    state.settings.lock().unwrap().approved_origins.clone()
}

/// Tell the Rust side which wallet the user currently has open in the UI.
/// The browser-extension bridge uses this to scope `getwalletinfo` lookups
/// to the right wallet. Pass `None` (omit the field) on logout.
#[cfg(desktop)]
#[tauri::command]
fn set_active_wallet(name: Option<String>, state: tauri::State<'_, AppState>) {
    *state.active_wallet.lock().unwrap() = name.filter(|s| !s.is_empty());
}

/// Revoke a previously-approved origin. Called from the wallet settings UI.
#[cfg(desktop)]
#[tauri::command]
fn revoke_approved_origin(
    origin: String,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    let mut settings = state.settings.lock().unwrap();
    settings.approved_origins.retain(|o| o != &origin);
    let _ = fs::create_dir_all(settings_dir());
    fs::write(
        settings_path(),
        serde_json::to_string_pretty(&*settings).unwrap_or_default(),
    )
    .map_err(|e| format!("write settings: {}", e))?;
    Ok(())
}

// ── App ──

#[cfg(mobile)]
#[tauri::mobile_entry_point]
fn mobile_entry() {
    run();
}

pub fn run() {
    #[cfg(desktop)]
    let _ = rustls::crypto::ring::default_provider().install_default();

    let settings = load_settings();
    proxy::dns::set_port(network_ports(DEFAULT_NETWORK).dns);

    tauri::Builder::default()
        // Registers the `fistbump://` URL scheme so the browser extension can
        // launch us when the wallet isn't running. Bundling adds the scheme to
        // Info.plist on macOS, the Windows registry, and a .desktop file on
        // Linux. We don't need the runtime API — we just want the OS to bring
        // the app up; the extension's HTTP fetch handles everything else.
        .plugin(tauri_plugin_deep_link::init())
        .manage(AppState {
            settings: Mutex::new(settings),
            log_lines: Mutex::new(Vec::new()),
            fbd_pid: Mutex::new(None),
            is_quitting: AtomicBool::new(false),
            #[cfg(desktop)]
            pending_migration: Mutex::new(None),
            #[cfg(desktop)]
            active_wallet: Mutex::new(None),
        })
        .invoke_handler(tauri::generate_handler![
            rpc_call,
            get_log,
            get_api_key_cmd,
            get_wallet_build_hash,
            get_fbd_bundled_hash,
            open_external,
            browse,
            scan_qr,
            scan_qr_stop,
            get_settings,
            set_miner_address,
            toggle_mining,
            restart_node,
            biometric_available,
            biometric_save,
            biometric_load,
            biometric_delete,
            #[cfg(desktop)]
            get_proxy_status,
            #[cfg(desktop)]
            toggle_proxy,
            #[cfg(desktop)]
            renew_ca,
            #[cfg(desktop)]
            setup_proxy,
            #[cfg(desktop)]
            get_pending_migration,
            #[cfg(desktop)]
            resolve_migration,
            #[cfg(desktop)]
            resolve_ext_request,
            #[cfg(desktop)]
            list_approved_origins,
            #[cfg(desktop)]
            revoke_approved_origin,
            #[cfg(desktop)]
            set_active_wallet,
            #[cfg(desktop)]
            ledger_get_account_xpub,
            #[cfg(desktop)]
            ledger_sign_pstx,
        ])
        .setup(|app| {
            // On desktop, check whether we should offer to copy wallets from a
            // legacy `~/.fbd/wallets` install before starting fbd. If there's a
            // candidate, stash it and defer start_node until the frontend calls
            // `resolve_migration`. On Android we just start fbd immediately.
            #[cfg(desktop)]
            {
                if let Some(candidate) = detect_migration_candidate() {
                    println!(
                        "[fistbump] legacy fbd install detected at {} ({} wallet(s)) — waiting for user decision",
                        candidate.source, candidate.wallet_count
                    );
                    let state = app.state::<AppState>();
                    *state.pending_migration.lock().unwrap() = Some(candidate);
                } else {
                    start_node(app.handle().clone());
                }
            }
            #[cfg(target_os = "android")]
            start_node(app.handle().clone());

            #[cfg(desktop)]
            {
                let ca = proxy::ca::CertAuthority::load_or_create(&settings_base_dir())
                    .expect("failed to initialize CA");
                let ca = std::sync::Arc::new(ca);

                // Only install CA/PAC if not already done (avoids Touch ID prompt)
                if !proxy::pac::is_ca_installed(&ca.cert_path) {
                    println!("[fistbump] root CA not yet trusted, deferring to setup wizard");
                } else {
                    println!("[fistbump] root CA already trusted");
                    // Install PAC on launch if proxy is enabled
                    let settings = load_settings();
                    if settings.proxy_enabled {
                        if !proxy::pac::is_pac_installed() {
                            proxy::install_pac();
                            println!("[fistbump] PAC proxy installed on launch");
                        } else {
                            println!("[fistbump] PAC proxy already configured");
                        }
                    }
                }

                proxy::start_proxy(ca, Some(app.handle().clone()));

                // Bind the Unix socket the browser extension talks to via the
                // `fistbump-bridge` native messaging host. Must come before
                // installing the native host JSON so the bridge has something
                // to connect to as soon as it spawns.
                proxy::extension::start_ipc_listener(app.handle().clone());

                // Drop our native messaging host JSON into every Chromium-family
                // browser's NativeMessagingHosts directory so the extension can
                // launch us via chrome.runtime.connectNative without a prompt.
                proxy::extension::install_native_messaging_host(&app.handle());
            }

            let _ = app;
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error building tauri app")
        .run(|_app_handle, _event| {
            #[cfg(any(desktop, target_os = "android"))]
            if let tauri::RunEvent::Exit = _event {
                let state = _app_handle.state::<AppState>();
                stop_node(&state);
            }
            #[cfg(desktop)]
            if let tauri::RunEvent::Exit = _event {
                // Always clean up PAC on exit
                proxy::remove_pac();
            }
        });
}
