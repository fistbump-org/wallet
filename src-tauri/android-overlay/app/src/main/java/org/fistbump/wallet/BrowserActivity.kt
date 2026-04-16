package org.fistbump.wallet

import android.annotation.SuppressLint
import android.content.res.ColorStateList
import android.graphics.Color
import android.graphics.drawable.GradientDrawable
import android.graphics.drawable.RippleDrawable
import android.net.http.SslCertificate
import android.net.http.SslError
import android.os.Bundle
import android.text.InputType
import android.util.TypedValue
import android.view.Gravity
import android.view.KeyEvent
import android.view.View
import android.view.ViewGroup
import android.view.inputmethod.EditorInfo
import android.view.inputmethod.InputMethodManager
import android.webkit.SslErrorHandler
import android.webkit.WebChromeClient
import android.webkit.WebResourceError
import android.webkit.WebResourceRequest
import android.webkit.WebView
import android.webkit.WebViewClient
import android.widget.EditText
import android.widget.FrameLayout
import android.widget.ImageButton
import android.widget.LinearLayout
import android.widget.ProgressBar
import androidx.activity.SystemBarStyle
import androidx.activity.enableEdgeToEdge
import androidx.appcompat.app.AppCompatActivity
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat
import androidx.webkit.ProxyConfig
import androidx.webkit.ProxyController
import androidx.webkit.WebViewFeature
import java.io.File
import java.security.cert.CertificateFactory
import java.security.cert.X509Certificate

/**
 * Full-screen browser launched as a modal Activity. Routes all traffic
 * through the local SOCKS5 proxy at 127.0.0.1:17351 (Fistbump name
 * resolution + DANE validation). Self-contained: toolbar with back/
 * URL bar/close, progress indicator, WebView. No overlay positioning.
 */
class BrowserActivity : AppCompatActivity() {

    companion object {
        const val EXTRA_URL = "url"
        const val EXTRA_DARK = "dark"
    }

    private lateinit var webView: WebView
    private lateinit var urlInput: EditText
    private lateinit var progressBar: ProgressBar
    private lateinit var backButton: ImageButton
    private lateinit var reloadButton: ImageButton
    private var dark: Boolean = true
    private var localCaCert: X509Certificate? = null

    // Wallet palette — matches ui/css/style.css dark theme
    private val bgColor get() = if (dark) 0xFF09090B.toInt() else 0xFFF4F4F5.toInt()
    private val borderColor get() = if (dark) 0xFF27272A.toInt() else 0xFFE4E4E7.toInt()
    private val inputBgColor get() = if (dark) 0xFF18181B.toInt() else 0xFFFFFFFF.toInt()
    private val textColor get() = if (dark) 0xFFE4E4E7.toInt() else 0xFF18181B.toInt()
    private val mutedColor get() = if (dark) 0xFFA1A1AA.toInt() else 0xFF71717A.toInt()
    private val dimColor get() = if (dark) 0xFF52525B.toInt() else 0xFFA1A1AA.toInt()
    private val accentColor = 0xFF22D3EE.toInt()

    override fun onCreate(savedInstanceState: Bundle?) {
        dark = intent.getBooleanExtra(EXTRA_DARK, true)
        // Force the system bar icon color to match our toolbar's theme.
        // Auto follows the system UI mode, which can mismatch our toolbar
        // if e.g. the phone is in light mode but the wallet runs dark —
        // that would render dark status bar icons on a dark toolbar,
        // making them invisible.
        val barStyle = if (dark) {
            SystemBarStyle.dark(Color.TRANSPARENT)
        } else {
            SystemBarStyle.light(Color.TRANSPARENT, Color.TRANSPARENT)
        }
        enableEdgeToEdge(statusBarStyle = barStyle, navigationBarStyle = barStyle)
        super.onCreate(savedInstanceState)
        val url = intent.getStringExtra(EXTRA_URL)

        setContentView(buildLayout())
        applyInsets()
        configureWebView()
        applyProxyThenLoad(url)
        if (url.isNullOrBlank()) urlInput.requestFocus()
    }

    private fun buildLayout(): View {
        val root = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setBackgroundColor(bgColor)
            fitsSystemWindows = false
        }

