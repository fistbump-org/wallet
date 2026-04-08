use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs;
#[cfg(any(desktop, target_os = "android"))]
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
#[cfg(any(desktop, target_os = "android"))]
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use tauri::Manager;

mod proxy;

const MAX_LOG_LINES: usize = 500;

// ── Network Defaults ──

const DEFAULT_NETWORK: &str = env!("FISTBUMP_NETWORK");

struct NetworkPorts {
    rpc: u16,
    dns: u16,
}

fn network_ports(network: &str) -> NetworkPorts {
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
}

pub struct AppState {
    settings: Mutex<Settings>,
    log_lines: Mutex<Vec<String>>,
    fbd_pid: Mutex<Option<u32>>,
    is_quitting: AtomicBool,
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

fn settings_dir() -> PathBuf {
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

fn cookie_path_for(network: &str) -> PathBuf {
    let base = fbd_data_dir();
    if network == "main" {
        base.join(".cookie")
    } else {
        base.join(network).join(".cookie")
    }
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
    let mut builder = client
        .post(&url)
        .header("Content-Type", "application/json")
        .json(&payload)
        .timeout(std::time::Duration::from_secs(120));

    if let Some(key) = get_api_key(&settings) {
        builder = builder.basic_auth("x", Some(key));
    }

    match builder.send().await {
        Ok(res) => match res.json::<Value>().await {
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
        },
        Err(e) => Ok(json!({ "error": e.to_string() })),
    }
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
    proxy::start_proxy(ca);
}

type BrowseFn = unsafe extern "C" fn(*const std::ffi::c_char, f64, f64, f64, f64, u8);
type BrowseHideFn = unsafe extern "C" fn();

static BROWSE_HANDLER: std::sync::OnceLock<BrowseFn> = std::sync::OnceLock::new();
static BROWSE_HIDE_HANDLER: std::sync::OnceLock<BrowseHideFn> = std::sync::OnceLock::new();

#[no_mangle]
pub extern "C" fn register_browse_handler(show: BrowseFn, hide: BrowseHideFn) {
    let _ = BROWSE_HANDLER.set(show);
    let _ = BROWSE_HIDE_HANDLER.set(hide);
}

#[tauri::command]
fn browse(app: tauri::AppHandle, url: String, top: f64, left: f64, width: f64, height: f64, dark: bool, header_height: Option<f64>, tab_bar_height: Option<f64>) -> Result<(), String> {
    // iOS: use native WKWebView via FFI
    if let Some(f) = BROWSE_HANDLER.get() {
        let c_str = std::ffi::CString::new(url).map_err(|e| e.to_string())?;
        unsafe { f(c_str.as_ptr(), top, left, width, height, if dark { 1 } else { 0 }); }
        return Ok(());
    }

    // Android: compute layout from native screen dimensions + safe area insets
    #[cfg(target_os = "android")]
    {
        let hh = header_height.unwrap_or(0.0) as i32;
        let tbh = tab_bar_height.unwrap_or(0.0) as i32;
        browse_android::browse(&url, hh, tbh, dark);
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

type BrowseErrorFn = unsafe extern "C" fn(f64, f64, f64, f64, u8, *const std::ffi::c_char, *const std::ffi::c_char, *const std::ffi::c_char);

static BROWSE_ERROR_HANDLER: std::sync::OnceLock<BrowseErrorFn> = std::sync::OnceLock::new();

#[no_mangle]
pub extern "C" fn register_browse_error_handler(f: BrowseErrorFn) {
    let _ = BROWSE_ERROR_HANDLER.set(f);
}

#[tauri::command]
fn browse_error(top: f64, left: f64, width: f64, height: f64, dark: bool, badge: String, title: String, message: String, header_height: Option<f64>, tab_bar_height: Option<f64>) {
    if let Some(f) = BROWSE_ERROR_HANDLER.get() {
        let b = std::ffi::CString::new(badge).unwrap_or_default();
        let t = std::ffi::CString::new(title).unwrap_or_default();
        let m = std::ffi::CString::new(message).unwrap_or_default();
        unsafe { f(top, left, width, height, if dark { 1 } else { 0 }, b.as_ptr(), t.as_ptr(), m.as_ptr()); }
        return;
    }
    #[cfg(target_os = "android")]
    {
        let hh = header_height.unwrap_or(0.0) as i32;
        let tbh = tab_bar_height.unwrap_or(0.0) as i32;
        browse_android::browse_error(hh, tbh, dark, &badge, &title, &message);
    }
}

#[tauri::command]
fn browse_hide() {
    if let Some(f) = BROWSE_HIDE_HANDLER.get() {
        unsafe { f(); }
        return;
    }
    #[cfg(target_os = "android")]
    browse_android::hide();
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

    let mut cmd = Command::new(&binary);
    cmd.args(["--log-level", "debug", "--network", DEFAULT_NETWORK])
        .arg("--datadir")
        .arg(&data_dir)
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
            proxy::start_proxy(ca);
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

// macOS: file-based storage + LAContext.evaluatePolicy for Touch ID
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

        // libdispatch
        fn dispatch_semaphore_create(value: isize) -> *mut c_void;
        fn dispatch_semaphore_signal(dsema: *mut c_void) -> isize;
        fn dispatch_semaphore_wait(dsema: *mut c_void, timeout: u64) -> isize;

        // Block runtime
        static _NSConcreteStackBlock: c_void;
    }

    const DISPATCH_TIME_FOREVER: u64 = !0;

    // Keychain service name for biometric-protected passphrases
    const SERVICE: &[u8] = b"org.fistbump.wallet\0";

    #[link(name = "Security", kind = "framework")]
    extern "C" {
        fn SecItemAdd(attributes: *const c_void, result: *mut c_void) -> i32;
        fn SecItemCopyMatching(query: *const c_void, result: *mut c_void) -> i32;
        fn SecItemDelete(query: *const c_void) -> i32;
        fn SecAccessControlCreateWithFlags(
            allocator: *const c_void,
            protection: *const c_void,
            flags: u64,
            error: *mut c_void,
        ) -> *mut c_void;

        static kSecClass: *const c_void;
        static kSecClassGenericPassword: *const c_void;
        static kSecAttrService: *const c_void;
        static kSecAttrAccount: *const c_void;
        static kSecValueData: *const c_void;
        static kSecReturnData: *const c_void;
        static kSecAttrAccessControl: *const c_void;
        static kSecAttrAccessibleWhenUnlockedThisDeviceOnly: *const c_void;
        static kSecUseAuthenticationContext: *const c_void;
    }

    // kSecAccessControlBiometryCurrentSet = 1 << 3
    const ACCESS_CONTROL_BIOMETRY: u64 = 1 << 3;

    /// Create a CFDictionary from key-value pairs using toll-free bridging with NSDictionary.
    unsafe fn make_dict(pairs: &[(*const c_void, *const c_void)]) -> *mut c_void {
        let cls = objc_getClass(b"NSMutableDictionary\0".as_ptr());
        type AllocFn = unsafe extern "C" fn(*mut c_void, *mut c_void) -> *mut c_void;
        type InitFn = unsafe extern "C" fn(*mut c_void, *mut c_void) -> *mut c_void;
        type SetFn = unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void, *mut c_void);
        let alloc: AllocFn = std::mem::transmute(objc_msgSend as *const ());
        let init: InitFn = std::mem::transmute(objc_msgSend as *const ());
        let set: SetFn = std::mem::transmute(objc_msgSend as *const ());
        let obj = alloc(cls, sel_registerName(b"alloc\0".as_ptr()));
        let obj = init(obj, sel_registerName(b"init\0".as_ptr()));
        for &(k, v) in pairs {
            set(obj, sel_registerName(b"setObject:forKey:\0".as_ptr()), v as *mut c_void, k as *mut c_void);
        }
        obj
    }

    /// Create an NSData from bytes.
    unsafe fn make_nsdata(bytes: &[u8]) -> *mut c_void {
        let cls = objc_getClass(b"NSData\0".as_ptr());
        type DataFn = unsafe extern "C" fn(*mut c_void, *mut c_void, *const u8, usize) -> *mut c_void;
        let data_fn: DataFn = std::mem::transmute(objc_msgSend as *const ());
        data_fn(cls, sel_registerName(b"dataWithBytes:length:\0".as_ptr()), bytes.as_ptr(), bytes.len())
    }

    /// Create an NSString from a Rust str (must be null-terminated).
    unsafe fn make_nsstring(s: &str) -> *mut c_void {
        let cls = objc_getClass(b"NSString\0".as_ptr());
        let cstr = std::ffi::CString::new(s).unwrap_or_default();
        type StrFn = unsafe extern "C" fn(*mut c_void, *mut c_void, *const i8) -> *mut c_void;
        let str_fn: StrFn = std::mem::transmute(objc_msgSend as *const ());
        str_fn(cls, sel_registerName(b"stringWithUTF8String:\0".as_ptr()), cstr.as_ptr())
    }

    /// Get bytes from NSData.
    unsafe fn nsdata_bytes(data: *mut c_void) -> Option<Vec<u8>> {
        if data.is_null() { return None; }
        type BytesFn = unsafe extern "C" fn(*mut c_void, *mut c_void) -> *const u8;
        type LenFn = unsafe extern "C" fn(*mut c_void, *mut c_void) -> usize;
        let bytes_fn: BytesFn = std::mem::transmute(objc_msgSend as *const ());
        let len_fn: LenFn = std::mem::transmute(objc_msgSend as *const ());
        let ptr = bytes_fn(data, sel_registerName(b"bytes\0".as_ptr()));
        let len = len_fn(data, sel_registerName(b"length\0".as_ptr()));
        if ptr.is_null() || len == 0 { return None; }
        Some(std::slice::from_raw_parts(ptr, len).to_vec())
    }

    const KBOOL_TRUE: *const c_void = 1 as *const c_void; // kCFBooleanTrue



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

    fn bio_dir() -> PathBuf {
        crate::settings_dir().join("biometric")
    }

    fn bio_path(wallet: &str) -> PathBuf {
        bio_dir().join(wallet)
    }

    /// Derive an encryption key from hardware UUID + salt using PBKDF2.
    /// 600,000 iterations makes brute-force impractical even if UUID is known.
    fn derive_key(salt: &[u8]) -> [u8; 32] {
        use ring::pbkdf2;
        use std::num::NonZeroU32;

        let uuid = std::process::Command::new("ioreg")
            .args(["-rd1", "-c", "IOPlatformExpertDevice"])
            .output()
            .ok()
            .and_then(|o| {
                let text = String::from_utf8_lossy(&o.stdout);
                text.lines()
                    .find(|l| l.contains("IOPlatformUUID"))
                    .and_then(|l| l.split('"').nth(3))
                    .map(|s| s.to_string())
            })
            .unwrap_or_default();

        let mut password = Vec::new();
        password.extend_from_slice(b"fistbump-bio-v3:");
        password.extend_from_slice(uuid.as_bytes());

        let mut key = [0u8; 32];
        pbkdf2::derive(
            pbkdf2::PBKDF2_HMAC_SHA256,
            NonZeroU32::new(600_000).unwrap(),
            salt,
            &password,
            &mut key,
        );
        key
    }

    /// Encrypt with AES-256-GCM. File format: [12-byte nonce | 16-byte salt | ciphertext+tag]
    pub fn save(wallet: &str, passphrase: &str) -> bool {
        use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, AES_256_GCM};
        use ring::rand::{SecureRandom, SystemRandom};

        let dir = bio_dir();
        let _ = std::fs::create_dir_all(&dir);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
        }

        let rng = SystemRandom::new();
        let mut nonce_bytes = [0u8; 12];
        let mut salt = [0u8; 16];
        if rng.fill(&mut nonce_bytes).is_err() { return false; }
        if rng.fill(&mut salt).is_err() { return false; }

        let key_bytes = derive_key(&salt);
        let key = match UnboundKey::new(&AES_256_GCM, &key_bytes) {
            Ok(k) => LessSafeKey::new(k),
            Err(_) => return false,
        };

        let nonce = match Nonce::try_assume_unique_for_key(&nonce_bytes) {
            Ok(n) => n,
            Err(_) => return false,
        };

        let mut data = passphrase.as_bytes().to_vec();
        if key.seal_in_place_append_tag(nonce, Aad::empty(), &mut data).is_err() {
            return false;
        }

        // File: nonce (12) + salt (16) + ciphertext+tag
        let mut file_data = Vec::with_capacity(12 + 16 + data.len());
        file_data.extend_from_slice(&nonce_bytes);
        file_data.extend_from_slice(&salt);
        file_data.extend_from_slice(&data);

        std::fs::write(bio_path(wallet), &file_data).is_ok()
    }

