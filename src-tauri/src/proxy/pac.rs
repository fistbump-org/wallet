//! PAC script serving and platform-specific system proxy / CA setup.

use std::path::Path;
use std::process::Command;

pub const PROXY_PORT: u16 = 17350;
pub const SOCKS_PORT: u16 = 17351;

/// PAC script — routes all traffic through the proxy so it can decide ICANN vs Fistbump.
/// Exceptions: localhost, IP addresses.
pub fn pac_script() -> String {
    format!(
        r#"function FindProxyForURL(url, host) {{
    if (host === 'localhost' || host === '127.0.0.1' || host === '::1') {{
        return 'DIRECT';
    }}
    if (/^\d+\.\d+\.\d+\.\d+$/.test(host)) {{
        return 'DIRECT';
    }}
    return 'PROXY 127.0.0.1:{}; DIRECT';
}}"#,
        PROXY_PORT
    )
}

// ── macOS ──

#[cfg(target_os = "macos")]
fn network_services() -> Vec<String> {
    let output = Command::new("networksetup")
        .arg("-listallnetworkservices")
        .output();
    match output {
        Ok(o) => {
            let text = String::from_utf8_lossy(&o.stdout);
            text.lines()
                .skip(1)
                .map(|s| s.trim_start_matches('*').trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        }
        Err(_) => vec![],
    }
}

#[cfg(target_os = "macos")]
pub fn is_pac_installed() -> bool {
    let expected = format!("http://127.0.0.1:{}/.fistbump/proxy.pac", PROXY_PORT);
    for service in network_services() {
        if let Ok(o) = Command::new("networksetup")
            .args(["-getautoproxyurl", &service])
            .output()
        {
            let text = String::from_utf8_lossy(&o.stdout);
            if text.contains(&expected) && text.contains("Enabled: Yes") {
                return true;
            }
        }
    }
    false
}

#[cfg(target_os = "macos")]
pub fn install_pac() {
    let pac_url = format!("http://127.0.0.1:{}/.fistbump/proxy.pac", PROXY_PORT);
    for service in network_services() {
        let _ = Command::new("networksetup")
            .args(["-setautoproxyurl", &service, &pac_url])
            .output();
        let _ = Command::new("networksetup")
            .args(["-setautoproxystate", &service, "on"])
            .output();
    }
    println!("[fistbump] installed PAC proxy for all network services");
}

#[cfg(target_os = "macos")]
pub fn remove_pac() {
    for service in network_services() {
        let _ = Command::new("networksetup")
            .args(["-setautoproxystate", &service, "off"])
            .output();
    }
    println!("[fistbump] removed PAC proxy settings");
}

#[cfg(target_os = "macos")]
pub fn is_ca_installed(cert_path: &Path) -> bool {
    let output = Command::new("security")
        .args(["verify-cert", "-c"])
        .arg(cert_path)
        .output();
    match output {
        Ok(o) => o.status.success(),
        Err(_) => false,
    }
}

#[cfg(target_os = "macos")]
pub fn install_ca_cert(cert_path: &Path) {
    let home = dirs::home_dir().unwrap_or_default();
    let keychain = home.join("Library/Keychains/login.keychain-db");

    let output = Command::new("security")
        .args(["add-trusted-cert", "-r", "trustRoot", "-k"])
        .arg(&keychain)
        .arg(cert_path)
        .output();

    match output {
        Ok(o) if o.status.success() => {
            println!("[fistbump] installed root CA to login keychain");
        }
        Ok(o) => {
            let err = String::from_utf8_lossy(&o.stderr);
            if err.contains("already exists") {
                println!("[fistbump] root CA already in keychain");
            } else {
                println!("[fistbump] failed to install CA: {}", err);
            }
        }
        Err(e) => {
            println!("[fistbump] failed to run security command: {}", e);
        }
    }
}

// ── Windows ──

#[cfg(target_os = "windows")]
const INET_SETTINGS_KEY: &str =
    r"HKCU\Software\Microsoft\Windows\CurrentVersion\Internet Settings";

#[cfg(target_os = "windows")]
pub fn is_pac_installed() -> bool {
    let expected = format!("http://127.0.0.1:{}/.fistbump/proxy.pac", PROXY_PORT);
    let output = Command::new("reg")
        .args(["query", INET_SETTINGS_KEY, "/v", "AutoConfigURL"])
        .output();
    match output {
        Ok(o) => {
            let text = String::from_utf8_lossy(&o.stdout);
            text.contains(&expected)
        }
        Err(_) => false,
    }
}

#[cfg(target_os = "windows")]
pub fn install_pac() {
    let pac_url = format!("http://127.0.0.1:{}/.fistbump/proxy.pac", PROXY_PORT);
    let _ = Command::new("reg")
        .args([
            "add",
            INET_SETTINGS_KEY,
            "/v",
            "AutoConfigURL",
            "/t",
            "REG_SZ",
            "/d",
            &pac_url,
            "/f",
        ])
        .output();
    // Signal WinINET to pick up the change
    notify_inet_change();
    println!("[fistbump] installed PAC proxy via registry");
}

#[cfg(target_os = "windows")]
pub fn remove_pac() {
    let _ = Command::new("reg")
        .args([
            "delete",
            INET_SETTINGS_KEY,
            "/v",
            "AutoConfigURL",
            "/f",
        ])
        .output();
    notify_inet_change();
    println!("[fistbump] removed PAC proxy from registry");
}

/// Nudge WinINET to re-read proxy settings via a small PowerShell snippet.
#[cfg(target_os = "windows")]
fn notify_inet_change() {
    let ps = r#"
Add-Type -TypeDefinition @"
using System.Runtime.InteropServices;
public class WinInet {
    [DllImport("wininet.dll", SetLastError=true)]
    public static extern bool InternetSetOption(System.IntPtr h, int o, System.IntPtr b, int l);
}
"@
[WinInet]::InternetSetOption([System.IntPtr]::Zero, 39, [System.IntPtr]::Zero, 0) # INTERNET_OPTION_SETTINGS_CHANGED
[WinInet]::InternetSetOption([System.IntPtr]::Zero, 37, [System.IntPtr]::Zero, 0) # INTERNET_OPTION_REFRESH
"#;
    let _ = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", ps])
        .output();
}

#[cfg(target_os = "windows")]
pub fn is_ca_installed(cert_path: &Path) -> bool {
    // Search the user root store for our CA by subject name
    let output = Command::new("certutil")
        .args(["-store", "-user", "Root", "Fistbump Local CA"])
        .output();
    match output {
        Ok(o) => {
            // certutil exits 0 and prints cert info if found
            if o.status.success() {
                let text = String::from_utf8_lossy(&o.stdout);
                text.contains("Fistbump Local CA")
            } else {
                false
            }
        }
        Err(_) => false,
    }
}

#[cfg(target_os = "windows")]
pub fn install_ca_cert(cert_path: &Path) {
    // -user: current user store (no admin needed, shows security dialog)
    let output = Command::new("certutil")
        .args(["-addstore", "-user", "Root"])
        .arg(cert_path)
        .output();

    match output {
        Ok(o) if o.status.success() => {
            println!("[fistbump] installed root CA to user certificate store");
        }
        Ok(o) => {
            let err = String::from_utf8_lossy(&o.stderr);
            let out = String::from_utf8_lossy(&o.stdout);
            if out.contains("already in store") || err.contains("already in store") {
                println!("[fistbump] root CA already in certificate store");
            } else {
                println!("[fistbump] failed to install CA: {} {}", out.trim(), err.trim());
            }
        }
        Err(e) => {
            println!("[fistbump] failed to run certutil: {}", e);
        }
    }
}

// ── Linux ──

#[cfg(target_os = "linux")]
fn has_command(cmd: &str) -> bool {
    Command::new("which")
        .arg(cmd)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Detect the desktop environment.
#[cfg(target_os = "linux")]
fn desktop_env() -> Option<String> {
    std::env::var("XDG_CURRENT_DESKTOP")
        .or_else(|_| std::env::var("DESKTOP_SESSION"))
        .ok()
        .map(|s| s.to_uppercase())
}

/// Install libnss3-tools (or equivalent) if certutil is missing.
/// Uses pkexec for a graphical password prompt.
#[cfg(target_os = "linux")]
fn ensure_certutil() -> bool {
    if has_command("certutil") {
        return true;
    }
    println!("[fistbump] certutil not found, installing...");

    let result = if has_command("apt-get") {
        Command::new("pkexec")
            .args(["apt-get", "install", "-y", "libnss3-tools"])
            .output()
    } else if has_command("dnf") {
        Command::new("pkexec")
            .args(["dnf", "install", "-y", "nss-tools"])
            .output()
    } else if has_command("pacman") {
        Command::new("pkexec")
            .args(["pacman", "-S", "--noconfirm", "nss"])
            .output()
    } else {
        println!("[fistbump] no supported package manager found");
        return false;
    };

    match result {
        Ok(o) if o.status.success() => {
            println!("[fistbump] certutil installed successfully");
            true
        }
        Ok(o) => {
            let err = String::from_utf8_lossy(&o.stderr);
            println!("[fistbump] failed to install certutil: {}", err.trim());
            false
        }
        Err(e) => {
            println!("[fistbump] pkexec error: {}", e);
            false
        }
    }
}

#[cfg(target_os = "linux")]
pub fn is_pac_installed() -> bool {
    let expected = format!("http://127.0.0.1:{}/.fistbump/proxy.pac", PROXY_PORT);
    // Try GNOME/GTK
    if let Ok(o) = Command::new("gsettings")
        .args(["get", "org.gnome.system.proxy", "mode"])
        .output()
    {
        let mode = String::from_utf8_lossy(&o.stdout).trim().to_string();
        if mode.contains("auto") {
            if let Ok(o2) = Command::new("gsettings")
                .args(["get", "org.gnome.system.proxy", "autoconfig-url"])
                .output()
            {
                let url = String::from_utf8_lossy(&o2.stdout).trim().to_string();
                if url.contains(&expected) {
                    return true;
                }
            }
        }
    }
    // Try KDE
    if let Ok(o) = Command::new("kreadconfig5")
        .args([
            "--group",
            "Proxy Settings",
            "--key",
            "ProxyType",
            "--file",
            "kioslaverc",
        ])
        .output()
    {
        let ptype = String::from_utf8_lossy(&o.stdout).trim().to_string();
        if ptype == "2" {
            // 2 = PAC
            if let Ok(o2) = Command::new("kreadconfig5")
                .args([
                    "--group",
                    "Proxy Settings",
                    "--key",
                    "Proxy Config Script",
                    "--file",
                    "kioslaverc",
                ])
                .output()
            {
                let url = String::from_utf8_lossy(&o2.stdout).trim().to_string();
                if url.contains(&expected) {
                    return true;
                }
            }
        }
    }
    false
}

#[cfg(target_os = "linux")]
pub fn install_pac() {
    let pac_url = format!("http://127.0.0.1:{}/.fistbump/proxy.pac", PROXY_PORT);
    let de = desktop_env().unwrap_or_default();

    if de.contains("GNOME") || de.contains("UNITY") || de.contains("CINNAMON") || de.contains("MATE") || de.contains("BUDGIE") {
        let _ = Command::new("gsettings")
            .args(["set", "org.gnome.system.proxy", "mode", "auto"])
            .output();
        let _ = Command::new("gsettings")
            .args([
                "set",
                "org.gnome.system.proxy",
                "autoconfig-url",
                &pac_url,
            ])
            .output();
        println!("[fistbump] installed PAC proxy via gsettings (GNOME)");
    } else if de.contains("KDE") || de.contains("PLASMA") {
        let _ = Command::new("kwriteconfig5")
            .args([
                "--group",
                "Proxy Settings",
                "--key",
                "ProxyType",
                "2",
                "--file",
                "kioslaverc",
            ])
            .output();
        let _ = Command::new("kwriteconfig5")
            .args([
                "--group",
                "Proxy Settings",
                "--key",
                "Proxy Config Script",
                &pac_url,
                "--file",
                "kioslaverc",
            ])
            .output();
        println!("[fistbump] installed PAC proxy via kwriteconfig5 (KDE)");
    } else {
        // Fallback: try gsettings anyway (Chromium respects it even on non-GNOME)
        let _ = Command::new("gsettings")
            .args(["set", "org.gnome.system.proxy", "mode", "auto"])
            .output();
        let _ = Command::new("gsettings")
            .args([
                "set",
                "org.gnome.system.proxy",
                "autoconfig-url",
                &pac_url,
            ])
            .output();
        println!("[fistbump] installed PAC proxy via gsettings (fallback)");
    }
}

#[cfg(target_os = "linux")]
pub fn remove_pac() {
    // Reset GNOME
    let _ = Command::new("gsettings")
        .args(["set", "org.gnome.system.proxy", "mode", "none"])
        .output();
    let _ = Command::new("gsettings")
        .args(["set", "org.gnome.system.proxy", "autoconfig-url", "''"])
        .output();
    // Reset KDE
    let _ = Command::new("kwriteconfig5")
        .args([
            "--group",
            "Proxy Settings",
            "--key",
            "ProxyType",
            "0",
            "--file",
            "kioslaverc",
        ])
        .output();
    println!("[fistbump] removed PAC proxy settings");
}

/// Collect every NSS database on the system that a browser might consult:
/// - Chromium-family shared store at `~/.pki/nssdb`
/// - Per-snap Chromium-family stores under `~/snap/<name>/current/.pki/nssdb`
/// - Per-profile Firefox cert stores (regular + snap)
#[cfg(target_os = "linux")]
fn nss_db_paths() -> Vec<std::path::PathBuf> {
    let home = dirs::home_dir().unwrap_or_default();
    let mut paths: Vec<std::path::PathBuf> = Vec::new();

    // Chromium-family: shared NSS store used by non-sandboxed Chrome/Chromium.
    paths.push(home.join(".pki/nssdb"));

    // Snap-confined browsers each see their own $HOME at ~/snap/<name>/current.
    let snap_dir = home.join("snap");
    if snap_dir.is_dir() {
        for name in &["chromium", "google-chrome", "brave", "opera", "vivaldi"] {
            let snap_root = snap_dir.join(name);
            if !snap_root.exists() {
                continue;
            }
            paths.push(snap_root.join("current/.pki/nssdb"));
        }
    }

    // Firefox: per-profile NSS DBs from profiles.ini. Handles both regular and snap Firefox.
    paths.extend(firefox_profile_nssdbs(&home.join(".mozilla/firefox")));
    paths.extend(firefox_profile_nssdbs(
        &home.join("snap/firefox/common/.mozilla/firefox"),
    ));

    paths
}

/// Parse `<firefox_root>/profiles.ini` and return each profile directory that contains
/// (or will contain) an NSS database. Empty if profiles.ini is missing.
#[cfg(target_os = "linux")]
fn firefox_profile_nssdbs(firefox_root: &Path) -> Vec<std::path::PathBuf> {
    let profiles_ini = firefox_root.join("profiles.ini");
    let Ok(content) = std::fs::read_to_string(&profiles_ini) else {
        return Vec::new();
    };

    let mut out = Vec::new();
    let mut in_profile = false;
    let mut path: Option<String> = None;
    let mut is_relative = true;

    let flush = |in_profile: bool,
                 path: &mut Option<String>,
                 is_relative: &mut bool,
                 out: &mut Vec<std::path::PathBuf>| {
        if in_profile {
            if let Some(p) = path.take() {
                let full = if *is_relative {
                    firefox_root.join(&p)
                } else {
                    std::path::PathBuf::from(&p)
                };
                if full.is_dir() {
                    out.push(full);
                }
            }
        }
        *is_relative = true;
    };

    for line in content.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            flush(in_profile, &mut path, &mut is_relative, &mut out);
            in_profile = line.to_ascii_lowercase().starts_with("[profile");
            continue;
        }
        if !in_profile {
            continue;
        }
        if let Some(rest) = line.strip_prefix("Path=") {
            path = Some(rest.to_string());
        } else if let Some(rest) = line.strip_prefix("IsRelative=") {
            is_relative = rest.trim() == "1";
        }
    }
    flush(in_profile, &mut path, &mut is_relative, &mut out);

    out
}

