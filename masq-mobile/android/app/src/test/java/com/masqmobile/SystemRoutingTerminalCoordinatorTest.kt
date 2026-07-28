package com.masqmobile

import java.util.concurrent.CountDownLatch
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicInteger
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Test

class SystemRoutingTerminalCoordinatorTest {
  private val executors = mutableListOf<java.util.concurrent.ExecutorService>()

  @After
  fun tearDown() {
    executors.forEach { it.shutdownNow() }
  }

  @Test
  fun recreatedServiceResetJoinsOldInstanceCleanupBeforeClosingExactDescriptor() {
    val native = FakeNative(releaseOnStop = false)
    val translator = translator(native)
    val descriptor = FakeDescriptor()
    val policy = wholeDevicePolicy(41)
    val coordinator = coordinator()
    assertEquals(
        TranslatorStartResult.Started,
        translator.start(policy.revision, 7, 8080, 1500) { _, _, _, _ -> },
    )
    assertTrue(native.started.await(1, TimeUnit.SECONDS))
    val retained = coordinator.retain(descriptor, policy, translator)
    assertTrue(retained is TerminalLeaseRetainResult.Retained)

    val callers = Executors.newFixedThreadPool(2)
    executors += callers
    val oldInstanceCleanup = callers.submit<TerminalLeaseCloseResult> {
      coordinator.closeOrJoin(2_000)
    }
    assertTrue(native.stopRequested.await(1, TimeUnit.SECONDS))
    val recreatedServiceReset = callers.submit<TerminalLeaseCloseResult> {
      coordinator.closeOrJoin(2_000)
    }
    assertEquals(0, descriptor.closeCount.get())
    assertTrue(coordinator.snapshot()?.cleanupInProgress == true)

    native.allowReturn()
    val oldResult = oldInstanceCleanup.get(2, TimeUnit.SECONDS)
    val recreatedResult = recreatedServiceReset.get(2, TimeUnit.SECONDS)

    assertTrue(oldResult is TerminalLeaseCloseResult.Closed)
    assertEquals(oldResult, recreatedResult)
    assertEquals(1, descriptor.closeCount.get())
    assertNull(coordinator.snapshot())
  }

  @Test
  fun timeoutRetainsDescriptorPolicyAndNativeOwnerUntilAJoinedRetryFinishes() {
    val native = FakeNative(releaseOnStop = false)
    val translator = translator(native)
    val descriptor = FakeDescriptor()
    val policy = wholeDevicePolicy(42)
    val coordinator = coordinator()
    translator.start(policy.revision, 8, 8080, 1500) { _, _, _, _ -> }
    assertTrue(native.started.await(1, TimeUnit.SECONDS))
    coordinator.retain(descriptor, policy, translator)

    val timedOut = coordinator.closeOrJoin(5)

    assertTrue(timedOut is TerminalLeaseCloseResult.StopFailed)
    assertEquals(
        TranslatorStopResult.TimedOutKeepBlocking,
        (timedOut as TerminalLeaseCloseResult.StopFailed).result,
    )
    assertEquals(0, descriptor.closeCount.get())
    assertSame(policy, coordinator.snapshot()?.policy)
    assertNull(coordinator.beginStart())

    native.allowReturn()
    assertTrue(native.returned.await(1, TimeUnit.SECONDS))
    assertTrue(coordinator.closeOrJoin(1_000) is TerminalLeaseCloseResult.Closed)
    assertEquals(1, descriptor.closeCount.get())
    assertNotNull(coordinator.beginStart())
  }

  @Test
  fun revocationInvalidatesCaptureWithoutReleasingTheDescriptorBeforeNativeCleanup() {
    val native = FakeNative(releaseOnStop = false)
    val translator = translator(native)
    val descriptor = FakeDescriptor()
    val policy = wholeDevicePolicy(47)
    val coordinator = coordinator()
    translator.start(policy.revision, 12, 8080, 1500) { _, _, _, _ -> }
    assertTrue(native.started.await(1, TimeUnit.SECONDS))
    coordinator.retain(descriptor, policy, translator, captureValid = true)

    val cleanupExecutor = Executors.newSingleThreadExecutor()
    executors += cleanupExecutor
    val cleanup = cleanupExecutor.submit<TerminalLeaseCloseResult> {
      coordinator.closeOrJoin(2_000)
    }
    assertTrue(native.stopRequested.await(1, TimeUnit.SECONDS))

    assertTrue(coordinator.invalidateCapture(descriptor, translator))

    assertFalse(checkNotNull(coordinator.snapshot()).captureValid)
    assertEquals(0, descriptor.closeCount.get())
    native.allowReturn()
    assertTrue(cleanup.get(2, TimeUnit.SECONDS) is TerminalLeaseCloseResult.Closed)
    assertEquals(1, descriptor.closeCount.get())
    assertNull(coordinator.snapshot())
  }

