package com.masqmobile

import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.content.pm.ServiceInfo
import android.net.ConnectivityManager
import android.net.Network
import android.net.NetworkCapabilities
import android.net.NetworkRequest
import android.os.Build
import android.os.Handler
import android.os.IBinder
import android.os.Looper
import android.os.PowerManager
import android.os.SystemClock
import android.util.Log
import androidx.core.app.NotificationCompat
import androidx.core.app.ServiceCompat
import androidx.core.content.ContextCompat
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.CountDownLatch
import java.util.concurrent.Executors
import java.util.concurrent.Future
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicLong
import java.util.concurrent.atomic.AtomicReference
import org.json.JSONObject

private val ROUTE_PROOF_REFRESH_ERROR_CODES =
    setOf(
        "E_PRIVATE_ROUTE_REFRESH_FAILED",
        "E_PRIVATE_ROUTE_REFRESH_NOT_READY",
        "E_PRIVATE_ROUTE_REFRESH_UNAVAILABLE",
    )

private const val MASQ_RECOVERY_LOG_TAG = "MasqRecovery"

private fun logMasqRecovery(message: String) {
  Log.i(MASQ_RECOVERY_LOG_TAG, message)
}

internal enum class MasqSessionNotificationState {
  CONNECTING,
  CONNECTED,
  ATTENTION,
}

internal enum class MasqModuleStartAdmissionDecision {
  ACCEPTED,
  SERVICE_TAKEOVER,
  TIMED_OUT,
  ;

  fun allowsModuleDiscovery(): Boolean = this != SERVICE_TAKEOVER
}

internal class MasqModuleStartAdmissionGate {
  private val completion = CountDownLatch(1)
  private val decision = AtomicReference<MasqModuleStartAdmissionDecision?>(null)

  fun complete(next: MasqModuleStartAdmissionDecision): Boolean {
    if (!decision.compareAndSet(null, next)) return false
    completion.countDown()
    return true
  }

  fun await(timeoutMillis: Long): MasqModuleStartAdmissionDecision {
    require(timeoutMillis > 0L)
    return try {
      if (completion.await(timeoutMillis, TimeUnit.MILLISECONDS)) {
        decision.get() ?: MasqModuleStartAdmissionDecision.TIMED_OUT
      } else {
        MasqModuleStartAdmissionDecision.TIMED_OUT
      }
    } catch (_: InterruptedException) {
      Thread.currentThread().interrupt()
      MasqModuleStartAdmissionDecision.SERVICE_TAKEOVER
    }
  }
}

internal fun shouldInvalidateForegroundStartAfterServiceTakeover(
    admissionCompleted: Boolean,
): Boolean = !admissionCompleted

internal data class MasqSessionCoreSnapshot(
    val phase: String,
    val connectedNeighbors: Int,
    val routeStage: Int,
    val proxyPort: Int = 0,
    val engineGeneration: Long = 0,
    val routeProofGeneration: Long = 0,
    val routeProofRefresh: MasqRouteProofRefreshResult? = null,
    val lastError: String? = null,
)

internal data class MasqRouteProofRefreshResult(
    val attempted: Boolean,
    val succeeded: Boolean,
    val errorCode: String?,
)

internal fun masqSessionCoreSnapshot(statusJson: String): MasqSessionCoreSnapshot? =
    runCatching {
          JSONObject(statusJson).let { status ->
            val routeProofRefresh =
                status.optJSONObject("routeProofRefresh")?.let { refresh ->
                  MasqRouteProofRefreshResult(
                      attempted = refresh.optBoolean("attempted", false),
                      succeeded = refresh.optBoolean("succeeded", false),
                      errorCode =
                          refresh
                              .optString("errorCode")
                              .takeIf(ROUTE_PROOF_REFRESH_ERROR_CODES::contains),
                  )
                }
            MasqSessionCoreSnapshot(
                phase = status.optString("phase"),
                connectedNeighbors = status.optInt("connectedNeighbors", 0),
                routeStage = status.optInt("routeStage", 0),
                proxyPort = status.optInt("proxyPort", 0),
                engineGeneration = status.optLong("engineGeneration", 0L),
                routeProofGeneration = status.optLong("routeProofGeneration", 0L),
                routeProofRefresh = routeProofRefresh,
                lastError = safeLastErrorValue(status.opt("lastError")),
            )
          }
        }
        .getOrNull()

internal fun MasqSessionCoreSnapshot.isHealthyConnectedSession(): Boolean =
    phase == "connected" &&
        connectedNeighbors > 0 &&
        routeStage >= 2 &&
        proxyPort in 1..65535 &&
        engineGeneration > 0 &&
        lastError == null

internal fun MasqSessionCoreSnapshot.isEntryConnectedAwaitingRoute(): Boolean =
    phase == "connecting" &&
        connectedNeighbors > 0 &&
        routeStage == 1 &&
        proxyPort in 1..65535 &&
        engineGeneration > 0 &&
        lastError == null

internal fun MasqSessionCoreSnapshot.hasTerminalEntryRecoverySignal(): Boolean =
    shouldQuarantineAttemptedEntryNodes(
        phase = phase,
        engineGeneration = engineGeneration,
        routeStage = routeStage,
        lastError = lastError,
    )

internal fun shouldApplyMasqSessionSnapshot(
    monitoredSessionGeneration: Long,
    currentSessionGeneration: Long,
    monitoredStartGeneration: Long,
    completedStartGeneration: Long,
    currentStartGeneration: Long,
    refreshRouteProof: Boolean,
    expectedRefreshEngineGeneration: Long,
    snapshot: MasqSessionCoreSnapshot?,
    networkAvailable: Boolean,
    monitoredNetworkEpoch: Long = 0L,
    currentNetworkEpoch: Long = 0L,
): Boolean =
    monitoredSessionGeneration == currentSessionGeneration &&
        monitoredStartGeneration == completedStartGeneration &&
        monitoredStartGeneration == currentStartGeneration &&
        monitoredNetworkEpoch == currentNetworkEpoch &&
        (!refreshRouteProof ||
            expectedRefreshEngineGeneration <= 0L ||
            snapshot == null ||
            snapshot.engineGeneration == expectedRefreshEngineGeneration) &&
        (!refreshRouteProof || networkAvailable) &&
        (snapshot?.isHealthyConnectedSession() != true || networkAvailable)

internal const val ROUTE_PROOF_REFRESH_INTERVAL_MILLIS = 4 * 60_000L
internal const val ROUTE_PROOF_REFRESH_RETRY_INITIAL_MILLIS = 15_000L
internal const val ROUTE_PROOF_REFRESH_RETRY_MAX_MILLIS = 60_000L
internal const val ROUTE_PROOF_REFRESH_FAILURES_BEFORE_RESTART = 3

internal data class MasqRouteProofRefreshSchedule(
    val startGeneration: Long = 0L,
    val engineGeneration: Long = 0L,
    val observedGeneration: Long = 0L,
    val deadlineElapsed: Long = 0L,
    val consecutiveFailures: Int = 0,
) {
  fun isDue(nowElapsed: Long, currentStartGeneration: Long): Boolean =
      startGeneration > 0L &&
          startGeneration == currentStartGeneration &&
          deadlineElapsed > 0L &&
          nowElapsed >= deadlineElapsed

  fun accepts(snapshot: MasqSessionCoreSnapshot?, currentStartGeneration: Long): Boolean =
      snapshot != null &&
          startGeneration == currentStartGeneration &&
          engineGeneration == snapshot.engineGeneration

  fun refreshSucceeded(
      snapshot: MasqSessionCoreSnapshot?,
      currentStartGeneration: Long,
  ): Boolean {
    if (snapshot?.isHealthyConnectedSession() != true || !accepts(snapshot, currentStartGeneration)) {
      return false
    }
    val reportedGeneration = snapshot.routeProofGeneration.coerceAtLeast(0L)
    val proofAdvanced = reportedGeneration > observedGeneration
    val nativeRefreshSucceeded =
        snapshot.routeProofRefresh?.let { it.attempted && it.succeeded } == true
    return nativeRefreshSucceeded || proofAdvanced
  }

  fun afterSnapshot(
      snapshot: MasqSessionCoreSnapshot?,
      currentStartGeneration: Long,
      nowElapsed: Long,
    refreshAttempted: Boolean,
  ): MasqRouteProofRefreshSchedule {
    if (snapshot == null) {
      return if (refreshAttempted && startGeneration == currentStartGeneration) {
        afterRefreshFailure(nowElapsed)
      } else {
        MasqRouteProofRefreshSchedule()
      }
    }
    if (!snapshot.isHealthyConnectedSession()) {
      return MasqRouteProofRefreshSchedule()
    }
    val reportedGeneration = snapshot.routeProofGeneration.coerceAtLeast(0L)
    val sameScope = accepts(snapshot, currentStartGeneration)
    if (!sameScope) {
      if (refreshAttempted) return MasqRouteProofRefreshSchedule()
      return MasqRouteProofRefreshSchedule(
          startGeneration = currentStartGeneration,
          engineGeneration = snapshot.engineGeneration,
          observedGeneration = reportedGeneration,
          deadlineElapsed = nowElapsed + ROUTE_PROOF_REFRESH_INTERVAL_MILLIS,
      )
    }
    val proofAdvanced = reportedGeneration > observedGeneration
    if (proofAdvanced || (refreshAttempted && refreshSucceeded(snapshot, currentStartGeneration))) {
      return copy(
          observedGeneration = maxOf(observedGeneration, reportedGeneration),
          deadlineElapsed = nowElapsed + ROUTE_PROOF_REFRESH_INTERVAL_MILLIS,
          consecutiveFailures = 0,
      )
    }
    if (refreshAttempted) return afterRefreshFailure(nowElapsed)
    return this
  }

  private fun afterRefreshFailure(nowElapsed: Long): MasqRouteProofRefreshSchedule {
    if (startGeneration <= 0L || engineGeneration <= 0L) {
      return MasqRouteProofRefreshSchedule()
    }
    val nextFailureCount = (consecutiveFailures + 1).coerceAtMost(3)
    val retryDelay =
        (ROUTE_PROOF_REFRESH_RETRY_INITIAL_MILLIS shl (nextFailureCount - 1))
            .coerceAtMost(ROUTE_PROOF_REFRESH_RETRY_MAX_MILLIS)
    return copy(
        deadlineElapsed = nowElapsed + retryDelay,
        consecutiveFailures = nextFailureCount,
    )
  }
}

internal enum class MasqPeriodicRouteProofFailureAction {
  RETAIN_ROUTE,
  FAIL_CLOSED_RESTART,
}

internal fun isNonMutatingRouteProofRefreshFailure(
    routeProofRefreshAttempted: Boolean,
    refreshSucceeded: Boolean,
    snapshot: MasqSessionCoreSnapshot?,
): Boolean =
    routeProofRefreshAttempted &&
        !refreshSucceeded &&
        (snapshot == null || snapshot.isHealthyConnectedSession())

internal fun masqPeriodicRouteProofFailureAction(
    routeProofRefreshAttempted: Boolean,
    forcedNetworkRouteProof: Boolean,
    refreshSucceeded: Boolean,
    nonMutatingRefreshFailure: Boolean,
    schedule: MasqRouteProofRefreshSchedule,
    currentStartGeneration: Long,
): MasqPeriodicRouteProofFailureAction =
    when {
      !routeProofRefreshAttempted ||
          forcedNetworkRouteProof ||
          refreshSucceeded ||
          !nonMutatingRefreshFailure -> MasqPeriodicRouteProofFailureAction.RETAIN_ROUTE
      schedule.startGeneration != currentStartGeneration ||
          schedule.engineGeneration <= 0L -> MasqPeriodicRouteProofFailureAction.RETAIN_ROUTE
      schedule.consecutiveFailures >= ROUTE_PROOF_REFRESH_FAILURES_BEFORE_RESTART ->
          MasqPeriodicRouteProofFailureAction.FAIL_CLOSED_RESTART
      else -> MasqPeriodicRouteProofFailureAction.RETAIN_ROUTE
    }

internal data class MasqPeriodicRouteProofRestartScope(
    val sessionGeneration: Long,
    val startGeneration: Long,
    val engineGeneration: Long,
    val networkEpoch: Long,
) {
  fun applies(
      currentSessionGeneration: Long,
      currentStartGeneration: Long,
      currentNetworkEpoch: Long,
      networkAvailable: Boolean,
  ): Boolean =
      networkAvailable &&
          sessionGeneration > 0L &&
          startGeneration > 0L &&
          engineGeneration > 0L &&
          sessionGeneration == currentSessionGeneration &&
          startGeneration == currentStartGeneration &&
          networkEpoch == currentNetworkEpoch
}

internal data class MasqCoreRouteRestartEscalationScope(
    val startGeneration: Long,
    val engineGeneration: Long,
    val networkEpoch: Long,
) {
  init {
    require(startGeneration > 0L)
    require(engineGeneration > 0L)
    require(networkEpoch > 0L)
  }

  fun applies(
      currentStartGeneration: Long,
      currentNetworkEpoch: Long,
      networkAvailable: Boolean,
  ): Boolean =
      networkAvailable &&
          startGeneration == currentStartGeneration &&
          networkEpoch == currentNetworkEpoch
}

internal enum class MasqCoreRouteRestartEscalationDecision {
  REJECT_STALE,
  DEDUPLICATE,
  SCHEDULE,
}

