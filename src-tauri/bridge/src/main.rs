//! fistbump-bridge — Chrome native messaging host for the Fistbump browser
//! extension. Compiled as a separate binary so it doesn't pull in the wallet
//! crate's heavy dependencies (Tauri, gtk, reqwest, etc.) and stays small +
//! fast to spawn.
//!
//! Lifecycle:
//!   1. Chrome calls `chrome.runtime.connectNative('org.fistbump.wallet')`.
//!   2. Chrome reads the host JSON manifest at
//!      `~/Library/Application Support/<browser>/NativeMessagingHosts/...`,
//!      verifies our extension ID against `allowed_origins`, then spawns
//!      this binary with stdio piped.
//!   3. We try to connect to `~/.fistbump/extension.sock`. If the wallet
//!      isn't running, the socket file isn't there — we `open` the bundled
//!      .app and poll until the socket appears.
//!   4. Then we proxy bytes back and forth: every native messaging frame
//!      from Chrome (4-byte little-endian length prefix + JSON body) is
//!      forwarded as-is to the wallet over the Unix socket, and every
//!      response frame from the wallet is written straight back to Chrome
//!      stdout. The wire formats are identical so we never have to parse.
//!   5. Loop until either side hangs up, then exit.
//!
//! Security model:
//!   - Chrome verifies the calling extension's ID before spawning us, so
//!     a random local extension can't impersonate the Fistbump extension.
//!   - The Unix socket lives at 0600 in the user's home, so other users
//!     on the same machine can't read it.
//!   - We don't authenticate the *socket* connection — same-user processes
//!     can in principle still connect to it directly, bypassing the bridge.
//!     That's the same threat-model trade-off the wallet makes for everything
//!     else in `~/.fistbump/`.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// Where the wallet binds its Unix socket. Mirrored on the wallet side.
fn socket_path() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    home.join(".fistbump").join("extension.sock")
}

/// Walk up from this binary's location to find the .app bundle root, so we
/// can `open` it and let macOS launch the wallet. We're at
/// `<App>/Contents/Resources/fistbump-bridge`, so `../..` is the bundle.
fn enclosing_app_bundle() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let resources = exe.parent()?;          // <App>/Contents/Resources
    let contents = resources.parent()?;     // <App>/Contents
    let app = contents.parent()?;           // <App>
    if app.extension().and_then(|e| e.to_str()) == Some("app") {
        Some(app.to_path_buf())
    } else {
        None
    }
}

/// Try to connect to the wallet's IPC socket. Returns immediately on success.
fn try_connect() -> Option<UnixStream> {
    UnixStream::connect(&socket_path()).ok()
}

/// Tell macOS to launch the wallet. Best-effort: any error means we just won't
/// have a socket to talk to and the next `try_connect` will keep failing.
fn launch_wallet() {
    let Some(app) = enclosing_app_bundle() else {
        return;
    };
    let _ = std::process::Command::new("open").arg(&app).status();
}

/// Poll until either we connect to the socket or `timeout` elapses.
fn wait_for_socket(timeout: Duration) -> Option<UnixStream> {
    let interval = Duration::from_millis(100);
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if let Some(s) = try_connect() {
            return Some(s);
        }
        std::thread::sleep(interval);
    }
    None
}

/// Write an `{"error": "..."}` response in native messaging frame format.
/// Used when we have to bail before we can establish a real socket connection.
fn write_error(msg: &str) {
    let body = format!("{{\"error\":{}}}", json_escape(msg));
    let len = body.len() as u32;
    let mut stdout = std::io::stdout();
    let _ = stdout.write_all(&len.to_le_bytes());
    let _ = stdout.write_all(body.as_bytes());
    let _ = stdout.flush();
}

