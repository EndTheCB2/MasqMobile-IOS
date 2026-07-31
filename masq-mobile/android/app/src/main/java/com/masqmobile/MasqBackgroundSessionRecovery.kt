package com.masqmobile

import android.content.Context
import java.io.File
import java.util.concurrent.CompletableFuture
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean
import org.json.JSONArray
import org.json.JSONObject

internal enum class MasqBackgroundRecoveryResult {
  ACTIVE,
  STARTED,
  CANCELLED,
  FAILED,
}

/**
 * Restores only a previously user-requested consumer session after Android reclaims the process.
 *
 * The persisted flag contains no wallet, peer, IP, or location data. Wallet restoration continues
 * to use the device-bound Android Keystore store.
 */
internal class MasqBackgroundSessionRecovery(
    private val context: Context,
    private val isSessionDesired: () -> Boolean,
) {
  private val preferences =
      context.getSharedPreferences(CONSUMER_PREFERENCES, Context.MODE_PRIVATE)
  private val walletStore = SecureWalletStore(context)
  private val entryNodeDiscovery = EntryNodeDiscovery(context)

  fun recover(isRecoveryCurrent: () -> Boolean): MasqBackgroundRecoveryResult {
    fun canContinue(): Boolean = isSessionDesired() && isRecoveryCurrent()

    if (!canContinue() || !MasqCoreJni.isAvailable) {
      return MasqBackgroundRecoveryResult.CANCELLED
    }
    val initialSnapshot =
        runOnCore {
          masqSessionCoreSnapshot(MasqCoreJni.nativeGetStatus())
        } ?: return MasqBackgroundRecoveryResult.FAILED
    if (initialSnapshot?.isHealthyConnectedSession() == true) {
      return MasqBackgroundRecoveryResult.ACTIVE
    }

    val recoveryGeneration =
        synchronized(MasqCoreLifecycle.lock) {
          if (!canContinue()) {
            return@synchronized null
          }
          MasqCoreLifecycle.startGeneration.incrementAndGet()
        } ?: return MasqBackgroundRecoveryResult.CANCELLED
    fun ownsRecovery(): Boolean =
        !Thread.currentThread().isInterrupted &&
            canContinue() &&
            MasqCoreLifecycle.startGeneration.get() == recoveryGeneration

    val savedConfig =
        preferences.getString(SAVED_CONFIG_KEY, null)
            ?: return MasqBackgroundRecoveryResult.FAILED
    val config =
        runCatching { migrateConfig(savedConfig) }.getOrNull()
            ?: return MasqBackgroundRecoveryResult.FAILED
    val chain = config.optString("chain")
    if (chain.isBlank()) return MasqBackgroundRecoveryResult.FAILED
    val preferredNodes =
        config.optJSONArray("neighbors")?.let { nodes ->
          (0 until nodes.length()).mapNotNull { index ->
            nodes.optString(index).takeIf(String::isNotBlank)
          }
        } ?: emptyList()
    if (!ownsRecovery()) return MasqBackgroundRecoveryResult.CANCELLED

    val discovery =
        runCatching { entryNodeDiscovery.discover(chain, preferredNodes) }
            .getOrNull()
            ?: return MasqBackgroundRecoveryResult.FAILED
    if (!ownsRecovery()) return MasqBackgroundRecoveryResult.CANCELLED
    val runtimeConfig =
        JSONObject(config.toString())
            .put("neighbors", JSONArray(discovery.runtimeDescriptors))
            .toString()
    val refreshedConfig =
        JSONObject(config.toString())
            .put("neighbors", JSONArray(discovery.persistentDescriptors))
            .toString()

    return runOnCore {
      synchronized(MasqCoreLifecycle.lock) {
        if (!ownsRecovery()) {
          return@synchronized MasqBackgroundRecoveryResult.CANCELLED
        }
        val currentStatus = JSONObject(MasqCoreJni.nativeGetStatus())
        val currentSnapshot = masqSessionCoreSnapshot(currentStatus.toString())
        if (currentSnapshot?.isHealthyConnectedSession() == true) {
          return@synchronized MasqBackgroundRecoveryResult.ACTIVE
        }
        val configured =
            MasqCoreJni.nativeConfigure(prepareNativeConfig(runtimeConfig))
        if (!statusSucceeded(configured)) {
          return@synchronized MasqBackgroundRecoveryResult.FAILED
        }
        if (!ownsRecovery()) {
          return@synchronized MasqBackgroundRecoveryResult.CANCELLED
        }
        val configuredStatus = JSONObject(configured)
        if (configuredStatus.isNull("walletAddress")) {
          val wallet =
              runCatching { walletStore.load() }.getOrNull()
                  ?: return@synchronized MasqBackgroundRecoveryResult.FAILED
          val imported = MasqCoreJni.nativeImportWallet(wallet)
          if (!statusSucceeded(imported)) {
            return@synchronized MasqBackgroundRecoveryResult.FAILED
          }
        }
        if (!ownsRecovery()) {
          return@synchronized MasqBackgroundRecoveryResult.CANCELLED
        }
        val started = MasqCoreJni.nativeStart()
        if (!statusSucceeded(started)) {
          return@synchronized MasqBackgroundRecoveryResult.FAILED
        }
        if (!ownsRecovery()) {
          return@synchronized MasqBackgroundRecoveryResult.CANCELLED
        }
        if (!preferences.edit().putString(SAVED_CONFIG_KEY, refreshedConfig).commit()) {
          runCatching { MasqCoreJni.nativeShutdown() }
          return@synchronized MasqBackgroundRecoveryResult.FAILED
        }
        MasqBackgroundRecoveryResult.STARTED
      }
    } ?: MasqBackgroundRecoveryResult.FAILED
  }

  private fun prepareNativeConfig(configJson: String): String {
    val dataDirectory = File(context.noBackupFilesDir, "masq-node")
    if (!dataDirectory.exists() && !dataDirectory.mkdirs()) {
      throw IllegalStateException("The MASQ data directory could not be created.")
    }
    return JSONObject(configJson)
        .apply {
          remove("configVersion")
          put("dataDirectory", dataDirectory.absolutePath)
        }
        .toString()
  }

  private fun migrateConfig(configJson: String): JSONObject =
      JSONObject(configJson).apply {
        put("configVersion", 2)
        if (!has("minHops")) put("minHops", 1)
        if (!has("exitCountry")) put("exitCountry", JSONObject.NULL)
        if (!has("exitCountryFallback")) put("exitCountryFallback", true)
      }

  private fun statusSucceeded(statusJson: String): Boolean {
    val phase = runCatching { JSONObject(statusJson).optString("phase") }.getOrNull()
    return phase in
        setOf(
            "unconfigured",
            "ready",
            "connecting",
            "connected",
            "paused",
            "stopping",
            "blocked",
        )
  }

  private fun <T> runOnCore(block: () -> T): T? {
    val result = CompletableFuture<T>()
    val operationLive = AtomicBoolean(true)
    val future =
        MasqCoreLifecycle.executor.submit {
          if (!operationLive.get()) return@submit
          runCatching(block)
          .onSuccess(result::complete)
          .onFailure(result::completeExceptionally)
        }
    return runCatching {
          result.get(CORE_OPERATION_TIMEOUT_SECONDS, TimeUnit.SECONDS)
        }
        .getOrElse {
          operationLive.set(false)
          future.cancel(true)
          null
        }
  }

  private companion object {
    const val CONSUMER_PREFERENCES = "masq-mobile-consumer"
    const val SAVED_CONFIG_KEY = "saved-consumer-config"
    const val CORE_OPERATION_TIMEOUT_SECONDS = 30L
  }
}