internal fun masqCoreRouteRestartEscalationDecision(
    existing: MasqCoreRouteRestartEscalationScope?,
    requested: MasqCoreRouteRestartEscalationScope,
    currentStartGeneration: Long,
    currentNetworkEpoch: Long,
    networkAvailable: Boolean,
): MasqCoreRouteRestartEscalationDecision =
    when {
      !requested.applies(
          currentStartGeneration = currentStartGeneration,
          currentNetworkEpoch = currentNetworkEpoch,
          networkAvailable = networkAvailable,
      ) -> MasqCoreRouteRestartEscalationDecision.REJECT_STALE
      existing == requested -> MasqCoreRouteRestartEscalationDecision.DEDUPLICATE
      else -> MasqCoreRouteRestartEscalationDecision.SCHEDULE
    }

internal enum class MasqCoreRouteRestartNativeAction {
  SHUTDOWN_EXACT_HEALTHY_ENGINE,
  RECOVER_UNHEALTHY_ENGINE,
  IGNORE_SUPERSEDED_ENGINE,
}

internal fun masqCoreRouteRestartNativeAction(
    snapshot: MasqSessionCoreSnapshot?,
    expectedEngineGeneration: Long,
): MasqCoreRouteRestartNativeAction =
    when {
      snapshot?.engineGeneration != expectedEngineGeneration ->
          MasqCoreRouteRestartNativeAction.IGNORE_SUPERSEDED_ENGINE
      snapshot.isHealthyConnectedSession() ->
          MasqCoreRouteRestartNativeAction.SHUTDOWN_EXACT_HEALTHY_ENGINE
      else -> MasqCoreRouteRestartNativeAction.RECOVER_UNHEALTHY_ENGINE
    }

internal fun isSuccessfulPeriodicRouteProofRestartSnapshot(
    snapshot: MasqSessionCoreSnapshot?,
    expectedEngineGeneration: Long,
): Boolean =
    expectedEngineGeneration > 0L &&
        snapshot?.engineGeneration == expectedEngineGeneration &&
        isSuccessfulNetworkRouteRestartSnapshot(snapshot)

internal fun scheduleAfterForcedNetworkProof(
    snapshot: MasqSessionCoreSnapshot?,
    currentStartGeneration: Long,
    nowElapsed: Long,
): MasqRouteProofRefreshSchedule =
    MasqRouteProofRefreshSchedule()
        .afterSnapshot(
            snapshot = snapshot,
            currentStartGeneration = currentStartGeneration,
            nowElapsed = nowElapsed,
            refreshAttempted = false,
        )

internal const val RECOVERY_STARTED_GRACE_MILLIS = 90_000L
internal const val STAGE_ONE_ROUTE_PROOF_SETTLE_MILLIS = 2_000L
private const val MAX_SESSION_RECOVERY_ATTEMPTS = 3
private val SESSION_RECOVERY_BACKOFF_MILLIS =
    longArrayOf(5_000L, 15_000L, 60_000L, 5 * 60_000L)
private val TERMINAL_ENTRY_RECOVERY_BACKOFF_MILLIS =
    longArrayOf(1_000L, 2_000L, 5_000L, 15_000L)
private val ROUTE_REBUILD_RECOVERY_BACKOFF_MILLIS =
    longArrayOf(2_000L, 5_000L, 15_000L, 30_000L)

internal data class MasqSessionRecoveryBackoff(
    val attempts: Int = 0,
    val notBeforeElapsed: Long = 0L,
    val stageOneProofNotBeforeElapsed: Long = 0L,
    val routeRebuildFailures: Int = 0,
) {
  fun afterHealthy(): MasqSessionRecoveryBackoff = MasqSessionRecoveryBackoff()

  fun afterStarted(nowElapsed: Long): MasqSessionRecoveryBackoff =
      copy(
          attempts = (attempts + 1).coerceAtMost(MAX_SESSION_RECOVERY_ATTEMPTS),
          notBeforeElapsed = nowElapsed + RECOVERY_STARTED_GRACE_MILLIS,
          stageOneProofNotBeforeElapsed =
              nowElapsed + STAGE_ONE_ROUTE_PROOF_SETTLE_MILLIS,
          routeRebuildFailures = 0,
      )

  fun afterFailed(): MasqSessionRecoveryBackoff =
      copy(
          attempts = (attempts + 1).coerceAtMost(MAX_SESSION_RECOVERY_ATTEMPTS),
          stageOneProofNotBeforeElapsed = 0L,
          routeRebuildFailures = 0,
      )

  fun afterStageOneProofFailed(nowElapsed: Long): MasqSessionRecoveryBackoff =
      copy(
          attempts = (attempts + 1).coerceAtMost(MAX_SESSION_RECOVERY_ATTEMPTS),
          notBeforeElapsed = nowElapsed,
          stageOneProofNotBeforeElapsed = 0L,
          routeRebuildFailures =
              (routeRebuildFailures + 1)
                  .coerceAtMost(ROUTE_REBUILD_RECOVERY_BACKOFF_MILLIS.size),
      )

  /**
   * A structured E_ENTRY_* diagnostic is emitted only after the native core's
   * own bounded handshake window has expired. It is therefore safe to release
   * the generic startup grace immediately while retaining the accumulated
   * retry count for later non-terminal failures.
   */
  fun afterTerminalEntrySignal(nowElapsed: Long): MasqSessionRecoveryBackoff =
      copy(
          notBeforeElapsed = nowElapsed,
          stageOneProofNotBeforeElapsed = 0L,
          routeRebuildFailures = 0,
      )

  fun terminalEntryRetryDelayMillis(): Long =
      TERMINAL_ENTRY_RECOVERY_BACKOFF_MILLIS[
          (attempts - 1).coerceIn(0, TERMINAL_ENTRY_RECOVERY_BACKOFF_MILLIS.lastIndex)]

  fun allowsAttempt(nowElapsed: Long): Boolean = nowElapsed >= notBeforeElapsed

  fun hasStageOneProofOpportunity(): Boolean = stageOneProofNotBeforeElapsed > 0L

  fun stageOneProofDelayMillis(nowElapsed: Long): Long =
      (stageOneProofNotBeforeElapsed - nowElapsed).coerceAtLeast(0L)

  fun routeRebuildRetryDelayMillis(): Long =
      ROUTE_REBUILD_RECOVERY_BACKOFF_MILLIS[
          (routeRebuildFailures - 1)
              .coerceIn(0, ROUTE_REBUILD_RECOVERY_BACKOFF_MILLIS.lastIndex)]

  fun nextDelayMillis(): Long =
      SESSION_RECOVERY_BACKOFF_MILLIS[
          attempts.coerceIn(0, SESSION_RECOVERY_BACKOFF_MILLIS.lastIndex)]
}

internal data class MasqStageOneProofScope(
    val identity: MasqRecoveryAttemptIdentity,
    val networkEpoch: Long,
) {
  init {
    require(networkEpoch > 0L)
  }

  fun applies(
      currentStartGeneration: Long,
      currentNetworkEpoch: Long,
  ): Boolean =
      identity.startGeneration == currentStartGeneration &&
          networkEpoch == currentNetworkEpoch
}

internal const val SESSION_RESTORE_DISPATCH_RETRY_MILLIS = 5_000L

internal enum class MasqSessionEnsureDecision {
  NOT_DESIRED,
  ALREADY_RUNNING,
  RETRY_THROTTLED,
  DISPATCH_RESTORE,
}

internal fun masqSessionEnsureDecision(
    persistedDesired: Boolean,
    activeInstanceLive: Boolean,
    nowElapsed: Long,
    lastDispatchElapsed: Long,
): MasqSessionEnsureDecision =
    when {
      !persistedDesired -> MasqSessionEnsureDecision.NOT_DESIRED
      activeInstanceLive -> MasqSessionEnsureDecision.ALREADY_RUNNING
      lastDispatchElapsed > 0L &&
          nowElapsed >= lastDispatchElapsed &&
          nowElapsed - lastDispatchElapsed < SESSION_RESTORE_DISPATCH_RETRY_MILLIS ->
          MasqSessionEnsureDecision.RETRY_THROTTLED
      else -> MasqSessionEnsureDecision.DISPATCH_RESTORE
    }

internal enum class MasqSessionNetworkTransition {
  UNCHANGED,
  LOST,
  RESTORED,
  REPLACED,
}

internal enum class MasqSessionNetworkRouteAction {
  STATUS,
  PROVE,
  RESTART,
}

internal enum class MasqSessionNetworkObservationAction {
  APPLY,
  DEFER_NEW,
  DEFER_EXISTING,
}

/**
 * Coalesces Android's duplicate availability/capability callbacks before they can advance the
 * native start generation. A tracked `onLost` remains authoritative and applies immediately;
 * implicit validation gaps or default-network replacements must survive one bounded re-sample.
 */
internal class MasqSessionNetworkTransitionCoalescer {
  private var hasPendingObservation = false
  private var pendingNetworkId: Long? = null

  fun observe(
      previousNetworkId: Long?,
      observedNetworkId: Long?,
      explicitlyLostNetworkId: Long?,
      confirmed: Boolean,
  ): MasqSessionNetworkObservationAction {
    val appliesImmediately =
        confirmed ||
            previousNetworkId == null ||
            observedNetworkId == previousNetworkId ||
            explicitlyLostNetworkId == previousNetworkId
    if (appliesImmediately) {
      reset()
      return MasqSessionNetworkObservationAction.APPLY
    }
    if (hasPendingObservation && pendingNetworkId == observedNetworkId) {
      return MasqSessionNetworkObservationAction.DEFER_EXISTING
    }
    hasPendingObservation = true
    pendingNetworkId = observedNetworkId
    return MasqSessionNetworkObservationAction.DEFER_NEW
  }

  fun reset() {
    hasPendingObservation = false
    pendingNetworkId = null
  }
}

internal data class MasqSessionActiveRouteSource(
    val networkId: Long,
    val engineGeneration: Long,
)

internal data class MasqSessionNetworkCandidate(
    val networkId: Long,
    val hasInternet: Boolean,
    val validated: Boolean,
    val notVpn: Boolean,
    val vpnTransport: Boolean = false,
    val matchesActiveVpnTransport: Boolean = false,
    val transportPreference: Int = 3,
)

internal data class MasqValidatedUnderlayNetwork(
    val network: Network,
    val capabilities: NetworkCapabilities,
)

internal fun selectMasqValidatedUnderlayNetworkId(
    activeNetworkId: Long?,
    previousNetworkId: Long?,
    candidates: List<MasqSessionNetworkCandidate>,
): Long? {
  val eligible =
      candidates
          .asSequence()
          .filter {
            it.hasInternet && it.validated && it.notVpn && !it.vpnTransport
          }
          .toList()
  val eligibleIds = eligible.mapTo(mutableSetOf()) { it.networkId }
  return when {
    activeNetworkId in eligibleIds -> activeNetworkId
    previousNetworkId in eligibleIds -> previousNetworkId
    else ->
        eligible
            .minWithOrNull(
                compareBy<MasqSessionNetworkCandidate> {
                      if (it.matchesActiveVpnTransport) 0 else 1
                    }
                    .thenBy { it.transportPreference }
                    .thenBy { it.networkId })
            ?.networkId
  }
}

private fun masqPhysicalTransportPreference(capabilities: NetworkCapabilities?): Int =
    when {
      capabilities?.hasTransport(NetworkCapabilities.TRANSPORT_ETHERNET) == true -> 0
      capabilities?.hasTransport(NetworkCapabilities.TRANSPORT_WIFI) == true -> 1
      capabilities?.hasTransport(NetworkCapabilities.TRANSPORT_CELLULAR) == true -> 2
      else -> 3
    }

private fun matchesMasqActiveVpnTransport(
    activeCapabilities: NetworkCapabilities?,
    candidateCapabilities: NetworkCapabilities?,
): Boolean =
    activeCapabilities?.hasTransport(NetworkCapabilities.TRANSPORT_VPN) == true &&
        candidateCapabilities != null &&
        listOf(
                NetworkCapabilities.TRANSPORT_ETHERNET,
                NetworkCapabilities.TRANSPORT_WIFI,
                NetworkCapabilities.TRANSPORT_CELLULAR,
            )
            .any { transport ->
              activeCapabilities.hasTransport(transport) &&
                  candidateCapabilities.hasTransport(transport)
            }

