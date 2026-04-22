package org.fistbump.wallet

import android.graphics.Color
import android.os.Bundle
import android.util.Log
import android.webkit.JavascriptInterface
import android.webkit.WebView
import androidx.activity.SystemBarStyle
import androidx.activity.enableEdgeToEdge
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat
import androidx.core.view.WindowInsetsControllerCompat
import androidx.webkit.WebViewCompat
import androidx.webkit.WebViewFeature

class MainActivity : TauriActivity() {
  private var webViewRef: WebView? = null
  private var sat = 0; private var sab = 0; private var sal = 0; private var sar = 0
  private var docStartHandle: Any? = null

  override fun onCreate(savedInstanceState: Bundle?) {
    enableEdgeToEdge(
      statusBarStyle = SystemBarStyle.auto(Color.TRANSPARENT, Color.TRANSPARENT),
      navigationBarStyle = SystemBarStyle.auto(Color.TRANSPARENT, Color.TRANSPARENT)
    )
    super.onCreate(savedInstanceState)
    BiometricBridge.setActivity(this)
    BrowserBridge.setActivity(this)
    // LedgerBleBridge needs the activity to hand to its permission launcher,
    // and the launcher must be registered during onCreate (ActivityResult
    // contract requirement). The bridge is a no-op until Rust calls into
    // it from the BLE code path.
    LedgerBleBridge.setActivity(this)
    BiometricBridge.registerVmWithRust()
    // btleplug's droidplug needs to resolve its Java companion classes
    // once at startup. This passes Kotlin's JNIEnv straight to
    // btleplug::platform::init on the Rust side.
    LedgerBleBridge.initBtleplug()

    ViewCompat.setOnApplyWindowInsetsListener(window.decorView) { _, insets ->
      val bars = insets.getInsets(
        WindowInsetsCompat.Type.systemBars() or WindowInsetsCompat.Type.displayCutout()
      )
      val d = resources.displayMetrics.density
      sat = (bars.top / d).toInt()
      sab = (bars.bottom / d).toInt()
      sal = (bars.left / d).toInt()
      sar = (bars.right / d).toInt()
      Log.d("Fistbump", "insets top=$sat bottom=$sab left=$sal right=$sar")
      applySafeAreaInsets()
      insets
    }
  }

  override fun onWebViewCreate(webView: WebView) {
    Log.d("Fistbump", "onWebViewCreate called")
    webViewRef = webView
    webView.addJavascriptInterface(SystemBarsBridge(), "FistbumpBars")
    applySafeAreaInsets()
    ViewCompat.requestApplyInsets(window.decorView)
  }

  private inner class SystemBarsBridge {
    @JavascriptInterface
    fun setLightMode(isLight: Boolean) {
      runOnUiThread {
        val c = WindowInsetsControllerCompat(window, window.decorView)
        c.isAppearanceLightStatusBars = isLight
        c.isAppearanceLightNavigationBars = isLight
      }
    }
  }

  private fun applySafeAreaInsets() {
    val wv = webViewRef ?: return
    val js = "(function(){var s=document.documentElement.style;" +
      "s.setProperty('--sat','${sat}px');" +
      "s.setProperty('--sab','${sab}px');" +
      "s.setProperty('--sal','${sal}px');" +
      "s.setProperty('--sar','${sar}px');" +
      "})()"
    wv.post { wv.evaluateJavascript(js, null) }
    if (WebViewFeature.isFeatureSupported(WebViewFeature.DOCUMENT_START_SCRIPT)) {
      (docStartHandle as? androidx.webkit.ScriptHandler)?.remove()
      docStartHandle = WebViewCompat.addDocumentStartJavaScript(wv, js, setOf("*"))
    }
  }
}
