package com.masqmobile

import android.content.Context
import java.io.File
import java.util.concurrent.CompletableFuture
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicReference
import org.json.JSONArray
import org.json.JSONObject

internal enum class MasqBackgroundRecoveryResult {
  ACTIVE,
  STARTED,
  CANCELLED,
  FAILED,
}

internal data class MasqRouteVerificationOutcome(
    val result: MasqBackgroundRecoveryResult,
    val snapshot: MasqSessionCoreSnapshot?,
)

internal fun freshestMasqRecoveryFailureSnapshot(
    initialSnapshot: MasqSessionCoreSnapshot,
    verification: MasqRouteVerificationOutcome,
): MasqSessionCoreSnapshot = verification.snapshot ?: initialSnapshot

internal data class MasqRecoveryAttemptIdentity(
    val startGeneration: Long,
    val engineGeneration: Long,
) {
  init {
    require(startGeneration >= 0L)
    require(engineGeneration > 0L)
  }
}

internal fun matchesMasqRecoveryAttemptIdentity(
    expected: MasqRecoveryAttemptIdentity,
    startGeneration: Long,
    snapshot: MasqSessionCoreSnapshot,
): Boolean =
    expected.startGeneration == startGeneration &&
        expected.engineGeneration == snapshot.engineGeneration

internal class MasqRecoveryRouteVerificationGate {
  private val claimedIdentity = AtomicReference<MasqRecoveryAttemptIdentity?>(null)

  fun claim(identity: MasqRecoveryAttemptIdentity): Boolean {
    while (true) {
      val current = claimedIdentity.get()
      if (current == identity) return false
      if (claimedIdentity.compareAndSet(current, identity)) return true
    }
  }
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
  private val routeVerificationGate = MasqRecoveryRouteVerificationGate()

  /**
   * Persists only the bounded public entry descriptors behind a route that the
   * background supervisor observed at stage two. This mirrors the foreground
   * status path, so screen-off sessions retain the same fast known-good retry.
   */
  fun recordKnownGoodRoute(snapshot: MasqSessionCoreSnapshot?) {
    if (snapshot?.isHealthyConnectedSession() != true) return
    runCatching {
      val saved = preferences.getString(SAVED_CONFIG_KEY, null) ?: return@runCatching
      val config = migrateConfig(saved)
      val chain = config.optString("chain")
      if (chain.isBlank()) return@runCatching
      val descriptors =
          config.optJSONArray("neighbors")?.let { nodes ->
            (0 until nodes.length()).mapNotNull { index ->
              nodes.optString(index).takeIf(String::isNotBlank)
            }
          } ?: emptyList()
      entryNodeDiscovery.recordKnownGoodRoute(chain, descriptors, snapshot)
    }
  }

  /**
   * De-prioritizes only the bounded public entry descriptors from the saved
   * consumer configuration. A periodic proof escalation pauses the native core
   * before recovery, so its next `paused` snapshot can no longer classify the
   * failed pair itself. Recording the hint before that stop lets the following
   * discovery prioritize fresh alternatives without persisting wallet,
   * destination, device, or browsing metadata.
   */
  fun recordSavedRouteProofFailure() {
    runCatching {
      val saved = preferences.getString(SAVED_CONFIG_KEY, null) ?: return@runCatching
      val config = migrateConfig(saved)
      val chain = config.optString("chain")
      if (chain.isBlank()) return@runCatching
      val descriptors =
          config.optJSONArray("neighbors")?.let { nodes ->
            (0 until nodes.length()).mapNotNull { index ->
              nodes.optString(index).takeIf(String::isNotBlank)
            }
          } ?: emptyList()
      entryNodeDiscovery.recordRouteProofFailure(chain, descriptors)
    }
  }

