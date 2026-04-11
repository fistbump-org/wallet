package org.fistbump.wallet

import android.annotation.SuppressLint
import android.content.Intent
import android.graphics.Color
import android.net.http.SslError
import android.os.Bundle
import android.text.InputType
import android.util.TypedValue
import android.view.Gravity
import android.view.KeyEvent
import android.view.View
import android.view.ViewGroup
import android.view.WindowManager
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
import androidx.activity.enableEdgeToEdge
import androidx.appcompat.app.AppCompatActivity
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat
import androidx.webkit.ProxyConfig
import androidx.webkit.ProxyController
import androidx.webkit.WebViewFeature

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
    private var dark: Boolean = true

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        dark = intent.getBooleanExtra(EXTRA_DARK, true)
        val url = intent.getStringExtra(EXTRA_URL)

        setContentView(buildLayout())
        applyInsets()
        configureWebView()
        applyProxyThenLoad(url)
    }

    private fun buildLayout(): View {
        val bg = if (dark) Color.parseColor("#09090b") else Color.parseColor("#f4f4f5")
        val bgToolbar = if (dark) Color.parseColor("#18181b") else Color.parseColor("#ffffff")
        val border = if (dark) Color.parseColor("#27272a") else Color.parseColor("#e4e4e7")
        val textColor = if (dark) Color.parseColor("#e4e4e7") else Color.parseColor("#18181b")
        val mutedColor = if (dark) Color.parseColor("#71717a") else Color.parseColor("#a1a1aa")
        val accentColor = Color.parseColor("#22d3ee")

        val root = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setBackgroundColor(bg)
            fitsSystemWindows = false
        }

        // ── Toolbar row ──────────────────────────────────────────
        val toolbar = LinearLayout(this).apply {
            orientation = LinearLayout.HORIZONTAL
            setBackgroundColor(bgToolbar)
            gravity = Gravity.CENTER_VERTICAL
            setPadding(dp(8), dp(6), dp(8), dp(6))
        }

        backButton = ImageButton(this).apply {
            setImageResource(android.R.drawable.ic_media_previous)
            background = null
            setColorFilter(mutedColor)
            contentDescription = "Back"
            isEnabled = false
            setOnClickListener { if (webView.canGoBack()) webView.goBack() }
            layoutParams = LinearLayout.LayoutParams(dp(40), dp(40))
        }
        toolbar.addView(backButton)

        urlInput = EditText(this).apply {
            hint = "Enter address..."
            setHintTextColor(mutedColor)
            setTextColor(textColor)
            setTextSize(TypedValue.COMPLEX_UNIT_SP, 15f)
            inputType = InputType.TYPE_TEXT_VARIATION_URI or InputType.TYPE_CLASS_TEXT
            imeOptions = EditorInfo.IME_ACTION_GO
            background = null
            setSingleLine(true)
            setPadding(dp(12), dp(8), dp(12), dp(8))
            isSelectAllOnFocus = true
            setOnEditorActionListener { _, actionId, event ->
                val isEnter = actionId == EditorInfo.IME_ACTION_GO ||
                    event?.keyCode == KeyEvent.KEYCODE_ENTER
                if (isEnter) { navigate(text.toString()); true } else false
            }
            layoutParams = LinearLayout.LayoutParams(
                0, LinearLayout.LayoutParams.WRAP_CONTENT, 1f
            ).apply { marginStart = dp(4); marginEnd = dp(4) }
        }
        toolbar.addView(urlInput)

        val closeButton = ImageButton(this).apply {
            setImageResource(android.R.drawable.ic_menu_close_clear_cancel)
            background = null
            setColorFilter(mutedColor)
            contentDescription = "Close"
            setOnClickListener { finish() }
            layoutParams = LinearLayout.LayoutParams(dp(40), dp(40))
        }
        toolbar.addView(closeButton)

        root.addView(
            toolbar,
            LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.MATCH_PARENT,
                LinearLayout.LayoutParams.WRAP_CONTENT
            )
        )

        // ── Progress bar (thin line under toolbar) ──────────────
        progressBar = ProgressBar(this, null, android.R.attr.progressBarStyleHorizontal).apply {
            max = 100
            progress = 0
            visibility = View.GONE
            progressTintList = android.content.res.ColorStateList.valueOf(accentColor)
        }
        root.addView(
            progressBar,
            LinearLayout.LayoutParams(LinearLayout.LayoutParams.MATCH_PARENT, dp(2))
        )

        // Toolbar bottom border
        val divider = View(this).apply { setBackgroundColor(border) }
        root.addView(divider, LinearLayout.LayoutParams(LinearLayout.LayoutParams.MATCH_PARENT, 1))

        // ── WebView ─────────────────────────────────────────────
        webView = WebView(this).apply {
            setBackgroundColor(bg)
        }
        root.addView(
            webView,
            LinearLayout.LayoutParams(0, 0, 1f).apply {
                width = LinearLayout.LayoutParams.MATCH_PARENT
                height = 0
            }
        )

        return root
    }

    private fun applyInsets() {
        // Pad the toolbar for the status bar, and the webview for the nav bar.
        val root = findViewById<ViewGroup>(android.R.id.content).getChildAt(0) as LinearLayout
        ViewCompat.setOnApplyWindowInsetsListener(root) { _, insets ->
            val bars = insets.getInsets(
                WindowInsetsCompat.Type.systemBars() or WindowInsetsCompat.Type.displayCutout()
            )
            val toolbar = root.getChildAt(0)
            toolbar.setPadding(
                toolbar.paddingLeft + bars.left,
                dp(6) + bars.top,
                toolbar.paddingRight + bars.right,
                dp(6)
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
                // our local CA-minted cert — always accept.
                handler?.proceed()
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
