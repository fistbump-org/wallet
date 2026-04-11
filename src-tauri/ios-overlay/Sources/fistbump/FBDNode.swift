import Foundation
import UIKit
import WebKit
import Network
import Logging
import Node
import LocalAuthentication
import Security

/// Rust FFI: push a log line into the app's log buffer
@_silgen_name("push_log_line")
private func _pushLogLine(_ ptr: UnsafePointer<CChar>?)

/// Rust FFI: register a URL opener callback
@_silgen_name("register_url_handler")
private func _registerURLHandler(_ f: @convention(c) (UnsafePointer<CChar>?) -> Void)

/// Rust FFI: register a node restart callback
@_silgen_name("register_restart_handler")
private func _registerRestartHandler(_ f: @convention(c) () -> Void)

/// Send a line directly to the Rust log buffer, bypassing stdout/stderr.
private func fbLog(_ msg: String) {
    msg.withCString { _pushLogLine($0) }
}

/// C-callable URL opener (passed as function pointer to Rust)
private func openURL(_ urlPtr: UnsafePointer<CChar>?) {
    guard let urlPtr = urlPtr,
          let urlStr = String(validatingUTF8: urlPtr),
          let url = URL(string: urlStr) else { return }
    DispatchQueue.main.async {
        UIApplication.shared.open(url)
    }
}

// MARK: - LogHandler that pushes directly to Rust via FFI

/// swift-log handler that sends log output directly to the Rust log buffer.
/// This bypasses stdout/stderr entirely, avoiding iOS pipe capture issues.
private struct DirectLogHandler: LogHandler {
    var logLevel: Logger.Level = .debug
    var metadata: Logger.Metadata = [:]
    private let label: String

    init(label: String) {
        self.label = label
    }

    subscript(metadataKey key: String) -> Logger.Metadata.Value? {
        get { metadata[key] }
        set { metadata[key] = newValue }
    }

    func log(
        level: Logger.Level,
        message: Logger.Message,
        metadata: Logger.Metadata?,
        source: String,
        file: String,
        function: String,
        line: UInt
    ) {
        let lvl = levelChar(level)
        let src = source.lowercased().padding(toLength: 8, withPad: " ", startingAt: 0)
        var text = "\(lvl)) \(src)| \(message)"
        if let meta = metadata, !meta.isEmpty {
            let pairs = meta.sorted { $0.key < $1.key }.map { "\($0.key)=\($0.value)" }.joined(separator: " ")
            text += " \(pairs)"
        }
        fbLog(text)
        // Broadcast to WebSocket subscribers (if node is running).
        NodeContext.logBroadcast?(text)
    }

    private func levelChar(_ level: Logger.Level) -> Character {
        switch level {
        case .trace:    return "T"
        case .debug:    return "D"
        case .info:     return "I"
        case .notice:   return "N"
        case .warning:  return "W"
        case .error:    return "E"
        case .critical: return "C"
        }
    }
}

// MARK: - Entry point

/// C-callable restart — invoked from Rust when mining settings change.
private func restartNode() {
    Task { await FBDNode.shared.restart() }
}

/// C-callable entry point for main.mm
@_silgen_name("start_dane_proxy")
private func _startDaneProxy()

@_cdecl("start_fbd_node")
func startFBDNode() {
    _registerURLHandler(openURL)
    _registerRestartHandler(restartNode)
    registerBiometricFFI()
    registerBrowseFFI()
    registerQRScanFFI()
    _startDaneProxy()

    // Bootstrap swift-log to push directly to Rust log buffer via FFI.
    // On iOS, the default os_log backend and stdout/stderr pipe capture are unreliable.
    LoggingSystem.bootstrap { label in DirectLogHandler(label: label) }

    fbLog("[fistbump] log system initialized")
    FBDNode.shared.start()
    fixTauriWebViewLayout()
}

