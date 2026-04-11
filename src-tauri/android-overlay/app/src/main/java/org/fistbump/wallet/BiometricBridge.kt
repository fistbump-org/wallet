package org.fistbump.wallet

import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Base64
import androidx.biometric.BiometricManager
import androidx.biometric.BiometricPrompt
import androidx.core.content.ContextCompat
import androidx.fragment.app.FragmentActivity
import java.security.KeyStore
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/**
 * Bridge between Rust (JNI) and Android biometric APIs.
 *
 * Stores wallet passphrases in app-private SharedPreferences,
 * gated behind BiometricPrompt authentication.
 */
object BiometricBridge {

    private var activity: FragmentActivity? = null

    /** Called from MainActivity.onCreate to provide the activity reference. */
    @JvmStatic
    fun setActivity(activity: FragmentActivity) {
        this.activity = activity
    }

    /** Pass the JavaVM to Rust via JNI so it can call back into Kotlin. */
    @JvmStatic
    fun registerVmWithRust() {
        try {
            nativeRegisterVm()
        } catch (_: UnsatisfiedLinkError) {
            // Native lib not ready yet — Rust will use dlsym fallback
        }
    }

    @JvmStatic
    private external fun nativeRegisterVm()

    /** Check if biometric authentication (fingerprint/face) is available. */
    @JvmStatic
    fun isAvailable(): Boolean {
        val ctx = activity ?: return false
        val manager = BiometricManager.from(ctx)
        return manager.canAuthenticate(BiometricManager.Authenticators.BIOMETRIC_STRONG) ==
            BiometricManager.BIOMETRIC_SUCCESS
    }

    private const val KEYSTORE_ALIAS_PREFIX = "fistbump_bio_"
    private const val PREFS_NAME = "biometric_encrypted"

    /** Get or create a biometric-bound AES key in the Android Keystore. */
    private fun getOrCreateKey(wallet: String): SecretKey {
        val alias = KEYSTORE_ALIAS_PREFIX + wallet
        val ks = KeyStore.getInstance("AndroidKeyStore")
        ks.load(null)
        if (ks.containsAlias(alias)) {
            return ks.getKey(alias, null) as SecretKey
        }
        val spec = KeyGenParameterSpec.Builder(alias,
            KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
            .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
            .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
            .setUserAuthenticationRequired(true)
            .setInvalidatedByBiometricEnrollment(true)
            .build()
        val gen = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore")
        gen.init(spec)
        return gen.generateKey()
    }

    /** Store a passphrase encrypted with a biometric-bound key. */
    @JvmStatic
    fun save(wallet: String, passphrase: String): Boolean {
        val ctx = activity ?: return false
        return try {
            val key = getOrCreateKey(wallet)
            val cipher = Cipher.getInstance("AES/GCM/NoPadding")
            cipher.init(Cipher.ENCRYPT_MODE, key)
            val encrypted = cipher.doFinal(passphrase.toByteArray(Charsets.UTF_8))
            val iv = cipher.iv
            // Store IV + encrypted data as base64
            val combined = iv + encrypted
            val encoded = Base64.encodeToString(combined, Base64.NO_WRAP)
            val prefs = ctx.getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE)
            prefs.edit().putString(wallet, encoded).commit()
        } catch (e: Exception) {
            android.util.Log.e("BiometricBridge", "save failed", e)
            false
        }
    }

    /**
     * Retrieve a passphrase after biometric authentication.
     * Shows the system biometric prompt, authenticates the crypto operation,
     * and decrypts the passphrase on success.
     */
    @JvmStatic
    fun load(wallet: String): String? {
        val act = activity ?: return null
        val prefs = act.getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE)
        val encoded = prefs.getString(wallet, null) ?: return null
        val combined = Base64.decode(encoded, Base64.NO_WRAP)
        if (combined.size < 13) return null // GCM IV is 12 bytes minimum
        val iv = combined.copyOfRange(0, 12)
        val encrypted = combined.copyOfRange(12, combined.size)

        val key = try { getOrCreateKey(wallet) } catch (_: Exception) { return null }
        val cipher = try {
            Cipher.getInstance("AES/GCM/NoPadding").also {
                it.init(Cipher.DECRYPT_MODE, key, GCMParameterSpec(128, iv))
            }
        } catch (_: Exception) { return null }

        val latch = CountDownLatch(1)
        var result: String? = null

        act.runOnUiThread {
            val executor = ContextCompat.getMainExecutor(act)
            val callback = object : BiometricPrompt.AuthenticationCallback() {
                override fun onAuthenticationSucceeded(r: BiometricPrompt.AuthenticationResult) {
                    try {
                        val authedCipher = r.cryptoObject?.cipher ?: cipher
                        val decrypted = authedCipher.doFinal(encrypted)
                        result = String(decrypted, Charsets.UTF_8)
                    } catch (e: Exception) {
                        android.util.Log.e("BiometricBridge", "decrypt failed", e)
                    }
                    latch.countDown()
                }
                override fun onAuthenticationError(errorCode: Int, errString: CharSequence) {
                    latch.countDown()
                }
            }
            val prompt = BiometricPrompt(act, executor, callback)
            val info = BiometricPrompt.PromptInfo.Builder()
                .setTitle("Unlock Wallet")
                .setSubtitle("Authenticate to unlock your wallet")
                .setNegativeButtonText("Use Passphrase")
                .setAllowedAuthenticators(BiometricManager.Authenticators.BIOMETRIC_STRONG)
                .build()
            prompt.authenticate(info, BiometricPrompt.CryptoObject(cipher))
        }

        latch.await(60, TimeUnit.SECONDS)
        return result
    }

    /** Delete the stored passphrase and its encryption key. */
    @JvmStatic
    fun delete(wallet: String) {
        val ctx = activity ?: return
        val prefs = ctx.getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE)
        prefs.edit().remove(wallet).apply()
        try {
            val ks = KeyStore.getInstance("AndroidKeyStore")
            ks.load(null)
            ks.deleteEntry(KEYSTORE_ALIAS_PREFIX + wallet)
        } catch (_: Exception) {}
    }
}