  fun recover(
      isRecoveryCurrent: () -> Boolean,
      expectedRouteVerificationIdentity: MasqRecoveryAttemptIdentity? = null,
  ): MasqBackgroundRecoveryResult {
    fun canContinue(): Boolean = isSessionDesired() && isRecoveryCurrent()

    if (!canContinue() || !MasqCoreJni.isAvailable) {
      return MasqBackgroundRecoveryResult.CANCELLED
    }
    val initialStartGeneration = MasqCoreLifecycle.startGeneration.get()
    val initialSnapshot =
        runOnCore {
          masqSessionCoreSnapshot(MasqCoreJni.nativeGetStatus())
        }
    if (
        !canContinue() ||
            MasqCoreLifecycle.startGeneration.get() != initialStartGeneration
    ) {
      return MasqBackgroundRecoveryResult.CANCELLED
    }
    if (initialSnapshot == null) return MasqBackgroundRecoveryResult.FAILED
    if (
        expectedRouteVerificationIdentity != null &&
            !matchesMasqRecoveryAttemptIdentity(
                expected = expectedRouteVerificationIdentity,
                startGeneration = initialStartGeneration,
                snapshot = initialSnapshot,
            )
    ) {
      return MasqBackgroundRecoveryResult.CANCELLED
    }
    if (initialSnapshot.isHealthyConnectedSession()) {
      return MasqBackgroundRecoveryResult.ACTIVE
    }
    if (
        expectedRouteVerificationIdentity != null &&
            !initialSnapshot.isEntryConnectedAwaitingRoute()
    ) {
      // A stage-one proof request is observational for one exact native
      // identity. It must never fall through into discovery or restart a core
      // that changed state while the short settle delay was pending.
      return MasqBackgroundRecoveryResult.CANCELLED
    }
    var routeProofFailed =
        shouldDeprioritizeAttemptedEntryNodes(
            phase = initialSnapshot.phase,
            engineGeneration = initialSnapshot.engineGeneration,
            routeStage = initialSnapshot.routeStage,
            lastError = initialSnapshot.lastError,
        )
    var failureSnapshot = initialSnapshot
    if (initialSnapshot.isEntryConnectedAwaitingRoute()) {
      val identity =
          MasqRecoveryAttemptIdentity(
              startGeneration = initialStartGeneration,
              engineGeneration = initialSnapshot.engineGeneration,
          )
      if (routeVerificationGate.claim(identity)) {
        val verification = verifyExistingRoute(::canContinue, identity)
        when (verification.result) {
          MasqBackgroundRecoveryResult.ACTIVE,
          MasqBackgroundRecoveryResult.CANCELLED,
          MasqBackgroundRecoveryResult.STARTED -> return verification.result
          MasqBackgroundRecoveryResult.FAILED -> {
            routeProofFailed = true
            failureSnapshot =
                freshestMasqRecoveryFailureSnapshot(initialSnapshot, verification)
            // Continue in this recovery cycle. A failed exit-route proof does not
            // quarantine a proven entry peer; only an explicit entry-handshake
            // diagnostic in the freshest post-proof snapshot below can do that.
          }
        }
      } else {
        if (expectedRouteVerificationIdentity != null) {
          return MasqBackgroundRecoveryResult.CANCELLED
        }
        routeProofFailed = true
      }
    }

    val recoveryGeneration =
        synchronized(MasqCoreLifecycle.lock) {
          if (!canContinue()) {
            return@synchronized null
          }
          if (MasqCoreLifecycle.startGeneration.get() != initialStartGeneration) {
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
    if (routeProofFailed) {
      entryNodeDiscovery.recordRouteProofFailure(chain, preferredNodes)
    }
    if (
        shouldQuarantineAttemptedEntryNodes(
            phase = failureSnapshot.phase,
            engineGeneration = failureSnapshot.engineGeneration,
            routeStage = failureSnapshot.routeStage,
            lastError = failureSnapshot.lastError,
        )
    ) {
      entryNodeDiscovery.recordConnectionFailure(chain, preferredNodes)
    }
    if (!ownsRecovery()) return MasqBackgroundRecoveryResult.CANCELLED

    val discovery =
        try {
          entryNodeDiscovery.discover(chain, preferredNodes, ::ownsRecovery)
        } catch (_: EntryNodeDiscoveryCancelledException) {
          return MasqBackgroundRecoveryResult.CANCELLED
        } catch (_: Exception) {
          return MasqBackgroundRecoveryResult.FAILED
        }
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

  private fun verifyExistingRoute(
      canContinue: () -> Boolean,
      identity: MasqRecoveryAttemptIdentity,
  ): MasqRouteVerificationOutcome {
    val result =
        runOnCore {
        if (
            !canContinue() ||
                MasqCoreLifecycle.startGeneration.get() != identity.startGeneration
        ) {
          return@runOnCore MasqRouteVerificationOutcome(
              MasqBackgroundRecoveryResult.CANCELLED,
              null,
          )
        }
        val verified = MasqCoreJni.nativePreflightProxy()
        if (
            !canContinue() ||
                MasqCoreLifecycle.startGeneration.get() != identity.startGeneration
        ) {
          return@runOnCore MasqRouteVerificationOutcome(
              MasqBackgroundRecoveryResult.CANCELLED,
              null,
          )
        }
        val snapshot = masqSessionCoreSnapshot(verified)
        if (snapshot != null && snapshot.engineGeneration != identity.engineGeneration) {
          return@runOnCore MasqRouteVerificationOutcome(
              MasqBackgroundRecoveryResult.CANCELLED,
              snapshot,
          )
        }
        if (
            snapshot?.isHealthyConnectedSession() == true
        ) {
          MasqRouteVerificationOutcome(MasqBackgroundRecoveryResult.ACTIVE, snapshot)
        } else {
          MasqRouteVerificationOutcome(MasqBackgroundRecoveryResult.FAILED, snapshot)
        }
      }
    if (
        !canContinue() ||
            MasqCoreLifecycle.startGeneration.get() != identity.startGeneration
    ) {
      return MasqRouteVerificationOutcome(MasqBackgroundRecoveryResult.CANCELLED, null)
    }
    return result
        ?: MasqRouteVerificationOutcome(MasqBackgroundRecoveryResult.FAILED, null)
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
    // The native end-to-end TLS route proof has a 12-second absolute deadline.
    // Leave bounded scheduling margin so the result is observed instead of
    // timing out the future at the same instant as the native operation.
    const val CORE_OPERATION_TIMEOUT_SECONDS = 20L
  }
}