        // ── Toolbar ── (blends with bg, separated only by a thin bottom border)
        val toolbar = LinearLayout(this).apply {
            orientation = LinearLayout.HORIZONTAL
            setBackgroundColor(bgColor)
            gravity = Gravity.CENTER_VERTICAL
            setPadding(dp(8), dp(8), dp(8), dp(8))
        }

        backButton = makeIconButton(R.drawable.ic_arrow_left, "Back") {
            if (webView.canGoBack()) webView.goBack()
        }.apply { isEnabled = false }
        toolbar.addView(backButton)

        urlInput = EditText(this).apply {
            hint = "Enter address..."
            setHintTextColor(dimColor)
            setTextColor(textColor)
            setTextSize(TypedValue.COMPLEX_UNIT_SP, 15f)
            inputType = InputType.TYPE_TEXT_VARIATION_URI or InputType.TYPE_CLASS_TEXT
            imeOptions = EditorInfo.IME_ACTION_GO
            setSingleLine(true)
            setSelectAllOnFocus(true)
            setPadding(dp(14), dp(8), dp(14), dp(8))
            background = GradientDrawable().apply {
                shape = GradientDrawable.RECTANGLE
                cornerRadius = dp(8).toFloat()
                setColor(inputBgColor)
                setStroke(1, borderColor)
            }
            setOnEditorActionListener { _, actionId, event ->
                val isEnter = actionId == EditorInfo.IME_ACTION_GO ||
                    event?.keyCode == KeyEvent.KEYCODE_ENTER
                if (isEnter) { navigate(text.toString()); true } else false
            }
            layoutParams = LinearLayout.LayoutParams(
                0, dp(38), 1f
            ).apply { marginStart = dp(6); marginEnd = dp(6) }
        }
        toolbar.addView(urlInput)

        reloadButton = makeIconButton(R.drawable.ic_reload, "Reload") {
            webView.reload()
        }
        toolbar.addView(reloadButton)

        val closeButton = makeIconButton(R.drawable.ic_close, "Close") {
            finish()
        }
        toolbar.addView(closeButton)