    pub fn load(wallet: &str) -> Option<String> {
        use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, AES_256_GCM};

        let path = bio_path(wallet);
        if !path.exists() { return None; }

        // Touch ID required before decryption
        if !evaluate_biometric("Unlock wallet") { return None; }

        let file_data = std::fs::read(&path).ok()?;
        if file_data.len() < 12 + 16 + 16 { return None; } // nonce + salt + min tag

        let nonce_bytes = &file_data[..12];
        let salt = &file_data[12..28];
        let mut ciphertext = file_data[28..].to_vec();

        let key_bytes = derive_key(salt);
        let key = LessSafeKey::new(UnboundKey::new(&AES_256_GCM, &key_bytes).ok()?);
        let nonce = Nonce::try_assume_unique_for_key(nonce_bytes).ok()?;

        let plaintext = key.open_in_place(nonce, Aad::empty(), &mut ciphertext).ok()?;
        String::from_utf8(plaintext.to_vec()).ok()
    }

    pub fn delete(wallet: &str) {
        let _ = std::fs::remove_file(bio_path(wallet));
    }

    /// Show Touch ID prompt via LAContext.evaluatePolicy and block until done.
    fn evaluate_biometric(reason: &str) -> bool {
        // Block layout for ^(BOOL success, NSError *error)
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

        unsafe extern "C" fn reply_invoke(block: *mut ReplyBlock, success: bool, _err: *mut c_void) {
            *(*block).success = success;
            dispatch_semaphore_signal((*block).semaphore);
        }

        unsafe {
            let cls = objc_getClass(b"LAContext\0".as_ptr());
            if cls.is_null() { return false; }

            type AllocFn = unsafe extern "C" fn(*mut c_void, *mut c_void) -> *mut c_void;
            type InitFn = unsafe extern "C" fn(*mut c_void, *mut c_void) -> *mut c_void;
            type EvalFn = unsafe extern "C" fn(*mut c_void, *mut c_void, isize, *mut c_void, *mut ReplyBlock);
            type ReleaseFn = unsafe extern "C" fn(*mut c_void, *mut c_void);

            let alloc: AllocFn = std::mem::transmute(objc_msgSend as *const ());
            let init: InitFn = std::mem::transmute(objc_msgSend as *const ());
            let eval: EvalFn = std::mem::transmute(objc_msgSend as *const ());
            let release: ReleaseFn = std::mem::transmute(objc_msgSend as *const ());

            let ctx = alloc(cls, sel_registerName(b"alloc\0".as_ptr()));
            let ctx = init(ctx, sel_registerName(b"init\0".as_ptr()));

            // Create NSString for reason
            let ns_cls = objc_getClass(b"NSString\0".as_ptr());
            type StrInitFn = unsafe extern "C" fn(*mut c_void, *mut c_void, *const u8, usize, u64) -> *mut c_void;
            let str_init: StrInitFn = std::mem::transmute(objc_msgSend as *const ());
            let reason_obj = alloc(ns_cls, sel_registerName(b"alloc\0".as_ptr()));
            let reason_obj = str_init(
                reason_obj,
                sel_registerName(b"initWithBytes:length:encoding:\0".as_ptr()),
                reason.as_ptr(), reason.len(), 4, // NSUTF8StringEncoding = 4
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

            eval(ctx,
                 sel_registerName(b"evaluatePolicy:localizedReason:reply:\0".as_ptr()),
                 1, // LAPolicyDeviceOwnerAuthenticationWithBiometrics
                 reason_obj as *mut c_void,
                 &mut block);

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

    pub fn browse(url: &str, header_height: i32, tab_bar_height: i32, dark: bool) {
        let vm = match get_vm() { Some(v) => v, None => return };
        let mut env = match vm.attach_current_thread() { Ok(e) => e, Err(_) => return };
        let cls = match env.find_class("org/fistbump/wallet/BrowserBridge") {
            Ok(c) => c, Err(_) => return,
        };
        let jurl = match env.new_string(url) { Ok(s) => s, Err(_) => return };
        let _ = env.call_static_method(
            cls, "browse", "(Ljava/lang/String;IIZ)V",
            &[JValue::Object(&jurl), JValue::Int(header_height),
              JValue::Int(tab_bar_height), JValue::Bool(dark as u8)],
        );
    }

    pub fn browse_error(header_height: i32, tab_bar_height: i32, dark: bool,
                        badge: &str, title: &str, message: &str) {
        let vm = match get_vm() { Some(v) => v, None => return };
        let mut env = match vm.attach_current_thread() { Ok(e) => e, Err(_) => return };
        let cls = match env.find_class("org/fistbump/wallet/BrowserBridge") {
            Ok(c) => c, Err(_) => return,
        };
        let jb = match env.new_string(badge) { Ok(s) => s, Err(_) => return };
        let jt = match env.new_string(title) { Ok(s) => s, Err(_) => return };
        let jm = match env.new_string(message) { Ok(s) => s, Err(_) => return };
        let _ = env.call_static_method(
            cls, "browseError",
            "(IIZLjava/lang/String;Ljava/lang/String;Ljava/lang/String;)V",
            &[JValue::Int(header_height), JValue::Int(tab_bar_height),
              JValue::Bool(dark as u8), JValue::Object(&jb), JValue::Object(&jt), JValue::Object(&jm)],
        );
    }

    pub fn hide() {
        let vm = match get_vm() { Some(v) => v, None => return };
        let mut env = match vm.attach_current_thread() { Ok(e) => e, Err(_) => return };
        let cls = match env.find_class("org/fistbump/wallet/BrowserBridge") {
            Ok(c) => c, Err(_) => return,
        };
        let _ = env.call_static_method(cls, "hide", "()V", &[]);
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
    { return Ok(biometric_macos::save(&wallet, &passphrase)); }
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
        .manage(AppState {
            settings: Mutex::new(settings),
            log_lines: Mutex::new(Vec::new()),
            fbd_pid: Mutex::new(None),
            is_quitting: AtomicBool::new(false),
        })
        .invoke_handler(tauri::generate_handler![
            rpc_call,
            get_log,
            get_api_key_cmd,
            get_wallet_build_hash,
            get_fbd_bundled_hash,
            open_external,
            browse,
            browse_error,
            browse_hide,
            scan_qr,
            scan_qr_stop,
            get_settings,
            set_miner_address,
            toggle_mining,
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
        ])
        .setup(|app| {
            #[cfg(any(desktop, target_os = "android"))]
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

                proxy::start_proxy(ca);
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