  @Test
  fun staleServiceCannotInvalidateCaptureOwnedByANewerExactLease() {
    val native = FakeNative(releaseOnStop = true)
    val newerTranslator = translator(native)
    val staleTranslator =
        translator(
            object : PacketTunnelNativeApi {
              override fun start(tunFd: Int, proxyPort: Int, mtu: Int) =
                  MasqPacketTunnelJni.START_FAILED

              override fun requestStop() = false

              override fun snapshot() =
                  PacketTunnelSnapshot(PacketTunnelNativeState.IDLE, null, null)
            })
    val newerDescriptor = FakeDescriptor()
    val staleDescriptor = FakeDescriptor()
    val coordinator = coordinator()
    coordinator.retain(
        newerDescriptor,
        wholeDevicePolicy(48),
        newerTranslator,
        captureValid = true,
    )

    assertFalse(
        coordinator.invalidateCapture(
            staleDescriptor,
            staleTranslator,
            adoptedEpoch = checkNotNull(coordinator.snapshot()).epoch + 1,
        ),
    )
    assertTrue(checkNotNull(coordinator.snapshot()).captureValid)
    assertTrue(
        coordinator.invalidateCapture(newerDescriptor, newerTranslator),
    )
    assertFalse(checkNotNull(coordinator.snapshot()).captureValid)
  }

  @Test
  fun recreatedServiceCanInvalidateOnlyTheExactLeaseEpochItAdopted() {
    val native = FakeNative(releaseOnStop = true)
    val retainedTranslator = translator(native)
    val recreatedTranslator =
        translator(
            object : PacketTunnelNativeApi {
              override fun start(tunFd: Int, proxyPort: Int, mtu: Int) =
                  MasqPacketTunnelJni.START_FAILED

              override fun requestStop() = false

              override fun snapshot() =
                  PacketTunnelSnapshot(PacketTunnelNativeState.IDLE, null, null)
            })
    val retainedDescriptor = FakeDescriptor()
    val coordinator = coordinator()
    coordinator.retain(
        retainedDescriptor,
        wholeDevicePolicy(49),
        retainedTranslator,
        captureValid = true,
    )
    val adoptedEpoch = checkNotNull(coordinator.snapshot()).epoch

    assertTrue(
        coordinator.invalidateCapture(
            resource = null,
            translator = recreatedTranslator,
            adoptedEpoch = adoptedEpoch,
        ),
    )
    assertFalse(checkNotNull(coordinator.snapshot()).captureValid)
  }

  @Test
  fun revocationDuringTransferredCleanupInvalidatesTheExactAdoptedLease() {
    val native = FakeNative(releaseOnStop = false)
    val translator = translator(native)
    val descriptor = FakeDescriptor()
    val coordinator = coordinator()
    assertEquals(
        TranslatorStartResult.Started,
        translator.start(51, 13, 8080, 1500) { _, _, _, _ -> },
    )
    assertTrue(native.started.await(1, TimeUnit.SECONDS))
    val retained =
        coordinator.retain(
            descriptor,
            wholeDevicePolicy(51),
            translator,
            captureValid = true,
        )
    val cleanupExecutor = Executors.newSingleThreadExecutor()
    executors += cleanupExecutor
    val cleanup = cleanupExecutor.submit<TerminalLeaseCloseResult> {
      coordinator.closeOrJoin(2_000)
    }
    assertTrue(native.stopRequested.await(1, TimeUnit.SECONDS))

    // The service has already relinquished its local descriptor, so revoke
    // can identify cleanup ownership only by the exact adopted lease epoch.
    assertTrue(
        coordinator.invalidateCapture(
            resource = null,
            translator = translator,
            adoptedEpoch = retained.epoch,
        ))
    assertFalse(checkNotNull(coordinator.snapshot()).captureValid)

    native.allowReturn()
    assertTrue(cleanup.get(2, TimeUnit.SECONDS) is TerminalLeaseCloseResult.Closed)
    assertEquals(1, descriptor.closeCount.get())
  }

  @Test
  fun falseDescriptorCloseInvalidatesCaptureButRetainsBookkeeping() {
    assertIndeterminateCloseInvalidatesCapture { false }
  }

