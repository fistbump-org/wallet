//! TCP proxy listener — handles HTTP forward proxy, HTTPS CONNECT, and PAC serving.

/// Log via FFI on iOS (where println from spawned threads is lost), println elsewhere.
macro_rules! plog {
    ($($arg:tt)*) => {{
        let msg = format!($($arg)*);
        println!("{}", msg);
        #[cfg(target_os = "ios")]
        {
            extern "C" { fn push_log_line(ptr: *const std::ffi::c_char); }
            if let Ok(c) = std::ffi::CString::new(msg) {
                unsafe { push_log_line(c.as_ptr()); }
            }
        }
    }};
}

use super::ca::SharedCA;
use super::dane;
use super::dns;
use super::icann;
use super::pac;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName};
use rustls::ClientConfig;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream, ToSocketAddrs};
use std::sync::{Arc, Mutex};
use std::time::Instant;

// ── Caches ──

struct CachedCert {
    chain_der: Vec<Vec<u8>>,
    key_der: Vec<u8>,
    created: Instant,
}

struct CachedResolve {
    result: dns::ResolvedName,
    created: Instant,
}

const CERT_CACHE_TTL_SECS: u64 = 3600; // 1 hour
const DNS_CACHE_TTL_SECS: u64 = 300;   // 5 minutes

static CERT_CACHE: std::sync::LazyLock<Mutex<HashMap<String, CachedCert>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

static DNS_CACHE: std::sync::LazyLock<Mutex<HashMap<String, CachedResolve>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

/// Hosts the user has chosen to proceed to despite missing/invalid DANE.
static DANE_BYPASS: std::sync::LazyLock<Mutex<std::collections::HashSet<String>>> =
    std::sync::LazyLock::new(|| Mutex::new(std::collections::HashSet::new()));

fn cached_resolve(name: &str) -> Option<dns::ResolvedName> {
    {
        let cache = DNS_CACHE.lock().unwrap();
        if let Some(cached) = cache.get(name) {
            if cached.created.elapsed().as_secs() < DNS_CACHE_TTL_SECS {
                return Some(cached.result.clone());
            }
        }
    }
    let result = dns::resolve(name)?;
    let mut cache = DNS_CACHE.lock().unwrap();
    cache.insert(name.to_string(), CachedResolve {
        result: result.clone(),
        created: Instant::now(),
    });
    Some(result)
}

fn cached_mint(
    ca: &SharedCA,
    hostname: &str,
) -> Result<(Vec<CertificateDer<'static>>, PrivateKeyDer<'static>), BoxError> {
    {
        let cache = CERT_CACHE.lock().unwrap();
        if let Some(cached) = cache.get(hostname) {
            if cached.created.elapsed().as_secs() < CERT_CACHE_TTL_SECS {
                let chain = cached.chain_der.iter()
                    .map(|d| CertificateDer::from(d.clone()))
                    .collect();
                let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(cached.key_der.clone()));
                return Ok((chain, key));
            }
        }
    }
    let (chain, key) = ca.mint_cert(hostname).map_err(|e| format!("{}", e))?;
    let chain_der: Vec<Vec<u8>> = chain.iter().map(|c| c.as_ref().to_vec()).collect();
    let key_der = match &key {
        PrivateKeyDer::Pkcs8(k) => k.secret_pkcs8_der().to_vec(),
        _ => vec![],
    };
    let mut cache = CERT_CACHE.lock().unwrap();
    cache.insert(hostname.to_string(), CachedCert {
        chain_der,
        key_der,
        created: Instant::now(),
    });
    Ok((chain, key))
}

/// Raise the file-descriptor soft limit to the hard limit.
///
/// macOS defaults to a soft limit of 256 FDs, which is easily exhausted by
/// the proxy (each CONNECT tunnel holds ~6 FDs). Without this, heavy
/// browsing causes `accept()` to fail with EMFILE, the listener thread
/// exits, and the PAC is left pointing at a dead port — blocking all traffic.
#[cfg(unix)]
fn raise_fd_limit() {
    use libc::{getrlimit, setrlimit, rlimit, RLIMIT_NOFILE};
    unsafe {
        let mut lim = rlimit { rlim_cur: 0, rlim_max: 0 };
        if getrlimit(RLIMIT_NOFILE, &mut lim) == 0 && lim.rlim_cur < lim.rlim_max {
            let old = lim.rlim_cur;
            lim.rlim_cur = lim.rlim_max;
            if setrlimit(RLIMIT_NOFILE, &lim) == 0 {
                plog!("[fistbump] raised fd limit {} → {}", old, lim.rlim_max);
            }
        }
    }
}