/// iOS 18 regression: WKWebView insets the CSS layout viewport by the safe areas,
/// so `env(safe-area-inset-*)` can't see the status bar / home indicator area and
/// the Tauri window background shows through. Force a zero viewport inset and
/// disable scrollview content-inset auto-adjustment so viewport-fit=cover works.
private func fixTauriWebViewLayout(attempt: Int = 0) {
    DispatchQueue.main.async {
        guard let window = UIApplication.shared.connectedScenes
                .compactMap({ $0 as? UIWindowScene })
                .first?.windows.first,
              let rootView = window.rootViewController?.view,
              let webView = rootView.subviews.compactMap({ $0 as? WKWebView }).first
        else {
            if attempt < 40 {
                DispatchQueue.main.asyncAfter(deadline: .now() + 0.05) {
                    fixTauriWebViewLayout(attempt: attempt + 1)
                }
            }
            return
        }
        webView.scrollView.contentInsetAdjustmentBehavior = .never
        webView.scrollView.contentInset = .zero
        webView.scrollView.verticalScrollIndicatorInsets = .zero
        webView.scrollView.horizontalScrollIndicatorInsets = .zero
        webView.scrollView.automaticallyAdjustsScrollIndicatorInsets = false
        if #available(iOS 15.5, *) {
            webView.setMinimumViewportInset(.zero, maximumViewportInset: .zero)
        }
        webView.setNeedsLayout()
        webView.layoutIfNeeded()
    }
}

/// Manages the in-process fbd node on iOS.
/// Uses a serial restart queue to prevent concurrent start/stop operations.
class FBDNode {
    static let shared = FBDNode()

    private var node: FullNode?
    private var nodeTask: Task<Void, Never>?
    private var isRestarting = false

    private let docs = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask).first!

    /// Read mining settings from the shared settings.json file.
    private func loadMinerAddress() -> String? {
        let settingsPath = docs.appendingPathComponent(".fistbump/settings.json")
        guard let data = try? Data(contentsOf: settingsPath),
              let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let enabled = json["miningEnabled"] as? Bool, enabled,
              let addr = json["minerAddress"] as? String, !addr.isEmpty else {
            return nil
        }
        return addr
    }

    /// Start the node in a background task.
    func start() {
        guard nodeTask == nil else { return }

        let dataDir = docs.appendingPathComponent("fbd").path
        let minerAddr = loadMinerAddress()

        if let addr = minerAddr {
            fbLog("[fistbump] starting node, mining to \(addr)")
        } else {
            fbLog("[fistbump] starting node, mining disabled")
        }

        let config = NodeConfig(
            network: .main,
            dataDir: dataDir,
            rpcHost: "127.0.0.1",
            rpcNoAuth: true,
            nsHost: "127.0.0.1",
            minerAddress: minerAddr,
            logLevel: .debug
        )

        let fullNode = FullNode(config: config)
        self.node = fullNode

        nodeTask = Task {
            do {
                let components = try fullNode.initialize()
                try await fullNode.start(components: components)
            } catch {
                fbLog("[fistbump] fbd error: \(error)")
            }
        }
    }

    /// Gracefully stop the node and wait for full shutdown (DB locks released).
    func stop() async {
        node?.requestShutdown()
        // Await the node task — start(components:) runs its full graceful
        // shutdown sequence (close peers, close databases) before returning.
        await nodeTask?.value
        nodeTask = nil
        node = nil
    }

    /// Restart the node (picks up new settings).
    /// Ignores concurrent calls — only one restart at a time.
    func restart() async {
        guard !isRestarting else {
            fbLog("[fistbump] restart already in progress, skipping")
            return
        }
        isRestarting = true
        fbLog("[fistbump] restarting node for settings change...")
        await stop()
        fbLog("[fistbump] node stopped, restarting...")
        start()
        isRestarting = false
    }
}

// MARK: - Biometric Keychain

/// Biometric-protected Keychain storage for wallet passphrases.
private enum BiometricKeychain {

    static let service = "org.fistbump.wallet"

    static func isAvailable() -> Bool {
        let context = LAContext()
        var error: NSError?
        return context.canEvaluatePolicy(.deviceOwnerAuthenticationWithBiometrics, error: &error)
    }