  @Test
  fun throwingDescriptorCloseInvalidatesCaptureButRetainsBookkeeping() {
    assertIndeterminateCloseInvalidatesCapture {
      throw IllegalStateException("close outcome is indeterminate")
    }
  }

  @Test
  fun notificationRefusalCloseFailureKeepsExactDescriptorOwnedForRetry() {
    val closeAttempts = AtomicInteger()
    val native =
        object : PacketTunnelNativeApi {
          override fun start(tunFd: Int, proxyPort: Int, mtu: Int) =
              MasqPacketTunnelJni.START_FAILED

          override fun requestStop() = false

          override fun snapshot() =
              PacketTunnelSnapshot(PacketTunnelNativeState.IDLE, null, null)
        }
    val translator = translator(native)
    val descriptor = FakeDescriptor()
    val coordinator =
        SystemRoutingTerminalCoordinator<FakeDescriptor>(
            closeResource = {
              closeAttempts.incrementAndGet()
              false
            })
    coordinator.retain(
        descriptor,
        wholeDevicePolicy(52),
        translator,
        captureValid = true,
    )

    assertTrue(
        coordinator.closeOrJoin(1_000) is
            TerminalLeaseCloseResult.DescriptorCloseFailed)
    assertEquals(1, closeAttempts.get())
    assertFalse(checkNotNull(coordinator.snapshot()).captureValid)
    assertTrue(coordinator.blocksNewStart())
  }

  @Test
  fun unexpectedNativeReturnStaysCapturedUntilExplicitCleanupClosesItOnce() {
    val native =
        FakeNative(
            releaseOnStop = true,
            result = MasqPacketTunnelJni.START_UNEXPECTED_CLEAN_RETURN,
        )
    val translator = translator(native)
    val descriptor = FakeDescriptor()
    val coordinator = coordinator()
    translator.start(43, 9, 8080, 1500) { _, _, _, _ -> }
    assertTrue(native.started.await(1, TimeUnit.SECONDS))
    coordinator.retain(descriptor, wholeDevicePolicy(43), translator)
    native.allowReturn()
    assertTrue(native.returned.await(1, TimeUnit.SECONDS))

    // The translator callback alone must not close the retained blocker.
    assertEquals(0, descriptor.closeCount.get())
    assertNotNull(coordinator.snapshot())
    val result = coordinator.closeOrJoin(1_000)

    assertTrue(result is TerminalLeaseCloseResult.Closed)
    assertEquals(1, descriptor.closeCount.get())
    assertNull(coordinator.snapshot())
    assertFalse(coordinator.blocksNewStart())
    assertEquals(TerminalLeaseCloseResult.NoLease, coordinator.closeOrJoin(1_000))
    assertEquals(1, descriptor.closeCount.get())
  }

  @Test
  fun staleNativeGenerationCannotCloseOrReleaseTheTerminalLease() {
    val native = FakeNative(releaseOnStop = true, generationSkewOnReturn = 1)
    val translator = translator(native)
    val descriptor = FakeDescriptor()
    val coordinator = coordinator()
    translator.start(44, 10, 8080, 1500) { _, _, _, _ -> }
    assertTrue(native.started.await(1, TimeUnit.SECONDS))
    val ownership = translator.ownership()
    coordinator.retain(descriptor, wholeDevicePolicy(44), translator)

    val result = coordinator.closeOrJoin(1_000)

    assertEquals(
        TerminalLeaseCloseResult.StopFailed(
            checkNotNull(coordinator.snapshot()).epoch,
            TranslatorStopResult.NativeStateNotIdleKeepBlocking,
        ),
        result,
    )
    assertEquals(ownership, coordinator.snapshot()?.ownership)
    assertEquals(0, descriptor.closeCount.get())
  }

  @Test
  fun exactResourceIdentityAndEpochPreventAConflictingDescriptorFromBeingClosed() {
    val native = FakeNative(releaseOnStop = true)
    val translator = translator(native)
    val first = FakeDescriptor()
    val conflicting = FakeDescriptor()
    val coordinator = coordinator()
    translator.start(45, 11, 8080, 1500) { _, _, _, _ -> }
    assertTrue(native.started.await(1, TimeUnit.SECONDS))
    val retained = coordinator.retain(first, wholeDevicePolicy(45), translator)
    val conflict = coordinator.retain(conflicting, wholeDevicePolicy(46), translator)

    assertEquals(retained.epoch, conflict.epoch)
    assertTrue(conflict is TerminalLeaseRetainResult.Conflict)
    assertTrue(coordinator.closeOrJoin(1_000) is TerminalLeaseCloseResult.Closed)
    assertEquals(1, first.closeCount.get())
    assertEquals(0, conflicting.closeCount.get())
  }

