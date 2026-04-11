package org.fistbump.wallet

import android.graphics.Color
import android.net.http.SslError
import android.view.ViewGroup
import android.webkit.SslErrorHandler
import android.webkit.WebChromeClient
import android.webkit.WebView
import android.webkit.WebViewClient
import android.widget.FrameLayout
import androidx.fragment.app.FragmentActivity
import androidx.webkit.ProxyConfig
import androidx.webkit.ProxyController
import androidx.webkit.WebViewFeature

/**
 * Bridge between Rust (JNI) and Android WebView for the in-app browser.
 * Creates a native WebView overlaid on the Tauri content area.
 */
object BrowserBridge {

    private var activity: FragmentActivity? = null
    private var webView: WebView? = null
    private var tauriWebView: WebView? = null
    private var proxyApplied: Boolean = false
    private var proxyPending: Boolean = false
    private val pendingLoads = mutableListOf<() -> Unit>()

    private fun withProxy(action: () -> Unit) {
        if (proxyApplied) { action(); return }
        pendingLoads.add(action)
        if (proxyPending) return
        if (!WebViewFeature.isFeatureSupported(WebViewFeature.PROXY_OVERRIDE)) {
            proxyApplied = true
            pendingLoads.forEach { it() }
            pendingLoads.clear()
            return
        }
        proxyPending = true
        val proxyConfig = ProxyConfig.Builder()
            .addProxyRule("socks5://127.0.0.1:17351")
            .addBypassRule("tauri.localhost")
            .addBypassRule("localhost")
            .addBypassRule("127.0.0.1")
            .build()
        ProxyController.getInstance().setProxyOverride(
            proxyConfig,
            { it.run() },
            {
                proxyApplied = true
                proxyPending = false
                pendingLoads.forEach { it() }
                pendingLoads.clear()
            }
        )
    }

    @JvmStatic
    fun setActivity(activity: FragmentActivity) {
        this.activity = activity
    }

    @JvmStatic
    fun browse(url: String, top: Int, left: Int, width: Int, height: Int, dark: Boolean) {
        val act = activity ?: return
        act.runOnUiThread {
            if (tauriWebView == null) {
                val root = act.window.decorView as? ViewGroup
                tauriWebView = findWebView(root)
            }
            val contentView = act.findViewById<FrameLayout>(android.R.id.content)

            android.util.Log.d("FistbumpBrowser", "browse: top=$top left=$left w=$width h=$height reuse=${webView != null}")

            val wv = webView ?: createWebView(act)
            wv.setBackgroundColor(if (dark) Color.parseColor("#09090b") else Color.parseColor("#f4f4f5"))
            wv.webViewClient = makeWebViewClient(dark)
            wv.alpha = 0f

            (wv.parent as? ViewGroup)?.removeView(wv)
            val params = FrameLayout.LayoutParams(width, height)
            params.topMargin = top
            params.leftMargin = left
            contentView.addView(wv, params)
            webView = wv

            // Route through the local SOCKS5 proxy. Applied once and kept set
            // (with bypass rules for Tauri's frontend host) — re-applying on
            // every browse introduced a ~30s delay on the second invocation.
            withProxy { wv.loadUrl(url) }
        }
    }

    private fun createWebView(act: FragmentActivity): WebView {
        val wv = WebView(act)
        wv.settings.javaScriptEnabled = true
        wv.settings.domStorageEnabled = true
        wv.settings.loadWithOverviewMode = true
        wv.settings.useWideViewPort = true
        wv.webChromeClient = WebChromeClient()
        return wv
    }

    private fun makeWebViewClient(dark: Boolean): WebViewClient = object : WebViewClient() {
        override fun onPageCommitVisible(view: WebView?, url: String?) {
            super.onPageCommitVisible(view, url)
            view?.animate()?.alpha(1f)?.setDuration(100)?.start()
            if (url != null && !url.startsWith("fistbump://") && !url.startsWith("about:")) {
                val escaped = url.replace("\\", "\\\\").replace("'", "\\'")
                view?.evaluateJavascript("document.title") { title ->
                    val t = title?.trim('"') ?: ""
                    if (t == "fistbump-error") return@evaluateJavascript
                    tauriWebView?.evaluateJavascript(
                        "if(window._onBrowseURL)window._onBrowseURL('$escaped')", null
                    )
                }
            }
        }

        override fun onReceivedSslError(view: WebView?, handler: SslErrorHandler?, error: SslError?) {
            handler?.proceed()
        }

        override fun onReceivedError(view: WebView?, request: android.webkit.WebResourceRequest?, error: android.webkit.WebResourceError?) {
            if (request?.isForMainFrame != true) return
            val host = request?.url?.host ?: "this site"
            val code = error?.errorCode ?: 0
            val badge: String; val title: String; val message: String
            when (code) {
                ERROR_TIMEOUT -> {
                    badge = "CONNECTION FAILED"; title = "No Response"
                    message = "The server for <strong>$host</strong> did not respond."
                }
                ERROR_HOST_LOOKUP -> {
                    badge = "NOT FOUND"; title = "Server Not Found"
                    message = "The server for <strong>$host</strong> could not be found."
                }
                ERROR_CONNECT -> {
                    badge = "CONNECTION FAILED"; title = "Cannot Connect"
                    message = "Could not connect to the server for <strong>$host</strong>."
                }
                else -> {
                    badge = "DANE VALIDATION FAILED"; title = "Connection Not Secure"
                    message = "The certificate presented by <strong>$host</strong> does not match its on-chain TLSA record. The connection has been blocked."
                }
            }
            view?.stopLoading()
            view?.alpha = 1f
            view?.loadDataWithBaseURL("fistbump://error", buildErrorHtml(dark, badge, title, message), "text/html", "utf-8", "fistbump://error")
        }
    }

