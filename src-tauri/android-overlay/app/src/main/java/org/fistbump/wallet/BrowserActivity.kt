package org.fistbump.wallet

import android.annotation.SuppressLint
import android.content.res.ColorStateList
import android.graphics.Color
import android.graphics.drawable.Drawable
import android.graphics.drawable.GradientDrawable
import android.graphics.drawable.LayerDrawable
import android.graphics.drawable.RippleDrawable
import android.net.http.SslError
import android.os.Bundle
import android.text.Editable
import android.text.InputType
import android.text.TextWatcher
import android.util.TypedValue
import android.view.Gravity
import android.view.KeyEvent
import android.view.MotionEvent
import android.view.View
import android.view.ViewGroup
import androidx.core.content.ContextCompat
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
    private lateinit var reloadButton: ImageButton
    private var dark: Boolean = true

    // Wallet palette — matches ui/css/style.css dark theme
    private val bgColor get() = if (dark) 0xFF09090B.toInt() else 0xFFF4F4F5.toInt()
    private val borderColor get() = if (dark) 0xFF27272A.toInt() else 0xFFE4E4E7.toInt()
    private val inputBgColor get() = if (dark) 0xFF18181B.toInt() else 0xFFFFFFFF.toInt()
    private val textColor get() = if (dark) 0xFFE4E4E7.toInt() else 0xFF18181B.toInt()
    private val mutedColor get() = if (dark) 0xFFA1A1AA.toInt() else 0xFF71717A.toInt()
    private val dimColor get() = if (dark) 0xFF52525B.toInt() else 0xFFA1A1AA.toInt()
    private val accentColor = 0xFF22D3EE.toInt()

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        dark = intent.getBooleanExtra(EXTRA_DARK, true)
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

        // Matches ui/css/style.css `.mobile-browser-input-wrap input`:
        // height 36dp, padding 0 28 0 10, radius 6dp, font 15sp,
        // #18181b fill, 1dp #27272a border. A 3dp cyan glow ring appears
        // on focus (LayerDrawable outer ring, analogous to the web's
        // box-shadow: 0 0 0 3px rgba(87,199,237,0.1)).
        urlInput = EditText(this).apply {
            hint = "Enter address..."
            setHintTextColor(dimColor)
            setTextColor(textColor)
            setTextSize(TypedValue.COMPLEX_UNIT_SP, 15f)
            inputType = InputType.TYPE_TEXT_VARIATION_URI or InputType.TYPE_CLASS_TEXT
            imeOptions = EditorInfo.IME_ACTION_GO
            setSingleLine(true)
            setSelectAllOnFocus(true)
            // Left/right padding accounts for the glow ring inset (dp(3))
            // plus the CSS spec's 10dp text inset and 6dp clear-button inset.
            // The compound drawable (20dp X) is drawn at the right edge of
            // the padding area, so paddingRight = 3dp ring + 6dp inset = 9dp.
            setPadding(dp(13), dp(3), dp(9), dp(3))
            compoundDrawablePadding = dp(8)
            background = makeInputBackground(focused = false)
            setOnFocusChangeListener { _, hasFocus ->
                background = makeInputBackground(focused = hasFocus)
            }
            setOnEditorActionListener { _, actionId, event ->
                val isEnter = actionId == EditorInfo.IME_ACTION_GO ||
                    event?.keyCode == KeyEvent.KEYCODE_ENTER
                if (isEnter) { navigate(text.toString()); true } else false
            }
            layoutParams = LinearLayout.LayoutParams(
                0, dp(42), 1f  // 36dp input + 3dp glow ring on top & bottom
            ).apply { marginStart = dp(6); marginEnd = dp(6) }
        }
        attachClearButton(urlInput)
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

    // Reserves a 3dp-wide transparent region on each side for the
    // focus glow ring, so that both the unfocused and focused states
    // render the inner input at identical coordinates — no text reflow.
    private fun makeInputBackground(focused: Boolean): Drawable {
        val glowAlpha = 0x1A22D3EE.toInt()  // cyan at ~10% alpha
        val focusBorder = 0xFFA1A1AA.toInt() // --border-focus from the CSS

        val inner = GradientDrawable().apply {
            shape = GradientDrawable.RECTANGLE
            cornerRadius = dp(6).toFloat()
            setColor(inputBgColor)
            setStroke(1, if (focused) focusBorder else borderColor)
        }
        val outer = GradientDrawable().apply {
            shape = GradientDrawable.RECTANGLE
            cornerRadius = dp(9).toFloat()
            setColor(if (focused) glowAlpha else Color.TRANSPARENT)
        }
        val layers = LayerDrawable(arrayOf(outer, inner))
        layers.setLayerInset(1, dp(3), dp(3), dp(3), dp(3))
        return layers
    }

    @SuppressLint("ClickableViewAccessibility")
    private fun attachClearButton(input: EditText) {
        val size = dp(20)
        val clear = ContextCompat.getDrawable(this, R.drawable.ic_close)?.mutate()?.apply {
            setBounds(0, 0, size, size)
            setTint(mutedColor)
        }
        fun update() {
            input.setCompoundDrawables(null, null, if (input.text.isNullOrEmpty()) null else clear, null)
        }
        input.addTextChangedListener(object : TextWatcher {
            override fun beforeTextChanged(s: CharSequence?, a: Int, b: Int, c: Int) {}
            override fun onTextChanged(s: CharSequence?, a: Int, b: Int, c: Int) {}
            override fun afterTextChanged(s: Editable?) { update() }
        })
        input.setOnTouchListener { _, event ->
            if (event.action == MotionEvent.ACTION_UP) {
                val drawable = input.compoundDrawables[2] ?: return@setOnTouchListener false
                val right = input.width - input.paddingEnd
                val left = right - drawable.bounds.width()
                if (event.x >= left - dp(8) && event.x <= right + dp(8)) {
                    input.setText("")
                    input.requestFocus()
                    (getSystemService(INPUT_METHOD_SERVICE) as InputMethodManager)
                        .showSoftInput(input, InputMethodManager.SHOW_IMPLICIT)
                    return@setOnTouchListener true
                }
            }
            false
        }
        update()
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