  @Test
  fun resetAndStartPermitsAreMutuallyExclusiveAcrossServiceInstances() {
    val coordinator = coordinator()
    val startEpoch = checkNotNull(coordinator.beginStart())
    assertNull(coordinator.beginExplicitReset())
    coordinator.finishStart(startEpoch)

    val resetEpoch = checkNotNull(coordinator.beginExplicitReset())
    assertNull(coordinator.beginStart())
    assertTrue(coordinator.blocksNewStart())
    coordinator.finishExplicitReset(resetEpoch)

    val nextStart = coordinator.beginStart()
    assertNotNull(nextStart)
    assertTrue(coordinator.blocksNewStart())
    coordinator.finishStart(checkNotNull(nextStart))
    assertFalse(coordinator.blocksNewStart())
  }

  private fun coordinator() =
      SystemRoutingTerminalCoordinator<FakeDescriptor>(
          closeResource = { descriptor -> descriptor.close() },
      )

  private fun assertIndeterminateCloseInvalidatesCapture(
      close: (FakeDescriptor) -> Boolean,
  ) {
    val native =
        object : PacketTunnelNativeApi {
          override fun start(tunFd: Int, proxyPort: Int, mtu: Int) =
              MasqPacketTunnelJni.START_FAILED

          override fun requestStop() = false

          override fun snapshot() =
              PacketTunnelSnapshot(PacketTunnelNativeState.IDLE, null, null)
        }
    val translator = translator(native)
    val descriptor = FakeDescriptor()
    val coordinator =
        SystemRoutingTerminalCoordinator<FakeDescriptor>(
            closeResource = close,
        )
    coordinator.retain(
        descriptor,
        wholeDevicePolicy(50),
        translator,
        captureValid = true,
    )

    val result = coordinator.closeOrJoin(1_000)

    assertTrue(result is TerminalLeaseCloseResult.DescriptorCloseFailed)
    assertFalse(checkNotNull(coordinator.snapshot()).captureValid)
    assertTrue(coordinator.blocksNewStart())
  }

  private fun translator(native: PacketTunnelNativeApi): SystemRoutingTranslator {
    val executor = Executors.newSingleThreadExecutor()
    executors += executor
    return SystemRoutingTranslator(native, executor)
  }

  private fun wholeDevicePolicy(revision: Long) =
      DesiredSystemRoutingPolicy(
          schemaVersion = SystemRoutingPolicyStore.CURRENT_SCHEMA_VERSION,
          revision = revision,
          desiredMode = SystemRoutingMode.WHOLE_DEVICE,
          selectedApps = emptyList(),
          explicitConsentTimestampMs = 1,
          failClosedDesired = false,
      )

  private class FakeDescriptor {
    val closeCount = AtomicInteger()

    fun close(): Boolean {
      closeCount.incrementAndGet()
      return true
    }
  }

  private class FakeNative(
      private val releaseOnStop: Boolean,
      private val result: Int = MasqPacketTunnelJni.START_STOPPED,
      private val generationSkewOnReturn: Long = 0,
  ) : PacketTunnelNativeApi {
    val started = CountDownLatch(1)
    val stopRequested = CountDownLatch(1)
    val returned = CountDownLatch(1)
    private val release = CountDownLatch(1)
    @Volatile private var state = PacketTunnelNativeState.IDLE
    @Volatile private var generation = 0L

    override fun start(tunFd: Int, proxyPort: Int, mtu: Int): Int {
      generation += 1
      state = PacketTunnelNativeState.RUNNING
      started.countDown()
      release.await()
      generation += generationSkewOnReturn
      state =
          if (result == MasqPacketTunnelJni.START_STOPPED) {
            PacketTunnelNativeState.IDLE
          } else {
            PacketTunnelNativeState.FAILED
          }
      returned.countDown()
      return result
    }

    override fun requestStop(): Boolean {
      if (state != PacketTunnelNativeState.RUNNING) return false
      state = PacketTunnelNativeState.STOPPING
      stopRequested.countDown()
      if (releaseOnStop) release.countDown()
      return true
    }

    override fun snapshot() =
        PacketTunnelSnapshot(
            state = state,
            generation = generation.takeIf { it > 0 },
            lastResult = null,
        )

    fun allowReturn() {
      release.countDown()
    }
  }
}
