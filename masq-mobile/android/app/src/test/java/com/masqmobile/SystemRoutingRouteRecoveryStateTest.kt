package com.masqmobile

import java.util.concurrent.CountDownLatch
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicInteger
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class SystemRoutingRouteRecoveryStateTest {
  @Test
  fun onlyAnExplicitStartMayOmitTheAndroidNetworkEpoch() {
    assertTrue(
        systemRoutingNetworkEpochMatches(
            expectedNetworkEpoch = null,
            explicitStartRequest = true,
            currentNetworkEpoch = null,
        ))
    assertFalse(
        systemRoutingNetworkEpochMatches(
            expectedNetworkEpoch = null,
            explicitStartRequest = false,
            currentNetworkEpoch = 4L,
        ))
    assertFalse(
        systemRoutingNetworkEpochMatches(
            expectedNetworkEpoch = -1L,
            explicitStartRequest = false,
            currentNetworkEpoch = 4L,
        ))
    assertTrue(
        systemRoutingNetworkEpochMatches(
            expectedNetworkEpoch = 4L,
            explicitStartRequest = false,
            currentNetworkEpoch = 4L,
        ))
    assertFalse(
        systemRoutingNetworkEpochMatches(
            expectedNetworkEpoch = 4L,
            explicitStartRequest = false,
            currentNetworkEpoch = 5L,
        ))
  }

  private val executors = mutableListOf<java.util.concurrent.ExecutorService>()

  @After
  fun tearDown() {
    executors.forEach { it.shutdownNow() }
  }

  @Test
  fun activeRouteLossStopsTranslatorButRetainsTunAsFailClosedBlocker() {
    val native = RouteLossNative()
    val executor = Executors.newSingleThreadExecutor().also(executors::add)
    val translator = SystemRoutingTranslator(native, executor)
    val tun = FakeTunDescriptor(fd = 31)
    val policy = wholeDevicePolicy(revision = 81L)
    assertEquals(
        TranslatorStartResult.Started,
        translator.start(policy.revision, tun.fd, 8080, 1500) { _, _, _, _ -> },
    )
    assertTrue(native.started.await(1, TimeUnit.SECONDS))
    val active = status(policy, tun, translatorReady = true, coreRouteReady = true)
    assertTrue(active.active)

    val recoveryState = SystemRoutingRouteRecoveryState()
    val loss =
        checkNotNull(
            recoveryState.observeRouteLoss(
                captureOrRecoveryNeedsBlocking = active.active,
            ))
    val stopped = translator.stopAndAwait(policy.revision, 1_000L)
    recoveryState.completeRouteLoss(loss)
    val blocked = status(policy, tun, translatorReady = false, coreRouteReady = false)

    assertEquals(TranslatorStopResult.SafeToClose, stopped)
    assertEquals(PacketTunnelNativeState.IDLE, native.snapshot().state)
    assertFalse(translator.hasOwnedRun())
    assertFalse(tun.closed)
    assertTrue(blocked.tunPresent)
    assertEquals(SystemRoutingPhase.BLOCKED, blocked.phase)
    assertEquals(SystemRoutingTrafficDisposition.BLOCKED, blocked.trafficDisposition)
  }

  @Test
  fun oneFreshMatchingRouteSchedulesExactlyOneRestart() {
    val state = SystemRoutingRouteRecoveryState()
    val loss =
        checkNotNull(
            state.observeRouteLoss(captureOrRecoveryNeedsBlocking = true),
        )
    state.completeRouteLoss(loss)
    val exactIdentity =
        SystemRoutingCoreRouteIdentity(
            proxyPort = 9080,
            coreGeneration = 44L,
            engineGeneration = 12L,
            networkEpoch = 3L,
        )

    val first =
        checkNotNull(
            state.observeProvenRoute(exactIdentity, routeAlreadyExact = false),
        )
    assertNull(state.observeProvenRoute(exactIdentity, routeAlreadyExact = false))
    assertTrue(state.recoveryIsCurrent(first))

    val starts = AtomicInteger(0)
    if (state.recoveryIsCurrent(first)) {
      assertEquals(exactIdentity.proxyPort, first.identity.proxyPort)
      assertEquals(exactIdentity.coreGeneration, first.identity.coreGeneration)
      assertEquals(exactIdentity.engineGeneration, first.identity.engineGeneration)
      assertEquals(exactIdentity.networkEpoch, first.identity.networkEpoch)
      starts.incrementAndGet()
    }
    state.completeRecovery(first)

    assertNull(state.observeProvenRoute(exactIdentity, routeAlreadyExact = true))
    assertEquals(1, starts.get())
  }

  @Test
  fun laterLossInvalidatesAQueuedRecoveryBeforeItCanRestart() {
    val state = SystemRoutingRouteRecoveryState()
    val staleIdentity = SystemRoutingCoreRouteIdentity(8080, 50L, 20L, 4L)
    val stale =
        checkNotNull(
            state.observeProvenRoute(staleIdentity, routeAlreadyExact = false),
        )
    val loss =
        checkNotNull(
            state.observeRouteLoss(captureOrRecoveryNeedsBlocking = true),
        )

    assertFalse(state.recoveryIsCurrent(stale))
    state.completeRecovery(stale)
    state.completeRouteLoss(loss)

    val freshIdentity = SystemRoutingCoreRouteIdentity(9090, 51L, 21L, 5L)
    val fresh =
        checkNotNull(
            state.observeProvenRoute(freshIdentity, routeAlreadyExact = false),
        )
    assertTrue(state.recoveryIsCurrent(fresh))
    assertEquals(51L, fresh.identity.coreGeneration)
  }

  @Test
  fun failedPreflightTupleIsSuppressedUntilIdentityOrLossEpochChanges() {
    val state = SystemRoutingRouteRecoveryState()
    val failedIdentity = SystemRoutingCoreRouteIdentity(8080, 50L, 20L, 4L)
    val failed =
        checkNotNull(
            state.observeProvenRoute(
                identity = failedIdentity,
                routeAlreadyExact = false,
            ))

    assertTrue(state.recordFailedRecoveryPreflight(failed))
    state.completeRecovery(failed)
    assertNull(state.observeProvenRoute(failedIdentity, routeAlreadyExact = false))

    val newerIdentity = SystemRoutingCoreRouteIdentity(9090, 51L, 21L, 5L)
    assertTrue(
        state.observeProvenRoute(newerIdentity, routeAlreadyExact = false) != null,
    )

    val loss =
        checkNotNull(
            state.observeRouteLoss(captureOrRecoveryNeedsBlocking = true),
        )
    state.completeRouteLoss(loss)
    assertTrue(
        state.observeProvenRoute(failedIdentity, routeAlreadyExact = false) != null,
    )
  }

  @Test
  fun aNewAndroidNetworkEpochInvalidatesFailedPreflightSuppression() {
    val state = SystemRoutingRouteRecoveryState()
    val oldNetwork = SystemRoutingCoreRouteIdentity(8080, 50L, 20L, 4L)
    val failed =
        checkNotNull(
            state.observeProvenRoute(
                identity = oldNetwork,
                routeAlreadyExact = false,
            ))
    assertTrue(state.recordFailedRecoveryPreflight(failed))
    state.completeRecovery(failed)
    assertNull(state.observeProvenRoute(oldNetwork, routeAlreadyExact = false))

    val restoredNetwork = oldNetwork.copy(networkEpoch = 5L)
    val fresh =
        checkNotNull(
            state.observeProvenRoute(
                identity = restoredNetwork,
                routeAlreadyExact = false,
            ))
    assertTrue(state.recoveryIsCurrent(fresh))
    assertEquals(5L, fresh.identity.networkEpoch)
  }

  private fun status(
      policy: DesiredSystemRoutingPolicy,
      tun: FakeTunDescriptor,
      translatorReady: Boolean,
      coreRouteReady: Boolean,
  ) =
      SystemRoutingStatus.derive(
          supported = true,
          desiredRevision = policy.revision,
          desiredMode = policy.desiredMode,
          desiredSelectedApps = policy.selectedApps,
          failClosedDesired = policy.failClosedDesired,
          appliedRevision = policy.revision,
          appliedMode = policy.desiredMode,
          appliedSelectedApps = policy.selectedApps,
          transition = SystemRoutingTransition.IDLE,
          tunPresent = !tun.closed,
          translatorReady = translatorReady,
          coreRouteReady = coreRouteReady,
          alwaysOn = false,
          lockdown = false,
      )

  private fun wholeDevicePolicy(revision: Long) =
      DesiredSystemRoutingPolicy(
          schemaVersion = SystemRoutingPolicyStore.CURRENT_SCHEMA_VERSION,
          revision = revision,
          desiredMode = SystemRoutingMode.WHOLE_DEVICE,
          selectedApps = emptyList(),
          explicitConsentTimestampMs = 1_000L,
          failClosedDesired = true,
      )

  private data class FakeTunDescriptor(
      val fd: Int,
      var closed: Boolean = false,
  )

  private class RouteLossNative : PacketTunnelNativeApi {
    val started = CountDownLatch(1)
    private val stop = CountDownLatch(1)
    @Volatile private var state = PacketTunnelNativeState.IDLE
    @Volatile private var generation = 0L

    override fun start(tunFd: Int, proxyPort: Int, mtu: Int): Int {
      generation += 1L
      state = PacketTunnelNativeState.RUNNING
      started.countDown()
      stop.await()
      state = PacketTunnelNativeState.IDLE
      return MasqPacketTunnelJni.START_STOPPED
    }

    override fun requestStop(): Boolean {
      if (state != PacketTunnelNativeState.RUNNING) return false
      state = PacketTunnelNativeState.STOPPING
      stop.countDown()
      return true
    }

    override fun snapshot() =
        PacketTunnelSnapshot(
            state = state,
            generation = generation.takeIf { it > 0L },
            lastResult = null,
        )
  }
}