    static func save(passphrase: String, wallet: String) -> Bool {
        delete(wallet: wallet)
        guard let data = passphrase.data(using: .utf8) else { return false }
        guard let access = SecAccessControlCreateWithFlags(
            nil,
            kSecAttrAccessibleWhenUnlockedThisDeviceOnly,
            .biometryCurrentSet,
            nil
        ) else { return false }
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: wallet,
            kSecValueData as String: data,
            kSecAttrAccessControl as String: access,
        ]
        return SecItemAdd(query as CFDictionary, nil) == errSecSuccess
    }

    static func load(wallet: String) -> String? {
        let context = LAContext()
        context.localizedReason = "Unlock wallet"
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: wallet,
            kSecReturnData as String: true,
            kSecUseAuthenticationContext as String: context,
        ]
        var result: AnyObject?
        let status = SecItemCopyMatching(query as CFDictionary, &result)
        guard status == errSecSuccess, let data = result as? Data else { return nil }
        return String(data: data, encoding: .utf8)
    }

    static func delete(wallet: String) {
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: wallet,
        ]
        SecItemDelete(query as CFDictionary)
    }
}

// MARK: - Biometric FFI

@_silgen_name("register_biometric_handlers")
private func _registerBiometricHandlers(
    _ available: @convention(c) () -> Bool,
    _ save: @convention(c) (UnsafePointer<CChar>?, UnsafePointer<CChar>?) -> Bool,
    _ load: @convention(c) (UnsafePointer<CChar>?) -> UnsafePointer<CChar>?,
    _ delete: @convention(c) (UnsafePointer<CChar>?) -> Void
)

private func biometricAvailable() -> Bool {
    BiometricKeychain.isAvailable()
}

private func biometricSave(
    _ walletPtr: UnsafePointer<CChar>?,
    _ passphrasePtr: UnsafePointer<CChar>?
) -> Bool {
    guard let walletPtr, let passphrasePtr,
          let wallet = String(validatingUTF8: walletPtr),
          let passphrase = String(validatingUTF8: passphrasePtr) else { return false }
    return BiometricKeychain.save(passphrase: passphrase, wallet: wallet)
}

private var _loadResultBuffer: [CChar] = []

private func biometricLoad(_ walletPtr: UnsafePointer<CChar>?) -> UnsafePointer<CChar>? {
    guard let walletPtr, let wallet = String(validatingUTF8: walletPtr) else { return nil }
    guard let passphrase = BiometricKeychain.load(wallet: wallet) else { return nil }
    _loadResultBuffer = Array(passphrase.utf8CString)
    return _loadResultBuffer.withUnsafeBufferPointer { $0.baseAddress }
}

private func biometricDelete(_ walletPtr: UnsafePointer<CChar>?) {
    guard let walletPtr, let wallet = String(validatingUTF8: walletPtr) else { return }
    BiometricKeychain.delete(wallet: wallet)
}

func registerBiometricFFI() {
    _registerBiometricHandlers(biometricAvailable, biometricSave, biometricLoad, biometricDelete)
}

// MARK: - Inline Browser (WKWebView)

@_silgen_name("register_browse_handler")
private func _registerBrowseHandler(
    _ show: @convention(c) (UnsafePointer<CChar>?, Double, Double, Double, Double, UInt8) -> Void,
    _ hide: @convention(c) () -> Void
)

private func browseShow(_ urlPtr: UnsafePointer<CChar>?, _ top: Double, _ left: Double, _ width: Double, _ height: Double, _ dark: UInt8) {
    guard let urlPtr = urlPtr,
          let urlStr = String(validatingUTF8: urlPtr),
          let url = URL(string: urlStr) else { return }
    let frame = CGRect(x: left, y: top, width: width, height: height)
    let isDark = dark != 0
    DispatchQueue.main.async {
        InlineBrowser.shared.navigate(to: url, frame: frame, dark: isDark)
    }
}

private func browseHide() {
    DispatchQueue.main.async {
        InlineBrowser.shared.hide()
    }
}

@_silgen_name("register_browse_error_handler")
private func _registerBrowseErrorHandler(
    _ f: @convention(c) (Double, Double, Double, Double, UInt8, UnsafePointer<CChar>?, UnsafePointer<CChar>?, UnsafePointer<CChar>?) -> Void
)