        // Wrap toolbar so the progress bar can overlay its bottom edge without
        // pushing the webview down.
        progressBar = ProgressBar(this, null, android.R.attr.progressBarStyleHorizontal).apply {
            max = 100
            progress = 0
            visibility = View.GONE
            progressTintList = ColorStateList.valueOf(accentColor)
            progressBackgroundTintList = ColorStateList.valueOf(Color.TRANSPARENT)
        }
        val toolbarWrap = FrameLayout(this).apply {
            addView(
                toolbar,
                FrameLayout.LayoutParams(
                    FrameLayout.LayoutParams.MATCH_PARENT,
                    FrameLayout.LayoutParams.WRAP_CONTENT
                )
            )
            addView(
                progressBar,
                FrameLayout.LayoutParams(
                    FrameLayout.LayoutParams.MATCH_PARENT,
                    dp(2),
                    Gravity.BOTTOM
                )
            )
        }
        root.addView(
            toolbarWrap,
            LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.MATCH_PARENT,
                LinearLayout.LayoutParams.WRAP_CONTENT
            )
        )

        // Thin bottom border below the toolbar
        val divider = View(this).apply { setBackgroundColor(borderColor) }
        root.addView(divider, LinearLayout.LayoutParams(LinearLayout.LayoutParams.MATCH_PARENT, 1))

        // ── WebView ──
        webView = WebView(this).apply {
            setBackgroundColor(bgColor)
        }
        root.addView(
            webView,
            LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.MATCH_PARENT,
                0,
                1f
            )
        )

        return root
    }

    private fun makeIconButton(
        drawableRes: Int,
        desc: String,
        onClick: () -> Unit
    ): ImageButton {
        val size = dp(40)
        val rippleMask = GradientDrawable().apply {
            shape = GradientDrawable.OVAL
            setColor(Color.WHITE)
        }
        return ImageButton(this).apply {
            setImageResource(drawableRes)
            imageTintList = ColorStateList.valueOf(mutedColor)
            background = RippleDrawable(
                ColorStateList.valueOf(
                    if (dark) 0x22FFFFFF.toInt() else 0x22000000.toInt()
                ),
                null,
                rippleMask
            )
            contentDescription = desc
            scaleType = android.widget.ImageView.ScaleType.CENTER_INSIDE
            setPadding(dp(8), dp(8), dp(8), dp(8))
            layoutParams = LinearLayout.LayoutParams(size, size)
            setOnClickListener { onClick() }
        }
    }

    private fun applyInsets() {
        val root = findViewById<ViewGroup>(android.R.id.content).getChildAt(0) as LinearLayout
        ViewCompat.setOnApplyWindowInsetsListener(root) { _, insets ->
            val bars = insets.getInsets(
                WindowInsetsCompat.Type.systemBars() or WindowInsetsCompat.Type.displayCutout()
            )
            // Toolbar is the first child (FrameLayout wrap), its first child is the
            // actual toolbar LinearLayout that carries the padding.
            val toolbar = (root.getChildAt(0) as ViewGroup).getChildAt(0)
            toolbar.setPadding(
                dp(8) + bars.left,
                dp(8) + bars.top,
                dp(8) + bars.right,
                dp(8)
            )
            webView.setPadding(bars.left, 0, bars.right, bars.bottom)
            insets
        }
    }

    @SuppressLint("SetJavaScriptEnabled")
    private fun configureWebView() {
        webView.settings.apply {
            javaScriptEnabled = true
            domStorageEnabled = true
            loadWithOverviewMode = true
            useWideViewPort = true
            mediaPlaybackRequiresUserGesture = false
        }
        webView.webChromeClient = object : WebChromeClient() {
            override fun onProgressChanged(view: WebView?, newProgress: Int) {
                progressBar.progress = newProgress
                progressBar.visibility = if (newProgress in 1..99) View.VISIBLE else View.GONE
            }
        }
        webView.webViewClient = object : WebViewClient() {
            override fun onPageStarted(view: WebView?, url: String?, favicon: android.graphics.Bitmap?) {
                if (!urlInput.hasFocus() && url != null && !url.startsWith("fistbump://")) {
                    urlInput.setText(stripScheme(url))
                }
                backButton.isEnabled = view?.canGoBack() == true
            }

            override fun onPageFinished(view: WebView?, url: String?) {
                backButton.isEnabled = view?.canGoBack() == true
            }

            override fun onReceivedSslError(view: WebView?, handler: SslErrorHandler?, error: SslError?) {
                // Proxy has already DANE-validated upstream certs and is presenting
                // our local CA-minted cert. Only accept errors for certs actually
                // signed by our local CA — never blanket-proceed.
                if (error != null && isSignedByLocalCA(error.certificate)) {
                    handler?.proceed()
                } else {
                    handler?.cancel()
                }
            }

            override fun onReceivedError(view: WebView?, request: WebResourceRequest?, error: WebResourceError?) {
                if (request?.isForMainFrame != true) return
                showError(request?.url?.host ?: "this site", error?.errorCode ?: 0)
            }
        }
    }

    private fun applyProxyThenLoad(initialUrl: String?) {
        val load = { if (!initialUrl.isNullOrBlank()) webView.loadUrl(initialUrl) }
        if (!WebViewFeature.isFeatureSupported(WebViewFeature.PROXY_OVERRIDE)) {
            load()
            return
        }
        val proxyConfig = ProxyConfig.Builder()
            .addProxyRule("socks5://127.0.0.1:17351")
            .addBypassRule("tauri.localhost")
            .addBypassRule("localhost")
            .addBypassRule("127.0.0.1")
            .build()
        ProxyController.getInstance().setProxyOverride(proxyConfig, { it.run() }, { load() })
    }

    private fun navigate(input: String) {
        var url = input.trim().trimEnd('.').lowercase()
        if (url.isEmpty()) return
        if (!url.startsWith("http://") && !url.startsWith("https://")) url = "http://$url"
        urlInput.clearFocus()
        (getSystemService(INPUT_METHOD_SERVICE) as InputMethodManager)
            .hideSoftInputFromWindow(urlInput.windowToken, 0)
        webView.loadUrl(url)
    }

    private fun loadLocalCA(): X509Certificate? {
        localCaCert?.let { return it }
        val caFile = File(filesDir, ".fistbump/proxy-ca.crt")
        if (!caFile.exists()) return null
        return try {
            val factory = CertificateFactory.getInstance("X.509")
            val cert = caFile.inputStream().use {
                factory.generateCertificate(it) as X509Certificate
            }
            localCaCert = cert
            cert
        } catch (_: Exception) {
            null
        }
    }

    @Suppress("DEPRECATION")
    private fun isSignedByLocalCA(serverCert: SslCertificate?): Boolean {
        if (serverCert == null) return false
        val ca = loadLocalCA() ?: return false
        val bundle = SslCertificate.saveState(serverCert) ?: return false
        val x509 = bundle.getSerializable("x509-certificate") as? X509Certificate ?: return false
        return try {
            x509.verify(ca.publicKey)
            true
        } catch (_: Exception) {
            false
        }
    }

    private fun showError(host: String, code: Int) {
        val (badge, title, message) = when (code) {
            WebViewClient.ERROR_TIMEOUT ->
                Triple("CONNECTION FAILED", "No Response",
                    "The server for <strong>$host</strong> did not respond.")
            WebViewClient.ERROR_HOST_LOOKUP ->
                Triple("NOT FOUND", "Server Not Found",
                    "The server for <strong>$host</strong> could not be found.")
            WebViewClient.ERROR_CONNECT ->
                Triple("CONNECTION FAILED", "Cannot Connect",
                    "Could not connect to the server for <strong>$host</strong>.")
            WebViewClient.ERROR_FAILED_SSL_HANDSHAKE ->
                Triple("CERTIFICATE ERROR", "Connection Not Secure",
                    "The SSL certificate for <strong>$host</strong> is not valid. The connection has been blocked.")
            else ->
                Triple("DANE VALIDATION FAILED", "Connection Not Secure",
                    "The certificate presented by <strong>$host</strong> does not match its on-chain TLSA record. The connection has been blocked.")
        }
        webView.stopLoading()
        webView.loadDataWithBaseURL(
            "fistbump://error", errorHtml(badge, title, message),
            "text/html", "utf-8", "fistbump://error"
        )
    }

    private fun errorHtml(badge: String, title: String, message: String): String {
        val bg = if (dark) "#09090b" else "#f4f4f5"
        val text = if (dark) "#e4e4e7" else "#18181b"
        val muted = if (dark) "#a1a1aa" else "#71717a"
        val badgeBg = if (dark) "#7f1d1d" else "#fee2e2"
        val badgeText = if (dark) "#fca5a5" else "#dc2626"
        val titleColor = if (dark) "#f87171" else "#dc2626"
        return """<html><head><meta name="viewport" content="width=device-width,initial-scale=1"><style>
            html,body{height:100%;margin:0}
            body{font-family:sans-serif;background:$bg;color:$text;
                 display:flex;align-items:center;justify-content:center;
                 text-align:center;padding:24px;box-sizing:border-box}
            .box{max-width:360px}
            h2{color:$titleColor;font-size:18px;margin:0 0 12px}
            p{font-size:14px;color:$muted;line-height:1.5;margin:0}
            .badge{display:inline-block;background:$badgeBg;color:$badgeText;font-size:11px;
                   padding:2px 8px;border-radius:4px;margin-bottom:16px;font-weight:600}
            </style></head><body><div class="box">
            <div class="badge">$badge</div><h2>$title</h2><p>$message</p>
            </div></body></html>""".trimIndent()
    }

    override fun onBackPressed() {
        if (webView.canGoBack()) webView.goBack() else super.onBackPressed()
    }

    override fun onDestroy() {
        (webView.parent as? ViewGroup)?.removeView(webView)
        webView.destroy()
        super.onDestroy()
    }

    private fun stripScheme(url: String): String =
        url.removePrefix("https://").removePrefix("http://").removeSuffix("/")

    private fun dp(value: Int): Int =
        (value * resources.displayMetrics.density).toInt()
}
