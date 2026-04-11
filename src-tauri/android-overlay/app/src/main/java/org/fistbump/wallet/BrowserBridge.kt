package org.fistbump.wallet

import android.content.Intent
import androidx.fragment.app.FragmentActivity

/**
 * Bridge between Rust (JNI) and the Android browser. Fires off a
 * full-screen BrowserActivity — no overlay positioning, no layout math.
 */
object BrowserBridge {

    private var activity: FragmentActivity? = null

    @JvmStatic
    fun setActivity(activity: FragmentActivity) {
        this.activity = activity
    }

    @JvmStatic
    fun browse(url: String, dark: Boolean) {
        val act = activity ?: return
        act.runOnUiThread {
            val intent = Intent(act, BrowserActivity::class.java)
                .putExtra(BrowserActivity.EXTRA_URL, url)
                .putExtra(BrowserActivity.EXTRA_DARK, dark)
            act.startActivity(intent)
        }
    }
}