    @JvmStatic
    fun browseError(top: Int, left: Int, width: Int, height: Int, dark: Boolean,
                    badge: String, title: String, message: String) {
        val act = activity ?: return
        act.runOnUiThread {
            webView?.let { wv ->
                (wv.parent as? ViewGroup)?.removeView(wv)
            }
            webView = null

            val uiBg = if (dark) Color.parseColor("#09090b") else Color.parseColor("#f4f4f5")

            if (tauriWebView == null) {
                val root = act.window.decorView as? ViewGroup
                tauriWebView = findWebView(root)
            }

            val contentView = act.findViewById<FrameLayout>(android.R.id.content)

            val wv = WebView(act)
            wv.setBackgroundColor(uiBg)

            val params = FrameLayout.LayoutParams(width, height)
            params.topMargin = top
            params.leftMargin = left

            contentView.addView(wv, params)
            webView = wv
            wv.loadDataWithBaseURL("fistbump://error", buildErrorHtml(dark, badge, title, message), "text/html", "utf-8", null)
        }
    }

    @JvmStatic
    fun hide() {
        val act = activity ?: return
        act.runOnUiThread {
            webView?.let { wv ->
                wv.stopLoading()
                (wv.parent as? ViewGroup)?.removeView(wv)
            }
            // Keep webView reference — reuse on next browse to avoid
            // re-initialization latency. Proxy stays set (bypass rules
            // cover Tauri's frontend host).
        }
    }

    private fun buildErrorHtml(dark: Boolean, badge: String, title: String, message: String): String {
        val bg = if (dark) "#09090b" else "#f4f4f5"
        val text = if (dark) "#e4e4e7" else "#18181b"
        val muted = if (dark) "#a1a1aa" else "#71717a"
        val badgeBg = if (dark) "#7f1d1d" else "#fee2e2"
        val badgeText = if (dark) "#fca5a5" else "#dc2626"
        val titleColor = if (dark) "#f87171" else "#dc2626"

        return """
        <html><head><meta name="viewport" content="width=device-width,initial-scale=1">
        <style>
        html, body { height: 100%; margin: 0; }
        body { font-family: sans-serif; background: $bg; color: $text;
               display: flex; align-items: center; justify-content: center;
               text-align: center; padding: 24px; box-sizing: border-box; }
        .box { max-width: 360px; }
        h2 { color: $titleColor; font-size: 18px; margin: 0 0 12px; }
        p { font-size: 14px; color: $muted; line-height: 1.5; margin: 0; }
        .badge { display: inline-block; background: $badgeBg; color: $badgeText; font-size: 11px;
                 padding: 2px 8px; border-radius: 4px; margin-bottom: 16px; font-weight: 600; }
        </style></head><body><div class="box">
        <div class="badge">$badge</div>
        <h2>$title</h2>
        <p>$message</p>
        </div></body></html>
        """.trimIndent()
    }

    private fun showSslError(view: WebView?, host: String, dark: Boolean) {
        view?.loadDataWithBaseURL("fistbump://error",
            buildErrorHtml(dark, "CERTIFICATE ERROR", "Certificate Not Valid",
                "The SSL certificate for <strong>$host</strong> is not valid. The connection has been blocked."),
            "text/html", "utf-8", null)
    }

    private fun isNonIcannTld(url: String): Boolean {
        val host = try {
            java.net.URI(url).host ?: return false
        } catch (_: Exception) { return false }
        val tld = host.split(".").lastOrNull() ?: return false
        // If the TLD isn't a known ICANN TLD, it's likely a fistbump name
        return tld.length > 0 && !COMMON_TLDS.contains(tld.lowercase())
    }

    private val COMMON_TLDS = setOf(
        "com", "org", "net", "edu", "gov", "mil", "int",
        "io", "co", "us", "uk", "de", "fr", "jp", "cn", "au", "ca", "br", "in", "ru",
        "app", "dev", "xyz", "info", "biz", "me", "tv", "cc", "ly", "ai", "gg",
    )

    private fun findWebView(view: android.view.View?): WebView? {
        if (view is WebView) return view
        if (view is ViewGroup) {
            for (i in 0 until view.childCount) {
                val found = findWebView(view.getChildAt(i))
                if (found != null) return found
            }
        }
        return null
    }
}
