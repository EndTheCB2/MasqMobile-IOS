package com.masqmobile

import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Base64
import java.nio.charset.StandardCharsets
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/** Stores the consumer wallet encrypted by a non-exportable, device-bound Android Keystore key. */
internal class SecureWalletStore(context: Context) {
  class UnreadableException :
      IllegalStateException(
          "The encrypted consumer wallet could not be read safely.",
      )

  sealed class PreservationRead {
    object Absent : PreservationRead()
    class Readable(val secret: String) : PreservationRead() {
      override fun toString(): String = "Readable([REDACTED])"
    }
    object Unreadable : PreservationRead()
  }

  private val preferences =
      context.getSharedPreferences(SECURE_PREFERENCES, Context.MODE_PRIVATE)

  fun save(secret: String) {
    val cipher = Cipher.getInstance(TRANSFORMATION)
    cipher.init(Cipher.ENCRYPT_MODE, getOrCreateKey())
    val encrypted = cipher.doFinal(secret.toByteArray(StandardCharsets.UTF_8))
    val saved =
        preferences
            .edit()
            .putString(ENCRYPTED_WALLET, Base64.encodeToString(encrypted, Base64.NO_WRAP))
            .putString(INITIALIZATION_VECTOR, Base64.encodeToString(cipher.iv, Base64.NO_WRAP))
            .commit()
    if (!saved) {
      throw IllegalStateException("The encrypted wallet could not be committed to device storage.")
    }
  }

  fun load(): String? {
    return when (val result = readForPreservation()) {
      PreservationRead.Absent -> null
      is PreservationRead.Readable -> result.secret
      PreservationRead.Unreadable -> throw UnreadableException()
    }
  }

  fun readForPreservation(): PreservationRead {
    val encryptedValue: String?
    val initializationVector: String?
    try {
      encryptedValue = preferences.getString(ENCRYPTED_WALLET, null)
      initializationVector = preferences.getString(INITIALIZATION_VECTOR, null)
    } catch (_: Exception) {
      return PreservationRead.Unreadable
    }
    if (encryptedValue == null && initializationVector == null) {
      return PreservationRead.Absent
    }
    if (encryptedValue == null || initializationVector == null) {
      return PreservationRead.Unreadable
    }
    return try {
      val key =
          keyStore().getKey(KEY_ALIAS, null) as? SecretKey
              ?: return PreservationRead.Unreadable
      val cipher = Cipher.getInstance(TRANSFORMATION)
      cipher.init(
          Cipher.DECRYPT_MODE,
          key,
          GCMParameterSpec(128, Base64.decode(initializationVector, Base64.NO_WRAP)),
      )
      val secret =
          String(
              cipher.doFinal(Base64.decode(encryptedValue, Base64.NO_WRAP)),
              StandardCharsets.UTF_8,
          )
      if (secret.isEmpty()) {
        PreservationRead.Unreadable
      } else {
        PreservationRead.Readable(secret)
      }
    } catch (_: Exception) {
      PreservationRead.Unreadable
    }
  }

  fun delete() {
    deleteEncryptedValue()
    val store = keyStore()
    if (store.containsAlias(KEY_ALIAS)) {
      store.deleteEntry(KEY_ALIAS)
    }
  }

  private fun getOrCreateKey(): SecretKey {
    val store = keyStore()
    (store.getKey(KEY_ALIAS, null) as? SecretKey)?.let { return it }
    val generator =
        KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, ANDROID_KEYSTORE)
    generator.init(
        KeyGenParameterSpec.Builder(
                KEY_ALIAS,
                KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT,
            )
            .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
            .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
            .setRandomizedEncryptionRequired(true)
            .setUserAuthenticationRequired(false)
            .build()
    )
    return generator.generateKey()
  }

  private fun deleteEncryptedValue() {
    val deleted =
        preferences.edit().remove(ENCRYPTED_WALLET).remove(INITIALIZATION_VECTOR).commit()
    if (!deleted) {
      throw IllegalStateException("The encrypted wallet could not be removed from device storage.")
    }
  }

  private fun keyStore(): KeyStore =
      KeyStore.getInstance(ANDROID_KEYSTORE).apply { load(null) }

  private companion object {
    const val ANDROID_KEYSTORE = "AndroidKeyStore"
    const val KEY_ALIAS = "masq-mobile-consumer-wallet"
    const val SECURE_PREFERENCES = "masq-mobile-secure-storage"
    const val ENCRYPTED_WALLET = "encrypted-consumer-wallet"
    const val INITIALIZATION_VECTOR = "consumer-wallet-iv"
    const val TRANSFORMATION = "AES/GCM/NoPadding"
  }
}
