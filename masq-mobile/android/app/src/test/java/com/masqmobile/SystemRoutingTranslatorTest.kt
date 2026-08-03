package com.masqmobile

import java.util.concurrent.CountDownLatch
import java.util.concurrent.Executors
import java.util.concurrent.Semaphore
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicInteger
import java.util.concurrent.atomic.AtomicLong
import java.util.concurrent.atomic.AtomicReference
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class SystemRoutingTranslatorTest {
  private val executors = mutableListOf<java.util.concurrent.ExecutorService>()

  @After
  fun tearDown() {
    executors.forEach { it.shutdownNow() }
  }

  @Test
  fun duplicateSameRevisionStartIsIdempotentAndDoesNotRunNativeTwice() {
    val native = FakeNative()
    val translator = translator(native)

    assertEquals(
        TranslatorStartResult.Started,
        translator.start(7, 3, 8080, 1500) { _, _, _, _ -> },
    )
    assertTrue(native.started.await(1, TimeUnit.SECONDS))
    assertEquals(
        TranslatorStartResult.Idempotent,
        translator.start(7, 3, 8080, 1500) { _, _, _, _ -> },
    )
    assertEquals(1, native.startCount)

    native.finishStop()
  }

  @Test
  fun stopWaitsForNativeRunReturnAndIdleBeforeDescriptorMayClose() {
    val native = FakeNative()
    val translator = translator(native)
    translator.start(8, 3, 8080, 1500) { _, _, _, _ -> }
    assertTrue(native.started.await(1, TimeUnit.SECONDS))

    val stopped = translator.stopAndAwait(8, 1_000)

    assertEquals(TranslatorStopResult.SafeToClose, stopped)
    assertTrue(native.returned.await(1, TimeUnit.SECONDS))
    assertEquals(PacketTunnelNativeState.IDLE, native.snapshot().state)
  }

  @Test
  fun nonblockingStopRequestPausesTrafficBeforeSerializedCleanupReleasesOwnership() {
    val native = FakeNative()
    val translator = translator(native)
    translator.start(81, 3, 8080, 1500) { _, _, _, _ -> }
    assertTrue(native.started.await(1, TimeUnit.SECONDS))
    val ownership = checkNotNull(translator.ownership())

    assertTrue(translator.requestStopWithoutRelease())
    assertTrue(native.returned.await(1, TimeUnit.SECONDS))
    assertTrue(translator.owns(ownership))

    assertEquals(
        TranslatorStopResult.SafeToClose,
        translator.stopAndAwait(81, 1_000),
    )
    assertFalse(translator.owns(ownership))
    assertFalse(translator.requestStopWithoutRelease())
  }

  @Test
  fun stopTimeoutKeepsFutureAndDescriptorOwned() {
    val native = FakeNative(ignoreStop = true)
    val translator = translator(native)
    translator.start(9, 3, 8080, 1500) { _, _, _, _ -> }
    assertTrue(native.started.await(1, TimeUnit.SECONDS))

    assertEquals(
        TranslatorStopResult.TimedOutKeepBlocking,
        translator.stopAndAwait(9, 5),
    )
    assertFalse(native.returned.await(20, TimeUnit.MILLISECONDS))
  }

  @Test
  fun unexpectedCleanReturnRemainsOwnedUntilExplicitCleanupThenBecomesSafeToClose() {
    val native = FakeNative(unexpectedResult = MasqPacketTunnelJni.START_UNEXPECTED_CLEAN_RETURN)
    val translator = translator(native)
    translator.start(10, 3, 8080, 1500) { _, _, _, _ -> }
    val ownership = checkNotNull(translator.ownership())
    assertTrue(native.started.await(1, TimeUnit.SECONDS))
    native.finishUnexpected()
    assertTrue(native.returned.await(1, TimeUnit.SECONDS))

    // The automatic return callback does not release the TUN/native owner.
    assertTrue(translator.owns(ownership))
    assertEquals(
        TranslatorStopResult.SafeToClose,
        translator.stopAndAwait(10, 100),
    )
    assertFalse(translator.owns(ownership))
    assertTrue(
        translator.confirmsProcessReleased(ownership),
    )
  }

  @Test
  fun unexpectedReturnWithDifferentNativeGenerationIsNeverReleased() {
    val native =
        FakeNative(
            unexpectedResult = MasqPacketTunnelJni.START_UNEXPECTED_CLEAN_RETURN,
            generationSkewOnReturn = 1,
        )
    val translator = translator(native)
    translator.start(18, 3, 8080, 1500) { _, _, _, _ -> }
    val ownership = checkNotNull(translator.ownership())
    assertTrue(native.started.await(1, TimeUnit.SECONDS))
    native.finishUnexpected()
    assertTrue(native.returned.await(1, TimeUnit.SECONDS))

    assertEquals(
        TranslatorStopResult.UnexpectedReturnKeepBlocking(
            MasqPacketTunnelJni.START_UNEXPECTED_CLEAN_RETURN),
        translator.stopAndAwait(18, 100),
    )
    assertTrue(translator.owns(ownership))
  }

  @Test
  fun startFailureBeforeNativeGenerationBeginsCanBeReleasedByExplicitCleanup() {
    val returned = CountDownLatch(1)
    val native =
        object : PacketTunnelNativeApi {
          override fun start(tunFd: Int, proxyPort: Int, mtu: Int): Int {
            throw IllegalStateException("JNI did not enter the native lifecycle")
          }

          override fun requestStop(): Boolean = false

          override fun snapshot() =
              PacketTunnelSnapshot(PacketTunnelNativeState.IDLE, null, null)
        }
    val translator = translator(native)
    translator.start(19, 3, 8080, 1500) { _, _, _, _ -> returned.countDown() }
    val ownership = checkNotNull(translator.ownership())
    assertTrue(returned.await(1, TimeUnit.SECONDS))

    assertTrue(translator.owns(ownership))
    assertEquals(
        TranslatorStopResult.SafeToClose,
        translator.stopAndAwait(19, 100),
    )
    assertTrue(
        translator.confirmsProcessReleased(ownership),
    )
  }

  @Test
  fun coreRoutePortChangeRestartsOnSameOpenDescriptorWithoutDirectGap() {
    val native = RestartableNative(expectedStarts = 2)
    val translator = translator(native)
    val descriptor = FakeOpenDescriptor(fd = 3)

    assertEquals(
        TranslatorStartResult.Started,
        translator.start(20, descriptor.fd, 8080, 1500) { _, _, _, _ -> },
    )
    assertTrue(native.awaitStartCount(1))
    assertEquals(
        TranslatorStartResult.ConfigurationConflict,
        translator.start(20, descriptor.fd, 9090, 1500) { _, _, _, _ -> },
    )

    assertEquals(
        TranslatorStopResult.SafeToClose,
        translator.stopAndAwait(20, 1_000),
    )
    assertFalse(descriptor.closed)
    assertEquals(
        TranslatorStartResult.Started,
        translator.start(20, descriptor.fd, 9090, 1500) { _, _, _, _ -> },
    )
    assertTrue(native.awaitStartCount(2))
    assertTrue(translator.isRunning(20))
    assertFalse(descriptor.closed)

    assertEquals(
        TranslatorStopResult.SafeToClose,
        translator.stopAndAwait(20, 1_000),
    )
  }

  @Test
  fun noOwnedRunMayCloseOnlyWhenNativeFailedAndNoProcessWrapperOwnsIt() {
    val native =
        object : PacketTunnelNativeApi {
          override fun start(tunFd: Int, proxyPort: Int, mtu: Int) =
              MasqPacketTunnelJni.START_FAILED

          override fun requestStop() = false

          override fun snapshot() =
              PacketTunnelSnapshot(PacketTunnelNativeState.FAILED, 4, "failed")
        }

    assertEquals(
        TranslatorStopResult.SafeToClose,
        translator(native).stopAndAwait(null, 100),
    )
  }

  @Test
  fun noOwnedRunCannotCloseWhileAnotherWrapperClaimedButHasNotEnteredNative() {
    val native = FakeNative()
    val blockedExecutor = Executors.newSingleThreadExecutor()
    executors += blockedExecutor
    val releaseExecutor = CountDownLatch(1)
    blockedExecutor.submit { releaseExecutor.await() }
    val owner = SystemRoutingTranslator(native, blockedExecutor)
    assertEquals(
        TranslatorStartResult.Started,
        owner.start(21, 3, 8080, 1500) { _, _, _, _ -> },
    )
    val observer = translator(native)

    assertEquals(
        TranslatorStopResult.NativeStateNotIdleKeepBlocking,
        observer.stopAndAwait(null, 100),
    )

    releaseExecutor.countDown()
    assertTrue(native.started.await(1, TimeUnit.SECONDS))
    assertEquals(
        TranslatorStopResult.SafeToClose,
        owner.stopAndAwait(21, 1_000),
    )
  }

  @Test
  fun laterNativeGenerationInvalidatesAnEarlierTerminalReleaseProof() {
    val native =
        FakeNative(unexpectedResult = MasqPacketTunnelJni.START_UNEXPECTED_CLEAN_RETURN)
    val translator = translator(native)
    translator.start(22, 3, 8080, 1500) { _, _, _, _ -> }
    val ownership = checkNotNull(translator.ownership())
    assertTrue(native.started.await(1, TimeUnit.SECONDS))
    native.finishUnexpected()
    assertTrue(native.returned.await(1, TimeUnit.SECONDS))
    assertEquals(
        TranslatorStopResult.SafeToClose,
        translator.stopAndAwait(22, 100),
    )
    assertTrue(
        translator.confirmsProcessReleased(ownership),
    )

    native.advanceToFailedGeneration()

    assertFalse(
        translator.confirmsProcessReleased(ownership),
    )
  }

  @Test
  fun staleOrUnknownTerminalResultNeverReleasesOwnership() {
    val native = FakeNative(unexpectedResult = MasqPacketTunnelJni.START_STALE_COMPLETION)
    val translator = translator(native)
    translator.start(23, 3, 8080, 1500) { _, _, _, _ -> }
    val ownership = checkNotNull(translator.ownership())
    assertTrue(native.started.await(1, TimeUnit.SECONDS))
    native.finishUnexpected()
    assertTrue(native.returned.await(1, TimeUnit.SECONDS))

    assertEquals(
        TranslatorStopResult.UnexpectedReturnKeepBlocking(
            MasqPacketTunnelJni.START_STALE_COMPLETION),
        translator.stopAndAwait(23, 100),
    )
    assertTrue(translator.owns(ownership))
  }

  @Test
  fun throwingReturnCallbackCannotHideTheExactNativeTerminalResult() {
    val native = FakeNative(unexpectedResult = MasqPacketTunnelJni.START_FAILED)
    val translator = translator(native)
    translator.start(24, 3, 8080, 1500) { _, _, _, _ ->
      throw IllegalStateException("status callback failed")
    }
    val ownership = checkNotNull(translator.ownership())
    assertTrue(native.started.await(1, TimeUnit.SECONDS))
    native.finishUnexpected()
    assertTrue(native.returned.await(1, TimeUnit.SECONDS))

    assertEquals(
        TranslatorStopResult.SafeToClose,
        translator.stopAndAwait(24, 100),
    )
    assertTrue(
        translator.confirmsProcessReleased(ownership),
    )
  }

  @Test
  fun stalePreBeginFailureCallbackCannotOwnRetryWithSameNativeGeneration() {
    val native = RetryAfterPreBeginFailureNative()
    val translator = translator(native)
    val staleCallbackOwnership = AtomicReference<TranslatorOwnership>()
    val firstReturned = CountDownLatch(1)
    translator.start(25, 3, 8080, 1500) {
        revision,
        generation,
        attempt,
        _ ->
      staleCallbackOwnership.set(
          TranslatorOwnership(revision, generation, attempt),
      )
      firstReturned.countDown()
    }
    assertTrue(firstReturned.await(1, TimeUnit.SECONDS))
    assertEquals(
        TranslatorStopResult.SafeToClose,
        translator.stopAndAwait(25, 100),
    )

    assertEquals(
        TranslatorStartResult.Started,
        translator.start(25, 3, 8080, 1500) { _, _, _, _ -> },
    )
    assertTrue(native.secondStarted.await(1, TimeUnit.SECONDS))
    val stale = checkNotNull(staleCallbackOwnership.get())
    val current = checkNotNull(translator.ownership())
    assertEquals(stale.revision, current.revision)
    assertEquals(stale.expectedNativeGeneration, current.expectedNativeGeneration)
    assertFalse(stale.runAttemptEpoch == current.runAttemptEpoch)
    assertFalse(translator.owns(stale))
    assertTrue(translator.owns(current))

    assertEquals(
        TranslatorStopResult.SafeToClose,
        translator.stopAndAwait(25, 1_000),
    )
  }

  @Test
  fun readinessRequiresNativeRunningWhileFutureStillOwnsRun() {
    val native = FakeNative()
    val translator = translator(native)
    translator.start(11, 3, 8080, 1500) { _, _, _, _ -> }
    assertTrue(native.started.await(1, TimeUnit.SECONDS))

    assertEquals(
        TranslatorReadiness.Ready,
        translator.awaitReadiness(11, 100, 1),
    )

    native.finishStop()
  }

  @Test
  fun exactRunningSnapshotRejectsWrongRevisionOrNativeGeneration() {
    val native = FakeNative()
    val translator = translator(native)
    translator.start(26, 3, 8080, 1500) { _, _, _, _ -> }
    assertTrue(native.started.await(1, TimeUnit.SECONDS))

    assertEquals(1L, translator.exactRunningSnapshot(26)?.generation)
    assertEquals(null, translator.exactRunningSnapshot(27))

    native.snapshotGenerationSkew = 1L
    assertEquals(null, translator.exactRunningSnapshot(26))
    assertFalse(translator.isRunning(26))

    native.snapshotGenerationSkew = 0L
    assertEquals(
        TranslatorStopResult.SafeToClose,
        translator.stopAndAwait(26, 1_000),
    )
  }

  @Test
  fun stopRetriesWhenCalledBeforeNativeRunHasEntered() {
    val native = FakeNative()
    val executor = Executors.newSingleThreadExecutor()
    executors += executor
    val releaseExecutor = CountDownLatch(1)
    executor.submit { releaseExecutor.await() }
    val translator = SystemRoutingTranslator(native, executor)
    translator.start(12, 3, 8080, 1500) { _, _, _, _ -> }
    val releaseThread =
        Thread {
          Thread.sleep(20)
          releaseExecutor.countDown()
        }
    releaseThread.start()

    assertEquals(
        TranslatorStopResult.SafeToClose,
        translator.stopAndAwait(12, 1_000),
    )
    releaseThread.join()
  }

  @Test
  fun rejectedExecutorSubmissionDoesNotCreateFalseOwnership() {
    val native = FakeNative()
    val executor = Executors.newSingleThreadExecutor()
    executor.shutdownNow()

    assertEquals(
        TranslatorStartResult.SubmissionFailed,
        SystemRoutingTranslator(native, executor)
            .start(13, 3, 8080, 1500) { _, _, _, _ -> },
    )
    assertEquals(0, native.startCount)
  }

  @Test
  fun duplicateRevisionWithChangedProxyPortIsNotIdempotent() {
    val native = FakeNative()
    val translator = translator(native)
    translator.start(14, 3, 8080, 1500) { _, _, _, _ -> }
    assertTrue(native.started.await(1, TimeUnit.SECONDS))

    assertEquals(
        TranslatorStartResult.ConfigurationConflict,
        translator.start(14, 3, 9090, 1500) { _, _, _, _ -> },
    )
    assertEquals(1, native.startCount)
    native.finishStop()
  }

  @Test
  fun concurrentWrappersCannotBothClaimTheSameNativeGeneration() {
    val native = FakeNative()
    val first = translator(native)
    val second = translator(native)
    val callers = Executors.newFixedThreadPool(2)
    executors += callers
    val ready = CountDownLatch(2)
    val start = CountDownLatch(1)
    val operations =
        listOf(first, second).map { wrapper ->
          callers.submit<TranslatorStartResult> {
            ready.countDown()
            start.await()
            wrapper.start(15, 3, 8080, 1500) { _, _, _, _ -> }
          }
        }
    assertTrue(ready.await(1, TimeUnit.SECONDS))
    start.countDown()
    val results = operations.map { it.get(1, TimeUnit.SECONDS) }
    assertTrue(native.started.await(1, TimeUnit.SECONDS))

    assertEquals(1, results.count { it == TranslatorStartResult.Started })
    assertEquals(1, results.count { it == TranslatorStartResult.NativeBusy })
    assertEquals(1, native.startCount)
    native.finishStop()
  }

  @Test
  fun returnCallbackCarriesExactGenerationAndStaleOwnershipIsRejectedAfterStop() {
    val native = FakeNative()
    val translator = translator(native)
    val returnedGeneration = AtomicLong()
    translator.start(16, 3, 8080, 1500) { _, generation, _, _ ->
      returnedGeneration.set(generation)
    }
    assertTrue(native.started.await(1, TimeUnit.SECONDS))
    val ownership = checkNotNull(translator.ownership())

    assertEquals(ownership.expectedNativeGeneration, 1L)
    assertTrue(translator.owns(ownership))
    assertEquals(
        TranslatorStopResult.SafeToClose,
        translator.stopAndAwait(16, 1_000),
    )
    assertEquals(ownership.expectedNativeGeneration, returnedGeneration.get())
    assertFalse(translator.owns(ownership))
  }

  @Test
  fun stoppedReturnWithDifferentNativeGenerationKeepsOwnership() {
    val native = FakeNative(generationSkewOnReturn = 1)
    val translator = translator(native)
    translator.start(17, 3, 8080, 1500) { _, _, _, _ -> }
    assertTrue(native.started.await(1, TimeUnit.SECONDS))
    val ownership = checkNotNull(translator.ownership())

    assertEquals(
        TranslatorStopResult.NativeStateNotIdleKeepBlocking,
        translator.stopAndAwait(17, 1_000),
    )
    assertTrue(translator.owns(ownership))
    assertFalse(
        translator.confirmsProcessReleased(ownership),
    )
  }

  private fun translator(native: PacketTunnelNativeApi): SystemRoutingTranslator {
    val executor = Executors.newSingleThreadExecutor()
    executors += executor
    return SystemRoutingTranslator(native, executor)
  }

  private class FakeNative(
      private val ignoreStop: Boolean = false,
      private val unexpectedResult: Int? = null,
      private val generationSkewOnReturn: Long = 0,
  ) : PacketTunnelNativeApi {
    val started = CountDownLatch(1)
    val returned = CountDownLatch(1)
    private val finish = CountDownLatch(1)
    @Volatile private var state = PacketTunnelNativeState.IDLE
    @Volatile private var generation = 0L
    @Volatile private var stopRequested = false
    @Volatile var startCount = 0
    @Volatile var snapshotGenerationSkew = 0L

    override fun start(tunFd: Int, proxyPort: Int, mtu: Int): Int {
      startCount += 1
      generation += 1
      state = PacketTunnelNativeState.RUNNING
      started.countDown()
      finish.await()
      val result =
          if (unexpectedResult != null) {
            state = PacketTunnelNativeState.FAILED
            unexpectedResult
          } else {
            state = PacketTunnelNativeState.IDLE
            MasqPacketTunnelJni.START_STOPPED
          }
      generation += generationSkewOnReturn
      returned.countDown()
      return result
    }

    override fun requestStop(): Boolean {
      if (state != PacketTunnelNativeState.RUNNING) return false
      stopRequested = true
      state = PacketTunnelNativeState.STOPPING
      if (!ignoreStop) finish.countDown()
      return true
    }

    override fun snapshot() =
        PacketTunnelSnapshot(
            state,
            generation.takeIf { it > 0 }?.plus(snapshotGenerationSkew),
            null,
        )

    fun finishStop() {
      stopRequested = true
      state = PacketTunnelNativeState.STOPPING
      finish.countDown()
    }

    fun finishUnexpected() {
      finish.countDown()
    }

    fun advanceToFailedGeneration() {
      generation += 1
      state = PacketTunnelNativeState.FAILED
    }
  }

  private data class FakeOpenDescriptor(
      val fd: Int,
      var closed: Boolean = false,
  )

  private class RestartableNative(
      private val expectedStarts: Int,
  ) : PacketTunnelNativeApi {
    private val starts = AtomicInteger()
    private val startSignals = Semaphore(0)
    private val finishes = Semaphore(0)
    @Volatile private var state = PacketTunnelNativeState.IDLE
    @Volatile private var generation = 0L

    override fun start(tunFd: Int, proxyPort: Int, mtu: Int): Int {
      check(starts.incrementAndGet() <= expectedStarts)
      generation += 1
      state = PacketTunnelNativeState.RUNNING
      startSignals.release()
      finishes.acquire()
      state = PacketTunnelNativeState.IDLE
      return MasqPacketTunnelJni.START_STOPPED
    }

    override fun requestStop(): Boolean {
      if (state != PacketTunnelNativeState.RUNNING) return false
      state = PacketTunnelNativeState.STOPPING
      finishes.release()
      return true
    }

    override fun snapshot() =
        PacketTunnelSnapshot(
            state = state,
            generation = generation.takeIf { it > 0 },
            lastResult = null,
        )

    fun awaitStartCount(count: Int): Boolean {
      while (starts.get() < count) {
        if (!startSignals.tryAcquire(1, TimeUnit.SECONDS)) return false
      }
      return true
    }
  }

  private class RetryAfterPreBeginFailureNative : PacketTunnelNativeApi {
    val secondStarted = CountDownLatch(1)
    private val secondFinish = CountDownLatch(1)
    private val calls = AtomicInteger()
    @Volatile private var state = PacketTunnelNativeState.IDLE
    @Volatile private var generation = 0L

    override fun start(tunFd: Int, proxyPort: Int, mtu: Int): Int {
      if (calls.incrementAndGet() == 1) {
        throw IllegalStateException("JNI did not enter native begin")
      }
      generation += 1
      state = PacketTunnelNativeState.RUNNING
      secondStarted.countDown()
      secondFinish.await()
      state = PacketTunnelNativeState.IDLE
      return MasqPacketTunnelJni.START_STOPPED
    }

    override fun requestStop(): Boolean {
      if (state != PacketTunnelNativeState.RUNNING) return false
      state = PacketTunnelNativeState.STOPPING
      secondFinish.countDown()
      return true
    }

    override fun snapshot() =
        PacketTunnelSnapshot(
            state,
            generation.takeIf { it > 0 },
            null,
        )
  }
}
