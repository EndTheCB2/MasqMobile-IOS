package com.masqmobile

import android.Manifest
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.net.VpnService
import android.os.Build
import android.os.ParcelFileDescriptor
import androidx.core.app.NotificationCompat
import androidx.core.content.ContextCompat
import java.util.concurrent.CompletableFuture
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicLong
import org.json.JSONObject

internal const val PUBLIC_MASQ_PACKAGE_ID = "com.endthecb2.masqmobile"
internal const val DOGFOOD_MASQ_PACKAGE_ID = "com.endthecb2.masqmobile.dogfood"
internal val MASQ_CONTROL_PLANE_PACKAGE_IDS =
    setOf(PUBLIC_MASQ_PACKAGE_ID, DOGFOOD_MASQ_PACKAGE_ID)

internal fun isMasqControlPlanePackage(packageId: String): Boolean =
    packageId in MASQ_CONTROL_PLANE_PACKAGE_IDS

internal fun installedMasqControlPlanePackages(
    isInstalled: (String) -> Boolean,
): List<String> = MASQ_CONTROL_PLANE_PACKAGE_IDS.filter(isInstalled)

class MasqVpnService : VpnService() {
  private val serviceEpoch = nextServiceEpoch()
  private val controlExecutor = Executors.newSingleThreadExecutor()
  private val handoffRetryExecutor = Executors.newSingleThreadScheduledExecutor()
  private val handoffRetryScheduled = AtomicBoolean(false)
  private val translatorExecutor = Executors.newSingleThreadExecutor()
  private val translator =
      SystemRoutingTranslator(JniPacketTunnelNativeApi, translatorExecutor)
  private val runtimeLock = Any()
  private val acknowledgementOwnershipLock = Any()
  private val ownedStartRequests = mutableSetOf<Long>()
  private val ownedStopRequests = mutableSetOf<Long>()
  private val ownedResetRequests = mutableSetOf<Long>()
  private lateinit var policyStore: SystemRoutingPolicyStore
  @Volatile private var tunnelDescriptor: ParcelFileDescriptor? = null
  @Volatile private var appliedPolicy: DesiredSystemRoutingPolicy? = null
  @Volatile private var revoked = false
  @Volatile private var localTunCaptureValid = false
  @Volatile private var destroyed = false
  @Volatile private var adoptedTerminalLeaseEpoch: Long? = null