/// Install cert into a single NSS database. Returns true on success.
#[cfg(target_os = "linux")]
fn install_to_nssdb(cert_path: &Path, nssdb_dir: &Path) -> bool {
    let nssdb = format!("sql:{}", nssdb_dir.display());

    let _ = std::fs::create_dir_all(nssdb_dir);

    // Create NSS db if it doesn't exist. Firefox profile dirs already have cert9.db;
    // chromium-family dirs (~/.pki/nssdb, snap) may not.
    if !nssdb_dir.join("cert9.db").exists() {
        let out = Command::new("certutil")
            .args(["-N", "-d", &nssdb, "--empty-password"])
            .output();
        if let Ok(o) = out {
            if !o.status.success() {
                let err = String::from_utf8_lossy(&o.stderr);
                println!(
                    "[fistbump] failed to create NSS db at {}: {}",
                    nssdb_dir.display(),
                    err.trim()
                );
                return false;
            }
        }
    }

    // Remove existing entry if re-installing (ignore errors — cert may not exist yet).
    let _ = Command::new("certutil")
        .args(["-D", "-d", &nssdb, "-n", "Fistbump Local CA"])
        .output();

    // Add cert. `C,,` = trusted root CA for SSL server auth (matches mkcert).
    let output = Command::new("certutil")
        .args([
            "-A",
            "-d",
            &nssdb,
            "-t",
            "C,,",
            "-n",
            "Fistbump Local CA",
            "-i",
        ])
        .arg(cert_path)
        .output();

    match output {
        Ok(o) if o.status.success() => {
            println!("[fistbump] installed CA to NSS: {}", nssdb_dir.display());
            true
        }
        Ok(o) => {
            let err = String::from_utf8_lossy(&o.stderr);
            println!(
                "[fistbump] NSS install failed ({}): {}",
                nssdb_dir.display(),
                err.trim()
            );
            false
        }
        Err(e) => {
            println!("[fistbump] certutil error ({}): {}", nssdb_dir.display(), e);
            false
        }
    }
}