@Suppress("DEPRECATION")
internal fun resolveMasqValidatedUnderlayNetwork(
    manager: ConnectivityManager,
    previousNetwork: Network? = null,
    previousNetworkId: Long? = previousNetwork?.networkHandle,
    excludedNetwork: Network? = null,
): MasqValidatedUnderlayNetwork? =
    runCatching {
          val activeNetwork = manager.activeNetwork
          val activeCapabilities = manager.getNetworkCapabilities(activeNetwork)
          val excludedNetworkId = excludedNetwork?.networkHandle
          val networksById = linkedMapOf<Long, Network>()
          fun addCandidate(network: Network?) {
            if (network == null || network.networkHandle == excludedNetworkId) return
            networksById.putIfAbsent(network.networkHandle, network)
          }
          addCandidate(activeNetwork)
          addCandidate(previousNetwork)
          manager.allNetworks.forEach(::addCandidate)

          val capabilitiesById = linkedMapOf<Long, NetworkCapabilities>()
          val candidates =
              networksById.values.map { network ->
                val capabilities = manager.getNetworkCapabilities(network)
                if (capabilities != null) {
                  capabilitiesById[network.networkHandle] = capabilities
                }
                MasqSessionNetworkCandidate(
                    networkId = network.networkHandle,
                    hasInternet =
                        capabilities?.hasCapability(
                            NetworkCapabilities.NET_CAPABILITY_INTERNET) == true,
                    validated =
                        capabilities?.hasCapability(
                            NetworkCapabilities.NET_CAPABILITY_VALIDATED) == true,
                    notVpn =
                        capabilities?.hasCapability(
                            NetworkCapabilities.NET_CAPABILITY_NOT_VPN) == true,
                    vpnTransport =
                        capabilities?.hasTransport(NetworkCapabilities.TRANSPORT_VPN) == true,
                    matchesActiveVpnTransport =
                        matchesMasqActiveVpnTransport(activeCapabilities, capabilities),
                    transportPreference = masqPhysicalTransportPreference(capabilities),
                )
              }
          val selectedId =
              selectMasqValidatedUnderlayNetworkId(
                  activeNetworkId = activeNetwork?.networkHandle,
                  previousNetworkId = previousNetworkId,
                  candidates = candidates,
              )
          val selectedNetwork = selectedId?.let(networksById::get)
          val selectedCapabilities = selectedId?.let(capabilitiesById::get)
          if (selectedNetwork == null || selectedCapabilities == null) {
            null
          } else {
            MasqValidatedUnderlayNetwork(selectedNetwork, selectedCapabilities)
          }
        }
        .getOrNull()

internal fun masqSessionNetworkRouteAction(
    proofRequiredEpoch: Long,
    restartRequiredEpoch: Long,
    currentNetworkEpoch: Long,
): MasqSessionNetworkRouteAction =
    when {
      currentNetworkEpoch <= 0L -> MasqSessionNetworkRouteAction.STATUS
      restartRequiredEpoch == currentNetworkEpoch -> MasqSessionNetworkRouteAction.RESTART
      proofRequiredEpoch == currentNetworkEpoch -> MasqSessionNetworkRouteAction.PROVE
      else -> MasqSessionNetworkRouteAction.STATUS
    }

internal fun isSuccessfulNetworkRouteRestartSnapshot(
    snapshot: MasqSessionCoreSnapshot?,
): Boolean = snapshot?.phase in setOf("paused", "ready", "unconfigured")

internal fun isSuccessfulNetworkRouteShutdownSnapshot(
    snapshot: MasqSessionCoreSnapshot?,
): Boolean = snapshot?.phase in setOf("ready", "unconfigured")

internal fun shouldConsumeMasqForcedNetworkProofEpoch(
    proofRequiredEpoch: Long,
    currentNetworkEpoch: Long,
    stopSucceeded: Boolean,
): Boolean =
    stopSucceeded &&
        currentNetworkEpoch > 0L &&
        proofRequiredEpoch == currentNetworkEpoch

internal fun shouldUpgradeMasqProofToRestartAfterSourceLoss(
    lostNetworkId: Long?,
    proofSourceNetworkId: Long?,
    proofRequiredEpoch: Long,
    currentNetworkEpoch: Long,
): Boolean =
    lostNetworkId != null &&
        lostNetworkId == proofSourceNetworkId &&
        currentNetworkEpoch > 0L &&
        proofRequiredEpoch == currentNetworkEpoch

internal fun shouldRestartAfterActiveRouteSourceLoss(
    lostNetworkId: Long?,
    activeRouteSource: MasqSessionActiveRouteSource?,
): Boolean =
    lostNetworkId != null &&
        activeRouteSource != null &&
        activeRouteSource.engineGeneration > 0L &&
        lostNetworkId == activeRouteSource.networkId

internal fun shouldDeferRecoveryToModuleOwnedConnectionAttempt(
    moduleOwnsAttempt: Boolean,
    moduleStartGeneration: Long,
    currentStartGeneration: Long,
    moduleNetworkEpoch: Long,
    currentNetworkEpoch: Long,
    nowElapsed: Long,
    moduleDeadlineElapsed: Long,
    snapshotHealthy: Boolean,
): Boolean =
    moduleOwnsAttempt &&
        moduleStartGeneration > 0L &&
        moduleStartGeneration == currentStartGeneration &&
        moduleNetworkEpoch == currentNetworkEpoch &&
        !snapshotHealthy &&
        moduleDeadlineElapsed > 0L &&
        nowElapsed < moduleDeadlineElapsed

internal fun shouldAcceptModuleOwnedConnectionAttempt(
    networkAvailable: Boolean,
    pendingNetworkAction: MasqSessionNetworkRouteAction,
    pendingServiceRouteMutation: Boolean = false,
): Boolean =
    networkAvailable &&
        pendingNetworkAction == MasqSessionNetworkRouteAction.STATUS &&
        !pendingServiceRouteMutation

internal fun shouldInvalidateModuleOwnedConnectionAttemptForNetworkTransition(
    moduleOwnsAttempt: Boolean,
    moduleStartGeneration: Long,
    currentStartGeneration: Long,
    moduleNetworkEpoch: Long,
    currentNetworkEpoch: Long,
): Boolean =
    moduleOwnsAttempt &&
        moduleStartGeneration > 0L &&
        moduleStartGeneration == currentStartGeneration &&
        moduleNetworkEpoch != currentNetworkEpoch

internal fun masqSessionNetworkTransition(
    previousAvailable: Boolean,
    previousNetworkId: Long?,
    currentAvailable: Boolean,
    currentNetworkId: Long?,
): MasqSessionNetworkTransition =
    when {
      previousAvailable && !currentAvailable -> MasqSessionNetworkTransition.LOST
      !previousAvailable && currentAvailable -> MasqSessionNetworkTransition.RESTORED
      previousAvailable &&
          currentAvailable &&
          previousNetworkId != null &&
          currentNetworkId != null &&
          previousNetworkId != currentNetworkId -> MasqSessionNetworkTransition.REPLACED
      else -> MasqSessionNetworkTransition.UNCHANGED
    }

internal fun masqSessionNotificationText(state: MasqSessionNotificationState): String =
    when (state) {
      MasqSessionNotificationState.CONNECTING ->
          "Building or restoring a private MASQ connection."
      MasqSessionNotificationState.CONNECTED ->
          "Private MASQ connection remains active while the screen is locked."
      MasqSessionNotificationState.ATTENTION ->
          "Waiting to restore the private MASQ connection."
    }

internal class MasqSessionIntentStore(context: Context) {
  private val applicationContext = context.applicationContext
  private val preferences =
      applicationContext.getSharedPreferences(PREFERENCES_NAME, Context.MODE_PRIVATE)

  fun isDesired(): Boolean = preferences.getBoolean(DESIRED_KEY, false)

  fun setDesired(desired: Boolean): Boolean =
      if (desired) {
        preferences.edit().putBoolean(DESIRED_KEY, true).commit()
      } else {
        preferences.edit().remove(DESIRED_KEY).commit()
      }

  fun clearDesiredFailClosed(): Boolean {
    if (setDesired(false) && !isDesired()) return true
    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.N) {
      if (applicationContext.deleteSharedPreferences(PREFERENCES_NAME)) return true
    }
    return !isDesired()
  }

  private companion object {
    const val PREFERENCES_NAME = "masq-mobile-background-session"
    const val DESIRED_KEY = "consumer-session-desired"
  }
}

/**
 * Keeps the in-process MASQ consumer engine alive during screen lock and app backgrounding.
 *
 * This service never creates a VPN interface and never changes whole-device or per-app routing.
 */
class MasqSessionService : Service() {
  private val mainHandler = Handler(Looper.getMainLooper())
  private val monitorInFlight = AtomicBoolean(false)
  private val recoveryEpoch = AtomicLong(0L)
  private val recoveryExecutor = Executors.newSingleThreadExecutor()
  private lateinit var intentStore: MasqSessionIntentStore
  private lateinit var recovery: MasqBackgroundSessionRecovery
  private lateinit var connectivityManager: ConnectivityManager
  private var generation = NO_GENERATION
  private var latestStartId = 0
  private var foregroundStarted = false
  private var destroyed = false
  private var stoppingExplicitly = false
  private var terminalObservations = 0
  private var recoveryBackoff = MasqSessionRecoveryBackoff()
  private var moduleStartupDeadlineElapsed = 0L
  private var moduleOwnsConnectionAttempt = false
  private var moduleOwnedStartGeneration = NO_GENERATION
  private var moduleOwnedNetworkEpoch = 0L
  private var connectingProgressDeadlineElapsed = 0L
  private var connectingNeighbors = 0
  private var connectingRouteStage = 0
  private var routeProofRefreshSchedule = MasqRouteProofRefreshSchedule()
  private var periodicRouteProofRestartScope: MasqPeriodicRouteProofRestartScope? = null
  private var coreRouteRestartEscalationScope: MasqCoreRouteRestartEscalationScope? = null
  private var recoveryDelayRunnable: Runnable? = null
  private var recoveryFuture: Future<*>? = null
  private var recoveryRunningToken = NO_RECOVERY_TOKEN
  private var networkAvailable = false
  private var validatedNetwork: Network? = null
  private val networkEpoch = AtomicLong(0L)
  private var networkRouteProofRequiredEpoch = 0L
  private var networkRouteProofSourceNetworkId: Long? = null
  private var networkRouteRestartRequiredEpoch = 0L
  private var activeRouteSource: MasqSessionActiveRouteSource? = null
  private var definitiveNetworkLossPending = false
  private var unavailableNetworkAwaitingLossConfirmation: Network? = null
  private val networkTransitionCoalescer = MasqSessionNetworkTransitionCoalescer()
  private var networkTransitionConfirmationRunnable: Runnable? = null
  private var screenOff = false
  private var cpuRequired = false
  private var wakeLock: PowerManager.WakeLock? = null
  private var networkCallbackRegistered = false
  private var screenReceiverRegistered = false