  override fun onCreate() {
    super.onCreate()
    claimStatusEpoch(serviceEpoch)
    adoptedTerminalLeaseEpoch = terminalCoordinator.snapshot()?.epoch
    policyStore =
        SystemRoutingPolicyStore(
            SharedPreferencesSystemRoutingPolicyStorage(
                getSharedPreferences(
                    SystemRoutingPolicyStore.PREFERENCES_NAME,
                    Context.MODE_PRIVATE,
                )))
    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
      getSystemService(NotificationManager::class.java)
          .createNotificationChannel(
              NotificationChannel(
                  NOTIFICATION_CHANNEL,
                  "MASQ system-routing dogfood",
                  NotificationManager.IMPORTANCE_LOW,
              ))
    }
  }

  override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
    startForeground(
        NOTIFICATION_ID,
        notification("MASQ dogfood routing is starting."),
    )
    when (intent?.action) {
      ACTION_RESET -> {
        val requestId = intent.getLongExtra(EXTRA_COMMAND_REQUEST_ID, NO_REQUEST)
        requestId.takeIf { it != NO_REQUEST }?.let(::ownResetRequest)
        controlExecutor.execute {
          handleExplicitReset(
              requestId = requestId.takeIf { it != NO_REQUEST },
          )
        }
        return START_NOT_STICKY
      }
      ACTION_START -> {
        if (!BuildConfig.MASQ_SYSTEM_TUNNEL_ENABLED) {
          controlExecutor.execute { handleDisabledBuild() }
          return START_NOT_STICKY
        }
        val requestId = intent.getLongExtra(EXTRA_COMMAND_REQUEST_ID, NO_REQUEST)
        requestId.takeIf { it != NO_REQUEST }?.let(::ownStartRequest)
        val revision = intent.getLongExtra(EXTRA_POLICY_REVISION, NO_REVISION)
        val proxyPort = intent.getIntExtra(EXTRA_PROXY_PORT, 0)
        val coreGeneration =
            intent.getLongExtra(EXTRA_CORE_GENERATION, NO_CORE_GENERATION)
        controlExecutor.execute {
          handleStart(
              revision = revision.takeIf { it > 0 },
              proxyPort = proxyPort,
              coreGeneration = coreGeneration.takeIf { it > 0 },
              requestId = requestId.takeIf { it != NO_REQUEST },
          )
        }
      }
      ACTION_STOP -> {
        val requestId = intent.getLongExtra(EXTRA_COMMAND_REQUEST_ID, NO_REQUEST)
        requestId.takeIf { it != NO_REQUEST }?.let(::ownStopRequest)
        val revision = intent.getLongExtra(EXTRA_POLICY_REVISION, NO_REVISION)
        controlExecutor.execute {
          handleStop(
              offRevision = revision.takeIf { it > 0 },
              requestId = requestId.takeIf { it != NO_REQUEST },
          )
        }
      }
      else -> {
        if (!BuildConfig.MASQ_SYSTEM_TUNNEL_ENABLED) {
          controlExecutor.execute { handleDisabledBuild() }
          return START_NOT_STICKY
        }
        controlExecutor.execute(::handleStickyRestart)
      }
    }
    return START_STICKY
  }

  private fun handleDisabledBuild() {
    if (destroyed) return
    val load = policyStore.loadForServiceStart()
    publish(
        load = load,
        transition = SystemRoutingTransition.BLOCKED,
        translatorReady = false,
        coreRouteReady = false,
        diagnostic = SystemRoutingDiagnostic.VPN_INTERFACE_UNAVAILABLE,
    )
    stopForeground(STOP_FOREGROUND_REMOVE)
    stopSelf()
  }

  private fun handleStickyRestart() {
    if (destroyed) return
    val load = policyStore.loadForServiceStart()
    val terminalLease = terminalCoordinator.snapshot()
    if (load !is SystemRoutingPolicyLoadResult.ExplicitOff &&
        terminalLease != null) {
      adoptedTerminalLeaseEpoch = terminalLease.epoch
      publishTerminalHandoffState(load, SystemRoutingDiagnostic.TRANSLATOR_RETURNED)
      val closeResult =
          terminalCoordinator.closeOrJoin(TRANSLATOR_STOP_TIMEOUT_MS)
      if (destroyed) return
      val remainingLease = terminalCoordinator.snapshot()
      if ((closeResult == TerminalLeaseCloseResult.NoLease ||
          closeResult is TerminalLeaseCloseResult.Closed) &&
          remainingLease == null) {
        // The newest service owns the next publication. Re-read policy and
        // establish a fresh blocking interface instead of leaving the old
        // owner's lease-backed status behind.
        handleStickyRestart()
        return
      }
      publishTerminalHandoffState(
          policyStore.loadForServiceStart(),
          terminalCloseDiagnostic(closeResult)
              ?: SystemRoutingDiagnostic.TRANSLATOR_RETURNED,
      )
      if (remainingLease != null) {
        scheduleStickyHandoffRetry()
      }
      return
    }
    if (load !is SystemRoutingPolicyLoadResult.ExplicitOff &&
        terminalCoordinator.blocksNewStart()) {
      publish(
          load = load,
          transition = SystemRoutingTransition.BLOCKED,
          translatorReady = false,
          coreRouteReady = false,
          diagnostic = SystemRoutingDiagnostic.TRANSLATOR_RETURNED,
          tunPresentOverride = false,
      )
      scheduleStickyHandoffRetry()
      return
    }
    when (load) {
      SystemRoutingPolicyLoadResult.Missing -> {
        if (tunnelDescriptor == null) {
          appliedPolicy = null
          publish(
              load = load,
              transition = SystemRoutingTransition.IDLE,
              translatorReady = false,
              coreRouteReady = false,
          )
          stopForeground(STOP_FOREGROUND_REMOVE)
          stopSelf()
        } else {
          publish(
              load = load,
              transition = SystemRoutingTransition.BLOCKED,
              translatorReady = false,
              coreRouteReady = false,
              diagnostic = SystemRoutingDiagnostic.CORRUPT_OR_PARTIAL_POLICY,
          )
        }
      }
      is SystemRoutingPolicyLoadResult.ExplicitOff ->
          handleStop(load.policy.revision, requestId = null)
      is SystemRoutingPolicyLoadResult.BlockRequired -> {
        publish(
            load = load,
            transition = SystemRoutingTransition.BLOCKED,
            translatorReady = false,
            coreRouteReady = false,
            diagnostic = load.reason,
        )
        updateNotification("Dogfood routing cannot restore an unsafe saved policy.")
      }
      is SystemRoutingPolicyLoadResult.Ready -> restoreBlockingTun(load)
    }
  }

  private fun restoreBlockingTun(load: SystemRoutingPolicyLoadResult.Ready) {
    val startEpoch = terminalCoordinator.beginStart()
    if (startEpoch == null) {
      publish(
          load = load,
          transition = SystemRoutingTransition.BLOCKED,
          translatorReady = false,
          coreRouteReady = false,
          diagnostic = SystemRoutingDiagnostic.TRANSLATOR_RETURNED,
      )
      return
    }
    try {
      val currentLoad = policyStore.loadForServiceStart()
      if (currentLoad !is SystemRoutingPolicyLoadResult.Ready ||
          currentLoad.policy != load.policy) {
        publish(
            load = currentLoad,
            transition = SystemRoutingTransition.BLOCKED,
            translatorReady = false,
            coreRouteReady = false,
            diagnostic = SystemRoutingDiagnostic.POLICY_REVISION_CONFLICT,
        )
        return
      }
      if (refuseActivationWithoutNotification(load, requestId = null)) {
        return
      }
      val refusal = unsupportedPolicyDiagnostic(load.policy)
      if (refusal != null) {
        publish(
            load = load,
            transition = SystemRoutingTransition.BLOCKED,
            translatorReady = false,
            coreRouteReady = false,
            diagnostic = refusal,
        )
        updateNotification("Dogfood routing cannot safely restore the saved policy.")
        return
      }
      when (val ensured = ensureBlockingTun(load.policy)) {
        EnsureTunResult.Ready -> {
          publish(
              load = load,
              transition = SystemRoutingTransition.BLOCKED,
              translatorReady = false,
              coreRouteReady = false,
              diagnostic = SystemRoutingDiagnostic.CORE_ROUTE_NOT_READY,
          )
          updateNotification("Captured traffic is blocked until the dogfood route reconnects.")
        }
        is EnsureTunResult.Failed -> {
          if (ensured.diagnostic ==
              SystemRoutingDiagnostic.NOTIFICATION_PERMISSION_REQUIRED) {
            if (refuseActivationWithoutNotification(load, requestId = null)) {
              return
            }
            // Permission was restored after establish; the exact descriptor
            // is locally owned and can continue as the blocking route.
            publish(
                load = load,
                transition = SystemRoutingTransition.BLOCKED,
                translatorReady = false,
                coreRouteReady = false,
                diagnostic = SystemRoutingDiagnostic.CORE_ROUTE_NOT_READY,
            )
            updateNotification(
                "Captured traffic is blocked until the dogfood route reconnects.")
          } else {
            publish(
                load = load,
                transition = SystemRoutingTransition.BLOCKED,
                translatorReady = false,
                coreRouteReady = false,
                diagnostic = ensured.diagnostic,
            )
            updateNotification("Dogfood routing could not restore the requested Android scope.")
          }
        }
        EnsureTunResult.ConflictingAppliedPolicy -> {
          publish(
              load = load,
              transition = SystemRoutingTransition.BLOCKED,
              translatorReady = false,
              coreRouteReady = false,
              diagnostic = SystemRoutingDiagnostic.POLICY_REVISION_CONFLICT,
          )
        }
      }
    } finally {
      terminalCoordinator.finishStart(startEpoch)
    }
  }

  private fun handleStart(
      revision: Long?,
      proxyPort: Int,
      coreGeneration: Long?,
      requestId: Long?,
  ) {
    if (destroyed) {
      requestId?.let {
        settleStart(it, null, "The MASQ VPN service was destroyed before activation.")
      }
      return
    }
    val startEpoch = terminalCoordinator.beginStart()
    if (startEpoch == null) {
      val load = policyStore.loadForServiceStart()
      val terminalLease = terminalCoordinator.snapshot()
      if (terminalLease != null) {
        adoptedTerminalLeaseEpoch = terminalLease.epoch
        publishTerminalHandoffState(load, SystemRoutingDiagnostic.TRANSLATOR_RETURNED)
        val closeResult =
            terminalCoordinator.closeOrJoin(TRANSLATOR_STOP_TIMEOUT_MS)
        if (destroyed) {
          requestId?.let {
            settleStart(it, null, "The MASQ VPN service was destroyed during tunnel handoff.")
          }
          return
        }
        val remainingLease = terminalCoordinator.snapshot()
        if ((closeResult == TerminalLeaseCloseResult.NoLease ||
            closeResult is TerminalLeaseCloseResult.Closed) &&
            remainingLease == null) {
          handleStart(revision, proxyPort, coreGeneration, requestId)
          return
        }
        publishTerminalHandoffState(
            policyStore.loadForServiceStart(),
            terminalCloseDiagnostic(closeResult)
                ?: SystemRoutingDiagnostic.TRANSLATOR_RETURNED,
        )
        requestId?.let {
          settleStart(
              it,
              statusJson(),
              "A retained MASQ tunnel could not complete safe handoff.",
          )
        }
        if (remainingLease != null) {
          scheduleStickyHandoffRetry()
        }
        return
      }
      publish(
          load = load,
          transition = SystemRoutingTransition.BLOCKED,
          translatorReady = false,
          coreRouteReady = false,
          diagnostic = SystemRoutingDiagnostic.TRANSLATOR_RETURNED,
      )
      requestId?.let {
        settleStart(
            it,
            statusJson(),
            "A retained MASQ tunnel is still stopping; retry after cleanup completes.",
        )
      }
      return
    }
    try {
      handleStartWithPermit(revision, proxyPort, coreGeneration, requestId)
    } finally {
      terminalCoordinator.finishStart(startEpoch)
    }
  }

  private fun publishTerminalHandoffState(
      load: SystemRoutingPolicyLoadResult,
      diagnostic: SystemRoutingDiagnostic,
  ) {
    val captureValid = terminalCoordinator.snapshot()?.captureValid == true
    publish(
        load = load,
        transition = SystemRoutingTransition.BLOCKED,
        translatorReady = false,
        coreRouteReady = false,
        diagnostic = diagnostic,
        tunPresentOverride = captureValid,
    )
    updateNotification(
        if (captureValid) {
          "Captured traffic remains blocked while terminal cleanup completes."
        } else {
          "Android capture is no longer valid; traffic may use the direct connection."
        })
  }

  private fun scheduleStickyHandoffRetry() {
    if (destroyed || !handoffRetryScheduled.compareAndSet(false, true)) return
    runCatching {
      handoffRetryExecutor.schedule(
          {
            handoffRetryScheduled.set(false)
            if (!destroyed) {
              runCatching { controlExecutor.execute(::handleStickyRestart) }
            }
          },
          HANDOFF_RETRY_DELAY_MS,
          TimeUnit.MILLISECONDS,
      )
    }.onFailure {
      handoffRetryScheduled.set(false)
    }
  }

  private fun handleStartWithPermit(
      revision: Long?,
      proxyPort: Int,
      coreGeneration: Long?,
      requestId: Long?,
  ) {
    if (destroyed) {
      requestId?.let {
        settleStart(it, null, "The MASQ VPN service was destroyed before activation.")
      }
      return
    }
    val load = policyStore.loadForServiceStart()
    if (load !is SystemRoutingPolicyLoadResult.Ready ||
        revision == null ||
        load.policy.revision != revision) {
      requestId?.let {
        settleStart(
            it,
            null,
            "The MASQ routing request is stale or conflicts with the saved policy.",
        )
      }
      return
    }
    val policy = load.policy
    if (refuseActivationWithoutNotification(load, requestId)) {
      return
    }
    val refusal = unsupportedPolicyDiagnostic(policy)
    if (refusal != null) {
      publish(
          load,
          SystemRoutingTransition.BLOCKED,
          translatorReady = false,
          coreRouteReady = false,
          diagnostic = refusal,
      )
      requestId?.let {
        settleStart(it, statusJson(), "This Android routing policy is not safely supported.")
      }
      return
    }
    when (val ensured = ensureBlockingTun(policy)) {
      is EnsureTunResult.Failed -> {
        if (ensured.diagnostic ==
            SystemRoutingDiagnostic.NOTIFICATION_PERMISSION_REQUIRED) {
          if (refuseActivationWithoutNotification(load, requestId)) {
            return
          }
          // Permission was restored after establish; continue with the exact
          // locally owned blocker.
        } else {
          publish(
              load,
              SystemRoutingTransition.BLOCKED,
              translatorReady = false,
              coreRouteReady = false,
              diagnostic = ensured.diagnostic,
          )
          requestId?.let {
            settleStart(it, statusJson(), "Android could not establish the requested VPN scope.")
          }
          return
        }
      }
      EnsureTunResult.ConflictingAppliedPolicy -> {
        publish(
            load,
            SystemRoutingTransition.BLOCKED,
            translatorReady = false,
            coreRouteReady = false,
            diagnostic = SystemRoutingDiagnostic.POLICY_REVISION_CONFLICT,
        )
        requestId?.let {
          settleStart(it, statusJson(), "A different routing revision still owns the VPN.")
        }
        return
      }
      EnsureTunResult.Ready -> Unit
    }

    if (!MasqPacketTunnelJni.isAvailable || proxyPort !in 1..65535) {
      publish(
          load,
          SystemRoutingTransition.BLOCKED,
          translatorReady = false,
          coreRouteReady = false,
          diagnostic = SystemRoutingDiagnostic.TRANSLATOR_NOT_READY,
      )
      requestId?.let {
        settleStart(it, statusJson(), "The MASQ packet translator is unavailable.")
      }
      return
    }

    if (!coreRouteReady(proxyPort, coreGeneration)) {
      publish(
          load,
          SystemRoutingTransition.BLOCKED,
          translatorReady = false,
          coreRouteReady = false,
          diagnostic = SystemRoutingDiagnostic.CORE_ROUTE_NOT_READY,
      )
      requestId?.let {
        settleStart(it, statusJson(), "The MASQ core route is not ready.")
      }
      return
    }

    val descriptor = tunnelDescriptor
    if (descriptor == null) {
      publish(
          load,
          SystemRoutingTransition.BLOCKED,
          translatorReady = false,
          coreRouteReady = false,
          diagnostic = SystemRoutingDiagnostic.VPN_INTERFACE_UNAVAILABLE,
      )
      requestId?.let {
        settleStart(it, statusJson(), "The Android VPN interface is unavailable.")
      }
      return
    }
    publish(
        load,
        SystemRoutingTransition.RECONNECTING,
        translatorReady = false,
        coreRouteReady = true,
    )
    var startResult =
        startTranslatorForCapturedDescriptor(policy, descriptor, proxyPort)
    if (startResult is TranslatorStartResult.ConfigurationConflict ||
        startResult is TranslatorStartResult.AlreadyReturned) {
      // The embedded core reserves a fresh loopback proxy port after a
      // reconnect. Keep the exact Android TUN open as the blocker, stop and
      // prove release of the old translator generation, then revalidate the
      // policy/core generation before binding that same descriptor to the new
      // port. The old return callback is generation-checked and cannot affect
      // the replacement.
      publish(
          load,
          SystemRoutingTransition.RECONNECTING,
          translatorReady = false,
          coreRouteReady = true,
      )
      val stopResult = stopTranslatorSafely(policy.revision)
      val currentLoad = policyStore.loadForServiceStart()
      val policyStillCurrent =
          currentLoad is SystemRoutingPolicyLoadResult.Ready &&
              currentLoad.policy == policy
      val captureStillCurrent =
          synchronized(runtimeLock) {
            !destroyed &&
                !revoked &&
                localTunCaptureValid &&
                tunnelDescriptor === descriptor &&
                appliedPolicy == policy
          }
      val routeStillCurrent =
          stopResult == TranslatorStopResult.SafeToClose &&
              policyStillCurrent &&
              captureStillCurrent &&
              coreRouteReady(proxyPort, coreGeneration)
      if (!routeStillCurrent) {
        val diagnostic =
            when {
              stopResult != TranslatorStopResult.SafeToClose ->
                  stopDiagnostic(stopResult)
              !policyStillCurrent ->
                  SystemRoutingDiagnostic.POLICY_REVISION_CONFLICT
              else -> SystemRoutingDiagnostic.CORE_ROUTE_NOT_READY
            }
        publish(
            currentLoad,
            SystemRoutingTransition.BLOCKED,
            translatorReady = false,
            coreRouteReady = false,
            diagnostic = diagnostic,
        )
        requestId?.let {
          settleStart(
              it,
              statusJson(),
              "The MASQ route changed while its captured tunnel was recovering.",
          )
        }
        return
      }
      startResult =
          startTranslatorForCapturedDescriptor(policy, descriptor, proxyPort)
    }
    if (startResult != TranslatorStartResult.Started &&
        startResult != TranslatorStartResult.Idempotent) {
      publish(
          load,
          SystemRoutingTransition.BLOCKED,
          translatorReady = false,
          coreRouteReady = false,
          diagnostic = SystemRoutingDiagnostic.TRANSLATOR_RETURNED,
      )
      requestId?.let {
        settleStart(it, statusJson(), "The MASQ packet translator is not running.")
      }
      return
    }

    when (
        translator.awaitReadiness(
            policy.revision,
            TRANSLATOR_READY_TIMEOUT_MS,
            TRANSLATOR_READY_POLL_MS,
        )) {
      TranslatorReadiness.Ready -> {
        val beforePreflightLoad = policyStore.loadForServiceStart()
        val beforePreflightCurrent =
            beforePreflightLoad is SystemRoutingPolicyLoadResult.Ready &&
                beforePreflightLoad.policy == policy &&
                captureIsCurrent(policy, descriptor) &&
                coreGeneration == MasqCoreLifecycle.startGeneration.get() &&
                requestId != null &&
                hasStartAcknowledgement(requestId)
        val preflightReady =
            beforePreflightCurrent &&
                coreRoutePreflightReady(proxyPort, coreGeneration)
        // The real CONNECT preflight can block for seconds. Re-read every
        // mutable authority after it returns before considering ACTIVE.
        val finalLoad = policyStore.loadForServiceStart()
        val finalPolicyCurrent =
            finalLoad is SystemRoutingPolicyLoadResult.Ready &&
                finalLoad.policy == policy
        val activated =
            if (!preflightReady || !finalPolicyCurrent || requestId == null) {
              false
            } else {
              synchronized(runtimeLock) {
                val exactCapture =
                    !destroyed &&
                        !revoked &&
                        localTunCaptureValid &&
                        tunnelDescriptor === descriptor &&
                        appliedPolicy == policy
                val translatorStillExact =
                    translator.isRunning(policy.revision)
                if (!exactCapture || !translatorStillExact) {
                  false
                } else {
                  // Core start/stop invalidates its generation while holding
                  // this same lock. Keep it through semantic bridge
                  // acknowledgement and publication, so a stale-core
                  // callback can neither authorize nor race a transient
                  // ACTIVE snapshot.
                  synchronized(MasqCoreLifecycle.lock) {
                    val exactCoreGeneration =
                        coreGeneration ==
                            MasqCoreLifecycle.startGeneration.get()
                    if (!exactCoreGeneration) {
                      false
                    } else {
                      val candidateStatus = activeCandidateStatusJson(policy)
                      val acknowledgementAccepted =
                          settleStart(requestId, candidateStatus, null)
                      val authorityStillExact =
                          acknowledgementAccepted &&
                              coreGeneration ==
                                  MasqCoreLifecycle.startGeneration.get() &&
                              !destroyed &&
                              !revoked &&
                              localTunCaptureValid &&
                              tunnelDescriptor === descriptor &&
                              appliedPolicy == policy &&
                              translator.isRunning(policy.revision)
                      if (!authorityStillExact) {
                        false
                      } else {
                        publish(
                            finalLoad,
                            SystemRoutingTransition.IDLE,
                            translatorReady = true,
                            coreRouteReady = true,
                            activeProxyPort = proxyPort,
                        )
                        updateNotification(
                            "Captured IPv4 TCP/443 and virtual DNS are using MASQ.")
                        true
                      }
                    }
                  }
                }
              }
            }
        if (!activated) {
          val permissionRevoked = revoked
          val stopAfterFailedActivation =
              stopTranslatorSafely(policy.revision)
          publish(
              finalLoad,
              if (permissionRevoked) {
                SystemRoutingTransition.REVOKED
              } else {
                SystemRoutingTransition.BLOCKED
              },
              translatorReady = false,
              coreRouteReady = false,
              diagnostic =
                  when {
                    permissionRevoked ->
                        SystemRoutingDiagnostic.PERMISSION_REVOKED
                    !finalPolicyCurrent ->
                        SystemRoutingDiagnostic.POLICY_REVISION_CONFLICT
                    stopAfterFailedActivation != TranslatorStopResult.SafeToClose ->
                        stopDiagnostic(stopAfterFailedActivation)
                    else -> SystemRoutingDiagnostic.CORE_ROUTE_NOT_READY
                  },
              tunPresentOverride = if (permissionRevoked) false else null,
          )
          if (requestId != null) {
            settleStart(
                requestId,
                statusJson(),
                if (permissionRevoked) {
                  "Android revoked MASQ VPN permission before activation."
                } else {
                  "The MASQ route changed or its start request expired before activation."
                },
            )
          }
        }
      }
      else -> {
        val stopAfterReadinessFailure =
            stopTranslatorSafely(policy.revision)
        publish(
            load,
            SystemRoutingTransition.BLOCKED,
            translatorReady = false,
            coreRouteReady = false,
            diagnostic =
                if (stopAfterReadinessFailure ==
                    TranslatorStopResult.SafeToClose) {
                  SystemRoutingDiagnostic.TRANSLATOR_NOT_READY
                } else {
                  stopDiagnostic(stopAfterReadinessFailure)
                },
        )
        requestId?.let {
          settleStart(it, statusJson(), "The MASQ packet translator did not become ready.")
        }
      }
    }
  }

  private fun startTranslatorForCapturedDescriptor(
      policy: DesiredSystemRoutingPolicy,
      descriptor: ParcelFileDescriptor,
      proxyPort: Int,
  ): TranslatorStartResult =
      synchronized(runtimeLock) {
        if (destroyed ||
            revoked ||
            tunnelDescriptor !== descriptor ||
            appliedPolicy != policy) {
          TranslatorStartResult.SubmissionFailed
        } else {
          translator.start(policy.revision, descriptor.fd, proxyPort, TUNNEL_MTU) {
              returnedRevision,
              returnedNativeGeneration,
              returnedRunAttemptEpoch,
              nativeResult ->
            runCatching {
              controlExecutor.execute {
                handleTranslatorReturn(
                    returnedRevision,
                    returnedNativeGeneration,
                    returnedRunAttemptEpoch,
                    nativeResult,
                )
              }
            }
          }
        }
      }

  private fun captureIsCurrent(
      policy: DesiredSystemRoutingPolicy,
      descriptor: ParcelFileDescriptor,
  ): Boolean =
      synchronized(runtimeLock) {
        !destroyed &&
            !revoked &&
            localTunCaptureValid &&
            tunnelDescriptor === descriptor &&
            appliedPolicy == policy
      }

  private fun activeCandidateStatusJson(
      policy: DesiredSystemRoutingPolicy,
  ): String =
      SystemRoutingStatus.derive(
              supported =
                  BuildConfig.MASQ_SYSTEM_TUNNEL_ENABLED &&
                      MasqPacketTunnelJni.isAvailable,
              desiredRevision = policy.revision,
              desiredMode = policy.desiredMode,
              desiredSelectedApps = policy.selectedApps,
              failClosedDesired = policy.failClosedDesired,
              appliedRevision = policy.revision,
              appliedMode = policy.desiredMode,
              appliedSelectedApps = policy.selectedApps,
              transition = SystemRoutingTransition.IDLE,
              tunPresent = true,
              translatorReady = true,
              coreRouteReady = true,
              alwaysOn = currentAlwaysOn(),
              lockdown = currentLockdown(),
              lastError = null,
          )
          .toJson()

  private fun handleTranslatorReturn(
      revision: Long,
      nativeGeneration: Long,
      runAttemptEpoch: Long,
      nativeResult: Int?,
  ) {
    if (destroyed) return
    if (!translator.owns(
            TranslatorOwnership(
                revision,
                nativeGeneration,
                runAttemptEpoch,
            ))) {
      return
    }
    val load = policyStore.loadForServiceStart()
    val applied = appliedPolicy
    if (applied?.revision != revision) return
    publish(
        load,
        SystemRoutingTransition.BLOCKED,
        translatorReady = false,
        coreRouteReady = false,
        diagnostic = SystemRoutingDiagnostic.TRANSLATOR_RETURNED,
    )
    updateNotification(
        if (nativeResult == MasqPacketTunnelJni.START_STOPPED) {
          "Captured traffic remains blocked while the dogfood translator is stopped."
        } else {
          "Captured traffic is blocked because the dogfood translator returned."
        })
  }

  private fun handleStop(
      offRevision: Long?,
      requestId: Long?,
  ) {
    if (destroyed) {
      requestId?.let {
        settleStop(it, null, "The MASQ VPN service was destroyed before shutdown.")
      }
      return
    }
    val resetEpoch = terminalCoordinator.beginExplicitReset()
    if (resetEpoch == null) {
      requestId?.let {
        settleStop(
            it,
            null,
            "Another MASQ tunnel start or shutdown is still in progress.",
        )
      }
      scheduleTerminalCleanupRetryIfBlocked()
      return
    }
    try {
      handleStopWithPermit(offRevision, requestId)
    } finally {
      terminalCoordinator.finishExplicitReset(resetEpoch)
    }
  }

  private fun handleStopWithPermit(
      offRevision: Long?,
      requestId: Long?,
  ) {
    if (destroyed) {
      requestId?.let {
        settleStop(it, null, "The MASQ VPN service was destroyed before shutdown.")
      }
      return
    }
    val load = policyStore.loadForServiceStart()
    if (load !is SystemRoutingPolicyLoadResult.ExplicitOff ||
        offRevision == null ||
        load.policy.revision != offRevision) {
      requestId?.let {
        settleStop(
            it,
            null,
            "The MASQ stop request is stale or conflicts with the saved policy.",
        )
      }
      return
    }
    publish(
        load,
        SystemRoutingTransition.STOPPING,
        translatorReady = false,
        coreRouteReady = false,
    )
    when (val close = stopAndCloseAllTunnelsSafely()) {
      TunnelCloseResult.Closed -> Unit
      is TunnelCloseResult.StopFailed -> {
        publish(
            load,
            SystemRoutingTransition.BLOCKED,
            translatorReady = false,
            coreRouteReady = false,
            diagnostic = close.diagnostic,
        )
        updateNotification("Captured traffic remains blocked because shutdown is incomplete.")
        requestId?.let {
          settleStop(it, null, "The MASQ packet translator did not stop safely.")
        }
        scheduleTerminalCleanupRetryIfBlocked()
        return
      }
      TunnelCloseResult.CloseFailed -> {
        publish(
            load,
            SystemRoutingTransition.BLOCKED,
            translatorReady = false,
            coreRouteReady = false,
            diagnostic = SystemRoutingDiagnostic.TUNNEL_CLOSE_FAILED,
        )
        requestId?.let {
          settleStop(it, null, "The Android VPN interface could not be closed.")
        }
        scheduleTerminalCleanupRetryIfBlocked()
        return
      }
    }
    publish(
        load,
        SystemRoutingTransition.IDLE,
        translatorReady = false,
        coreRouteReady = false,
    )
    val status = statusJson()
    requestId?.let { settleStop(it, status, null) }
    stopForeground(STOP_FOREGROUND_REMOVE)
    stopSelf()
  }

  private fun handleExplicitReset(requestId: Long?) {
    if (destroyed) {
      requestId?.let {
        settleReset(it, null, "The MASQ VPN service was destroyed before reset.")
      }
      return
    }
    val resetEpoch = terminalCoordinator.beginExplicitReset()
    if (resetEpoch == null) {
      requestId?.let {
        settleReset(
            it,
            null,
            "Another MASQ tunnel start or shutdown is still in progress.",
        )
      }
      return
    }
    try {
      handleExplicitResetWithPermit(requestId)
    } finally {
      terminalCoordinator.finishExplicitReset(resetEpoch)
    }
  }

  private fun handleExplicitResetWithPermit(requestId: Long?) {
    if (destroyed) {
      requestId?.let {
        settleReset(it, null, "The MASQ VPN service was destroyed before reset.")
      }
      return
    }
    val load = policyStore.loadForServiceStart()
    publish(
        load,
        SystemRoutingTransition.STOPPING,
        translatorReady = false,
        coreRouteReady = false,
    )
    when (val close = stopAndCloseAllTunnelsSafely()) {
      TunnelCloseResult.Closed -> Unit
      is TunnelCloseResult.StopFailed -> {
        publish(
            load,
            SystemRoutingTransition.BLOCKED,
            translatorReady = false,
            coreRouteReady = false,
            diagnostic = close.diagnostic,
        )
        updateNotification("Captured traffic remains blocked because reset shutdown is incomplete.")
        requestId?.let {
          settleReset(it, null, "The MASQ packet translator did not stop safely for reset.")
        }
        return
      }
      TunnelCloseResult.CloseFailed -> {
        publish(
            load,
            SystemRoutingTransition.BLOCKED,
            translatorReady = false,
            coreRouteReady = false,
            diagnostic = SystemRoutingDiagnostic.TUNNEL_CLOSE_FAILED,
        )
        requestId?.let {
          settleReset(it, null, "The Android VPN interface could not be closed for reset.")
        }
        return
      }
    }
    if (destroyed) return
    if (terminalCoordinator.snapshot() != null || !processNativeReleaseConfirmed()) {
      publish(
          load,
          SystemRoutingTransition.BLOCKED,
          translatorReady = false,
          coreRouteReady = false,
          diagnostic = SystemRoutingDiagnostic.TRANSLATOR_RETURNED,
      )
      requestId?.let {
        settleReset(
            it,
            null,
            "MASQ cannot reset until the retained native tunnel owner is released.",
        )
      }
      return
    }

    when (val clear = policyStore.clearAfterExplicitReset()) {
      SystemRoutingPolicyClearResult.Cleared -> {
        val missing = SystemRoutingPolicyLoadResult.Missing
        publish(
            missing,
            SystemRoutingTransition.IDLE,
            translatorReady = false,
            coreRouteReady = false,
            tunPresentOverride = false,
        )
        requestId?.let { settleReset(it, statusJson(), null) }
        stopForeground(STOP_FOREGROUND_REMOVE)
        stopSelf()
      }
      is SystemRoutingPolicyClearResult.IndeterminateClear -> {
        val blocked = SystemRoutingPolicyLoadResult.BlockRequired(clear.reason)
        publish(
            blocked,
            SystemRoutingTransition.BLOCKED,
            translatorReady = false,
            coreRouteReady = false,
            diagnostic = clear.reason,
            tunPresentOverride = false,
        )
        updateNotification("Dogfood routing reset could not verify policy removal.")
        requestId?.let {
          settleReset(it, null, "Android could not verify system-routing policy removal.")
        }
      }
    }
  }

  private fun ensureBlockingTun(policy: DesiredSystemRoutingPolicy): EnsureTunResult {
    notificationPermissionDiagnostic(policy)?.let {
      return EnsureTunResult.Failed(it)
    }
    val existing =
        synchronized(runtimeLock) {
          if (destroyed || revoked) {
            return EnsureTunResult.Failed(
                if (revoked) {
                  SystemRoutingDiagnostic.PERMISSION_REVOKED
                } else {
                  SystemRoutingDiagnostic.VPN_INTERFACE_UNAVAILABLE
                })
          }
          tunnelDescriptor
        }
    if (existing != null) {
      return if (localTunCaptureValid && appliedPolicy == policy) {
        EnsureTunResult.Ready
      } else {
        EnsureTunResult.ConflictingAppliedPolicy
      }
    }
    val packageDiagnostic = validateInstalledPackages(policy)
    if (packageDiagnostic != null) return EnsureTunResult.Failed(packageDiagnostic)

    return try {
      // Package existence is deliberately rechecked directly before Builder applies the scope.
      val immediatelyValidated = validateInstalledPackages(policy)
      if (immediatelyValidated != null) return EnsureTunResult.Failed(immediatelyValidated)
      val builder =
          Builder()
              .setSession("MASQ private route")
              .setMtu(TUNNEL_MTU)
              .addAddress("10.111.0.1", 32)
              .addAddress("fd00:111::1", 128)
              .addRoute("0.0.0.0", 0)
              .addRoute("::", 0)
              .addDnsServer("10.111.0.2")
              .setBlocking(false)
      when (policy.desiredMode) {
        SystemRoutingMode.WHOLE_DEVICE ->
            MASQ_CONTROL_PLANE_PACKAGE_IDS.forEach { packageId ->
              try {
                builder.addDisallowedApplication(packageId)
              } catch (_: PackageManager.NameNotFoundException) {
                // The companion public/dogfood package is optional; the running
                // package is always present and remains excluded.
              }
            }
        SystemRoutingMode.SELECTED_APPS ->
            policy.selectedApps.forEach(builder::addAllowedApplication)
        SystemRoutingMode.OFF ->
            return EnsureTunResult.Failed(SystemRoutingDiagnostic.INVALID_START_MODE)
      }
      builder.setConfigureIntent(
          PendingIntent.getActivity(
              this,
              0,
              Intent(this, MainActivity::class.java),
              PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
          ))
      synchronized(runtimeLock) {
        if (destroyed || revoked) {
          return@synchronized EnsureTunResult.Failed(
              if (revoked) {
                SystemRoutingDiagnostic.PERMISSION_REVOKED
              } else {
                SystemRoutingDiagnostic.VPN_INTERFACE_UNAVAILABLE
              })
        }
        notificationPermissionDiagnostic(policy)?.let {
          return@synchronized EnsureTunResult.Failed(it)
        }
        val descriptor =
            builder.establish()
                ?: return@synchronized EnsureTunResult.Failed(
                    SystemRoutingDiagnostic.VPN_INTERFACE_UNAVAILABLE)
        tunnelDescriptor = descriptor
        appliedPolicy = policy
        localTunCaptureValid = true
        notificationPermissionDiagnostic(policy)?.let {
          // Publish local ownership before returning the refusal. The caller
          // transfers this exact PFD through terminal close bookkeeping, so
          // an indeterminate close remains strongly owned and truthfully
          // reported instead of becoming an untracked captured route.
          return@synchronized EnsureTunResult.Failed(it)
        }
        EnsureTunResult.Ready
      }
    } catch (_: PackageManager.NameNotFoundException) {
      EnsureTunResult.Failed(SystemRoutingDiagnostic.PACKAGE_NOT_INSTALLED)
    } catch (_: Exception) {
      EnsureTunResult.Failed(SystemRoutingDiagnostic.VPN_INTERFACE_UNAVAILABLE)
    }
  }

  @Suppress("DEPRECATION")
  private fun validateInstalledPackages(
      policy: DesiredSystemRoutingPolicy,
  ): SystemRoutingDiagnostic? {
    if (policy.desiredMode == SystemRoutingMode.SELECTED_APPS &&
        policy.selectedApps.any(::isMasqControlPlanePackage)) {
      return SystemRoutingDiagnostic.OWN_PACKAGE_UNSUPPORTED
    }
    return policy.selectedApps
        .firstOrNull { packageId ->
          runCatching { packageManager.getApplicationInfo(packageId, 0) }.isFailure
        }
        ?.let { SystemRoutingDiagnostic.PACKAGE_NOT_INSTALLED }
  }

  private fun unsupportedPolicyDiagnostic(
      policy: DesiredSystemRoutingPolicy,
  ): SystemRoutingDiagnostic? =
      when {
        policy.failClosedDesired -> SystemRoutingDiagnostic.FAIL_CLOSED_UNSUPPORTED
        currentLockdown() -> SystemRoutingDiagnostic.LOCKDOWN_UNSUPPORTED
        currentAlwaysOn() -> SystemRoutingDiagnostic.ALWAYS_ON_UNSUPPORTED
        else -> null
      }

  private fun notificationPermissionDiagnostic(
      policy: DesiredSystemRoutingPolicy,
  ): SystemRoutingDiagnostic? =
      systemRoutingNotificationPermissionDiagnostic(
          sdkInt = Build.VERSION.SDK_INT,
          permissionGranted =
              Build.VERSION.SDK_INT < ANDROID_POST_NOTIFICATIONS_API_LEVEL ||
                  ContextCompat.checkSelfPermission(
                      this,
                      Manifest.permission.POST_NOTIFICATIONS,
                  ) == PackageManager.PERMISSION_GRANTED,
          desiredMode = policy.desiredMode,
      )

  private fun refuseActivationWithoutNotification(
      load: SystemRoutingPolicyLoadResult.Ready,
      requestId: Long?,
  ): Boolean {
    val diagnostic =
        notificationPermissionDiagnostic(load.policy) ?: return false
    val closeResult = stopAndCloseAllTunnelsSafely()
    publish(
        load,
        SystemRoutingTransition.BLOCKED,
        translatorReady = false,
        coreRouteReady = false,
        diagnostic =
            if (closeResult == TunnelCloseResult.CloseFailed) {
              SystemRoutingDiagnostic.TUNNEL_CLOSE_FAILED
            } else {
              diagnostic
            },
        tunPresentOverride =
            if (closeResult == TunnelCloseResult.Closed) false else null,
    )
    requestId?.let {
      settleStart(
          it,
          statusJson(),
          "Allow Android notifications before starting dogfood system routing.",
      )
    }
    if (closeResult == TunnelCloseResult.Closed) {
      stopForeground(STOP_FOREGROUND_REMOVE)
      stopSelf()
    } else {
      scheduleTerminalCleanupRetryIfBlocked()
    }
    return true
  }

  private fun coreRouteReady(
      proxyPort: Int,
      coreGeneration: Long?,
  ): Boolean =
      serializedCoreRouteCheck(proxyPort, coreGeneration) {
        MasqCoreJni.nativeGetStatus()
      }

  private fun coreRoutePreflightReady(
      proxyPort: Int,
      coreGeneration: Long?,
  ): Boolean =
      serializedCoreRouteCheck(proxyPort, coreGeneration) {
        // This performs a real CONNECT example.com:443 over the current MASQ
        // exit route. ACTIVE is never published from phase/port alone.
        MasqCoreJni.nativePreflightProxy()
      }

  private fun serializedCoreRouteCheck(
      proxyPort: Int,
      coreGeneration: Long?,
      operation: () -> String,
  ): Boolean {
    if (!MasqCoreJni.isAvailable ||
        proxyPort !in 1..65535 ||
        coreGeneration == null) {
      return false
    }
    val outcome = CompletableFuture<Boolean>()
    MasqCoreLifecycle.executor.execute {
      if (coreGeneration != MasqCoreLifecycle.startGeneration.get()) {
        outcome.complete(false)
        return@execute
      }
      val ready =
          runCatching { JSONObject(operation()) }
              .getOrNull()
              ?.let { status ->
                status.optString("phase") == "connected" &&
                    status.optInt("proxyPort", 0) == proxyPort &&
                    coreGeneration ==
                        MasqCoreLifecycle.startGeneration.get()
              } == true
      outcome.complete(ready)
    }
    return runCatching {
          outcome.get(CORE_PREFLIGHT_TIMEOUT_MS, TimeUnit.MILLISECONDS)
        }
        .getOrDefault(false)
  }

  private fun stopTranslatorSafely(revision: Long?): TranslatorStopResult =
      if (!MasqPacketTunnelJni.isAvailable && !translator.hasOwnedRun()) {
        TranslatorStopResult.SafeToClose
      } else {
        translator.stopAndAwait(revision, TRANSLATOR_STOP_TIMEOUT_MS)
      }

  private fun processNativeReleaseConfirmed(): Boolean =
      if (!MasqPacketTunnelJni.isAvailable && !translator.hasOwnedRun()) {
        true
      } else {
        translator.confirmsProcessReleased(expectedOwnership = null)
      }

  private fun stopAndCloseAllTunnelsSafely(): TunnelCloseResult =
      stopAndCloseTunnelSafely()

  private fun stopAndCloseTunnelSafely(): TunnelCloseResult =
      run {
        var retainResult: TerminalLeaseRetainResult? = null
        val snapshot =
            synchronized(runtimeLock) {
              Pair(tunnelDescriptor, appliedPolicy).also { (descriptor, policy) ->
                if (descriptor != null) {
                  // Transfer exact close ownership while the local descriptor
                  // identity is locked. From this point every competing
                  // stop/onDestroy joins the coordinator; no local path can
                  // race a second close.
                  retainResult =
                      terminalCoordinator.retain(
                          resource = descriptor,
                          policy = policy,
                          translator = translator,
                          captureValid = localTunCaptureValid,
                      )
                  if (retainResult !is TerminalLeaseRetainResult.Conflict) {
                    // Relinquish local ownership in the same critical section
                    // that transfers it. onDestroy can now only join this
                    // exact terminal lease; it cannot retain an already-closed
                    // descriptor during the close/remove completion window.
                    adoptedTerminalLeaseEpoch = retainResult?.epoch
                    tunnelDescriptor = null
                    appliedPolicy = null
                    localTunCaptureValid = false
                  }
                }
              }
            }
        val descriptor = snapshot.first
        val policy = snapshot.second
        if (retainResult is TerminalLeaseRetainResult.Conflict) {
          return@run TunnelCloseResult.StopFailed(
              SystemRoutingDiagnostic.TRANSLATOR_RETURNED)
        }
        val closeResult =
            if (descriptor != null || terminalCoordinator.snapshot() != null) {
              terminalCoordinator.closeOrJoin(TRANSLATOR_STOP_TIMEOUT_MS)
            } else {
              val stopResult = stopTranslatorSafely(policy?.revision)
              if (stopResult == TranslatorStopResult.SafeToClose) {
                TerminalLeaseCloseResult.NoLease
              } else {
                TerminalLeaseCloseResult.StopFailed(
                    epoch = 0,
                    result = stopResult,
                )
              }
            }
        val tunnelResult = terminalCloseResult(closeResult)
        if (tunnelResult != TunnelCloseResult.Closed) {
          return@run tunnelResult
        }
        synchronized(runtimeLock) {
          val closedEpoch =
              (closeResult as? TerminalLeaseCloseResult.Closed)?.epoch
          if (closedEpoch != null &&
              adoptedTerminalLeaseEpoch == closedEpoch) {
            adoptedTerminalLeaseEpoch = null
          } else if (closeResult == TerminalLeaseCloseResult.NoLease &&
              terminalCoordinator.snapshot() == null) {
            adoptedTerminalLeaseEpoch = null
          }
          // A successful transfer already cleared the exact local owner.
          // Reject any impossible reappearance instead of touching a newer
          // descriptor.
          if (descriptor != null && tunnelDescriptor === descriptor) {
            return@synchronized TunnelCloseResult.CloseFailed
          }
          TunnelCloseResult.Closed
        }
      }

  private fun scheduleTerminalCleanupRetryIfBlocked() {
    if (terminalCoordinator.blocksNewStart()) {
      scheduleStickyHandoffRetry()
    }
  }

  private fun terminalCloseResult(
      result: TerminalLeaseCloseResult,
  ): TunnelCloseResult =
      when (result) {
        TerminalLeaseCloseResult.NoLease,
        is TerminalLeaseCloseResult.Closed -> TunnelCloseResult.Closed
        is TerminalLeaseCloseResult.StopFailed ->
            TunnelCloseResult.StopFailed(stopDiagnostic(result.result))
        is TerminalLeaseCloseResult.JoinTimedOut ->
            TunnelCloseResult.StopFailed(
                SystemRoutingDiagnostic.TRANSLATOR_STOP_TIMEOUT)
        is TerminalLeaseCloseResult.ProcessOwnershipNotReleased,
        is TerminalLeaseCloseResult.StaleCompletion ->
            TunnelCloseResult.StopFailed(SystemRoutingDiagnostic.TRANSLATOR_RETURNED)
        is TerminalLeaseCloseResult.DescriptorCloseFailed ->
            TunnelCloseResult.CloseFailed
      }

  private fun stopDiagnostic(result: TranslatorStopResult): SystemRoutingDiagnostic =
      when (result) {
        TranslatorStopResult.TimedOutKeepBlocking,
        TranslatorStopResult.StopWasNotAcceptedKeepBlocking ->
            SystemRoutingDiagnostic.TRANSLATOR_STOP_TIMEOUT
        else -> SystemRoutingDiagnostic.TRANSLATOR_RETURNED
      }

  private fun terminalCloseDiagnostic(
      result: TerminalLeaseCloseResult,
  ): SystemRoutingDiagnostic? =
      when (result) {
        TerminalLeaseCloseResult.NoLease,
        is TerminalLeaseCloseResult.Closed -> null
        is TerminalLeaseCloseResult.JoinTimedOut ->
            SystemRoutingDiagnostic.TRANSLATOR_STOP_TIMEOUT
        is TerminalLeaseCloseResult.DescriptorCloseFailed ->
            SystemRoutingDiagnostic.TUNNEL_CLOSE_FAILED
        is TerminalLeaseCloseResult.StopFailed -> stopDiagnostic(result.result)
        is TerminalLeaseCloseResult.ProcessOwnershipNotReleased,
        is TerminalLeaseCloseResult.StaleCompletion ->
            SystemRoutingDiagnostic.TRANSLATOR_RETURNED
      }

  private fun ownStartRequest(requestId: Long) {
    synchronized(acknowledgementOwnershipLock) {
      ownedStartRequests += requestId
    }
  }

  private fun ownStopRequest(requestId: Long) {
    synchronized(acknowledgementOwnershipLock) {
      ownedStopRequests += requestId
    }
  }

  private fun ownResetRequest(requestId: Long) {
    synchronized(acknowledgementOwnershipLock) {
      ownedResetRequests += requestId
    }
  }

  private fun settleStart(
      requestId: Long,
      status: String?,
      error: String?,
  ): Boolean {
    synchronized(acknowledgementOwnershipLock) {
      ownedStartRequests.remove(requestId)
    }
    return acknowledgeStart(requestId, status, error)
  }

  private fun settleStop(requestId: Long, status: String?, error: String?) {
    synchronized(acknowledgementOwnershipLock) {
      ownedStopRequests.remove(requestId)
    }
    acknowledgeStop(requestId, status, error)
  }

  private fun settleReset(requestId: Long, status: String?, error: String?) {
    synchronized(acknowledgementOwnershipLock) {
      ownedResetRequests.remove(requestId)
    }
    acknowledgeReset(requestId, status, error)
  }

  private fun settleOwnedRequests(error: String) {
    val owned =
        synchronized(acknowledgementOwnershipLock) {
          Triple(
              ownedStartRequests.toList(),
              ownedStopRequests.toList(),
              ownedResetRequests.toList(),
          ).also {
            ownedStartRequests.clear()
            ownedStopRequests.clear()
            ownedResetRequests.clear()
          }
        }
    owned.first.forEach { acknowledgeStart(it, null, error) }
    owned.second.forEach { acknowledgeStop(it, null, error) }
    owned.third.forEach { acknowledgeReset(it, null, error) }
  }

  private fun publish(
      load: SystemRoutingPolicyLoadResult,
      transition: SystemRoutingTransition,
      translatorReady: Boolean,
      coreRouteReady: Boolean,
      diagnostic: SystemRoutingDiagnostic? = null,
      tunPresentOverride: Boolean? = null,
      activeProxyPort: Int? = null,
  ) {
    if (destroyed) return
    val desired =
        when (load) {
          is SystemRoutingPolicyLoadResult.Ready -> load.policy
          is SystemRoutingPolicyLoadResult.ExplicitOff -> load.policy
          else -> null
        }
    val terminalLease = terminalCoordinator.snapshot()
    val permissionRevoked = revoked
    val localCaptureValid = tunnelDescriptor != null && localTunCaptureValid
    val terminalCaptureValid = terminalLease?.captureValid == true
    val tunPresent =
        if (permissionRevoked) {
          false
        } else {
          tunPresentOverride ?: (localCaptureValid || terminalCaptureValid)
        }
    val applied =
        if (tunPresent) {
          appliedPolicy.takeIf { localCaptureValid }
              ?: terminalLease?.policy.takeIf { terminalCaptureValid }
        } else {
          null
        }
    updateStatus(
        status =
            SystemRoutingStatus.derive(
                supported =
                    BuildConfig.MASQ_SYSTEM_TUNNEL_ENABLED &&
                        MasqPacketTunnelJni.isAvailable,
                desiredRevision = desired?.revision,
                desiredMode = desired?.desiredMode ?: SystemRoutingMode.OFF,
                desiredSelectedApps = desired?.selectedApps ?: emptyList(),
                failClosedDesired = desired?.failClosedDesired ?: false,
                appliedRevision = applied?.revision,
                appliedMode = applied?.desiredMode ?: SystemRoutingMode.OFF,
                appliedSelectedApps = applied?.selectedApps ?: emptyList(),
                transition =
                    if (permissionRevoked) {
                      SystemRoutingTransition.REVOKED
                    } else {
                      transition
                    },
                tunPresent = tunPresent,
                translatorReady = translatorReady,
                coreRouteReady = coreRouteReady,
                alwaysOn = currentAlwaysOn(),
                lockdown = currentLockdown(),
                lastError =
                    if (permissionRevoked &&
                        diagnostic != SystemRoutingDiagnostic.TUNNEL_CLOSE_FAILED) {
                      SystemRoutingDiagnostic.PERMISSION_REVOKED
                    } else {
                      diagnostic
                    },
            ),
        activeProxyPort = activeProxyPort,
        ownerEpoch = serviceEpoch,
    )
  }

  override fun onRevoke() {
    val revokedOwnership =
        synchronized(runtimeLock) {
          revoked = true
          localTunCaptureValid = false
          Pair(tunnelDescriptor, adoptedTerminalLeaseEpoch)
        }
    terminalCoordinator.invalidateCapture(
        resource = revokedOwnership.first,
        translator = translator,
        adoptedEpoch = revokedOwnership.second,
    )
    val revokedLoad =
        runCatching { policyStore.loadForServiceStart() }
            .getOrElse {
              SystemRoutingPolicyLoadResult.BlockRequired(
                  SystemRoutingDiagnostic.POLICY_READ_FAILED)
            }
    // Android capture is already deactivated when this callback begins.
    // Publish that truth synchronously; cleanup can wait behind a busy
    // control executor without leaving a stale ACTIVE snapshot.
    publish(
        revokedLoad,
        SystemRoutingTransition.REVOKED,
        translatorReady = false,
        coreRouteReady = false,
        diagnostic = SystemRoutingDiagnostic.PERMISSION_REVOKED,
        tunPresentOverride = false,
    )
    controlExecutor.execute {
      if (destroyed) return@execute
      val load = policyStore.loadForServiceStart()
      publish(
          load,
          SystemRoutingTransition.REVOKED,
          translatorReady = false,
          coreRouteReady = false,
          diagnostic = SystemRoutingDiagnostic.PERMISSION_REVOKED,
          tunPresentOverride = false,
      )
      settleOwnedRequests("Android revoked MASQ VPN permission.")
      val closeResult = stopAndCloseTunnelSafely()
      if (closeResult != TunnelCloseResult.Closed) {
        publish(
            load,
            SystemRoutingTransition.REVOKED,
            translatorReady = false,
            coreRouteReady = false,
            diagnostic =
                if (closeResult == TunnelCloseResult.CloseFailed) {
                  SystemRoutingDiagnostic.TUNNEL_CLOSE_FAILED
                } else {
                  SystemRoutingDiagnostic.PERMISSION_REVOKED
                },
            tunPresentOverride = false,
        )
      }
      stopForeground(STOP_FOREGROUND_REMOVE)
      stopSelf()
    }
    super.onRevoke()
  }

  override fun onDestroy() {
    var retainResult: TerminalLeaseRetainResult? = null
    val terminalSnapshot =
        synchronized(runtimeLock) {
          destroyed = true
          val localPolicy = appliedPolicy
          val localDescriptor = tunnelDescriptor
          val localCaptureValid = localTunCaptureValid
          localDescriptor?.let { descriptor ->
              // Ownership transfer is atomic with the destroyed/local
              // snapshot. Any in-flight local stop now joins this exact lease.
              retainResult =
                  terminalCoordinator.retain(
                      resource = descriptor,
                      policy = localPolicy,
                      translator = translator,
                      captureValid = localCaptureValid,
                  )
              if (retainResult !is TerminalLeaseRetainResult.Conflict) {
                adoptedTerminalLeaseEpoch = retainResult?.epoch
                tunnelDescriptor = null
                appliedPolicy = null
                localTunCaptureValid = false
              }
            }
          val retainedLease = terminalCoordinator.snapshot()
          val localStillOwned =
              localDescriptor != null && tunnelDescriptor === localDescriptor
          ServiceDestructionSnapshot(
              localDescriptor = localDescriptor,
              retainedAppliedPolicy =
                  localPolicy.takeIf {
                    localStillOwned && localCaptureValid
                  }
                      ?: retainedLease?.takeIf { it.captureValid }?.policy,
              tunPresent = localStillOwned || retainedLease != null,
              captureValid =
                  (localStillOwned && localCaptureValid) ||
                      retainedLease?.captureValid == true,
          )
        }
    handoffRetryExecutor.shutdownNow()
    val load =
        runCatching { policyStore.loadForServiceStart() }
            .getOrElse {
              SystemRoutingPolicyLoadResult.BlockRequired(
                  SystemRoutingDiagnostic.POLICY_READ_FAILED)
            }
    val alwaysOn = currentAlwaysOn()
    val lockdown = currentLockdown()
    publishServiceDestroyed(
        load = load,
        alwaysOn = alwaysOn,
        lockdown = lockdown,
        retainedAppliedPolicy = terminalSnapshot.retainedAppliedPolicy,
        tunPresent = terminalSnapshot.tunPresent,
        captureValid = terminalSnapshot.captureValid,
        diagnostic =
            if (revoked) SystemRoutingDiagnostic.PERMISSION_REVOKED else null,
        ownerEpoch = serviceEpoch,
    )
    settleOwnedRequests("The MASQ VPN service was destroyed before its command completed.")
    runCatching {
      controlExecutor.execute {
        val closeResult =
            if (retainResult is TerminalLeaseRetainResult.Conflict) {
              TunnelCloseResult.StopFailed(
                  SystemRoutingDiagnostic.TRANSLATOR_RETURNED)
            } else {
              // This also prioritizes an adopted process-global lease when
              // this recreated service owns no local descriptor.
              stopAndCloseTunnelSafely()
            }
        val safelyClosed = closeResult == TunnelCloseResult.Closed
        if (safelyClosed) {
          synchronized(runtimeLock) {
            if (tunnelDescriptor === terminalSnapshot.localDescriptor) {
              tunnelDescriptor = null
              appliedPolicy = null
              localTunCaptureValid = false
            }
          }
        }
        val finalLoad =
            runCatching { policyStore.loadForServiceStart() }
                .getOrElse {
                  SystemRoutingPolicyLoadResult.BlockRequired(
                      SystemRoutingDiagnostic.POLICY_READ_FAILED)
                }
        val retainedAfterClose =
            synchronized(runtimeLock) {
              val retainedLease = terminalCoordinator.snapshot()
              if (tunnelDescriptor == null && retainedLease == null) {
                appliedPolicy = null
              }
              Pair(
                  appliedPolicy.takeIf {
                    tunnelDescriptor != null && localTunCaptureValid
                  }
                      ?: retainedLease?.takeIf { it.captureValid }?.policy,
                  (tunnelDescriptor != null && localTunCaptureValid) ||
                      retainedLease?.captureValid == true,
              )
            }
        publishServiceDestroyed(
            load = finalLoad,
            alwaysOn = alwaysOn,
            lockdown = lockdown,
            retainedAppliedPolicy = retainedAfterClose.first,
            tunPresent = retainedAfterClose.second,
            captureValid = retainedAfterClose.second,
            diagnostic =
                if (revoked) {
                  SystemRoutingDiagnostic.PERMISSION_REVOKED
                } else {
                  when (closeResult) {
                    TunnelCloseResult.Closed -> null
                    is TunnelCloseResult.StopFailed -> closeResult.diagnostic
                    TunnelCloseResult.CloseFailed ->
                        SystemRoutingDiagnostic.TUNNEL_CLOSE_FAILED
                  }
                },
            ownerEpoch = serviceEpoch,
        )
        translatorExecutor.shutdown()
        controlExecutor.shutdown()
      }
    }.onFailure {
      publishServiceDestroyed(
          load = load,
          alwaysOn = alwaysOn,
          lockdown = lockdown,
          retainedAppliedPolicy = terminalSnapshot.retainedAppliedPolicy,
          tunPresent = terminalSnapshot.tunPresent,
          captureValid = terminalSnapshot.captureValid,
          diagnostic =
              if (revoked) {
                SystemRoutingDiagnostic.PERMISSION_REVOKED
              } else {
                SystemRoutingDiagnostic.INTERNAL_ERROR
              },
          ownerEpoch = serviceEpoch,
      )
      translatorExecutor.shutdown()
      controlExecutor.shutdown()
    }
    super.onDestroy()
  }

  private fun currentAlwaysOn(): Boolean =
      Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q && isAlwaysOn

  private fun currentLockdown(): Boolean =
      Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q && isLockdownEnabled

  private fun notification(message: String) =
      NotificationCompat.Builder(this, NOTIFICATION_CHANNEL)
          .setSmallIcon(R.mipmap.ic_launcher)
          .setContentTitle("MASQ system-routing dogfood")
          .setContentText(message)
          .setStyle(
              NotificationCompat.BigTextStyle()
                  .bigText("$message $DOGFOOD_ROUTING_LIMITS"))
          .setContentIntent(
              PendingIntent.getActivity(
                  this,
                  0,
                  Intent(this, MainActivity::class.java),
                  PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
              ))
          .setOngoing(true)
          .setOnlyAlertOnce(true)
          .build()

  private fun updateNotification(message: String) {
    if (destroyed) return
    getSystemService(NotificationManager::class.java)
        .notify(NOTIFICATION_ID, notification(message))
  }

  private sealed interface EnsureTunResult {
    data object Ready : EnsureTunResult

    data object ConflictingAppliedPolicy : EnsureTunResult

    data class Failed(val diagnostic: SystemRoutingDiagnostic) : EnsureTunResult
  }

  private sealed interface TunnelCloseResult {
    data object Closed : TunnelCloseResult

    data class StopFailed(val diagnostic: SystemRoutingDiagnostic) : TunnelCloseResult

    data object CloseFailed : TunnelCloseResult
  }

  private data class ServiceDestructionSnapshot(
      val localDescriptor: ParcelFileDescriptor?,
      val retainedAppliedPolicy: DesiredSystemRoutingPolicy?,
      val tunPresent: Boolean,
      val captureValid: Boolean,
  )

  companion object {
    const val ACTION_START = "com.masqmobile.START_SYSTEM_TUNNEL"
    const val ACTION_STOP = "com.masqmobile.STOP_SYSTEM_TUNNEL"
    const val ACTION_RESET = "com.masqmobile.RESET_SYSTEM_TUNNEL"
    const val EXTRA_POLICY_REVISION = "policyRevision"
    const val EXTRA_PROXY_PORT = "proxyPort"
    const val EXTRA_CORE_GENERATION = "coreGeneration"
    const val EXTRA_COMMAND_REQUEST_ID = "commandRequestId"
    private const val NOTIFICATION_CHANNEL = "masq-system-tunnel"
    private const val NOTIFICATION_ID = 4107
    private const val TUNNEL_MTU = 1500
    private const val NO_REQUEST = -1L
    private const val NO_REVISION = -1L
    private const val NO_CORE_GENERATION = -1L
    private const val TRANSLATOR_READY_TIMEOUT_MS = 3_000L
    private const val TRANSLATOR_READY_POLL_MS = 20L
    private const val TRANSLATOR_STOP_TIMEOUT_MS = 10_000L
    private const val CORE_PREFLIGHT_TIMEOUT_MS = 15_000L
    private const val HANDOFF_RETRY_DELAY_MS = 1_000L
    private const val DOGFOOD_ROUTING_LIMITS =
        "Dogfood preview: only captured IPv4 TCP/443 and virtual DNS are translated through MASQ. " +
            "All other captured IP traffic, including other TCP ports, non-DNS UDP, IPv6, ICMP " +
            "and unknown transports, is blocked while capture remains valid. Activation tests a " +
            "real CONNECT to example.com:443 through the MASQ exit without requesting a page. " +
            "Installed MASQ packages are excluded when the route is created. Package IDs and " +
            "consent timestamps stay on-device; Android snapshots scope by UID, so turn routing " +
            "off before package changes and reapply it. Shared-UID apps and attached restricted " +
            "profiles can share scope; work profiles are separate. The temporary loopback proxy " +
            "is unauthenticated and must not be exposed outside trusted-device testing. " +
            "Direct traffic can resume after service or process death. Android Always-on VPN and " +
            "\"Block connections without VPN\" are unsupported."
    private val statusLock = Any()
    private val acknowledgementLock = Any()
    private val startAcknowledgements =
        mutableMapOf<Long, (TunnelAcknowledgement) -> Boolean>()
    private val stopAcknowledgements =
        mutableMapOf<Long, (TunnelAcknowledgement) -> Boolean>()
    private val resetAcknowledgements =
        mutableMapOf<Long, (TunnelAcknowledgement) -> Boolean>()
    private val serviceEpochCounter = AtomicLong(1L)
    private val terminalCoordinator =
        SystemRoutingTerminalCoordinator<ParcelFileDescriptor>(
            closeResource = { descriptor ->
              runCatching { descriptor.close() }.isSuccess
            })
    private var currentStatusOwnerEpoch = 0L
    private var currentStatus =
        SystemRoutingStatus.derive(
            supported = false,
            desiredRevision = null,
            desiredMode = SystemRoutingMode.OFF,
            desiredSelectedApps = emptyList(),
            failClosedDesired = false,
            appliedRevision = null,
            appliedMode = SystemRoutingMode.OFF,
            appliedSelectedApps = emptyList(),
            transition = SystemRoutingTransition.IDLE,
            tunPresent = false,
            translatorReady = false,
            coreRouteReady = false,
            alwaysOn = false,
            lockdown = false,
        )
    private var currentProxyPort: Int? = null

    fun statusJson(): String = synchronized(statusLock) { currentStatus.toJson() }

    private fun nextServiceEpoch(): Long {
      val epoch = serviceEpochCounter.getAndIncrement()
      check(epoch > 0 && epoch < Long.MAX_VALUE) {
        "The MASQ VPN service epoch is exhausted."
      }
      return epoch
    }

    private fun claimStatusEpoch(ownerEpoch: Long) {
      synchronized(statusLock) {
        if (ownerEpoch > currentStatusOwnerEpoch) {
          currentStatusOwnerEpoch = ownerEpoch
        }
      }
    }

    fun publishServiceDestroyed(
        load: SystemRoutingPolicyLoadResult,
        alwaysOn: Boolean,
        lockdown: Boolean,
        retainedAppliedPolicy: DesiredSystemRoutingPolicy? = null,
        tunPresent: Boolean = false,
        captureValid: Boolean = true,
        diagnostic: SystemRoutingDiagnostic? = null,
        ownerEpoch: Long,
    ): String =
        synchronized(statusLock) {
          if (ownerEpoch < currentStatusOwnerEpoch) {
            return@synchronized currentStatus.toJson()
          }
          currentStatusOwnerEpoch = ownerEpoch
          currentStatus =
              systemRoutingStatusAfterServiceDestroyed(
                  load = load,
                  supported =
                      BuildConfig.MASQ_SYSTEM_TUNNEL_ENABLED &&
                          MasqPacketTunnelJni.isAvailable,
                  alwaysOn = alwaysOn,
                  lockdown = lockdown,
                  retainedAppliedPolicy = retainedAppliedPolicy,
                  tunPresent = tunPresent,
                  captureValid = captureValid,
                  diagnostic = diagnostic,
              )
          currentProxyPort = null
          currentStatus.toJson()
        }

    fun publishDesiredPolicy(
        load: SystemRoutingPolicyLoadResult,
        transition: SystemRoutingTransition? = null,
    ): String =
        synchronized(statusLock) {
          val desired =
              when (load) {
                is SystemRoutingPolicyLoadResult.Ready -> load.policy
                is SystemRoutingPolicyLoadResult.ExplicitOff -> load.policy
                else -> null
              }
          val prior = currentStatus
          currentStatus =
              SystemRoutingStatus.derive(
                  supported =
                      BuildConfig.MASQ_SYSTEM_TUNNEL_ENABLED &&
                          MasqPacketTunnelJni.isAvailable,
                  desiredRevision = desired?.revision,
                  desiredMode = desired?.desiredMode ?: SystemRoutingMode.OFF,
                  desiredSelectedApps = desired?.selectedApps ?: emptyList(),
                  failClosedDesired = desired?.failClosedDesired ?: false,
                  appliedRevision = prior.appliedRevision,
                  appliedMode = prior.appliedMode,
                  appliedSelectedApps = prior.appliedSelectedApps,
                  transition =
                      transition
                          ?: if (load is SystemRoutingPolicyLoadResult.BlockRequired) {
                            SystemRoutingTransition.BLOCKED
                          } else {
                            transitionFor(prior.phase)
                          },
                  tunPresent = prior.tunPresent,
                  translatorReady = prior.translatorReady,
                  coreRouteReady = prior.coreRouteReady,
                  alwaysOn = prior.alwaysOn,
                  lockdown = prior.lockdown,
                  lastError =
                      (load as? SystemRoutingPolicyLoadResult.BlockRequired)?.reason,
              )
          if (!currentStatus.active) currentProxyPort = null
          currentStatus.toJson()
        }

    fun publishCoreRouteHealth(
        load: SystemRoutingPolicyLoadResult,
        coreConnected: Boolean,
        proxyPort: Int,
    ): String {
      publishDesiredPolicy(load)
      return synchronized(statusLock) {
        val prior = currentStatus
        val exactCoreRoute =
            coreConnected &&
                proxyPort in 1..65535 &&
                currentProxyPort == proxyPort
        val desired =
            when (load) {
              is SystemRoutingPolicyLoadResult.Ready -> load.policy
              is SystemRoutingPolicyLoadResult.ExplicitOff -> load.policy
              else -> null
            }
        currentStatus =
            SystemRoutingStatus.derive(
                supported =
                    BuildConfig.MASQ_SYSTEM_TUNNEL_ENABLED &&
                        MasqPacketTunnelJni.isAvailable,
                desiredRevision = desired?.revision,
                desiredMode = desired?.desiredMode ?: SystemRoutingMode.OFF,
                desiredSelectedApps = desired?.selectedApps ?: emptyList(),
                failClosedDesired = desired?.failClosedDesired ?: false,
                appliedRevision = prior.appliedRevision,
                appliedMode = prior.appliedMode,
                appliedSelectedApps = prior.appliedSelectedApps,
                transition = transitionFor(prior.phase),
                tunPresent = prior.tunPresent,
                translatorReady = prior.translatorReady,
                coreRouteReady = exactCoreRoute && prior.coreRouteReady,
                alwaysOn = prior.alwaysOn,
                lockdown = prior.lockdown,
                lastError =
                    when {
                      load is SystemRoutingPolicyLoadResult.BlockRequired -> load.reason
                      prior.active && !exactCoreRoute ->
                          SystemRoutingDiagnostic.CORE_ROUTE_NOT_READY
                      else -> prior.lastError
                    },
            )
        if (!currentStatus.active) currentProxyPort = null
        currentStatus.toJson()
      }
    }

    fun registerStartAcknowledgement(
        requestId: Long,
        callback: (TunnelAcknowledgement) -> Boolean,
    ) = registerAcknowledgement(startAcknowledgements, requestId, callback)

    fun cancelStartAcknowledgement(requestId: Long) =
        cancelAcknowledgement(startAcknowledgements, requestId)

    private fun hasStartAcknowledgement(requestId: Long): Boolean =
        synchronized(acknowledgementLock) {
          startAcknowledgements.containsKey(requestId)
        }

    fun registerStopAcknowledgement(
        requestId: Long,
        callback: (TunnelAcknowledgement) -> Boolean,
    ) = registerAcknowledgement(stopAcknowledgements, requestId, callback)

    fun cancelStopAcknowledgement(requestId: Long) =
        cancelAcknowledgement(stopAcknowledgements, requestId)

    fun registerResetAcknowledgement(
        requestId: Long,
        callback: (TunnelAcknowledgement) -> Boolean,
    ) = registerAcknowledgement(resetAcknowledgements, requestId, callback)

    fun cancelResetAcknowledgement(requestId: Long) =
        cancelAcknowledgement(resetAcknowledgements, requestId)

    private fun updateStatus(
        status: SystemRoutingStatus,
        activeProxyPort: Int?,
        ownerEpoch: Long,
    ) {
      synchronized(statusLock) {
        if (ownerEpoch < currentStatusOwnerEpoch) return
        currentStatusOwnerEpoch = ownerEpoch
        currentStatus = status
        currentProxyPort =
            if (status.active &&
                activeProxyPort != null &&
                activeProxyPort in 1..65535) {
              activeProxyPort
            } else {
              null
            }
      }
    }

    private fun acknowledgeStart(requestId: Long, status: String?, error: String?) =
        acknowledge(startAcknowledgements, requestId, status, error)

    private fun acknowledgeStop(requestId: Long, status: String?, error: String?) =
        acknowledge(stopAcknowledgements, requestId, status, error)

    private fun acknowledgeReset(requestId: Long, status: String?, error: String?) =
        acknowledge(resetAcknowledgements, requestId, status, error)

    private fun registerAcknowledgement(
        acknowledgements: MutableMap<Long, (TunnelAcknowledgement) -> Boolean>,
        requestId: Long,
        callback: (TunnelAcknowledgement) -> Boolean,
    ) {
      synchronized(acknowledgementLock) {
        check(!acknowledgements.containsKey(requestId)) {
          "A MASQ tunnel acknowledgement is already registered."
        }
        acknowledgements[requestId] = callback
      }
    }

    private fun cancelAcknowledgement(
        acknowledgements: MutableMap<Long, (TunnelAcknowledgement) -> Boolean>,
        requestId: Long,
    ) {
      synchronized(acknowledgementLock) {
        acknowledgements.remove(requestId)
      }
    }

    private fun acknowledge(
        acknowledgements: MutableMap<Long, (TunnelAcknowledgement) -> Boolean>,
        requestId: Long,
        status: String?,
        error: String?,
    ): Boolean {
      val callback =
          synchronized(acknowledgementLock) {
            acknowledgements.remove(requestId)
          }
      return runCatching {
            callback?.invoke(TunnelAcknowledgement(status, error)) == true
          }
          .getOrDefault(false)
    }

    private fun transitionFor(phase: SystemRoutingPhase): SystemRoutingTransition =
        when (phase) {
          SystemRoutingPhase.REQUESTING_PERMISSION ->
              SystemRoutingTransition.REQUESTING_PERMISSION
          SystemRoutingPhase.STARTING_BLOCKING ->
              SystemRoutingTransition.STARTING_BLOCKING
          SystemRoutingPhase.RECONNECTING -> SystemRoutingTransition.RECONNECTING
          SystemRoutingPhase.BLOCKED -> SystemRoutingTransition.BLOCKED
          SystemRoutingPhase.STOPPING -> SystemRoutingTransition.STOPPING
          SystemRoutingPhase.REVOKED -> SystemRoutingTransition.REVOKED
          SystemRoutingPhase.OFF, SystemRoutingPhase.ACTIVE -> SystemRoutingTransition.IDLE
        }

    data class TunnelAcknowledgement(
        val status: String?,
        val error: String?,
    )
  }
}