private func browseShowError(_ top: Double, _ left: Double, _ width: Double, _ height: Double, _ dark: UInt8, _ badgePtr: UnsafePointer<CChar>?, _ titlePtr: UnsafePointer<CChar>?, _ msgPtr: UnsafePointer<CChar>?) {
    let frame = CGRect(x: left, y: top, width: width, height: height)
    let isDark = dark != 0
    let badge = badgePtr.flatMap { String(validatingUTF8: $0) } ?? ""
    let title = titlePtr.flatMap { String(validatingUTF8: $0) } ?? ""
    let message = msgPtr.flatMap { String(validatingUTF8: $0) } ?? ""
    DispatchQueue.main.async {
        InlineBrowser.shared.showError(frame: frame, dark: isDark, badge: badge, title: title, message: message)
    }
}

func registerBrowseFFI() {
    _registerBrowseHandler(browseShow, browseHide)
    _registerBrowseErrorHandler(browseShowError)
}

/// Manages a native WKWebView overlaid on the Tauri webview content area.
/// Routes requests through the local DANE proxy and accepts the local CA cert.
class InlineBrowser: NSObject, WKNavigationDelegate {
    static let shared = InlineBrowser()

    private var webView: WKWebView?
    private var showingError = false

    /// Reference to the Tauri WKWebView so we can push URL updates into it.
    private weak var tauriWebView: WKWebView?

    /// The local CA certificate (for accepting DANE proxy's minted certs).
    private var caCert: SecCertificate?