/// Check if cert exists in a single NSS database.
#[cfg(target_os = "linux")]
fn check_nssdb(nssdb_dir: &Path) -> bool {
    let nssdb = format!("sql:{}", nssdb_dir.display());
    Command::new("certutil")
        .args(["-L", "-d", &nssdb, "-n", "Fistbump Local CA"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[cfg(target_os = "linux")]
pub fn is_ca_installed(cert_path: &Path) -> bool {
    let _ = cert_path;
    // Check all NSS databases (standard + snap)
    for db in nss_db_paths() {
        if db.join("cert9.db").exists() && check_nssdb(&db) {
            return true;
        }
    }
    // Also check system store
    Path::new("/usr/local/share/ca-certificates/fistbump-local-ca.crt").exists()
}

#[cfg(target_os = "linux")]
pub fn install_ca_cert(cert_path: &Path) {
    // Ensure certutil is available — auto-install libnss3-tools if needed.
    if !ensure_certutil() {
        println!("[fistbump] cannot install CA without certutil — skipping NSS install");
        install_ca_system(cert_path);
        return;
    }

    // Install to every NSS database we can find: chromium-family + Firefox per-profile.
    let dbs = nss_db_paths();
    let mut successes = 0usize;
    let total = dbs.len();
    for db in &dbs {
        if install_to_nssdb(cert_path, db) {
            successes += 1;
        }
    }

    if successes == 0 {
        println!(
            "[fistbump] WARNING: CA was not installed into any NSS database. \
             Browsers will show SSL errors for fistbump names. \
             Checked {} path(s).",
            total
        );
    } else {
        println!(
            "[fistbump] installed CA into {}/{} NSS database(s). \
             Restart any open browsers for the trust to take effect.",
            successes, total
        );
    }

    // Also install system-wide (covers curl, wget, Electron apps, etc.).
    install_ca_system(cert_path);
}

/// Install CA cert system-wide via pkexec (covers OpenSSL-based apps, curl, etc.).
/// Always runs `update-ca-certificates` so an already-copied cert still gets registered.
#[cfg(target_os = "linux")]
fn install_ca_system(cert_path: &Path) {
    let sys_dest = Path::new("/usr/local/share/ca-certificates/fistbump-local-ca.crt");

    // If the destination already has identical contents, we still need to make sure
    // update-ca-certificates has been run at least once.
    let already_matches = match (std::fs::read(cert_path), std::fs::read(sys_dest)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    };

    let script = if already_matches {
        "update-ca-certificates".to_string()
    } else {
        format!(
            "cp '{}' '{}' && update-ca-certificates",
            cert_path.display(),
            sys_dest.display()
        )
    };

    let output = Command::new("pkexec").args(["sh", "-c", &script]).output();
    match output {
        Ok(o) if o.status.success() => {
            println!("[fistbump] installed root CA to system certificate store");
        }
        Ok(o) => {
            let err = String::from_utf8_lossy(&o.stderr);
            // pkexec exits 126/127 if the user cancels the prompt — don't flood the log.
            if o.status.code() == Some(126) || o.status.code() == Some(127) {
                println!("[fistbump] system CA install cancelled by user");
            } else {
                println!("[fistbump] system CA install failed: {}", err.trim());
            }
        }
        Err(e) => {
            println!("[fistbump] pkexec error for system CA: {}", e);
        }
    }
}