  private val monitorRunnable =
      object : Runnable {
        override fun run() {
          if (destroyed || !isSessionDesired()) return
          // Default-network callbacks are best effort and their registration
          // can fail. Re-sample on the existing bounded monitor cadence so a
          // lost/restored or replaced network cannot leave recovery dormant.
          refreshNetworkState()
          if (!networkAvailable) {
            mainHandler.postDelayed(this, STATUS_POLL_INTERVAL_MILLIS)
            return
          }
          if (!monitorInFlight.compareAndSet(false, true)) {
            mainHandler.postDelayed(this, STATUS_POLL_INTERVAL_MILLIS)
            return
          }
          val monitoredSessionGeneration = generation
          val monitoredStartGeneration = MasqCoreLifecycle.startGeneration.get()
          val monitoredNetworkEpoch = networkEpoch.get()
          val networkRouteAction =
              masqSessionNetworkRouteAction(
                  proofRequiredEpoch = networkRouteProofRequiredEpoch,
                  restartRequiredEpoch = networkRouteRestartRequiredEpoch,
                  currentNetworkEpoch = monitoredNetworkEpoch,
              )
          val pendingCoreRouteRestart = coreRouteRestartEscalationScope
          val forcedCoreRouteRestart =
              pendingCoreRouteRestart?.applies(
                  currentStartGeneration = monitoredStartGeneration,
                  currentNetworkEpoch = monitoredNetworkEpoch,
                  networkAvailable = networkAvailable,
              ) == true
          if (forcedCoreRouteRestart) {
            logMasqRecovery(
                "CORE_RESTART_DISPATCH start_generation=$monitoredStartGeneration " +
                    "engine_generation=${pendingCoreRouteRestart?.engineGeneration ?: 0L} " +
                    "network_epoch=$monitoredNetworkEpoch",
            )
          }
          if (pendingCoreRouteRestart != null && !forcedCoreRouteRestart) {
            // A newer native start or Android network supersedes the exact stale
            // engine whose end-to-end VPN preflight failed.
            coreRouteRestartEscalationScope = null
          }
          val forcedNetworkRouteRestart =
              networkRouteAction == MasqSessionNetworkRouteAction.RESTART
          val forcedNetworkRouteProof =
              networkRouteAction == MasqSessionNetworkRouteAction.PROVE &&
                  !forcedCoreRouteRestart
          val pendingPeriodicRouteProofRestart = periodicRouteProofRestartScope
          val periodicRouteProofRestart =
              networkRouteAction == MasqSessionNetworkRouteAction.STATUS &&
                  !forcedCoreRouteRestart &&
                  pendingPeriodicRouteProofRestart?.applies(
                      currentSessionGeneration = monitoredSessionGeneration,
                      currentStartGeneration = monitoredStartGeneration,
                      currentNetworkEpoch = monitoredNetworkEpoch,
                      networkAvailable = networkAvailable,
                  ) == true
          if (
              pendingPeriodicRouteProofRestart != null &&
                  !periodicRouteProofRestart &&
                  networkRouteAction == MasqSessionNetworkRouteAction.STATUS
          ) {
            // A lifecycle or network generation change invalidates a queued
            // periodic stop before it can mutate a newer native route.
            periodicRouteProofRestartScope = null
          }
          val routeRestart =
              forcedNetworkRouteRestart ||
                  forcedCoreRouteRestart ||
                  periodicRouteProofRestart
          val refreshRouteProof =
              networkAvailable &&
                  !routeRestart &&
                  (forcedNetworkRouteProof ||
                      routeProofRefreshSchedule.isDue(
                          SystemClock.elapsedRealtime(),
                          monitoredStartGeneration,
                      ))
          val expectedRefreshEngineGeneration =
              if (refreshRouteProof) routeProofRefreshSchedule.engineGeneration else 0L
          MasqCoreLifecycle.executor.execute {
            var coreRouteRestartRecoveryWithoutShutdown = false
            var coreRouteRestartSuperseded = false
            var snapshot =
                if (!MasqCoreJni.isAvailable) {
                  null
                } else {
                  runCatching {
                        masqSessionCoreSnapshot(
                            when {
                              forcedNetworkRouteRestart &&
                                  monitoredSessionGeneration == generation &&
                                  monitoredStartGeneration ==
                                      MasqCoreLifecycle.startGeneration.get() &&
                                  monitoredNetworkEpoch == networkEpoch.get() ->
                                  MasqCoreJni.nativeShutdown()
                              forcedCoreRouteRestart &&
                                  monitoredSessionGeneration == generation &&
                                  monitoredStartGeneration ==
                                      MasqCoreLifecycle.startGeneration.get() &&
                                  monitoredNetworkEpoch == networkEpoch.get() -> {
                                val observedJson = MasqCoreJni.nativeGetStatus()
                                val observed = masqSessionCoreSnapshot(observedJson)
                                when (
                                    masqCoreRouteRestartNativeAction(
                                        snapshot = observed,
                                        expectedEngineGeneration =
                                            pendingCoreRouteRestart?.engineGeneration ?: 0L,
                                    )) {
                                  MasqCoreRouteRestartNativeAction.SHUTDOWN_EXACT_HEALTHY_ENGINE ->
                                      MasqCoreJni.nativeShutdown()
                                  MasqCoreRouteRestartNativeAction.RECOVER_UNHEALTHY_ENGINE -> {
                                    coreRouteRestartRecoveryWithoutShutdown = true
                                    observedJson
                                  }
                                  MasqCoreRouteRestartNativeAction.IGNORE_SUPERSEDED_ENGINE -> {
                                    coreRouteRestartSuperseded = true
                                    observedJson
                                  }
                                }
                              }
                              periodicRouteProofRestart &&
                                  monitoredSessionGeneration == generation &&
                                  monitoredStartGeneration ==
                                      MasqCoreLifecycle.startGeneration.get() &&
                                  monitoredNetworkEpoch == networkEpoch.get() ->
                                  MasqCoreJni.nativeStop()
                              routeRestart -> return@runCatching null
                              refreshRouteProof -> MasqCoreJni.nativeRefreshRouteProof()
                              else -> MasqCoreJni.nativeGetStatus()
                            })
                      }
                      .getOrNull()
                }
            val forcedRouteRestartSucceeded =
                (forcedNetworkRouteRestart || forcedCoreRouteRestart) &&
                    isSuccessfulNetworkRouteShutdownSnapshot(snapshot) &&
                    (!forcedCoreRouteRestart ||
                        snapshot?.engineGeneration == pendingCoreRouteRestart?.engineGeneration)
            val periodicRouteProofRestartSucceeded =
                periodicRouteProofRestart &&
                    isSuccessfulPeriodicRouteProofRestartSnapshot(
                        snapshot,
                        pendingPeriodicRouteProofRestart?.engineGeneration ?: 0L,
                    )
            val forcedNetworkProofSucceeded =
                forcedNetworkRouteProof &&
                    snapshot?.isHealthyConnectedSession() == true &&
                    snapshot.routeProofRefresh?.let { it.attempted && it.succeeded } == true
            var forcedNetworkProofShutdownSucceeded = false
            if (
                forcedNetworkRouteProof &&
                    !forcedNetworkProofSucceeded &&
                    monitoredStartGeneration == MasqCoreLifecycle.startGeneration.get() &&
                    monitoredNetworkEpoch == networkEpoch.get() &&
                    MasqCoreJni.isAvailable
            ) {
              // A route that belonged to a replaced Android network cannot be
              // accepted through BackgroundRecovery's healthy short-circuit.
              // Fully tear down that stale engine and its old-network sockets
              // before rebuilding it. A normal nativeStop deliberately keeps
              // the actor runtime warm and is therefore insufficient here.
              snapshot =
                  runCatching { masqSessionCoreSnapshot(MasqCoreJni.nativeShutdown()) }
                      .getOrNull()
              forcedNetworkProofShutdownSucceeded =
                  isSuccessfulNetworkRouteShutdownSnapshot(snapshot)
            }
            val completedStartGeneration = MasqCoreLifecycle.startGeneration.get()
            mainHandler.post {
              monitorInFlight.set(false)
              if (!destroyed && isSessionDesired()) {
                val sessionGenerationStable = generation == monitoredSessionGeneration
                val startGenerationStable =
                    monitoredStartGeneration == completedStartGeneration &&
                        monitoredStartGeneration == MasqCoreLifecycle.startGeneration.get()
                val refreshEngineStable =
                    !refreshRouteProof ||
                        expectedRefreshEngineGeneration <= 0L ||
                        snapshot == null ||
                        snapshot.engineGeneration == expectedRefreshEngineGeneration
                if (
                    shouldApplyMasqSessionSnapshot(
                        monitoredSessionGeneration = monitoredSessionGeneration,
                        currentSessionGeneration = generation,
                        monitoredStartGeneration = monitoredStartGeneration,
                        completedStartGeneration = completedStartGeneration,
                        currentStartGeneration = MasqCoreLifecycle.startGeneration.get(),
                        refreshRouteProof = refreshRouteProof,
                        expectedRefreshEngineGeneration = expectedRefreshEngineGeneration,
                        snapshot = snapshot,
                        networkAvailable = networkAvailable,
                        monitoredNetworkEpoch = monitoredNetworkEpoch,
                        currentNetworkEpoch = networkEpoch.get(),
                    )
                ) {
                  if (
                      coreRouteRestartSuperseded &&
                          coreRouteRestartEscalationScope == pendingCoreRouteRestart
                  ) {
                    coreRouteRestartEscalationScope = null
                  }
                  applyCoreSnapshot(
                      snapshot,
                      monitoredStartGeneration,
                      refreshRouteProof,
                      forcedNetworkRouteProof,
                      forcedNetworkProofSucceeded,
                      forcedNetworkProofShutdownSucceeded,
                      forcedNetworkRouteRestart,
                      forcedCoreRouteRestart && !coreRouteRestartSuperseded,
                      forcedRouteRestartSucceeded,
                      coreRouteRestartRecoveryWithoutShutdown,
                      periodicRouteProofRestart,
                      periodicRouteProofRestartSucceeded,
                      pendingPeriodicRouteProofRestart,
                      pendingCoreRouteRestart,
                  )
                } else if (
                    sessionGenerationStable &&
                        (!startGenerationStable || !refreshEngineStable)
                ) {
                  // Never attach an old status/probe to a newer native attempt.
                  routeProofRefreshSchedule = MasqRouteProofRefreshSchedule()
                }
                mainHandler.postDelayed(this, STATUS_POLL_INTERVAL_MILLIS)
              }
            }
          }
        }
      }

  private val renewWakeLockRunnable =
      object : Runnable {
        override fun run() {
          if (destroyed || !shouldHoldWakeLock()) {
            releaseWakeLock()
            return
          }
          acquireTimedWakeLock(forceRenewal = true)
        }
      }

  private val screenReceiver =
      object : BroadcastReceiver() {
        override fun onReceive(context: Context?, intent: Intent?) {
          when (intent?.action) {
            Intent.ACTION_SCREEN_OFF -> screenOff = true
            Intent.ACTION_SCREEN_ON -> screenOff = false
            else -> return
          }
          refreshWakeLock()
        }
      }

  private val networkCallback =
      object : ConnectivityManager.NetworkCallback() {
        override fun onAvailable(network: Network) {
          mainHandler.post { refreshNetworkState() }
        }

        override fun onLost(network: Network) {
          mainHandler.post { refreshNetworkState(lostNetwork = network) }
        }

        override fun onCapabilitiesChanged(
            network: Network,
            networkCapabilities: NetworkCapabilities,
        ) {
          mainHandler.post { refreshNetworkState() }
        }
      }

  override fun onCreate() {
    super.onCreate()
    networkEpoch.set(nextProcessNetworkEpoch())
    synchronized(lifecycleAuthorityLock) {
      activeInstance.set(this)
      stoppingExplicitly = false
    }
    intentStore = MasqSessionIntentStore(this)
    recovery = MasqBackgroundSessionRecovery(this, ::isSessionDesired)
    connectivityManager =
        getSystemService(Context.CONNECTIVITY_SERVICE) as ConnectivityManager
    screenOff = !(getSystemService(Context.POWER_SERVICE) as PowerManager).isInteractive
    validatedNetwork = currentValidatedNetwork()
    networkAvailable = validatedNetwork != null
    createNotificationChannel()
    registerLifecycleSignals()
  }

  override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
    synchronized(lifecycleAuthorityLock) {
      activeInstance.set(this)
      stoppingExplicitly = false
      lastRestoreDispatchElapsed.set(0L)
    }
    latestStartId = startId
    if (!intentStore.isDesired()) {
      desiredGeneration.set(NO_GENERATION)
      showForegroundState(MasqSessionNotificationState.ATTENTION)
      stopIfUndesired(startId)
      return START_NOT_STICKY
    }