/// Start the proxy listeners (HTTP + SOCKS5) in background threads.
pub fn start_proxy(ca: SharedCA, _app: Option<tauri::AppHandle>) {
    #[cfg(unix)]
    raise_fd_limit();

    // Ensure rustls has a crypto provider installed (required on iOS where
    // feature auto-detection doesn't work).
    let _ = rustls::crypto::ring::default_provider().install_default();

    // Start SOCKS5 proxy (used by iOS WKWebView for both HTTP and HTTPS)
    let socks_ca = ca.clone();
    std::thread::spawn(move || {
        let addr = format!("127.0.0.1:{}", pac::SOCKS_PORT);
        let mut listener = match TcpListener::bind(&addr) {
            Ok(l) => l,
            Err(e) => {
                plog!("[fistbump] socks5 failed to bind {}: {}", addr, e);
                return;
            }
        };
        plog!("[fistbump] socks5 listening on {}", addr);

        loop {
            for stream in listener.incoming() {
                match stream {
                    Ok(s) => {
                        let ca = socks_ca.clone();
                        std::thread::spawn(move || {
                            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                handle_socks5(s, &ca)
                            }));
                            match result {
                                Ok(Err(e)) => {
                                    let msg = e.to_string();
                                    if !msg.contains("connection reset")
                                        && !msg.contains("broken pipe")
                                        && !msg.contains("eof")
                                    {
                                        plog!("[fistbump] socks5 error: {}", msg);
                                    }
                                }
                                Err(panic) => {
                                    let msg = panic.downcast_ref::<String>()
                                        .map(|s| s.as_str())
                                        .or_else(|| panic.downcast_ref::<&str>().copied())
                                        .unwrap_or("unknown");
                                    plog!("[fistbump] socks5 PANIC: {}", msg);
                                }
                                Ok(Ok(())) => {}
                            }
                        });
                    }
                    Err(e) => {
                        plog!("[fistbump] socks5 accept error: {}, rebinding...", e);
                        break;
                    }
                }
            }
            // Listener socket is dead (iOS background suspend), rebind
            std::thread::sleep(std::time::Duration::from_secs(1));
            match TcpListener::bind(&addr) {
                Ok(l) => { listener = l; plog!("[fistbump] socks5 rebound on {}", addr); }
                Err(e) => { plog!("[fistbump] socks5 rebind failed: {}", e); return; }
            }
        }
    });

    // Start HTTP proxy (used by desktop PAC and Android ProxyController)
    let app_for_proxy = _app.clone();
    std::thread::spawn(move || {
        let addr = format!("127.0.0.1:{}", pac::PROXY_PORT);
        let mut listener = match TcpListener::bind(&addr) {
            Ok(l) => l,
            Err(e) => {
                plog!("[fistbump] proxy failed to bind {}: {}", addr, e);
                return;
            }
        };
        plog!("[fistbump] proxy listening on {}", addr);

        loop {
            for stream in listener.incoming() {
                match stream {
                    Ok(s) => {
                        let ca = ca.clone();
                        let app = app_for_proxy.clone();
                        std::thread::spawn(move || {
                            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                handle_connection(s, &ca, app.as_ref())
                            }));
                            match result {
                                Ok(Err(e)) => {
                                    let msg = e.to_string();
                                    if !msg.contains("connection reset")
                                        && !msg.contains("broken pipe")
                                        && !msg.contains("eof")
                                    {
                                        plog!("[fistbump] proxy error: {}", msg);
                                    }
                                }
                                Err(panic) => {
                                    let msg = panic.downcast_ref::<String>()
                                        .map(|s| s.as_str())
                                        .or_else(|| panic.downcast_ref::<&str>().copied())
                                        .unwrap_or("unknown");
                                    plog!("[fistbump] proxy PANIC: {}", msg);
                                }
                                Ok(Ok(())) => {}
                            }
                        });
                    }
                    Err(e) => {
                        plog!("[fistbump] proxy accept error: {}, rebinding...", e);
                        break;
                    }
                }
            }
            // Listener socket is dead (iOS background suspend), rebind
            std::thread::sleep(std::time::Duration::from_secs(1));
            match TcpListener::bind(&addr) {
                Ok(l) => { listener = l; plog!("[fistbump] proxy rebound on {}", addr); }
                Err(e) => { plog!("[fistbump] proxy rebind failed: {}", e); return; }
            }
        }
    });
}

type BoxError = Box<dyn std::error::Error + Send + Sync>;

fn handle_connection(stream: TcpStream, ca: &SharedCA, app: Option<&tauri::AppHandle>) -> Result<(), BoxError> {
    // Short timeout for reading the initial request line + headers.
    // Tunnel handlers override with longer timeouts for the relay phase.
    // The extension /connect endpoint blocks waiting for user approval, so
    // it sets its own much longer timeouts before calling recv_timeout.
    stream.set_read_timeout(Some(std::time::Duration::from_secs(10)))?;
    stream.set_write_timeout(Some(std::time::Duration::from_secs(10)))?;

    let mut reader = BufReader::new(stream.try_clone()?);
    let mut writer = stream;

    // Read request line
    let mut request_line = String::new();
    reader.read_line(&mut request_line)?;
    let parts: Vec<&str> = request_line.trim().split_whitespace().collect();
    if parts.len() < 3 {
        return Ok(());
    }

    let method = parts[0].to_string();
    let target = parts[1].to_string();

    // Read all headers
    let mut headers = Vec::new();
    loop {
        let mut line = String::new();
        reader.read_line(&mut line)?;
        if line.trim().is_empty() {
            break;
        }
        headers.push(line);
    }

    // The browser extension used to talk to us via HTTP at /.fistbump/ext/*
    // on this very listener. That route is gone — extension traffic now goes
    // through the Unix socket served by `proxy::extension::start_ipc_listener`,
    // reached via the `fistbump-bridge` native messaging host. We keep `app`
    // around for future hooks but ignore it for now.
    let _ = app;

    // Serve PAC file
    if target == "/.fistbump/proxy.pac"
        || target == format!("http://127.0.0.1:{}/.fistbump/proxy.pac", pac::PROXY_PORT)
    {
        return serve_pac(&mut writer);
    }

    // DANE bypass: user clicked "Proceed anyway" on the warning page
    if target.starts_with("/.fistbump/dane-bypass?host=")
        || target.starts_with(&format!("http://127.0.0.1:{}/.fistbump/dane-bypass?host=", pac::PROXY_PORT))
    {
        if let Some(host) = target.split("host=").nth(1) {
            let host = host.trim();
            if !host.is_empty() {
                plog!("[fistbump] DANE bypass accepted for: {}", host);
                DANE_BYPASS.lock().unwrap().insert(host.to_string());
                let redirect = format!("https://{}/", host);
                let resp = format!(
                    "HTTP/1.1 302 Found\r\nLocation: {}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    redirect
                );
                writer.write_all(resp.as_bytes())?;
                writer.flush()?;
                return Ok(());
            }
        }
    }

    if method.eq_ignore_ascii_case("CONNECT") {
        // Drain any bytes BufReader read ahead from the shared fd.
        // If the client sent the TLS ClientHello in the same TCP segment as
        // the CONNECT request, BufReader consumed it. We must forward those
        // bytes to the upstream at the start of the tunnel.
        let buffered = reader.buffer().to_vec();
        handle_connect(&mut writer, &target, ca, &buffered)
    } else {
        handle_http(&mut writer, &method, &target, &headers, &mut reader)
    }
}

fn serve_pac(writer: &mut TcpStream) -> Result<(), BoxError> {
    let body = pac::pac_script();
    let response = format!(
        "HTTP/1.1 200 OK\r\n\
         Content-Type: application/x-ns-proxy-autoconfig\r\n\
         Cache-Control: max-age=3600\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\r\n{}",
        body.len(),
        body
    );
    writer.write_all(response.as_bytes())?;
    Ok(())
}

/// Extract the TLD from a hostname. "www.example" → "example", "example" → "example"
fn extract_tld(host: &str) -> &str {
    host.rsplit('.').next().unwrap_or(host)
}