/// Minimal JSON string escape. We avoid pulling in serde_json just for one
/// error message — keep the binary lean.
fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn main() {
    // Try to connect to the wallet first *without* launching it. We need
    // to know whether the wallet is already running before deciding what
    // to do with the first incoming message.
    let mut maybe_socket = try_connect();

    // Read the first native messaging frame from Chrome. We buffer it so
    // we can inspect its type before deciding whether to launch the wallet,
    // then forward the same bytes along if we proceed.
    let mut stdin = std::io::stdin();
    let mut stdout = std::io::stdout();

    let mut len_buf = [0u8; 4];
    if stdin.read_exact(&mut len_buf).is_err() {
        return;
    }
    let len = u32::from_le_bytes(len_buf) as usize;
    if len == 0 || len > 1024 * 1024 {
        return;
    }
    let mut first_msg = vec![0u8; len];
    if stdin.read_exact(&mut first_msg).is_err() {
        return;
    }

    // Passive probes (type=info) should never launch the wallet — clicking
    // the extension's toolbar icon would be a surprising way to boot the
    // desktop app. If the wallet isn't already up, answer the probe
    // ourselves with a synthetic "not running" response and exit.
    let first_is_info = bytes_contains(&first_msg, b"\"type\":\"info\"")
        || bytes_contains(&first_msg, b"\"type\": \"info\"");

    if maybe_socket.is_none() && first_is_info {
        let body = br#"{"ok":true,"running":false,"connected":false}"#;
        let body_len = (body.len() as u32).to_le_bytes();
        let _ = stdout.write_all(&body_len);
        let _ = stdout.write_all(body);
        let _ = stdout.flush();
        return;
    }

    // For any other request, launch the wallet if needed and connect.
    if maybe_socket.is_none() {
        launch_wallet();
        maybe_socket = wait_for_socket(Duration::from_secs(10));
    }

    let mut socket = match maybe_socket {
        Some(s) => s,
        None => {
            write_error("could not start the Fistbump wallet");
            return;
        }
    };

    // Best-effort: don't block forever if either side stalls. Each native
    // messaging exchange is small and quick, but the wallet may take a few
    // seconds to pop a modal — give it room.
    let _ = socket.set_read_timeout(Some(Duration::from_secs(120)));
    let _ = socket.set_write_timeout(Some(Duration::from_secs(10)));

    // Forward the buffered first frame to the wallet and pipe the response
    // back to Chrome, then enter the general relay loop for any follow-ups.
    if socket.write_all(&len_buf).is_err() { return; }
    if socket.write_all(&first_msg).is_err() { return; }
    if socket.flush().is_err() { return; }

    let mut resp_len_buf = [0u8; 4];
    if socket.read_exact(&mut resp_len_buf).is_err() { return; }
    let resp_len = u32::from_le_bytes(resp_len_buf) as usize;
    if resp_len > 1024 * 1024 { return; }
    let mut resp = vec![0u8; resp_len];
    if socket.read_exact(&mut resp).is_err() { return; }
    if stdout.write_all(&resp_len_buf).is_err() { return; }
    if stdout.write_all(&resp).is_err() { return; }
    let _ = stdout.flush();

    if let Err(e) = relay_loop(&mut socket) {
        // Don't panic — Chrome would surface a generic disconnect. We've
        // already written whatever we managed to before the error.
        let _ = e;
    }
}

/// Find `needle` as a substring of `haystack`. Used to detect whether the
/// first native messaging frame is a `"type":"info"` request without
/// pulling in a JSON parser for one fast-path check.
fn bytes_contains(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() || needle.len() > haystack.len() {
        return false;
    }
    let last = haystack.len() - needle.len();
    for i in 0..=last {
        if &haystack[i..i + needle.len()] == needle {
            return true;
        }
    }
    false
}

/// Forward native messaging frames between Chrome stdio and the wallet socket
/// in both directions. Returns when either side closes or we hit an I/O error.
fn relay_loop(socket: &mut UnixStream) -> std::io::Result<()> {
    let mut stdin = std::io::stdin();
    let mut stdout = std::io::stdout();

    loop {
        // ─── Read one frame from Chrome ───
        let mut len_buf = [0u8; 4];
        if let Err(e) = stdin.read_exact(&mut len_buf) {
            // Clean EOF when Chrome disconnects the port.
            if e.kind() == std::io::ErrorKind::UnexpectedEof {
                return Ok(());
            }
            return Err(e);
        }
        let len = u32::from_le_bytes(len_buf) as usize;
        if len == 0 || len > 1024 * 1024 {
            return Ok(());
        }
        let mut msg = vec![0u8; len];
        stdin.read_exact(&mut msg)?;

        // ─── Forward to wallet ───
        socket.write_all(&len_buf)?;
        socket.write_all(&msg)?;
        socket.flush()?;

        // ─── Read one frame back from wallet ───
        let mut resp_len_buf = [0u8; 4];
        socket.read_exact(&mut resp_len_buf)?;
        let resp_len = u32::from_le_bytes(resp_len_buf) as usize;
        if resp_len > 1024 * 1024 {
            return Ok(());
        }
        let mut resp = vec![0u8; resp_len];
        socket.read_exact(&mut resp)?;

        // ─── Write to Chrome ───
        stdout.write_all(&resp_len_buf)?;
        stdout.write_all(&resp)?;
        stdout.flush()?;
    }
}