    private func loadCACert() -> SecCertificate? {
        if let cached = caCert { return cached }
        let docs = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask).first!
        let caPath = docs.appendingPathComponent(".fistbump/proxy-ca.crt")
        guard let pem = try? String(contentsOf: caPath, encoding: .utf8) else { return nil }
        // Extract DER from PEM
        let lines = pem.components(separatedBy: "\n")
            .filter { !$0.hasPrefix("-----") && !$0.isEmpty }
        let base64 = lines.joined()
        guard let derData = Data(base64Encoded: base64) else { return nil }
        let cert = SecCertificateCreateWithData(nil, derData as CFData)
        caCert = cert
        return cert
    }

    func navigate(to url: URL, frame: CGRect, dark: Bool) {
        let bg = dark ? UIColor(red: 0x09/255, green: 0x09/255, blue: 0x0b/255, alpha: 1)
                      : UIColor(red: 0xf4/255, green: 0xf4/255, blue: 0xf5/255, alpha: 1)

        // Remove any existing webview — ensures no stale connections or cache.
        if let wv = webView {
            wv.removeFromSuperview()
            webView = nil
        }

        guard let window = UIApplication.shared.connectedScenes
            .compactMap({ $0 as? UIWindowScene })
            .first?.windows.first,
              let rootView = window.rootViewController?.view else { return }

        // Find the Tauri WKWebView to send URL updates to.
        tauriWebView = rootView.subviews.compactMap { $0 as? WKWebView }.first

        let config = WKWebViewConfiguration()
        config.allowsInlineMediaPlayback = true
        // Use non-persistent storage — no cached connections or data between navigations.
        config.websiteDataStore = .nonPersistent()

        // Route all traffic through the local SOCKS5 proxy (handles both HTTP and HTTPS).
        if #available(iOS 17.0, *) {
            let endpoint = NWEndpoint.hostPort(host: "127.0.0.1", port: 17351)
            let proxyConfig = ProxyConfiguration(socksv5Proxy: endpoint)
            config.websiteDataStore.proxyConfigurations = [proxyConfig]
        }

        let wv = WKWebView(frame: frame, configuration: config)
        wv.isOpaque = false
        wv.backgroundColor = bg
        wv.scrollView.backgroundColor = bg
        wv.scrollView.contentInsetAdjustmentBehavior = .never
        wv.underPageBackgroundColor = bg
        wv.navigationDelegate = self

        rootView.addSubview(wv)
        self.webView = wv
        showingError = false
        wv.load(URLRequest(url: url))
    }

    func hide() {
        webView?.removeFromSuperview()
        webView = nil
    }

    func showError(frame: CGRect, dark: Bool, badge: String, title: String, message: String) {
        // Remove existing webview
        if let wv = webView {
            wv.removeFromSuperview()
            webView = nil
        }

        guard let window = UIApplication.shared.connectedScenes
            .compactMap({ $0 as? UIWindowScene })
            .first?.windows.first,
              let rootView = window.rootViewController?.view else { return }

        let bg = dark ? "#09090b" : "#f4f4f5"
        let text = dark ? "#e4e4e7" : "#18181b"
        let muted = dark ? "#a1a1aa" : "#71717a"
        let badgeBg = dark ? "#7f1d1d" : "#fee2e2"
        let badgeText = dark ? "#fca5a5" : "#dc2626"
        let titleColor = dark ? "#f87171" : "#dc2626"

        let html = """
        <html><head><meta name="viewport" content="width=device-width,initial-scale=1">
        <style>
        html, body { height: 100%; margin: 0; }
        body { font-family: -apple-system, sans-serif; background: \(bg); color: \(text);
               display: flex; align-items: center; justify-content: center;
               text-align: center; padding: 24px; box-sizing: border-box; }
        .box { max-width: 360px; }
        h2 { color: \(titleColor); font-size: 18px; margin: 0 0 12px; }
        p { font-size: 14px; color: \(muted); line-height: 1.5; margin: 0; }
        .badge { display: inline-block; background: \(badgeBg); color: \(badgeText); font-size: 11px;
                 padding: 2px 8px; border-radius: 4px; margin-bottom: 16px; font-weight: 600; }
        </style></head><body><div class="box">
        <div class="badge">\(badge)</div>
        <h2>\(title)</h2>
        <p>\(message)</p>
        </div></body></html>
        """

        let config = WKWebViewConfiguration()
        let wv = WKWebView(frame: frame, configuration: config)
        wv.isOpaque = true
        let uiBg = dark ? UIColor(red: 0x09/255, green: 0x09/255, blue: 0x0b/255, alpha: 1)
                        : UIColor(red: 0xf4/255, green: 0xf4/255, blue: 0xf5/255, alpha: 1)
        wv.backgroundColor = uiBg
        wv.scrollView.backgroundColor = uiBg
        wv.scrollView.contentInsetAdjustmentBehavior = .never

        rootView.addSubview(wv)
        self.webView = wv
        wv.loadHTMLString(html, baseURL: URL(string: "fistbump://error"))
    }

    // MARK: - WKNavigationDelegate

    func webView(_ webView: WKWebView, didStartProvisionalNavigation navigation: WKNavigation!) {
        // Show a loading spinner via injected JS
        webView.evaluateJavaScript("""
            if (!document.getElementById('_fb_spinner')) {
                var d = document.createElement('div');
                d.id = '_fb_spinner';
                d.style.cssText = 'position:fixed;top:0;left:0;right:0;height:3px;z-index:99999;background:linear-gradient(90deg,transparent,#22d3ee,transparent);animation:_fbs 1s infinite';
                var s = document.createElement('style');
                s.textContent = '@keyframes _fbs{0%{transform:translateX(-100%)}100%{transform:translateX(100%)}}';
                document.head.appendChild(s);
                document.body.appendChild(d);
            }
        """)
    }

    func webView(_ webView: WKWebView, didCommit navigation: WKNavigation!) {
        // Remove loading spinner
        webView.evaluateJavaScript("var e=document.getElementById('_fb_spinner');if(e)e.remove();")

        guard let url = webView.url?.absoluteString,
              !url.hasPrefix("fistbump://"),
              !url.hasPrefix("about:") else { return }

        // Check if this is a proxy error page (has <title>fistbump-error</title>)
        let escaped = url.replacingOccurrences(of: "\\", with: "\\\\")
            .replacingOccurrences(of: "'", with: "\\'")
        webView.evaluateJavaScript("document.title") { result, _ in
            if let title = result as? String, title == "fistbump-error" { return }
            self.tauriWebView?.evaluateJavaScript(
                "if(window._onBrowseURL)window._onBrowseURL('\(escaped)')"
            )
        }
    }

    /// Show an error page when navigation fails (e.g., DANE validation failure, no response, SSL error).
    func webView(_ webView: WKWebView, didFailProvisionalNavigation navigation: WKNavigation!, withError error: Error) {
        handleNavigationError(webView: webView, error: error)
    }

    func webView(_ webView: WKWebView, didFail navigation: WKNavigation!, withError error: Error) {
        handleNavigationError(webView: webView, error: error)
    }

    private func handleNavigationError(webView: WKWebView, error: Error) {
        guard !showingError else { return }
        showingError = true
        let host = webView.url?.host ?? "this site"
        let nsError = error as NSError
        fbLog("[fistbump] browser error: code=\(nsError.code) domain=\(nsError.domain) host=\(host)")

        let badge: String
        let title: String
        let message: String

        switch nsError.code {
        case NSURLErrorTimedOut:
            badge = "CONNECTION FAILED"
            title = "No Response"
            message = "The server for <strong>\(host)</strong> did not respond."
        case NSURLErrorServerCertificateUntrusted,
             NSURLErrorServerCertificateHasUnknownRoot,
             NSURLErrorServerCertificateHasBadDate,
             NSURLErrorServerCertificateNotYetValid:
            badge = "CERTIFICATE ERROR"
            title = "Certificate Not Valid"
            message = "The SSL certificate for <strong>\(host)</strong> is not valid. The connection has been blocked."
        case NSURLErrorSecureConnectionFailed:
            badge = "SSL ERROR"
            title = "Secure Connection Failed"
            message = "Could not establish a secure connection to <strong>\(host)</strong>."
        case NSURLErrorCannotFindHost:
            badge = "NOT FOUND"
            title = "Server Not Found"
            message = "The server for <strong>\(host)</strong> could not be found."
        case NSURLErrorCannotConnectToHost:
            badge = "CONNECTION FAILED"
            title = "Cannot Connect"
            message = "Could not connect to the server for <strong>\(host)</strong>."
        default:
            // Fistbump names going through the proxy — DANE failure drops the connection
            badge = "DANE VALIDATION FAILED"
            title = "Connection Not Secure"
            message = "The certificate presented by <strong>\(host)</strong> does not match its on-chain TLSA record. The connection has been blocked."
        }

        let frame = webView.frame
        showError(frame: frame, dark: true, badge: badge, title: title, message: message)
    }

    /// Accept the local DANE proxy's CA certificate for TLS connections.
    func webView(
        _ webView: WKWebView,
        didReceive challenge: URLAuthenticationChallenge,
        completionHandler: @escaping (URLSession.AuthChallengeDisposition, URLCredential?) -> Void
    ) {
        let host = challenge.protectionSpace.host
        fbLog("[fistbump] TLS challenge for \(host) method=\(challenge.protectionSpace.authenticationMethod)")

        guard challenge.protectionSpace.authenticationMethod == NSURLAuthenticationMethodServerTrust,
              let serverTrust = challenge.protectionSpace.serverTrust else {
            completionHandler(.performDefaultHandling, nil)
            return
        }

        // Add our local CA cert to the trust evaluation.
        if let ca = loadCACert() {
            SecTrustSetAnchorCertificates(serverTrust, [ca] as CFArray)
            SecTrustSetAnchorCertificatesOnly(serverTrust, false)
        }

        var error: CFError?
        if SecTrustEvaluateWithError(serverTrust, &error) {
            fbLog("[fistbump] TLS trust OK for \(host)")
            completionHandler(.useCredential, URLCredential(trust: serverTrust))
        } else {
            fbLog("[fistbump] TLS trust FAILED for \(host): \(error?.localizedDescription ?? "unknown")")
            completionHandler(.cancelAuthenticationChallenge, nil)
        }
    }
}