    val requestedGeneration =
        intent?.getLongExtra(EXTRA_GENERATION, NO_GENERATION) ?: NO_GENERATION
    when {
      intent?.action == ACTION_KEEP_SESSION && requestedGeneration > NO_GENERATION -> {
        val newestGeneration =
            desiredGeneration.get().takeIf { it > NO_GENERATION }
                ?: requestedGeneration.also(desiredGeneration::set)
        if (requestedGeneration != newestGeneration) {
          completeModuleStartAdmission(
              requestedGeneration,
              MasqModuleStartAdmissionDecision.SERVICE_TAKEOVER,
          )
        }
        beginModuleOwnedGeneration(newestGeneration)
      }
      intent?.action == ACTION_RESTORE_SESSION && requestedGeneration > NO_GENERATION -> {
        val newestGeneration =
            desiredGeneration.get().takeIf { it > NO_GENERATION }
                ?: requestedGeneration.also(desiredGeneration::set)
        if (newestGeneration == requestedGeneration) {
          beginRestoredGeneration(newestGeneration)
        } else {
          // A newer user-driven start superseded this queued restoration.
          // Monitor that exact generation without launching competing recovery.
          beginModuleOwnedGeneration(newestGeneration)
        }
      }
      intent == null -> {
        val recoveryGeneration =
            desiredGeneration.updateAndGet { current ->
              if (current > NO_GENERATION) current else RECOVERY_GENERATION
            }
        beginRestoredGeneration(recoveryGeneration)
      }
      else -> {
        synchronized(lifecycleAuthorityLock) {
          desiredGeneration.set(NO_GENERATION)
          intentStore.clearDesiredFailClosed()
        }
        showForegroundState(MasqSessionNotificationState.ATTENTION)
        stopIfUndesired(startId)
        return START_NOT_STICKY
      }
    }
    return START_STICKY
  }

  override fun onBind(intent: Intent?): IBinder? = null

  override fun onDestroy() {
    destroyed = true
    completeModuleStartAdmission(
        moduleOwnedStartGeneration,
        MasqModuleStartAdmissionDecision.SERVICE_TAKEOVER,
    )
    synchronized(lifecycleAuthorityLock) {
      activeInstance.compareAndSet(this, null)
    }
    cancelRecovery()
    mainHandler.removeCallbacks(monitorRunnable)
    mainHandler.removeCallbacks(renewWakeLockRunnable)
    cancelNetworkTransitionConfirmation()
    networkTransitionCoalescer.reset()
    releaseWakeLock()
    if (screenReceiverRegistered) {
      runCatching { unregisterReceiver(screenReceiver) }
      screenReceiverRegistered = false
    }
    if (networkCallbackRegistered) {
      runCatching { connectivityManager.unregisterNetworkCallback(networkCallback) }
      networkCallbackRegistered = false
    }
    recoveryExecutor.shutdownNow()
    if (foregroundStarted) {
      stopForeground(STOP_FOREGROUND_REMOVE)
      foregroundStarted = false
    }
    super.onDestroy()
  }

  private fun beginModuleOwnedGeneration(nextGeneration: Long) {
    cancelRecovery()
    generation = nextGeneration
    val currentNetworkEpoch = networkEpoch.get()
    val pendingNetworkAction =
        masqSessionNetworkRouteAction(
            proofRequiredEpoch = networkRouteProofRequiredEpoch,
            restartRequiredEpoch = networkRouteRestartRequiredEpoch,
            currentNetworkEpoch = currentNetworkEpoch,
        )
    val pendingPeriodicRouteMutation =
        periodicRouteProofRestartScope?.applies(
            currentSessionGeneration = nextGeneration,
            currentStartGeneration = nextGeneration,
            currentNetworkEpoch = currentNetworkEpoch,
            networkAvailable = networkAvailable,
        ) == true
    val pendingCoreRouteRestart =
        coreRouteRestartEscalationScope?.applies(
            currentStartGeneration = nextGeneration,
            currentNetworkEpoch = currentNetworkEpoch,
            networkAvailable = networkAvailable,
        ) == true
    if (periodicRouteProofRestartScope != null && !pendingPeriodicRouteMutation) {
      periodicRouteProofRestartScope = null
    }
    if (coreRouteRestartEscalationScope != null && !pendingCoreRouteRestart) {
      coreRouteRestartEscalationScope = null
    }
    if (
        !shouldAcceptModuleOwnedConnectionAttempt(
            networkAvailable = networkAvailable,
            pendingNetworkAction = pendingNetworkAction,
            pendingServiceRouteMutation =
                pendingPeriodicRouteMutation || pendingCoreRouteRestart,
        )
    ) {
      // A network loss or handover that was observed before this posted start
      // owns the next mutation. Fence the foreground discovery generation and
      // preserve the pending proof/restart epoch for the service monitor.
      val admissionCompleted =
          completeModuleStartAdmission(
              nextGeneration,
              MasqModuleStartAdmissionDecision.SERVICE_TAKEOVER,
          )
      val foregroundInvalidated =
          if (shouldInvalidateForegroundStartAfterServiceTakeover(admissionCompleted)) {
            // A five-second admission timeout is the liveness escape hatch. If
            // its gate has already been removed, fall back to generation
            // invalidation to fence any caller that may have started work.
            invalidateForegroundStartForServiceTakeover(nextGeneration)
          } else {
            false
          }
      if (pendingCoreRouteRestart && foregroundInvalidated) {
        coreRouteRestartEscalationScope =
            coreRouteRestartEscalationScope?.copy(
                startGeneration = MasqCoreLifecycle.startGeneration.get(),
            )
      }
      clearModuleOwnedConnectionAttempt()
      resetConnectingWatchdog()
      terminalObservations = 0
      recoveryBackoff = MasqSessionRecoveryBackoff()
      cpuRequired = networkAvailable
      showForegroundState(MasqSessionNotificationState.CONNECTING)
      refreshWakeLock()
      scheduleMonitor()
      return
    }
    moduleStartupDeadlineElapsed =
        SystemClock.elapsedRealtime() + MODULE_STARTUP_GRACE_MILLIS
    moduleOwnsConnectionAttempt = true
    moduleOwnedStartGeneration = nextGeneration
    moduleOwnedNetworkEpoch = currentNetworkEpoch
    resetConnectingWatchdog()
    routeProofRefreshSchedule = MasqRouteProofRefreshSchedule()
    periodicRouteProofRestartScope = null
    coreRouteRestartEscalationScope = null
    networkRouteProofRequiredEpoch = 0L
    networkRouteProofSourceNetworkId = null
    networkRouteRestartRequiredEpoch = 0L
    activeRouteSource = null
    definitiveNetworkLossPending = false
    unavailableNetworkAwaitingLossConfirmation = null
    terminalObservations = 0
    recoveryBackoff = MasqSessionRecoveryBackoff()
    cpuRequired = true
    showForegroundState(MasqSessionNotificationState.CONNECTING)
    refreshWakeLock()
    completeModuleStartAdmission(
        nextGeneration,
        MasqModuleStartAdmissionDecision.ACCEPTED,
    )
    scheduleMonitor()
  }

  private fun beginRestoredGeneration(nextGeneration: Long) {
    cancelRecovery()
    generation = nextGeneration
    moduleStartupDeadlineElapsed = 0L
    clearModuleOwnedConnectionAttempt()
    resetConnectingWatchdog()
    routeProofRefreshSchedule = MasqRouteProofRefreshSchedule()
    periodicRouteProofRestartScope = null
    coreRouteRestartEscalationScope = null
    networkRouteRestartRequiredEpoch = 0L
    networkRouteProofSourceNetworkId = null
    activeRouteSource = null
    definitiveNetworkLossPending = false
    unavailableNetworkAwaitingLossConfirmation = null
    terminalObservations = 0
    recoveryBackoff = MasqSessionRecoveryBackoff()
    // A restored foreground session can begin while the screen is already
    // off. Keep the bounded recovery lease whenever an underlay is available,
    // otherwise Android may suspend the first proof before it starts.
    cpuRequired = networkAvailable
    val currentNetworkEpoch = networkEpoch.get()
    networkRouteProofRequiredEpoch = currentNetworkEpoch
    showForegroundState(MasqSessionNotificationState.CONNECTING)
    refreshWakeLock()
    scheduleMonitor()
  }

  private fun clearModuleOwnedConnectionAttempt() {
    moduleOwnsConnectionAttempt = false
    moduleOwnedStartGeneration = NO_GENERATION
    moduleOwnedNetworkEpoch = 0L
  }

  private fun invalidateForegroundStartForServiceTakeover(
      expectedGeneration: Long,
  ): Boolean = MasqCoreLifecycle.invalidateForNetworkHandover(expectedGeneration)

  private fun releaseModuleOwnershipForNetworkTransition() {
    val currentStartGeneration = MasqCoreLifecycle.startGeneration.get()
    val currentNetworkEpoch = networkEpoch.get()
    if (
        shouldInvalidateModuleOwnedConnectionAttemptForNetworkTransition(
            moduleOwnsAttempt = moduleOwnsConnectionAttempt,
            moduleStartGeneration = moduleOwnedStartGeneration,
            currentStartGeneration = currentStartGeneration,
            moduleNetworkEpoch = moduleOwnedNetworkEpoch,
            currentNetworkEpoch = currentNetworkEpoch,
        )
    ) {
      invalidateForegroundStartForServiceTakeover(moduleOwnedStartGeneration)
    }
    clearModuleOwnedConnectionAttempt()
    moduleStartupDeadlineElapsed = 0L
  }

  private fun adoptGeneration(nextGeneration: Long): Boolean {
    synchronized(lifecycleAuthorityLock) {
      if (destroyed || stoppingExplicitly) return false
      recoveryEpoch.incrementAndGet()
    }
    mainHandler.post {
      if (!destroyed && isSessionDesired() && desiredGeneration.get() == nextGeneration) {
        beginModuleOwnedGeneration(nextGeneration)
      } else {
        completeModuleStartAdmission(
            nextGeneration,
            MasqModuleStartAdmissionDecision.SERVICE_TAKEOVER,
        )
      }
    }
    return true
  }

  private fun requestCoreRouteRestartFromVpnIfCurrent(
      expectedStartGeneration: Long,
      expectedEngineGeneration: Long,
      expectedNetworkEpoch: Long,
  ) {
    if (destroyed || !isSessionDesired() || !networkAvailable) return
    val currentStartGeneration = MasqCoreLifecycle.startGeneration.get()
    val currentNetworkEpoch = networkEpoch.get()
    if (expectedNetworkEpoch <= 0L || expectedNetworkEpoch != currentNetworkEpoch) {
      logMasqRecovery("VPN_ESCALATION_REJECTED reason=network_epoch")
      return
    }
    val requested =
        runCatching {
              MasqCoreRouteRestartEscalationScope(
                  startGeneration = expectedStartGeneration,
                  engineGeneration = expectedEngineGeneration,
                  networkEpoch = expectedNetworkEpoch,
              )
            }
            .getOrNull() ?: return
    when (
        masqCoreRouteRestartEscalationDecision(
            existing = coreRouteRestartEscalationScope,
            requested = requested,
            currentStartGeneration = currentStartGeneration,
            currentNetworkEpoch = currentNetworkEpoch,
            networkAvailable = networkAvailable,
        )) {
      MasqCoreRouteRestartEscalationDecision.REJECT_STALE -> {
        logMasqRecovery("VPN_ESCALATION_REJECTED reason=stale")
        return
      }
      MasqCoreRouteRestartEscalationDecision.DEDUPLICATE -> {
        // Re-kick the serialized monitor without adding another shutdown or
        // discovery job. This also recovers from a best-effort callback that
        // arrived while a prior status poll was completing.
        logMasqRecovery(
            "VPN_ESCALATION_DEDUPLICATED start_generation=$expectedStartGeneration " +
                "engine_generation=$expectedEngineGeneration network_epoch=$currentNetworkEpoch",
        )
        scheduleMonitor()
        return
      }
      MasqCoreRouteRestartEscalationDecision.SCHEDULE -> Unit
    }

    coreRouteRestartEscalationScope = requested
    logMasqRecovery(
        "VPN_ESCALATION_ACCEPTED start_generation=$expectedStartGeneration " +
            "engine_generation=$expectedEngineGeneration network_epoch=$currentNetworkEpoch",
    )
    cancelRecovery()
    routeProofRefreshSchedule = MasqRouteProofRefreshSchedule()
    periodicRouteProofRestartScope = null
    moduleStartupDeadlineElapsed = 0L
    clearModuleOwnedConnectionAttempt()
    resetConnectingWatchdog()
    terminalObservations = 0
    recoveryBackoff = MasqSessionRecoveryBackoff()
    cpuRequired = true
    updateNotification(MasqSessionNotificationState.CONNECTING)
    refreshWakeLock()
    scheduleMonitor()
  }

  private fun applyCoreSnapshot(
      snapshot: MasqSessionCoreSnapshot?,
      coreGeneration: Long,
      routeProofRefreshAttempted: Boolean = false,
      forcedNetworkRouteProof: Boolean = false,
      forcedNetworkProofSucceeded: Boolean = false,
      forcedNetworkProofShutdownSucceeded: Boolean = false,
      forcedNetworkRouteRestart: Boolean = false,
      forcedCoreRouteRestart: Boolean = false,
      forcedRouteRestartSucceeded: Boolean = false,
      coreRouteRestartRecoveryWithoutShutdown: Boolean = false,
      periodicRouteProofRestart: Boolean = false,
      periodicRouteProofRestartSucceeded: Boolean = false,
      monitoredPeriodicRouteProofRestartScope: MasqPeriodicRouteProofRestartScope? = null,
      monitoredCoreRouteRestartScope: MasqCoreRouteRestartEscalationScope? = null,
  ) {
    val now = SystemClock.elapsedRealtime()
    val refreshSucceeded =
        !routeProofRefreshAttempted ||
            routeProofRefreshSchedule.refreshSucceeded(snapshot, coreGeneration)
    val nonMutatingRefreshFailure =
        isNonMutatingRouteProofRefreshFailure(
            routeProofRefreshAttempted = routeProofRefreshAttempted,
            refreshSucceeded = refreshSucceeded,
            snapshot = snapshot,
        )
    routeProofRefreshSchedule =
        routeProofRefreshSchedule.afterSnapshot(
            snapshot,
            coreGeneration,
            now,
            routeProofRefreshAttempted,
        )
    val periodicFailureAction =
        masqPeriodicRouteProofFailureAction(
            routeProofRefreshAttempted = routeProofRefreshAttempted,
            forcedNetworkRouteProof = forcedNetworkRouteProof,
            refreshSucceeded = refreshSucceeded,
            nonMutatingRefreshFailure = nonMutatingRefreshFailure,
            schedule = routeProofRefreshSchedule,
            currentStartGeneration = coreGeneration,
        )
    if (forcedCoreRouteRestart && coreRouteRestartRecoveryWithoutShutdown) {
      routeProofRefreshSchedule = MasqRouteProofRefreshSchedule()
      if (coreRouteRestartEscalationScope == monitoredCoreRouteRestartScope) {
        logMasqRecovery("CORE_RESTART_RESULT action=recover_unhealthy success=true")
        coreRouteRestartEscalationScope = null
        activeRouteSource = null
        MasqVpnService.publishCoreRouteUnavailable(this)
        updateNotification(MasqSessionNotificationState.CONNECTING)
        requestRecovery(delayMillis = 0L)
      } else {
        logMasqRecovery("CORE_RESTART_RESULT action=recover_unhealthy success=false")
        updateNotification(MasqSessionNotificationState.ATTENTION)
      }
      refreshWakeLock()
      return
    }
    if (forcedNetworkRouteRestart || forcedCoreRouteRestart) {
      routeProofRefreshSchedule = MasqRouteProofRefreshSchedule()
      val exactNetworkRestart =
          !forcedNetworkRouteRestart ||
              networkRouteRestartRequiredEpoch == networkEpoch.get()
      val exactCoreRestart =
          !forcedCoreRouteRestart ||
              coreRouteRestartEscalationScope == monitoredCoreRouteRestartScope
      if (
          forcedRouteRestartSucceeded && exactNetworkRestart && exactCoreRestart
      ) {
        logMasqRecovery(
            "CORE_RESTART_RESULT action=shutdown success=true source=" +
                if (forcedCoreRouteRestart) "vpn_preflight" else "network_transition",
        )
        if (forcedNetworkRouteRestart) {
          networkRouteRestartRequiredEpoch = 0L
        }
        if (forcedCoreRouteRestart) {
          coreRouteRestartEscalationScope = null
        }
        activeRouteSource = null
        updateNotification(MasqSessionNotificationState.CONNECTING)
        requestRecovery(delayMillis = 0L)
      } else {
        logMasqRecovery(
            "CORE_RESTART_RESULT action=shutdown success=false source=" +
                if (forcedCoreRouteRestart) "vpn_preflight" else "network_transition",
        )
        updateNotification(MasqSessionNotificationState.ATTENTION)
      }
      refreshWakeLock()
      return
    }
    if (periodicRouteProofRestart) {
      routeProofRefreshSchedule = MasqRouteProofRefreshSchedule()
      if (
          periodicRouteProofRestartSucceeded &&
              periodicRouteProofRestartScope == monitoredPeriodicRouteProofRestartScope
      ) {
        periodicRouteProofRestartScope = null
        moduleStartupDeadlineElapsed = 0L
        resetConnectingWatchdog()
        terminalObservations = 0
        recoveryBackoff = MasqSessionRecoveryBackoff()
        cpuRequired = true
        updateNotification(MasqSessionNotificationState.CONNECTING)
        requestRecovery(delayMillis = 0L)
      } else {
        // The TUN was already fail-closed when escalation was queued. Keep it
        // blocked and retry the exact scoped stop; never let BackgroundRecovery
        // accept the still-healthy stale route through its ACTIVE shortcut.
        cpuRequired = true
        updateNotification(MasqSessionNotificationState.ATTENTION)
      }
      refreshWakeLock()
      return
    }
    if (forcedNetworkRouteProof) {
      if (forcedNetworkProofSucceeded &&
          networkRouteProofRequiredEpoch == networkEpoch.get()) {
        if (activeRouteSource == null &&
            snapshot?.engineGeneration?.let { it > 0L } == true) {
          networkRouteProofSourceNetworkId?.let { sourceNetworkId ->
            activeRouteSource =
                MasqSessionActiveRouteSource(
                    networkId = sourceNetworkId,
                    engineGeneration = snapshot.engineGeneration,
                )
          }
        }
        networkRouteProofRequiredEpoch = 0L
        networkRouteProofSourceNetworkId = null
        routeProofRefreshSchedule =
            scheduleAfterForcedNetworkProof(snapshot, coreGeneration, now)
      } else if (
          shouldConsumeMasqForcedNetworkProofEpoch(
              proofRequiredEpoch = networkRouteProofRequiredEpoch,
              currentNetworkEpoch = networkEpoch.get(),
              stopSucceeded = forcedNetworkProofShutdownSucceeded,
          )
      ) {
        // The old route was already fail-closed and has now been stopped. Start
        // a fresh discovery/recovery attempt on the replacement network now.
        // Consume this one-shot proof epoch before recovery: otherwise the next
        // monitor poll would prove and stop the new engine while it is still at
        // entry-connected route stage one.
        networkRouteProofRequiredEpoch = 0L
        networkRouteProofSourceNetworkId = null
        activeRouteSource = null
        routeProofRefreshSchedule = MasqRouteProofRefreshSchedule()
        requestRecovery(delayMillis = 0L)
        refreshWakeLock()
        return
      } else {
        // The old engine was not confirmed shut down. Keep captured traffic
        // fail-closed and promote this same epoch to a direct shutdown retry;
        // repeating the route proof first could add another full proof timeout
        // against an already stop-requested runtime.
        routeProofRefreshSchedule = MasqRouteProofRefreshSchedule()
        val currentNetworkEpoch = networkEpoch.get()
        if (networkRouteProofRequiredEpoch == currentNetworkEpoch) {
          networkRouteProofRequiredEpoch = 0L
          networkRouteProofSourceNetworkId = null
          networkRouteRestartRequiredEpoch = currentNetworkEpoch
        }
        cpuRequired = true
        updateNotification(MasqSessionNotificationState.ATTENTION)
        refreshWakeLock()
        return
      }
    }
    if (nonMutatingRefreshFailure && !refreshSucceeded) {
      if (
          periodicFailureAction ==
              MasqPeriodicRouteProofFailureAction.FAIL_CLOSED_RESTART
      ) {
        // Two transient proof failures are non-mutating. A third failure in the
        // exact same lifecycle/engine/network scope means the route can no
        // longer be trusted: block captured traffic now, then stop the native
        // route on the serialized executor before rotating discovery.
        periodicRouteProofRestartScope =
            MasqPeriodicRouteProofRestartScope(
                sessionGeneration = generation,
                startGeneration = coreGeneration,
                engineGeneration = routeProofRefreshSchedule.engineGeneration,
                networkEpoch = networkEpoch.get(),
            )
        recovery.recordSavedRouteProofFailure()
        routeProofRefreshSchedule = MasqRouteProofRefreshSchedule()
        cancelRecovery()
        moduleStartupDeadlineElapsed = 0L
        terminalObservations = 0
        cpuRequired = true
        MasqVpnService.publishCoreRouteUnavailable(this)
        updateNotification(MasqSessionNotificationState.CONNECTING)
        refreshWakeLock()
        return
      }
      // The first two transient failures keep the known route active and use
      // the bounded short-retry schedule.
      refreshWakeLock()
      return
    }
    if (snapshot?.isHealthyConnectedSession() != true) {
      MasqVpnService.publishCoreRouteUnavailable(this)
    }
    if (
        shouldDeferRecoveryToModuleOwnedConnectionAttempt(
            moduleOwnsAttempt = moduleOwnsConnectionAttempt,
            moduleStartGeneration = moduleOwnedStartGeneration,
            currentStartGeneration = coreGeneration,
            moduleNetworkEpoch = moduleOwnedNetworkEpoch,
            currentNetworkEpoch = networkEpoch.get(),
            nowElapsed = now,
            moduleDeadlineElapsed = moduleStartupDeadlineElapsed,
            snapshotHealthy = snapshot?.isHealthyConnectedSession() == true,
        )
    ) {
      // MasqCoreModule owns the bounded foreground discovery/readiness cycle.
      // The service remains a fail-closed observer until that window ends so a
      // single entry diagnostic cannot launch two competing discovery jobs.
      terminalObservations = 0
      cpuRequired = true
      updateNotification(MasqSessionNotificationState.CONNECTING)
      refreshWakeLock()
      return
    }
    if (snapshot?.isHealthyConnectedSession() != true) {
      clearModuleOwnedConnectionAttempt()
    }
    when {
      snapshot?.isHealthyConnectedSession() == true -> {
        if (activeRouteSource?.engineGeneration != snapshot.engineGeneration) {
          validatedNetwork?.networkHandle?.let { networkId ->
            activeRouteSource =
                MasqSessionActiveRouteSource(
                    networkId = networkId,
                    engineGeneration = snapshot.engineGeneration,
                )
          }
        }
        moduleStartupDeadlineElapsed = 0L
        clearModuleOwnedConnectionAttempt()
        connectingProgressDeadlineElapsed = 0L
        terminalObservations = 0
        recoveryBackoff = recoveryBackoff.afterHealthy()
        recovery.recordKnownGoodRoute(snapshot)
        logMasqRecovery(
            "SESSION_HEALTHY start_generation=$coreGeneration " +
                "engine_generation=${snapshot.engineGeneration}",
        )
        cpuRequired = true
        updateNotification(MasqSessionNotificationState.CONNECTED)
        refreshWakeLock()
        MasqVpnService.recoverCoreRouteIfNeeded(
            this,
            snapshot.proxyPort,
            coreGeneration,
            snapshot.engineGeneration,
        )
      }
      snapshot?.hasTerminalEntryRecoverySignal() == true -> {
        // Do not let the generic 90-second startup grace hide a native entry
        // handshake that has already produced a structured terminal result.
        // Rotate the failed pair immediately; the recovery layer records and
        // quarantines it before selecting fresh candidates.
        moduleStartupDeadlineElapsed = 0L
        connectingProgressDeadlineElapsed = 0L
        terminalObservations = 0
        recoveryBackoff = recoveryBackoff.afterTerminalEntrySignal(now)
        cpuRequired = true
        updateNotification(MasqSessionNotificationState.CONNECTING)
        refreshWakeLock()
        requestRecovery(delayMillis = recoveryBackoff.terminalEntryRetryDelayMillis())
      }
      snapshot?.isEntryConnectedAwaitingRoute() == true -> {
        terminalObservations = 0
        cpuRequired = true
        updateNotification(MasqSessionNotificationState.CONNECTING)
        refreshWakeLock()
        if (recoveryBackoff.hasStageOneProofOpportunity()) {
          val currentNetworkEpoch = networkEpoch.get()
          val proofScope =
              MasqStageOneProofScope(
                  identity =
                      MasqRecoveryAttemptIdentity(
                          startGeneration = coreGeneration,
                          engineGeneration = snapshot.engineGeneration,
                      ),
                  networkEpoch = currentNetworkEpoch,
              )
          requestRecovery(
              delayMillis = recoveryBackoff.stageOneProofDelayMillis(now),
              stageOneProofScope = proofScope,
          )
        } else {
          requestRecovery(delayMillis = nextRecoveryDelayMillis())
        }
      }
      snapshot?.phase == "connecting" && connectingIsMakingProgress(snapshot, now) -> {
        terminalObservations = 0
        cpuRequired = true
        updateNotification(MasqSessionNotificationState.CONNECTING)
        refreshWakeLock()
      }
      else -> {
        val unhealthyConnected = snapshot?.phase == "connected"
        if (!unhealthyConnected && now < moduleStartupDeadlineElapsed) {
          return
        }
        terminalObservations += 1
        if (terminalObservations >= TERMINAL_OBSERVATIONS_BEFORE_RECOVERY) {
          cpuRequired = false
          updateNotification(MasqSessionNotificationState.ATTENTION)
          refreshWakeLock()
          requestRecovery(nextRecoveryDelayMillis())
        }
      }
    }
  }

  private fun resetConnectingWatchdog() {
    connectingNeighbors = 0
    connectingRouteStage = 0
    connectingProgressDeadlineElapsed =
        SystemClock.elapsedRealtime() + CONNECTING_PROGRESS_TIMEOUT_MILLIS
  }

  private fun connectingIsMakingProgress(
      snapshot: MasqSessionCoreSnapshot,
      now: Long,
  ): Boolean {
    if (
        snapshot.connectedNeighbors > connectingNeighbors ||
            snapshot.routeStage > connectingRouteStage
    ) {
      connectingNeighbors = snapshot.connectedNeighbors
      connectingRouteStage = snapshot.routeStage
      connectingProgressDeadlineElapsed = now + CONNECTING_PROGRESS_TIMEOUT_MILLIS
    }
    return now < connectingProgressDeadlineElapsed
  }

  private fun requestRecovery(
      delayMillis: Long,
      stageOneProofScope: MasqStageOneProofScope? = null,
      routeRebuildRetry: Boolean = false,
  ) {
    val now = SystemClock.elapsedRealtime()
    val exactStageOneProof =
        stageOneProofScope?.applies(
            currentStartGeneration = MasqCoreLifecycle.startGeneration.get(),
            currentNetworkEpoch = networkEpoch.get(),
        ) == true && recoveryBackoff.hasStageOneProofOpportunity()
    if (
        destroyed ||
            !isSessionDesired() ||
            !networkAvailable ||
            (now < moduleStartupDeadlineElapsed && !exactStageOneProof) ||
            (!recoveryBackoff.allowsAttempt(now) && !exactStageOneProof)
    ) {
      return
    }
    // A Handler delay cannot wake a sleeping device by itself. Keep this bounded
    // lease through the backoff (at most five minutes) so the scheduled recovery
    // can actually begin while the screen remains locked. Refresh it even when
    // this call deduplicates against an already scheduled or running recovery.
    cpuRequired = true
    refreshWakeLock()
    if (recoveryRunningToken != NO_RECOVERY_TOKEN) return
    if (recoveryDelayRunnable != null) {
      if (delayMillis > 0L) return
      cancelRecovery()
    }

    val token = recoveryEpoch.incrementAndGet()
    logMasqRecovery(
        "RECOVERY_SCHEDULED delay_ms=$delayMillis token=$token " +
            "kind=${when {
              stageOneProofScope != null -> "stage_one_proof"
              routeRebuildRetry -> "route_rebuild"
              else -> "general"
            }}",
    )
    val delayedRecovery =
        Runnable {
          recoveryDelayRunnable = null
          val stageOneProofStillExact =
              stageOneProofScope == null ||
                  stageOneProofScope.applies(
                      currentStartGeneration = MasqCoreLifecycle.startGeneration.get(),
                      currentNetworkEpoch = networkEpoch.get(),
                  )
          if (
              !isRecoveryCurrent(token) ||
                  !networkAvailable ||
                  !stageOneProofStillExact
          ) {
            return@Runnable
          }
          recoveryRunningToken = token
          cpuRequired = true
          updateNotification(MasqSessionNotificationState.CONNECTING)
          refreshWakeLock()
          recoveryFuture = recoveryExecutor.submit {
            val result =
                recovery.recover(
                    isRecoveryCurrent = {
                      isRecoveryCurrent(token) &&
                          (stageOneProofScope == null ||
                              stageOneProofScope.applies(
                                  currentStartGeneration =
                                      MasqCoreLifecycle.startGeneration.get(),
                                  currentNetworkEpoch = networkEpoch.get(),
                              ))
                    },
                    expectedRouteVerificationIdentity = stageOneProofScope?.identity,
                )
            mainHandler.post {
              if (recoveryRunningToken == token) {
                recoveryRunningToken = NO_RECOVERY_TOKEN
                recoveryFuture = null
              }
              if (!isRecoveryCurrent(token) || !networkAvailable) return@post
              when (result) {
                MasqBackgroundRecoveryResult.ACTIVE -> {
                  logMasqRecovery("RECOVERY_RESULT result=active token=$token")
                  clearModuleOwnedConnectionAttempt()
                  recoveryBackoff = recoveryBackoff.afterHealthy()
                  moduleStartupDeadlineElapsed = 0L
                  terminalObservations = 0
                  resetConnectingWatchdog()
                  cpuRequired = true
                  refreshWakeLock()
                  // Re-sample immediately. This publishes CONNECTED and
                  // rebinds a blocked whole-device translator without paying
                  // another full healthy-monitor interval after the proof.
                  scheduleMonitor()
                }
                MasqBackgroundRecoveryResult.STARTED -> {
                  logMasqRecovery("RECOVERY_RESULT result=started token=$token")
                  clearModuleOwnedConnectionAttempt()
                  val now = SystemClock.elapsedRealtime()
                  recoveryBackoff = recoveryBackoff.afterStarted(now)
                  moduleStartupDeadlineElapsed =
                      maxOf(moduleStartupDeadlineElapsed, recoveryBackoff.notBeforeElapsed)
                  terminalObservations = 0
                  resetConnectingWatchdog()
                  cpuRequired = true
                  updateNotification(MasqSessionNotificationState.CONNECTING)
                  refreshWakeLock()
                  // A freshly started native engine usually reaches entry-only
                  // stage one within this short settle window. Observe it
                  // before the normal healthy cadence so its scoped route proof
                  // is not delayed by an otherwise unnecessary five seconds.
                  scheduleMonitor(STAGE_ONE_ROUTE_PROOF_SETTLE_MILLIS)
                }
                MasqBackgroundRecoveryResult.CANCELLED -> {
                  logMasqRecovery("RECOVERY_RESULT result=cancelled token=$token")
                  cpuRequired = false
                  refreshWakeLock()
                }
                MasqBackgroundRecoveryResult.FAILED -> {
                  logMasqRecovery("RECOVERY_RESULT result=failed token=$token")
                  if (stageOneProofScope != null || routeRebuildRetry) {
                    val now = SystemClock.elapsedRealtime()
                    recoveryBackoff = recoveryBackoff.afterStageOneProofFailed(now)
                    moduleStartupDeadlineElapsed = 0L
                  } else {
                    recoveryBackoff = recoveryBackoff.afterFailed()
                  }
                  cpuRequired = false
                  updateNotification(MasqSessionNotificationState.ATTENTION)
                  refreshWakeLock()
                  if (stageOneProofScope != null || routeRebuildRetry) {
                    requestRecovery(
                        delayMillis = recoveryBackoff.routeRebuildRetryDelayMillis(),
                        routeRebuildRetry = true,
                    )
                  } else {
                    requestRecovery(nextRecoveryDelayMillis())
                  }
                }
              }
            }
          }
        }
    recoveryDelayRunnable = delayedRecovery
    mainHandler.postDelayed(delayedRecovery, delayMillis)
  }

  private fun cancelRecovery() {
    recoveryEpoch.incrementAndGet()
    recoveryDelayRunnable?.let(mainHandler::removeCallbacks)
    recoveryDelayRunnable = null
    recoveryFuture?.cancel(true)
    recoveryFuture = null
    recoveryRunningToken = NO_RECOVERY_TOKEN
  }

  private fun isRecoveryCurrent(token: Long): Boolean =
      !destroyed && token == recoveryEpoch.get() && isSessionDesired()

  private fun nextRecoveryDelayMillis(): Long =
      recoveryBackoff.nextDelayMillis()

  private fun refreshNetworkState(
      lostNetwork: Network? = null,
      confirmImplicitTransition: Boolean = false,
  ) {
    val wasAvailable = networkAvailable
    val previousNetwork = validatedNetwork
    val explicitTrackedNetworkLoss =
        lostNetwork != null &&
            (lostNetwork == previousNetwork ||
                lostNetwork == unavailableNetworkAwaitingLossConfirmation)
    val currentNetwork =
        currentValidatedNetwork(excludedNetwork = lostNetwork)
    when (
        networkTransitionCoalescer.observe(
            previousNetworkId = previousNetwork?.networkHandle,
            observedNetworkId = currentNetwork?.networkHandle,
            explicitlyLostNetworkId = lostNetwork?.networkHandle,
            confirmed = confirmImplicitTransition,
        )
    ) {
      MasqSessionNetworkObservationAction.DEFER_NEW -> {
        scheduleNetworkTransitionConfirmation()
        return
      }
      MasqSessionNetworkObservationAction.DEFER_EXISTING -> return
      MasqSessionNetworkObservationAction.APPLY -> cancelNetworkTransitionConfirmation()
    }
    validatedNetwork = currentNetwork
    networkAvailable = currentNetwork != null
    val transition =
        masqSessionNetworkTransition(
            previousAvailable = wasAvailable,
            previousNetworkId = previousNetwork?.networkHandle,
            currentAvailable = networkAvailable,
            currentNetworkId = currentNetwork?.networkHandle,
        )
    if (transition != MasqSessionNetworkTransition.UNCHANGED) {
      networkEpoch.set(nextProcessNetworkEpoch())
      logMasqRecovery(
          "NETWORK_TRANSITION transition=${transition.name.lowercase()} " +
              "available=$networkAvailable network_epoch=${networkEpoch.get()}",
      )
    }
    if (!networkAvailable && explicitTrackedNetworkLoss) {
      definitiveNetworkLossPending = true
    }
    val pendingProofSourceLossConfirmed =
        shouldUpgradeMasqProofToRestartAfterSourceLoss(
            lostNetworkId = lostNetwork?.networkHandle,
            proofSourceNetworkId = networkRouteProofSourceNetworkId,
            proofRequiredEpoch = networkRouteProofRequiredEpoch,
            currentNetworkEpoch = networkEpoch.get(),
        )
    val activeRouteSourceLossConfirmed =
        shouldRestartAfterActiveRouteSourceLoss(
            lostNetworkId = lostNetwork?.networkHandle,
            activeRouteSource = activeRouteSource,
        )
    when (transition) {
      MasqSessionNetworkTransition.LOST -> {
        cancelRecovery()
        releaseModuleOwnershipForNetworkTransition()
        routeProofRefreshSchedule = MasqRouteProofRefreshSchedule()
        periodicRouteProofRestartScope = null
        networkRouteProofRequiredEpoch = 0L
        networkRouteProofSourceNetworkId = null
        networkRouteRestartRequiredEpoch = 0L
        activeRouteSource = null
        unavailableNetworkAwaitingLossConfirmation = previousNetwork
        definitiveNetworkLossPending = explicitTrackedNetworkLoss
        cpuRequired = false
        if (isSessionDesired()) {
          // A route proven on the lost Android network must never keep an
          // existing TUN translator active on assumption alone.
          MasqVpnService.publishCoreRouteUnavailable(this)
        }
        updateNotification(MasqSessionNotificationState.ATTENTION)
      }
      MasqSessionNetworkTransition.RESTORED -> {
        if (isSessionDesired()) {
          // An explicit loss of the tracked Android network already proves the
          // old socket path is gone. Pause it immediately and rebuild instead
          // of spending another route-proof timeout on a route that cannot live.
          MasqVpnService.publishCoreRouteUnavailable(this)
          cancelRecovery()
          routeProofRefreshSchedule = MasqRouteProofRefreshSchedule()
          periodicRouteProofRestartScope = null
          moduleStartupDeadlineElapsed = 0L
          releaseModuleOwnershipForNetworkTransition()
          recoveryBackoff = MasqSessionRecoveryBackoff()
          if (definitiveNetworkLossPending) {
            networkRouteProofRequiredEpoch = 0L
            networkRouteProofSourceNetworkId = null
            networkRouteRestartRequiredEpoch = networkEpoch.get()
            activeRouteSource = null
          } else {
            networkRouteRestartRequiredEpoch = 0L
            networkRouteProofRequiredEpoch = networkEpoch.get()
            networkRouteProofSourceNetworkId =
                unavailableNetworkAwaitingLossConfirmation?.networkHandle
          }
          definitiveNetworkLossPending = false
          unavailableNetworkAwaitingLossConfirmation = null
          cpuRequired = true
          scheduleMonitor()
        }
      }
      MasqSessionNetworkTransition.REPLACED -> {
        if (isSessionDesired()) {
          // A validated-to-validated handover may still preserve a working
          // socket path, so retain the bounded proof before restarting it.
          MasqVpnService.publishCoreRouteUnavailable(this)
          cancelRecovery()
          routeProofRefreshSchedule = MasqRouteProofRefreshSchedule()
          periodicRouteProofRestartScope = null
          moduleStartupDeadlineElapsed = 0L
          releaseModuleOwnershipForNetworkTransition()
          recoveryBackoff = MasqSessionRecoveryBackoff()
          if (explicitTrackedNetworkLoss || activeRouteSourceLossConfirmed) {
            networkRouteProofRequiredEpoch = 0L
            networkRouteProofSourceNetworkId = null
            networkRouteRestartRequiredEpoch = networkEpoch.get()
            activeRouteSource = null
          } else {
            networkRouteRestartRequiredEpoch = 0L
            networkRouteProofRequiredEpoch = networkEpoch.get()
            networkRouteProofSourceNetworkId = previousNetwork?.networkHandle
          }
          definitiveNetworkLossPending = false
          unavailableNetworkAwaitingLossConfirmation = null
          cpuRequired = true
          scheduleMonitor()
        }
      }
      MasqSessionNetworkTransition.UNCHANGED -> {
        if (
            isSessionDesired() &&
                (pendingProofSourceLossConfirmed || activeRouteSourceLossConfirmed)
        ) {
          // Android may announce the replacement underlay before it reports
          // that the old one was physically lost. Upgrade the pending proof in
          // place so callback ordering cannot add a proof timeout or leave the
          // old socket path classified as merely uncertain.
          MasqVpnService.publishCoreRouteUnavailable(this)
          cancelRecovery()
          routeProofRefreshSchedule = MasqRouteProofRefreshSchedule()
          periodicRouteProofRestartScope = null
          moduleStartupDeadlineElapsed = 0L
          releaseModuleOwnershipForNetworkTransition()
          recoveryBackoff = MasqSessionRecoveryBackoff()
          val confirmedLossEpoch = nextProcessNetworkEpoch()
          networkEpoch.set(confirmedLossEpoch)
          networkRouteProofRequiredEpoch = 0L
          networkRouteProofSourceNetworkId = null
          networkRouteRestartRequiredEpoch = confirmedLossEpoch
          activeRouteSource = null
          definitiveNetworkLossPending = false
          unavailableNetworkAwaitingLossConfirmation = null
          cpuRequired = true
          scheduleMonitor()
        }
      }
    }
    refreshWakeLock()
  }

  private fun scheduleNetworkTransitionConfirmation() {
    cancelNetworkTransitionConfirmation()
    lateinit var confirmation: Runnable
    confirmation =
        Runnable {
          if (networkTransitionConfirmationRunnable !== confirmation) return@Runnable
          networkTransitionConfirmationRunnable = null
          if (!destroyed && isSessionDesired()) {
            refreshNetworkState(confirmImplicitTransition = true)
          } else {
            networkTransitionCoalescer.reset()
          }
        }
    networkTransitionConfirmationRunnable = confirmation
    mainHandler.postDelayed(confirmation, NETWORK_TRANSITION_DEBOUNCE_MILLIS)
  }

  private fun cancelNetworkTransitionConfirmation() {
    networkTransitionConfirmationRunnable?.let(mainHandler::removeCallbacks)
    networkTransitionConfirmationRunnable = null
  }

  private fun currentValidatedNetwork(excludedNetwork: Network? = null): Network? =
      resolveMasqValidatedUnderlayNetwork(
              manager = connectivityManager,
              previousNetwork = validatedNetwork,
              excludedNetwork = excludedNetwork,
          )
          ?.network

  private fun scheduleMonitor(delayMillis: Long = 0L) {
    mainHandler.removeCallbacks(monitorRunnable)
    mainHandler.postDelayed(monitorRunnable, delayMillis.coerceAtLeast(0L))
  }

  private fun showForegroundState(state: MasqSessionNotificationState) {
    if (foregroundStarted) {
      updateNotification(state)
      return
    }
    val serviceType =
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
          ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE
        } else {
          0
        }
    ServiceCompat.startForeground(
        this,
        NOTIFICATION_ID,
        notification(state),
        serviceType,
    )
    foregroundStarted = true
  }

  private fun updateNotification(state: MasqSessionNotificationState) {
    if (!foregroundStarted || destroyed) return
    getSystemService(NotificationManager::class.java)
        .notify(NOTIFICATION_ID, notification(state))
  }

  private fun notification(state: MasqSessionNotificationState) =
      NotificationCompat.Builder(this, NOTIFICATION_CHANNEL)
          .setSmallIcon(R.mipmap.ic_launcher)
          .setContentTitle("MASQ private connection")
          .setContentText(masqSessionNotificationText(state))
          .setStyle(
              NotificationCompat.BigTextStyle()
                  .bigText(masqSessionNotificationText(state)))
          .setContentIntent(
              PendingIntent.getActivity(
                  this,
                  NOTIFICATION_REQUEST_CODE,
                  Intent(this, MainActivity::class.java),
                  PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
              ))
          .setCategory(NotificationCompat.CATEGORY_SERVICE)
          .setOngoing(true)
          .setOnlyAlertOnce(true)
          .setShowWhen(false)
          .build()

  private fun createNotificationChannel() {
    if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) return
    getSystemService(NotificationManager::class.java)
        .createNotificationChannel(
            NotificationChannel(
                NOTIFICATION_CHANNEL,
                "MASQ private connection",
                NotificationManager.IMPORTANCE_LOW,
            ).apply {
              description =
                  "Shows when the user-requested MASQ consumer connection is active."
              setShowBadge(false)
            })
  }

  private fun registerLifecycleSignals() {
    runCatching {
          ContextCompat.registerReceiver(
              this,
              screenReceiver,
              IntentFilter().apply {
                addAction(Intent.ACTION_SCREEN_OFF)
                addAction(Intent.ACTION_SCREEN_ON)
              },
              ContextCompat.RECEIVER_NOT_EXPORTED,
          )
        }
        .onSuccess { screenReceiverRegistered = true }
    val request =
        NetworkRequest.Builder()
            .addCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET)
            .addCapability(NetworkCapabilities.NET_CAPABILITY_NOT_VPN)
            .build()
    runCatching { connectivityManager.registerNetworkCallback(request, networkCallback) }
        .onSuccess { networkCallbackRegistered = true }
  }

  private fun shouldHoldWakeLock(): Boolean =
      isSessionDesired() && screenOff && networkAvailable && cpuRequired

  private fun refreshWakeLock() {
    if (shouldHoldWakeLock()) {
      acquireTimedWakeLock(forceRenewal = false)
    } else {
      mainHandler.removeCallbacks(renewWakeLockRunnable)
      releaseWakeLock()
    }
  }

  private fun acquireTimedWakeLock(forceRenewal: Boolean) {
    val lock =
        wakeLock
            ?: (getSystemService(Context.POWER_SERVICE) as PowerManager)
                .newWakeLock(
                    PowerManager.PARTIAL_WAKE_LOCK,
                    WAKE_LOCK_TAG,
                )
                .apply {
                  setReferenceCounted(false)
                  wakeLock = this
                }
    if (forceRenewal && lock.isHeld) {
      runCatching { lock.release() }
    }
    val scheduleRenewal = forceRenewal || !lock.isHeld
    if (!lock.isHeld) {
      lock.acquire(WAKE_LOCK_TIMEOUT_MILLIS)
    }
    if (scheduleRenewal) {
      mainHandler.removeCallbacks(renewWakeLockRunnable)
      mainHandler.postDelayed(renewWakeLockRunnable, WAKE_LOCK_RENEWAL_MILLIS)
    }
  }

  private fun releaseWakeLock() {
    wakeLock?.takeIf { it.isHeld }?.let { lock ->
      runCatching { lock.release() }
    }
    wakeLock = null
  }

  private fun isSessionDesired(): Boolean =
      desiredGeneration.get() > NO_GENERATION && intentStore.isDesired()

  private fun requestExplicitStop() {
    mainHandler.post {
      if (!destroyed && !isSessionDesired()) {
        stopIfUndesired(latestStartId)
      }
    }
  }

  private fun stopIfUndesired(startId: Int) {
    synchronized(lifecycleAuthorityLock) {
      if (desiredGeneration.get() > NO_GENERATION) return
      stoppingExplicitly = true
      activeInstance.compareAndSet(this, null)
    }
    cancelRecovery()
    generation = NO_GENERATION
    moduleStartupDeadlineElapsed = 0L
    clearModuleOwnedConnectionAttempt()
    routeProofRefreshSchedule = MasqRouteProofRefreshSchedule()
    periodicRouteProofRestartScope = null
    networkRouteProofRequiredEpoch = 0L
    networkRouteProofSourceNetworkId = null
    networkRouteRestartRequiredEpoch = 0L
    activeRouteSource = null
    definitiveNetworkLossPending = false
    unavailableNetworkAwaitingLossConfirmation = null
    recoveryBackoff = MasqSessionRecoveryBackoff()
    cpuRequired = false
    mainHandler.removeCallbacks(monitorRunnable)
    mainHandler.removeCallbacks(renewWakeLockRunnable)
    cancelNetworkTransitionConfirmation()
    networkTransitionCoalescer.reset()
    releaseWakeLock()
    if (foregroundStarted) {
      stopForeground(STOP_FOREGROUND_REMOVE)
      foregroundStarted = false
    }
    if (startId > 0) {
      stopSelfResult(startId)
    } else {
      stopSelf()
    }
  }

  companion object {
    private const val ACTION_KEEP_SESSION =
        "com.masqmobile.action.KEEP_CONSUMER_SESSION"
    private const val ACTION_RESTORE_SESSION =
        "com.masqmobile.action.RESTORE_CONSUMER_SESSION"
    private const val EXTRA_GENERATION = "masq_session_generation"
    private const val NOTIFICATION_CHANNEL = "masq-private-connection"
    private const val NOTIFICATION_ID = 901
    private const val NOTIFICATION_REQUEST_CODE = 901
    private const val NO_GENERATION = 0L
    private const val NO_RECOVERY_TOKEN = 0L
    private const val RECOVERY_GENERATION = 1L
    private const val STATUS_POLL_INTERVAL_MILLIS = 5_000L
    private const val MODULE_STARTUP_GRACE_MILLIS = 90_000L
    private const val CONNECTING_PROGRESS_TIMEOUT_MILLIS = 90_000L
    private const val NETWORK_TRANSITION_DEBOUNCE_MILLIS = 2_000L
    private const val MODULE_START_ADMISSION_TIMEOUT_MILLIS = 5_000L
    private const val TERMINAL_OBSERVATIONS_BEFORE_RECOVERY = 3
    private const val WAKE_LOCK_TIMEOUT_MILLIS = 10 * 60_000L
    private const val WAKE_LOCK_RENEWAL_MILLIS = 9 * 60_000L
    private const val WAKE_LOCK_TAG = "com.masqmobile:consumer-session"
    private val desiredGeneration = AtomicLong(NO_GENERATION)
    private val activeInstance = AtomicReference<MasqSessionService?>(null)
    private val lastRestoreDispatchElapsed = AtomicLong(0L)
    private val processNetworkEpoch = AtomicLong(1L)
    private val lifecycleAuthorityLock = Any()
    private val moduleStartAdmissions =
        ConcurrentHashMap<Long, MasqModuleStartAdmissionGate>()

    private fun nextProcessNetworkEpoch(): Long {
      while (true) {
        val current = processNetworkEpoch.get()
        check(current > 0L && current < Long.MAX_VALUE) {
          "The MASQ Android network epoch is exhausted."
        }
        if (processNetworkEpoch.compareAndSet(current, current + 1L)) {
          return current
        }
      }
    }

    internal fun awaitModuleStartAdmission(
        generation: Long,
    ): MasqModuleStartAdmissionDecision {
      val admission =
          moduleStartAdmissions[generation]
              ?: return MasqModuleStartAdmissionDecision.ACCEPTED
      val decision = admission.await(MODULE_START_ADMISSION_TIMEOUT_MILLIS)
      moduleStartAdmissions.remove(generation, admission)
      return decision
    }

    internal fun requestCoreRouteRestartIfCurrent(
        expectedStartGeneration: Long,
        expectedEngineGeneration: Long,
        expectedNetworkEpoch: Long,
    ) {
      if (
          expectedStartGeneration <= NO_GENERATION ||
              expectedEngineGeneration <= 0L ||
              expectedNetworkEpoch <= 0L
      ) {
        return
      }
      val service = activeInstance.get()?.takeIf { !it.destroyed && !it.stoppingExplicitly }
          ?: return
      service.mainHandler.post {
        service.requestCoreRouteRestartFromVpnIfCurrent(
            expectedStartGeneration = expectedStartGeneration,
            expectedEngineGeneration = expectedEngineGeneration,
            expectedNetworkEpoch = expectedNetworkEpoch,
        )
      }
    }

    internal fun currentNetworkEpochForCore(
        expectedStartGeneration: Long,
    ): Long? {
      if (
          expectedStartGeneration <= NO_GENERATION ||
              MasqCoreLifecycle.startGeneration.get() != expectedStartGeneration
      ) {
        return null
      }
      val service =
          activeInstance.get()?.takeIf { !it.destroyed && !it.stoppingExplicitly }
              ?: return null
      val epoch = service.networkEpoch.get()
      return epoch.takeIf {
        epoch > 0L &&
            activeInstance.get() === service &&
            !service.destroyed &&
            !service.stoppingExplicitly &&
            MasqCoreLifecycle.startGeneration.get() == expectedStartGeneration
      }
    }

    private fun completeModuleStartAdmission(
        generation: Long,
        decision: MasqModuleStartAdmissionDecision,
    ): Boolean {
      if (generation <= NO_GENERATION) return false
      return moduleStartAdmissions[generation]?.complete(decision) == true
    }

    private fun completeAllModuleStartAdmissions(
        decision: MasqModuleStartAdmissionDecision,
    ) {
      moduleStartAdmissions.values.forEach { admission -> admission.complete(decision) }
      moduleStartAdmissions.clear()
    }

    fun start(context: Context): Long {
      val intentStore = MasqSessionIntentStore(context)
      val generation: Long
      var adoptedByActiveService = false
      synchronized(lifecycleAuthorityLock) {
        if (!intentStore.setDesired(true)) {
          throw IllegalStateException(
              "Android could not persist the requested MASQ background session.",
          )
        }
        activeInstance.get()?.recoveryEpoch?.incrementAndGet()
        generation =
            synchronized(MasqCoreLifecycle.lock) {
              MasqCoreLifecycle.startGeneration.incrementAndGet()
            }
        moduleStartAdmissions[generation] = MasqModuleStartAdmissionGate()
        desiredGeneration.set(generation)
        if (activeInstance.get()?.adoptGeneration(generation) == true) {
          adoptedByActiveService = true
        }
      }
      if (adoptedByActiveService) return generation
      val intent =
          Intent(context, MasqSessionService::class.java)
              .setAction(ACTION_KEEP_SESSION)
              .putExtra(EXTRA_GENERATION, generation)
      try {
        ContextCompat.startForegroundService(context, intent)
      } catch (error: RuntimeException) {
        completeModuleStartAdmission(
            generation,
            MasqModuleStartAdmissionDecision.SERVICE_TAKEOVER,
        )
        moduleStartAdmissions.remove(generation)
        var cleanupSucceeded = true
        synchronized(lifecycleAuthorityLock) {
          if (desiredGeneration.compareAndSet(generation, NO_GENERATION)) {
            cleanupSucceeded = intentStore.clearDesiredFailClosed()
            context.stopService(Intent(context, MasqSessionService::class.java))
          }
        }
        if (!cleanupSucceeded) {
          throw IllegalStateException(
              "Android could not clear the failed MASQ background-session request.",
              error,
          )
        }
        throw error
      }
      return generation
    }

    fun stop(context: Context): Boolean {
      val intentStore = MasqSessionIntentStore(context)
      val instance: MasqSessionService?
      val persisted: Boolean
      synchronized(lifecycleAuthorityLock) {
        desiredGeneration.set(NO_GENERATION)
        completeAllModuleStartAdmissions(MasqModuleStartAdmissionDecision.SERVICE_TAKEOVER)
        persisted = intentStore.clearDesiredFailClosed()
        instance = activeInstance.get()
        instance?.let { service ->
          service.stoppingExplicitly = true
          service.recoveryEpoch.incrementAndGet()
          activeInstance.compareAndSet(service, null)
        }
        context.stopService(Intent(context, MasqSessionService::class.java))
      }
      if (instance != null) {
        instance.requestExplicitStop()
      }
      return persisted
    }

    /**
     * Re-dispatches a previously requested consumer session when Android has
     * reclaimed only this service while the fail-closed VPN service survives.
     *
     * The persisted user intent is authoritative: an explicit disconnect or
     * shutdown clears it, and this watchdog will then never restart MASQ.
     */
    internal fun ensureRunningIfDesired(context: Context): MasqSessionEnsureDecision {
      val applicationContext = context.applicationContext
      val intentStore = MasqSessionIntentStore(applicationContext)
      val nowElapsed = SystemClock.elapsedRealtime()
      val decision: MasqSessionEnsureDecision
      val recoveryGeneration: Long
      synchronized(lifecycleAuthorityLock) {
        val active = activeInstance.get()
        decision =
            masqSessionEnsureDecision(
                persistedDesired = intentStore.isDesired(),
                activeInstanceLive =
                    active != null && !active.destroyed && !active.stoppingExplicitly,
                nowElapsed = nowElapsed,
                lastDispatchElapsed = lastRestoreDispatchElapsed.get(),
            )
        if (decision != MasqSessionEnsureDecision.DISPATCH_RESTORE) {
          return decision
        }
        recoveryGeneration =
            desiredGeneration.updateAndGet { current ->
              if (current > NO_GENERATION) {
                current
              } else {
                MasqCoreLifecycle.startGeneration.get().takeIf { it > NO_GENERATION }
                    ?: RECOVERY_GENERATION
              }
            }
        lastRestoreDispatchElapsed.set(nowElapsed.coerceAtLeast(1L))
      }

      val restoreIntent =
          Intent(applicationContext, MasqSessionService::class.java)
              .setAction(ACTION_RESTORE_SESSION)
              .putExtra(EXTRA_GENERATION, recoveryGeneration)
      return try {
        ContextCompat.startForegroundService(applicationContext, restoreIntent)
        MasqSessionEnsureDecision.DISPATCH_RESTORE
      } catch (_: RuntimeException) {
        // Preserve the durable intent and retry after the bounded throttle. The
        // VPN retains its captured TUN in blocked mode in the meantime.
        MasqSessionEnsureDecision.RETRY_THROTTLED
      }
    }
  }
}
