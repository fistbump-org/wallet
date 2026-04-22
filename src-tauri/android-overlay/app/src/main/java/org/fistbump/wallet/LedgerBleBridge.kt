package org.fistbump.wallet

import android.Manifest
import android.content.pm.PackageManager
import android.os.Build
import androidx.activity.result.ActivityResultLauncher
import androidx.core.app.ActivityCompat
import androidx.fragment.app.FragmentActivity
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit

/**
 * Runtime permission bridge for BLE Ledger signing on Android.
 *
 * The manifest declares the permissions; the OS still requires runtime
 * consent on API 31+ before scanning or connecting. Rust calls
 * `ensurePermissions()` before its first BLE op and blocks on the latch
 * until the user grants or denies.
 *
 * btleplug's own JNI init reuses the JavaVM captured by
 * [BiometricBridge.nativeRegisterVm] — no separate VM registration here.
 */
object LedgerBleBridge {

    private var activity: FragmentActivity? = null
    private var permissionLauncher: ActivityResultLauncher<Array<String>>? = null
    @Volatile private var pendingLatch: CountDownLatch? = null
    @Volatile private var lastResult: Boolean = false

    /** Called from MainActivity.onCreate. */
    @JvmStatic
    fun setActivity(act: FragmentActivity) {
        activity = act
        // Register the permission launcher before any call to request —
        // ActivityResultContracts require registration during onCreate.
        permissionLauncher = act.registerForActivityResult(
            androidx.activity.result.contract.ActivityResultContracts.RequestMultiplePermissions()
        ) { grants ->
            lastResult = grants.values.all { it }
            pendingLatch?.countDown()
        }
    }

    /** Initialize btleplug's droidplug. Safe to call repeatedly — it's
     *  internally idempotent. MainActivity invokes this in onCreate so
     *  Rust can use BLE without re-initializing per call. */
    @JvmStatic
    fun initBtleplug() {
        try {
            nativeBtleplugInit()
        } catch (_: UnsatisfiedLinkError) {
            // Native lib not loaded yet — will be resolved by the time
            // Rust code actually invokes BLE.
        }
    }

    @JvmStatic
    private external fun nativeBtleplugInit()

    private fun requiredPermissions(): Array<String> =
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            // API 31+ uses BLUETOOTH_SCAN / BLUETOOTH_CONNECT. We declared
            // SCAN with neverForLocation so no location grant is needed.
            arrayOf(
                Manifest.permission.BLUETOOTH_SCAN,
                Manifest.permission.BLUETOOTH_CONNECT,
            )
        } else {
            // API <= 30: scanning requires FINE_LOCATION. The legacy
            // BLUETOOTH / BLUETOOTH_ADMIN are install-time only, not
            // requested at runtime.
            arrayOf(Manifest.permission.ACCESS_FINE_LOCATION)
        }

    @JvmStatic
    fun hasPermissions(): Boolean {
        val act = activity ?: return false
        for (p in requiredPermissions()) {
            if (ActivityCompat.checkSelfPermission(act, p) != PackageManager.PERMISSION_GRANTED) {
                return false
            }
        }
        return true
    }

    /**
     * Blocks the calling (Rust/binder) thread for up to 60s waiting on the
     * system permission dialog. Returns `true` on grant, `false` on deny /
     * timeout / missing activity.
     */
    @JvmStatic
    fun ensurePermissions(): Boolean {
        if (hasPermissions()) return true
        val act = activity ?: return false
        val launcher = permissionLauncher ?: return false

        val latch = CountDownLatch(1)
        pendingLatch = latch
        lastResult = false
        act.runOnUiThread {
            launcher.launch(requiredPermissions())
        }
        val settled = try {
            latch.await(60, TimeUnit.SECONDS)
        } catch (_: InterruptedException) {
            false
        }
        pendingLatch = null
        return settled && lastResult
    }
}