// MARK: - Native QR Scanner

import AVFoundation

@_silgen_name("register_qr_scan_handler")
private func _registerQRScanHandler(
    _ start: @convention(c) () -> Void,
    _ stop: @convention(c) () -> Void
)

/// Called from Rust when a QR code is scanned (or cancelled).
@_silgen_name("qr_scan_result")
private func _qrScanResult(_ ptr: UnsafePointer<CChar>?)

private func qrScanStart() {
    DispatchQueue.main.async {
        QRScanner.shared.start()
    }
}

private func qrScanStop() {
    DispatchQueue.main.async {
        QRScanner.shared.stop()
    }
}

func registerQRScanFFI() {
    _registerQRScanHandler(qrScanStart, qrScanStop)
}

class QRScanner: NSObject, AVCaptureMetadataOutputObjectsDelegate {
    static let shared = QRScanner()

    private var session: AVCaptureSession?
    private var previewLayer: AVCaptureVideoPreviewLayer?
    private var overlayView: UIView?

    func start() {
        guard session == nil else { return }

        let session = AVCaptureSession()
        guard let device = AVCaptureDevice.default(for: .video),
              let input = try? AVCaptureDeviceInput(device: device) else {
            // Send empty result to signal failure
            "".withCString { _qrScanResult($0) }
            return
        }

        session.addInput(input)

        let output = AVCaptureMetadataOutput()
        session.addOutput(output)
        output.setMetadataObjectsDelegate(self, queue: .main)
        output.metadataObjectTypes = [.qr]
        fbLog("[QR] session configured, available types: \(output.availableMetadataObjectTypes)")

        self.session = session

        guard let window = UIApplication.shared.connectedScenes
            .compactMap({ $0 as? UIWindowScene })
            .first?.windows.first,
              let rootView = window.rootViewController?.view else {
            "".withCString { _qrScanResult($0) }
            return
        }

        // Full-screen overlay
        let overlay = UIView(frame: rootView.bounds)
        overlay.backgroundColor = .black
        rootView.addSubview(overlay)
        self.overlayView = overlay

        let preview = AVCaptureVideoPreviewLayer(session: session)
        preview.videoGravity = .resizeAspectFill
        preview.frame = overlay.bounds
        overlay.layer.addSublayer(preview)
        self.previewLayer = preview

        // Cancel button
        let cancelBtn = UIButton(type: .system)
        cancelBtn.setTitle("Cancel", for: .normal)
        cancelBtn.titleLabel?.font = .systemFont(ofSize: 17, weight: .medium)
        cancelBtn.setTitleColor(.white, for: .normal)
        cancelBtn.addTarget(self, action: #selector(cancelTapped), for: .touchUpInside)
        cancelBtn.translatesAutoresizingMaskIntoConstraints = false
        overlay.addSubview(cancelBtn)
        NSLayoutConstraint.activate([
            cancelBtn.centerXAnchor.constraint(equalTo: overlay.centerXAnchor),
            cancelBtn.bottomAnchor.constraint(equalTo: overlay.safeAreaLayoutGuide.bottomAnchor, constant: -24),
        ])

        // Hint label
        let hint = UILabel()
        hint.text = "Point your camera at a recovery QR code"
        hint.textColor = .white
        hint.font = .systemFont(ofSize: 15)
        hint.translatesAutoresizingMaskIntoConstraints = false
        overlay.addSubview(hint)
        NSLayoutConstraint.activate([
            hint.centerXAnchor.constraint(equalTo: overlay.centerXAnchor),
            hint.topAnchor.constraint(equalTo: overlay.safeAreaLayoutGuide.topAnchor, constant: 24),
        ])

        DispatchQueue.global(qos: .userInitiated).async {
            session.startRunning()
            fbLog("[QR] session running: \(session.isRunning)")
        }
    }

    func stop() {
        session?.stopRunning()
        session = nil
        previewLayer?.removeFromSuperlayer()
        previewLayer = nil
        overlayView?.removeFromSuperview()
        overlayView = nil
    }

    @objc private func cancelTapped() {
        stop()
        "".withCString { _qrScanResult($0) }
    }

    func metadataOutput(_ output: AVCaptureMetadataOutput,
                        didOutput metadataObjects: [AVMetadataObject],
                        from connection: AVCaptureConnection) {
        fbLog("[QR] metadataOutput called, objects: \(metadataObjects.count)")
        for obj in metadataObjects {
            fbLog("[QR] type: \(obj.type.rawValue)")
            if let readable = obj as? AVMetadataMachineReadableCodeObject {
                fbLog("[QR] stringValue: \(readable.stringValue ?? "nil")")
            }
        }
        guard let obj = metadataObjects.first as? AVMetadataMachineReadableCodeObject,
              obj.type == .qr,
              let value = obj.stringValue, !value.isEmpty else { return }
        fbLog("[QR] scanned: \(value.prefix(30))...")
        stop()
        value.withCString { _qrScanResult($0) }
    }
}