/// Check if a hostname is a Fistbump name (TLD not in ICANN list).
fn is_fistbump_name(host: &str) -> bool {
    let tld = extract_tld(host);
    !icann::is_icann_tld(tld)
}

/// Serve an error page through the CONNECT tunnel.
/// Mints a cert for the host, establishes the tunnel, and sends an HTML error page.
fn serve_error_page(writer: &mut TcpStream, ca: &SharedCA, host: &str, badge: &str, title: &str, message: &str) -> Result<(), BoxError> {
    let (cert_chain, key) = cached_mint(ca, host)?;

    writer.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")?;
    writer.flush()?;

    let server_config = Arc::new(
        rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(cert_chain, key)
            .map_err(|e| format!("server config: {}", e))?,
    );
    let server_conn = rustls::ServerConnection::new(server_config)
        .map_err(|e| format!("server conn: {}", e))?;
    let mut browser_tls = rustls::StreamOwned::new(server_conn, writer.try_clone()?);
    browser_tls.flush()?;

    let error_html = format!(
        "<html><head><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
        <title>fistbump-error</title><style>\
        html,body{{height:100%;margin:0}}\
        body{{font-family:-apple-system,sans-serif;background:#09090b;color:#e4e4e7;\
        display:flex;align-items:center;justify-content:center;\
        text-align:center;padding:24px;box-sizing:border-box}}\
        .box{{max-width:360px}}\
        h2{{color:#f87171;font-size:18px;margin:0 0 12px}}\
        p{{font-size:14px;color:#a1a1aa;line-height:1.5;margin:0}}\
        .badge{{display:inline-block;background:#7f1d1d;color:#fca5a5;font-size:11px;\
        padding:2px 8px;border-radius:4px;margin-bottom:16px;font-weight:600}}\
        </style></head><body><div class=\"box\">\
        <div class=\"badge\">{}</div>\
        <h2>{}</h2>\
        <p>{}</p>\
        </div></body></html>", badge, title, message
    );
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        error_html.len(), error_html
    );
    let _ = browser_tls.write_all(response.as_bytes());
    let _ = browser_tls.flush();
    plog!("[fistbump] CONNECT: served error page for {}: {}", host, badge);
    Ok(())
}

/// Serve an error page as a plain HTTP response (for non-TLS proxy requests).
fn serve_http_error(writer: &mut TcpStream, badge: &str, title: &str, message: &str) -> Result<(), BoxError> {
    // Read and discard the incoming HTTP request before responding
    writer.set_read_timeout(Some(std::time::Duration::from_millis(500)))?;
    let mut discard = [0u8; 4096];
    let _ = writer.read(&mut discard);
    let error_html = format!(
        "<html><head><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
        <title>fistbump-error</title><style>\
        html,body{{height:100%;margin:0}}\
        body{{font-family:-apple-system,sans-serif;background:#09090b;color:#e4e4e7;\
        display:flex;align-items:center;justify-content:center;\
        text-align:center;padding:24px;box-sizing:border-box}}\
        .box{{max-width:360px}}\
        h2{{color:#f87171;font-size:18px;margin:0 0 12px}}\
        p{{font-size:14px;color:#a1a1aa;line-height:1.5;margin:0}}\
        .badge{{display:inline-block;background:#7f1d1d;color:#fca5a5;font-size:11px;\
        padding:2px 8px;border-radius:4px;margin-bottom:16px;font-weight:600}}\
        </style></head><body><div class=\"box\">\
        <div class=\"badge\">{}</div>\
        <h2>{}</h2>\
        <p>{}</p>\
        </div></body></html>", badge, title, message
    );
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        error_html.len(), error_html
    );
    writer.write_all(response.as_bytes())?;
    writer.flush()?;
    Ok(())
}

/// Handle a SOCKS5 connection — supports both HTTP and HTTPS tunneling.
fn handle_socks5(mut stream: TcpStream, ca: &SharedCA) -> Result<(), BoxError> {
    plog!("[fistbump] SOCKS5: new connection");
    stream.set_read_timeout(Some(std::time::Duration::from_secs(30)))?;
    stream.set_write_timeout(Some(std::time::Duration::from_secs(30)))?;

    // 1. Auth negotiation: client sends [0x05, nmethods, methods...]
    let mut buf = [0u8; 258];
    stream.read_exact(&mut buf[..2])?;
    if buf[0] != 0x05 { return Err("not SOCKS5".into()); }
    let nmethods = buf[1] as usize;
    stream.read_exact(&mut buf[..nmethods])?;
    // Reply: no auth required
    stream.write_all(&[0x05, 0x00])?;

    // 2. Connection request: [0x05, cmd, 0x00, atype, addr..., port_hi, port_lo]
    stream.read_exact(&mut buf[..4])?;
    if buf[0] != 0x05 || buf[1] != 0x01 { return Err("unsupported SOCKS5 command".into()); }
    let atype = buf[3];

    let host: String;
    match atype {
        0x01 => {
            // IPv4
            stream.read_exact(&mut buf[..4])?;
            host = format!("{}.{}.{}.{}", buf[0], buf[1], buf[2], buf[3]);
        }
        0x03 => {
            // Domain name
            stream.read_exact(&mut buf[..1])?;
            let len = buf[0] as usize;
            stream.read_exact(&mut buf[..len])?;
            host = String::from_utf8_lossy(&buf[..len]).to_string();
        }
        0x04 => {
            // IPv6
            stream.read_exact(&mut buf[..16])?;
            let mut parts = Vec::new();
            for i in 0..8 {
                parts.push(format!("{:x}", u16::from_be_bytes([buf[i*2], buf[i*2+1]])));
            }
            host = parts.join(":");
        }
        _ => return Err("unsupported address type".into()),
    }

    let mut port_buf = [0u8; 2];
    stream.read_exact(&mut port_buf)?;
    let port = u16::from_be_bytes(port_buf);

    plog!("[fistbump] SOCKS5 {}:{}", host, port);

    // For fistbump names, resolve via DNS and connect to the resolved IP.
    // For ICANN names, connect directly.
    let connect_host: String;
    let is_fistbump = is_fistbump_name(&host);

    if is_fistbump {
        match dns::resolve_full(&host) {
            dns::ResolveResult::Ok(r) => {
                connect_host = r.ipv4.as_deref()
                    .or(r.ipv6.as_deref())
                    .unwrap().to_string();
                plog!("[fistbump] SOCKS5: {} -> {}", host, connect_host);
            }
            other => {
                // Send SOCKS5 success (so we can serve error page for HTTPS,
                // or just fail for HTTP)
                let (badge, title, msg) = match other {
                    dns::ResolveResult::NotFound => ("NAME NOT FOUND", "Name Not Found",
                        format!("The name <strong>{}</strong> is not registered on the Fistbump blockchain.", host)),
                    dns::ResolveResult::NoRecords => ("NO RECORDS", "No DNS Records",
                        format!("The name <strong>{}</strong> is registered but has no DNS records configured.", host)),
                    dns::ResolveResult::NoWebsite => ("NO WEBSITE", "No Website Records",
                        format!("The name <strong>{}</strong> has DNS records but no A, AAAA, or CNAME record pointing to a website.", host)),
                    _ => ("DNS ERROR", "DNS Lookup Failed",
                        format!("Could not resolve <strong>{}</strong>. The node may still be syncing.", host)),
                };
                plog!("[fistbump] SOCKS5: {}: {}", host, badge);
                // Send SOCKS5 success so we can serve the error page
                stream.write_all(&[0x05, 0x00, 0x00, 0x01, 0,0,0,0, 0,0])?;
                if port == 443 {
                    // HTTPS: serve error page through TLS tunnel
                    return serve_socks_error_page(&mut stream, ca, &host, badge, title, &msg);
                }
                // HTTP: serve error page as plain HTTP response
                return serve_http_error(&mut stream, badge, title, &msg);
            }
        }
    } else {
        connect_host = host.clone();
    }

    // Connect to upstream
    let upstream_addr = format!("{}:{}", connect_host, port);
    let mut upstream = match TcpStream::connect(&upstream_addr) {
        Ok(s) => s,
        Err(e) => {
            plog!("[fistbump] SOCKS5: connect failed {}: {}", upstream_addr, e);
            if is_fistbump {
                stream.write_all(&[0x05, 0x00, 0x00, 0x01, 0,0,0,0, 0,0])?;
                let msg = format!("The server for <strong>{}</strong> did not respond.", host);
                if port == 443 {
                    return serve_socks_error_page(&mut stream, ca, &host,
                        "CONNECTION FAILED", "No Response", &msg);
                }
                return serve_http_error(&mut stream,
                    "CONNECTION FAILED", "No Response", &msg);
            }
            stream.write_all(&[0x05, 0x05, 0x00, 0x01, 0,0,0,0, 0,0])?;
            return Ok(());
        }
    };

    // Send SOCKS5 success
    stream.write_all(&[0x05, 0x00, 0x00, 0x01, 0,0,0,0, 0,0])?;

    // For HTTPS on fistbump names, do DANE validation (intercept TLS)
    if is_fistbump && port == 443 {
        return handle_socks5_tls(stream, upstream, ca, &host);
    }

    // For ICANN HTTPS, verify the upstream cert is valid before relaying.
    // WKWebView doesn't call didReceive challenge through SOCKS5 tunnels,
    // so we need to check here.
    if !is_fistbump && port == 443 {
        // Quick TLS probe: connect, check cert, then relay if valid
        upstream.set_read_timeout(Some(std::time::Duration::from_secs(10)))?;
        upstream.set_write_timeout(Some(std::time::Duration::from_secs(10)))?;

        let mut root_store = rustls::RootCertStore::empty();
        root_store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        let tls_config = ClientConfig::builder()
            .with_root_certificates(root_store)
            .with_no_client_auth();

        let server_name: ServerName<'static> = ServerName::try_from(host.clone())
            .unwrap_or_else(|_| ServerName::try_from("localhost".to_string()).unwrap());

        let mut conn = match rustls::ClientConnection::new(Arc::new(tls_config), server_name) {
            Ok(c) => c,
            Err(e) => {
                plog!("[fistbump] SOCKS5: TLS init failed for {}: {}", host, e);
                return serve_socks_error_page(&mut stream, ca, &host,
                    "SSL ERROR", "Secure Connection Failed",
                    &format!("Could not establish a secure connection to <strong>{}</strong>.", host));
            }
        };

        // Drive handshake
        loop {
            if conn.wants_write() { if conn.write_tls(&mut upstream).is_err() { break; } }
            if conn.wants_read() {
                match conn.read_tls(&mut upstream) {
                    Ok(0) => break,
                    Ok(_) => {
                        if let Err(e) = conn.process_new_packets() {
                            plog!("[fistbump] SOCKS5: TLS cert invalid for {}: {}", host, e);
                            return serve_socks_error_page(&mut stream, ca, &host,
                                "CERTIFICATE ERROR", "Certificate Not Valid",
                                &format!("The SSL certificate for <strong>{}</strong> is not valid. The connection has been blocked.", host));
                        }
                    }
                    Err(e) => {
                        plog!("[fistbump] SOCKS5: TLS read error for {}: {}", host, e);
                        return serve_socks_error_page(&mut stream, ca, &host,
                            "SSL ERROR", "Secure Connection Failed",
                            &format!("Could not establish a secure connection to <strong>{}</strong>.", host));
                    }
                }
            }
            if !conn.is_handshaking() { break; }
        }

        plog!("[fistbump] SOCKS5: TLS valid for {}, relaying", host);

        // Cert is valid — now we need to relay. But we've already consumed the
        // TLS handshake data from upstream. We need to MITM: present the upstream
        // cert's identity to the browser via our CA, then relay decrypted data.
        let upstream_tls = rustls::StreamOwned::new(conn, upstream);
        return handle_socks5_icann_relay(stream, upstream_tls, ca, &host);
    }

    // For everything else, plain bidirectional relay
    let mut upstream_clone = upstream.try_clone()?;
    let mut stream_clone = stream.try_clone()?;
    let h = std::thread::spawn(move || { let _ = std::io::copy(&mut upstream_clone, &mut stream_clone); });
    let mut upstream_write = upstream.try_clone()?;
    let _ = std::io::copy(&mut stream, &mut upstream_write);
    let _ = h.join();

    Ok(())
}

/// Relay a validated ICANN HTTPS connection through MITM (cert already verified).
fn handle_socks5_icann_relay(
    stream: TcpStream,
    mut upstream_tls: rustls::StreamOwned<rustls::ClientConnection, TcpStream>,
    ca: &SharedCA,
    host: &str,
) -> Result<(), BoxError> {
    let (cert_chain, key) = cached_mint(ca, host)?;
    let server_config = Arc::new(
        rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(cert_chain, key)
            .map_err(|e| format!("server config: {}", e))?,
    );
    let server_conn = rustls::ServerConnection::new(server_config)?;
    let mut browser_tls = rustls::StreamOwned::new(server_conn, stream.try_clone()?);
    browser_tls.flush()?;

    // Bidirectional TLS relay
    upstream_tls.sock.set_read_timeout(Some(std::time::Duration::from_millis(5)))?;
    let browser_raw = stream.try_clone()?;
    browser_raw.set_read_timeout(Some(std::time::Duration::from_millis(5)))?;

    let mut buf_b2u = [0u8; 16384];
    let mut buf_u2b = [0u8; 16384];
    loop {
        let mut did_work = false;
        match browser_tls.read(&mut buf_b2u) {
            Ok(0) => break, Ok(n) => { if upstream_tls.write_all(&buf_b2u[..n]).is_err() { break; } let _ = upstream_tls.flush(); did_work = true; }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock || e.kind() == std::io::ErrorKind::TimedOut => {}
            Err(_) => break,
        }
        match upstream_tls.read(&mut buf_u2b) {
            Ok(0) => break, Ok(n) => { if browser_tls.write_all(&buf_u2b[..n]).is_err() { break; } let _ = browser_tls.flush(); did_work = true; }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock || e.kind() == std::io::ErrorKind::TimedOut => {}
            Err(_) => break,
        }
        if !did_work { std::thread::sleep(std::time::Duration::from_millis(1)); }
    }
    Ok(())
}

/// Handle SOCKS5 TLS connection with DANE validation.
fn handle_socks5_tls(mut stream: TcpStream, mut upstream: TcpStream, ca: &SharedCA, host: &str) -> Result<(), BoxError> {
    upstream.set_read_timeout(Some(std::time::Duration::from_secs(30)))?;
    upstream.set_write_timeout(Some(std::time::Duration::from_secs(30)))?;

    let resolved = dns::resolve(host);
    let tlsa = resolved.as_ref().map(|r| &r.tlsa[..]).unwrap_or(&[]);

    let server_name: ServerName<'static> = ServerName::try_from(host.to_string())
        .unwrap_or_else(|_| ServerName::try_from("localhost".to_string()).unwrap());

    let danger_config = ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(DangerAcceptAll))
        .with_no_client_auth();

    let mut conn = match rustls::ClientConnection::new(Arc::new(danger_config), server_name) {
        Ok(c) => c,
        Err(e) => {
            plog!("[fistbump] SOCKS5 TLS init failed: {}", e);
            return Ok(());
        }
    };

    // Drive TLS handshake
    loop {
        if conn.wants_write() { conn.write_tls(&mut upstream)?; }
        if conn.wants_read() {
            if conn.read_tls(&mut upstream)? == 0 { return Ok(()); }
            conn.process_new_packets()?;
        }
        if !conn.is_handshaking() { break; }
    }

    let cert_der = conn.peer_certificates()
        .and_then(|c| c.first())
        .map(|c| c.as_ref().to_vec())
        .unwrap_or_default();

    let bypassed = DANE_BYPASS.lock().unwrap().contains(host);
    let dane_ok = dane::validate(&cert_der, tlsa);
    plog!("[fistbump] SOCKS5 DANE {}: {} (tlsa: {}, bypass: {})", host,
        if dane_ok { "PASS" } else { "FAIL" }, tlsa.len(), bypassed);

    if !dane_ok && !bypassed {
        let proceed_url = format!("http://127.0.0.1:{}/.fistbump/dane-bypass?host={}", pac::PROXY_PORT, host);
        if tlsa.is_empty() {
            return serve_socks_error_page(&mut stream, ca, host,
                "NO DANE RECORD", "Connection Not Verified",
                &format!("<strong>{}</strong> has no TLSA record, so the certificate cannot be verified.\
                <br><br><a href=\"{}\" style=\"color:#60a5fa\">Proceed anyway</a>", host, proceed_url));
        } else {
            return serve_socks_error_page(&mut stream, ca, host,
                "DANE VALIDATION FAILED", "Connection Not Secure",
                &format!("The certificate presented by <strong>{}</strong> does not match its on-chain TLSA record.\
                <br><br><a href=\"{}\" style=\"color:#60a5fa\">Proceed anyway</a>", host, proceed_url));
        }
    }

    // Mint cert and set up TLS for the browser side
    let (cert_chain, key) = cached_mint(ca, host)?;
    let server_config = Arc::new(
        rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(cert_chain, key)
            .map_err(|e| format!("server config: {}", e))?,
    );
    let server_conn = rustls::ServerConnection::new(server_config)?;
    let mut browser_tls = rustls::StreamOwned::new(server_conn, stream.try_clone()?);
    browser_tls.flush()?;

    let mut upstream_tls = rustls::StreamOwned::new(conn, upstream);

    // Bidirectional TLS relay
    upstream_tls.sock.set_read_timeout(Some(std::time::Duration::from_millis(5)))?;
    let browser_raw = stream.try_clone()?;
    browser_raw.set_read_timeout(Some(std::time::Duration::from_millis(5)))?;

    let mut buf_b2u = [0u8; 16384];
    let mut buf_u2b = [0u8; 16384];
    loop {
        let mut did_work = false;
        match browser_tls.read(&mut buf_b2u) {
            Ok(0) => break, Ok(n) => { if upstream_tls.write_all(&buf_b2u[..n]).is_err() { break; } let _ = upstream_tls.flush(); did_work = true; }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock || e.kind() == std::io::ErrorKind::TimedOut => {}
            Err(_) => break,
        }
        match upstream_tls.read(&mut buf_u2b) {
            Ok(0) => break, Ok(n) => { if browser_tls.write_all(&buf_u2b[..n]).is_err() { break; } let _ = browser_tls.flush(); did_work = true; }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock || e.kind() == std::io::ErrorKind::TimedOut => {}
            Err(_) => break,
        }
        if !did_work { std::thread::sleep(std::time::Duration::from_millis(1)); }
    }
    Ok(())
}

/// Serve an error page through a SOCKS5 connection (after SOCKS5 success reply).
fn serve_socks_error_page(stream: &mut TcpStream, ca: &SharedCA, host: &str, badge: &str, title: &str, message: &str) -> Result<(), BoxError> {
    let (cert_chain, key) = cached_mint(ca, host)?;
    let server_config = Arc::new(
        rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(cert_chain, key)
            .map_err(|e| format!("server config: {}", e))?,
    );
    let server_conn = rustls::ServerConnection::new(server_config)?;
    let mut tls = rustls::StreamOwned::new(server_conn, stream.try_clone()?);
    tls.flush()?;

    let error_html = format!(
        "<html><head><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
        <title>fistbump-error</title><style>\
        html,body{{height:100%;margin:0}}\
        body{{font-family:-apple-system,sans-serif;background:#09090b;color:#e4e4e7;\
        display:flex;align-items:center;justify-content:center;\
        text-align:center;padding:24px;box-sizing:border-box}}\
        .box{{max-width:360px}}\
        h2{{color:#f87171;font-size:18px;margin:0 0 12px}}\
        p{{font-size:14px;color:#a1a1aa;line-height:1.5;margin:0}}\
        .badge{{display:inline-block;background:#7f1d1d;color:#fca5a5;font-size:11px;\
        padding:2px 8px;border-radius:4px;margin-bottom:16px;font-weight:600}}\
        </style></head><body><div class=\"box\">\
        <div class=\"badge\">{}</div>\
        <h2>{}</h2>\
        <p>{}</p>\
        </div></body></html>", badge, title, message
    );
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        error_html.len(), error_html
    );
    let _ = tls.write_all(response.as_bytes());
    let _ = tls.flush();
    plog!("[fistbump] SOCKS5: served error page for {}: {}", host, badge);
    Ok(())
}

/// Handle HTTPS CONNECT — DANE-validating MITM proxy.
fn handle_connect(writer: &mut TcpStream, target: &str, ca: &SharedCA, buffered: &[u8]) -> Result<(), BoxError> {
    let (host, port) = parse_host_port(target)?;

    // ICANN TLD — straight TCP tunnel, no MITM
    if !is_fistbump_name(&host) {
        return tunnel_direct(writer, &host, port, buffered);
    }

    plog!("[fistbump] CONNECT {}:{}", host, port);

    // Resolve via fbd DNS
    let resolved = match dns::resolve_full(&host) {
        dns::ResolveResult::Ok(r) => r,
        dns::ResolveResult::NotFound => {
            plog!("[fistbump] CONNECT: name not registered: {}", host);
            return serve_error_page(writer, ca, &host,
                "NAME NOT FOUND", "Name Not Found",
                &format!("The name <strong>{}</strong> is not registered on the Fistbump blockchain.", host));
        }
        dns::ResolveResult::NoRecords => {
            plog!("[fistbump] CONNECT: no records: {}", host);
            return serve_error_page(writer, ca, &host,
                "NO RECORDS", "No DNS Records",
                &format!("The name <strong>{}</strong> is registered but has no DNS records configured.", host));
        }
        dns::ResolveResult::NoWebsite => {
            plog!("[fistbump] CONNECT: no website records: {}", host);
            return serve_error_page(writer, ca, &host,
                "NO WEBSITE", "No Website Records",
                &format!("The name <strong>{}</strong> has DNS records but no A, AAAA, or CNAME record pointing to a website.", host));
        }
        dns::ResolveResult::DnsError => {
            plog!("[fistbump] CONNECT: DNS query failed for {}", host);
            return serve_error_page(writer, ca, &host,
                "DNS ERROR", "DNS Lookup Failed",
                &format!("Could not resolve <strong>{}</strong>. The node may still be syncing.", host));
        }
    };
    let ip = resolved.ipv4.as_deref()
        .or(resolved.ipv6.as_deref())
        .unwrap()
        .to_string();
    plog!("[fistbump] CONNECT: {} -> {} (tlsa: {})", host, ip, resolved.tlsa.len());

    // Connect to upstream
    let upstream_addr = format!("{}:{}", ip, port);
    let mut upstream = match TcpStream::connect(&upstream_addr) {
        Ok(s) => s,
        Err(e) => {
            plog!("[fistbump] CONNECT: upstream connect failed: {}", e);
            return serve_error_page(writer, ca, &host,
                "CONNECTION FAILED", "No Response",
                &format!("The server for <strong>{}</strong> did not respond.", host));
        }
    };
    upstream.set_read_timeout(Some(std::time::Duration::from_secs(30)))?;
    upstream.set_write_timeout(Some(std::time::Duration::from_secs(30)))?;
    plog!("[fistbump] CONNECT: connected to upstream {}", upstream_addr);

    let server_name: ServerName<'static> = ServerName::try_from(host.clone())
        .unwrap_or_else(|_| ServerName::try_from("localhost".to_string()).unwrap());

    // For DANE, use danger-accept verifier so we can validate ourselves
    let danger_config = ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(DangerAcceptAll))
        .with_no_client_auth();

    let mut conn = match rustls::ClientConnection::new(Arc::new(danger_config), server_name) {
        Ok(c) => c,
        Err(e) => {
            plog!("[fistbump] CONNECT: TLS init failed: {}", e);
            let _ = writer.write_all(b"HTTP/1.1 502 TLS Init Failed\r\n\r\n");
            return Ok(());
        }
    };
    plog!("[fistbump] CONNECT: driving TLS handshake...");
    loop {
        if conn.wants_write() {
            match conn.write_tls(&mut upstream) {
                Ok(_) => {}
                Err(e) => {
                    plog!("[fistbump] CONNECT: TLS write error: {}", e);
                    let _ = writer.write_all(b"HTTP/1.1 502 TLS Handshake Failed\r\n\r\n");
                    return Ok(());
                }
            }
        }
        if conn.wants_read() {
            match conn.read_tls(&mut upstream) {
                Ok(0) => {
                    plog!("[fistbump] CONNECT: upstream closed during handshake");
                    let _ = writer.write_all(b"HTTP/1.1 502 TLS Handshake Failed\r\n\r\n");
                    return Ok(());
                }
                Ok(_) => {
                    if let Err(e) = conn.process_new_packets() {
                        plog!("[fistbump] CONNECT: TLS process error: {}", e);
                        let _ = writer.write_all(b"HTTP/1.1 502 TLS Handshake Failed\r\n\r\n");
                        return Ok(());
                    }
                }
                Err(e) => {
                    plog!("[fistbump] CONNECT: TLS read error: {}", e);
                    let _ = writer.write_all(b"HTTP/1.1 502 TLS Handshake Failed\r\n\r\n");
                    return Ok(());
                }
            }
        }
        if !conn.is_handshaking() {
            break;
        }
    }
    plog!("[fistbump] CONNECT: TLS handshake complete");
    let mut upstream_tls = rustls::StreamOwned::new(conn, upstream);

    let upstream_cert_der = upstream_tls
        .conn
        .peer_certificates()
        .and_then(|c| c.first())
        .map(|c| c.as_ref().to_vec())
        .unwrap_or_default();
    plog!("[fistbump] CONNECT: upstream cert size: {} bytes", upstream_cert_der.len());

    // DANE validation
    let bypassed = DANE_BYPASS.lock().unwrap().contains(&host);
    let dane_ok = dane::validate(&upstream_cert_der, &resolved.tlsa);
    plog!("[fistbump] CONNECT: DANE validation: {} (tlsa: {}, bypass: {})",
        if dane_ok { "PASS" } else { "FAIL" }, resolved.tlsa.len(), bypassed);
    if !dane_ok && !bypassed {
        let proceed_url = format!("http://127.0.0.1:{}/.fistbump/dane-bypass?host={}", pac::PROXY_PORT, host);
        if resolved.tlsa.is_empty() {
            return serve_error_page(writer, ca, &host,
                "NO DANE RECORD", "Connection Not Verified",
                &format!("<strong>{}</strong> has no TLSA record, so the certificate cannot be verified.\
                <br><br><a href=\"{}\" style=\"color:#60a5fa\">Proceed anyway</a>", host, proceed_url));
        } else {
            return serve_error_page(writer, ca, &host,
                "DANE VALIDATION FAILED", "Connection Not Secure",
                &format!("The certificate presented by <strong>{}</strong> does not match its on-chain TLSA record.\
                <br><br><a href=\"{}\" style=\"color:#60a5fa\">Proceed anyway</a>", host, proceed_url));
        }
    }

    // Mint a certificate for this hostname — cached
    let (cert_chain, key) = match cached_mint(ca, &host) {
        Ok(r) => r,
        Err(e) => {
            plog!("[fistbump] CONNECT: cert mint failed: {}", e);
            let _ = writer.write_all(b"HTTP/1.1 502 Internal Error\r\n\r\n");
            return Ok(());
        }
    };
    plog!("[fistbump] CONNECT: minted cert for {}", host);

    // Tell browser the tunnel is established
    writer.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")?;
    writer.flush()?;
    plog!("[fistbump] CONNECT: sent 200, starting TLS server");

    // Set up TLS server for the browser side
    let server_config = Arc::new(
        rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(cert_chain, key)
            .map_err(|e| format!("server config: {}", e))?,
    );

    let server_conn =
        rustls::ServerConnection::new(server_config).map_err(|e| format!("server conn: {}", e))?;
    let mut browser_tls = rustls::StreamOwned::new(server_conn, writer.try_clone()?);

    // Do the server-side TLS handshake explicitly
    match browser_tls.flush() {
        Ok(_) => plog!("[fistbump] CONNECT: browser TLS handshake complete"),
        Err(e) => {
            plog!("[fistbump] CONNECT: browser TLS handshake failed: {}", e);
            return Ok(());
        }
    }


    // Bidirectional shuttle — use two threads with the underlying sockets
    // Browser TLS ←→ Upstream TLS
    // We need to split read/write. StreamOwned doesn't clone, so use a
    // single-thread poll loop with short timeouts.
    upstream_tls.sock.set_read_timeout(Some(std::time::Duration::from_millis(5)))?;
    upstream_tls.sock.set_write_timeout(Some(std::time::Duration::from_secs(10)))?;
    // browser side: the underlying TCP is `writer`, set its timeout
    let browser_raw = writer.try_clone()?;
    browser_raw.set_read_timeout(Some(std::time::Duration::from_millis(5)))?;
    browser_raw.set_write_timeout(Some(std::time::Duration::from_secs(10)))?;

    let mut buf_b2u = [0u8; 16384];
    let mut buf_u2b = [0u8; 16384];

    loop {
        let mut did_work = false;

        // Browser → Upstream
        match browser_tls.read(&mut buf_b2u) {
            Ok(0) => break,
            Ok(n) => {
                if upstream_tls.write_all(&buf_b2u[..n]).is_err() {
                    break;
                }
                let _ = upstream_tls.flush();
                did_work = true;
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock
                || e.kind() == std::io::ErrorKind::TimedOut => {}
            Err(_) => break,
        }

        // Upstream → Browser
        match upstream_tls.read(&mut buf_u2b) {
            Ok(0) => break,
            Ok(n) => {
                if browser_tls.write_all(&buf_u2b[..n]).is_err() {
                    break;
                }
                let _ = browser_tls.flush();
                did_work = true;
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock
                || e.kind() == std::io::ErrorKind::TimedOut => {}
            Err(_) => break,
        }

        if !did_work {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }

    plog!("[fistbump] CONNECT: tunnel closed for {}", host);
    Ok(())
}

/// Straight TCP tunnel for non-Fistbump names.
fn tunnel_direct(writer: &mut TcpStream, host: &str, port: u16, buffered: &[u8]) -> Result<(), BoxError> {
    // Resolve + connect with a per-address timeout so a stalled CDN node
    // fails fast instead of hanging for the kernel's 75-second default.
    let addrs: Vec<std::net::SocketAddr> = format!("{}:{}", host, port).to_socket_addrs()?.collect();
    let timeout = std::time::Duration::from_secs(10);
    let upstream = match addrs.iter().find_map(|a| TcpStream::connect_timeout(a, timeout).ok()) {
        Some(s) => s,
        None => {
            // Tell the browser so it can show a real error / retry.
            let _ = writer.write_all(
                b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            );
            return Err(format!("tunnel_direct: connect to {}:{} failed", host, port).into());
        }
    };

    // Set timeouts on both sides so stalled tunnels don't leak threads.
    let tunnel_timeout = Some(std::time::Duration::from_secs(300));
    upstream.set_read_timeout(tunnel_timeout)?;
    upstream.set_write_timeout(Some(std::time::Duration::from_secs(30)))?;
    writer.set_read_timeout(tunnel_timeout)?;
    writer.set_write_timeout(Some(std::time::Duration::from_secs(30)))?;

    writer.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")?;

    // Forward any bytes the BufReader consumed ahead of the CONNECT headers
    // (e.g., TLS ClientHello sent in the same TCP segment).
    if !buffered.is_empty() {
        let mut up = upstream.try_clone()?;
        up.write_all(buffered)?;
    }

    let mut upstream_clone = upstream.try_clone()?;
    let mut writer_clone = writer.try_clone()?;

    // When one direction hits EOF or error, shut down the other side so
    // the peer thread wakes up immediately instead of blocking until the
    // 300-second read timeout. This keeps tunnel threads short-lived.
    let handle = std::thread::spawn(move || {
        let _ = std::io::copy(&mut upstream_clone, &mut writer_clone);
        let _ = writer_clone.shutdown(std::net::Shutdown::Both);
    });

    let mut upstream_write = upstream.try_clone()?;
    let _ = std::io::copy(writer, &mut upstream_write);
    let _ = upstream.shutdown(std::net::Shutdown::Both);
    let _ = handle.join();

    Ok(())
}

/// Handle plain HTTP proxy request.
fn handle_http(
    writer: &mut TcpStream,
    method: &str,
    target: &str,
    headers: &[String],
    reader: &mut BufReader<TcpStream>,
) -> Result<(), BoxError> {
    let url = if target.starts_with("http://") {
        target.to_string()
    } else {
        format!("http://{}", target)
    };

    let (host, port, path) = parse_http_url(&url)?;

    let connect_addr = if is_fistbump_name(&host) {
        match dns::resolve_full(&host) {
            dns::ResolveResult::Ok(r) => {
                let ip = r.ipv4.as_deref().or(r.ipv6.as_deref()).unwrap();
                format!("{}:{}", ip, port)
            }
            dns::ResolveResult::NotFound => {
                return serve_http_error(writer, "NAME NOT FOUND", "Name Not Found",
                    &format!("The name <strong>{}</strong> is not registered on the Fistbump blockchain.", host));
            }
            dns::ResolveResult::NoRecords => {
                return serve_http_error(writer, "NO RECORDS", "No DNS Records",
                    &format!("The name <strong>{}</strong> is registered but has no DNS records configured.", host));
            }
            dns::ResolveResult::NoWebsite => {
                return serve_http_error(writer, "NO WEBSITE", "No Website Records",
                    &format!("The name <strong>{}</strong> has DNS records but no A, AAAA, or CNAME record pointing to a website.", host));
            }
            dns::ResolveResult::DnsError => {
                return serve_http_error(writer, "DNS ERROR", "DNS Lookup Failed",
                    &format!("Could not resolve <strong>{}</strong>. The node may still be syncing.", host));
            }
        }
    } else {
        format!("{}:{}", host, port)
    };

    let mut upstream = match TcpStream::connect(&connect_addr) {
        Ok(s) => s,
        Err(_) => {
            return serve_http_error(writer, "CONNECTION FAILED", "No Response",
                &format!("The server for <strong>{}</strong> did not respond.", host));
        }
    };
    upstream.set_read_timeout(Some(std::time::Duration::from_secs(30)))?;
    upstream.set_write_timeout(Some(std::time::Duration::from_secs(30)))?;

    // Forward the request with rewritten path (proxy sends absolute URL, upstream wants relative)
    let request_line = format!("{} {} HTTP/1.1\r\n", method, path);
    upstream.write_all(request_line.as_bytes())?;

    let mut sent_host = false;
    for h in headers {
        let lower = h.to_lowercase();
        if lower.starts_with("host:") {
            upstream.write_all(format!("Host: {}\r\n", host).as_bytes())?;
            sent_host = true;
        } else if lower.starts_with("connection:") || lower.starts_with("proxy-connection:") {
            // Skip — we force Connection: close below
        } else {
            upstream.write_all(h.as_bytes())?;
        }
    }
    if !sent_host {
        upstream.write_all(format!("Host: {}\r\n", host).as_bytes())?;
    }
    // Force close so std::io::copy returns after the response instead of
    // blocking until the server's keep-alive timeout expires.
    upstream.write_all(b"Connection: close\r\n")?;
    upstream.write_all(b"\r\n")?;

    // Forward request body if Content-Length is set
    let content_length: usize = headers.iter()
        .find(|h| h.to_lowercase().starts_with("content-length:"))
        .and_then(|h| h.split(':').nth(1)?.trim().parse().ok())
        .unwrap_or(0);
    if content_length > 0 {
        let mut body = vec![0u8; content_length];
        reader.read_exact(&mut body)?;
        upstream.write_all(&body)?;
    }
    upstream.flush()?;

    // Relay response
    let _ = std::io::copy(&mut upstream, writer);

    Ok(())
}

fn parse_host_port(target: &str) -> Result<(String, u16), BoxError> {
    if let Some(colon) = target.rfind(':') {
        let host = target[..colon].to_string();
        let port: u16 = target[colon + 1..].parse().unwrap_or(443);
        Ok((host, port))
    } else {
        Ok((target.to_string(), 443))
    }
}

fn parse_http_url(url: &str) -> Result<(String, u16, String), BoxError> {
    let without_scheme = url.strip_prefix("http://").unwrap_or(url);
    let (host_port, path) = match without_scheme.find('/') {
        Some(i) => (&without_scheme[..i], &without_scheme[i..]),
        None => (without_scheme, "/"),
    };
    let (host, port) = if let Some(colon) = host_port.rfind(':') {
        (
            host_port[..colon].to_string(),
            host_port[colon + 1..].parse().unwrap_or(80),
        )
    } else {
        (host_port.to_string(), 80u16)
    };
    Ok((host, port, path.to_string()))
}

/// Certificate verifier that accepts all certs (DANE validates instead of PKI).
#[derive(Debug)]
struct DangerAcceptAll;

impl rustls::client::danger::ServerCertVerifier for DangerAcceptAll {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        vec![
            rustls::SignatureScheme::ECDSA_NISTP256_SHA256,
            rustls::SignatureScheme::ECDSA_NISTP384_SHA384,
            rustls::SignatureScheme::RSA_PSS_SHA256,
            rustls::SignatureScheme::RSA_PSS_SHA384,
            rustls::SignatureScheme::RSA_PSS_SHA512,
            rustls::SignatureScheme::RSA_PKCS1_SHA256,
            rustls::SignatureScheme::RSA_PKCS1_SHA384,
            rustls::SignatureScheme::RSA_PKCS1_SHA512,
            rustls::SignatureScheme::ED25519,
        ]
    }
}
