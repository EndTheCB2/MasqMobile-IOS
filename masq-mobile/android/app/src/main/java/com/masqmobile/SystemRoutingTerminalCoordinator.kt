package com.masqmobile

import java.util.concurrent.CompletableFuture
import java.util.concurrent.TimeUnit
import java.util.concurrent.TimeoutException
import java.util.concurrent.atomic.AtomicLong

internal sealed interface TerminalLeaseRetainResult {
  val epoch: Long

  data class Retained(override val epoch: Long) : TerminalLeaseRetainResult

  data class AlreadyRetained(override val epoch: Long) : TerminalLeaseRetainResult

  data class Conflict(override val epoch: Long) : TerminalLeaseRetainResult
}

internal sealed interface TerminalLeaseCloseResult {
  data object NoLease : TerminalLeaseCloseResult

  data class Closed(
      val epoch: Long,
      val expectedNativeGeneration: Long?,
  ) : TerminalLeaseCloseResult

  data class StopFailed(
      val epoch: Long,
      val result: TranslatorStopResult,
  ) : TerminalLeaseCloseResult

  data class ProcessOwnershipNotReleased(
      val epoch: Long,
      val expectedNativeGeneration: Long?,
  ) : TerminalLeaseCloseResult

  data class DescriptorCloseFailed(val epoch: Long) : TerminalLeaseCloseResult

  data class JoinTimedOut(val epoch: Long) : TerminalLeaseCloseResult

  data class StaleCompletion(val epoch: Long) : TerminalLeaseCloseResult
}

internal data class TerminalLeaseSnapshot(
    val epoch: Long,
    val policy: DesiredSystemRoutingPolicy?,
    val ownership: TranslatorOwnership?,
    val cleanupInProgress: Boolean,
    val captureValid: Boolean,
)

/**
 * Process-global owner of a terminal TUN descriptor and the exact translator Future using it.
 *
 * Service recreation must not infer safety from fresh instance fields. Every caller joins the
 * same cleanup Future and the exact descriptor remains strongly referenced until the translator
 * returns stopped, native state is idle for the same generation, and close succeeds.
 */
internal class SystemRoutingTerminalCoordinator<Resource : Any>(
    private val closeResource: (Resource) -> Boolean,
    private val epochCounter: AtomicLong = AtomicLong(1L),
) {
  private data class Lease<Resource : Any>(
      val epoch: Long,
      val resource: Resource,
      val policy: DesiredSystemRoutingPolicy?,
      val translator: SystemRoutingTranslator,
      val ownership: TranslatorOwnership?,
      var captureValid: Boolean,
      var cleanup: CompletableFuture<TerminalLeaseCloseResult>? = null,
  )

  private sealed interface CleanupAttempt {
    data class Lead<Resource : Any>(
        val lease: Lease<Resource>,
        val future: CompletableFuture<TerminalLeaseCloseResult>,
    ) : CleanupAttempt

    data class Join(
        val epoch: Long,
        val future: CompletableFuture<TerminalLeaseCloseResult>,
    ) : CleanupAttempt
  }

  private val lock = Any()
  private var lease: Lease<Resource>? = null
  private var resetEpoch: Long? = null
  private val activeStartEpochs = mutableSetOf<Long>()

  fun retain(
      resource: Resource,
      policy: DesiredSystemRoutingPolicy?,
      translator: SystemRoutingTranslator,
      captureValid: Boolean = true,
  ): TerminalLeaseRetainResult =
      synchronized(lock) {
        val current = lease
        if (current == null) {
          val epoch = nextEpoch()
          lease =
              Lease(
                  epoch = epoch,
                  resource = resource,
                  policy = policy,
                  translator = translator,
                  ownership = translator.ownership(),
                  captureValid = captureValid,
              )
          TerminalLeaseRetainResult.Retained(epoch)
        } else if (current.resource === resource && current.translator === translator) {
          TerminalLeaseRetainResult.AlreadyRetained(current.epoch)
        } else {
          TerminalLeaseRetainResult.Conflict(current.epoch)
        }
      }

  fun closeOrJoin(timeoutMs: Long): TerminalLeaseCloseResult {
    require(timeoutMs >= 0)
    val attempt =
        synchronized(lock) {
          val current = lease ?: return TerminalLeaseCloseResult.NoLease
          val running = current.cleanup
          if (running != null) {
            CleanupAttempt.Join(current.epoch, running)
          } else {
            val future = CompletableFuture<TerminalLeaseCloseResult>()
            current.cleanup = future
            CleanupAttempt.Lead(current, future)
          }
        }
    return when (attempt) {
      is CleanupAttempt.Join -> join(attempt, timeoutMs)
      is CleanupAttempt.Lead<*> -> {
        @Suppress("UNCHECKED_CAST")
        lead(attempt as CleanupAttempt.Lead<Resource>, timeoutMs)
      }
    }
  }

  fun snapshot(): TerminalLeaseSnapshot? =
      synchronized(lock) {
        lease?.let {
          TerminalLeaseSnapshot(
              epoch = it.epoch,
              policy = it.policy,
              ownership = it.ownership,
              cleanupInProgress = it.cleanup != null,
              captureValid = it.captureValid,
          )
        }
      }

  /**
   * Android has already deactivated capture when VpnService.onRevoke is
   * delivered. Retaining the descriptor for exact native cleanup must never
   * make that stale descriptor look like a blocking route.
   */
  fun invalidateCapture(
      resource: Resource?,
      translator: SystemRoutingTranslator,
      adoptedEpoch: Long? = null,
  ): Boolean =
      synchronized(lock) {
        val current = lease
        val exactLocalOwner =
            resource != null &&
                current?.resource === resource &&
                current.translator === translator
        val exactAdoptedLease =
            adoptedEpoch != null && current?.epoch == adoptedEpoch
        if (exactLocalOwner || exactAdoptedLease) {
          checkNotNull(current)
          current.captureValid = false
          true
        } else {
          false
        }
      }

  fun beginExplicitReset(): Long? =
      synchronized(lock) {
        if (resetEpoch != null || activeStartEpochs.isNotEmpty()) {
          null
        } else {
          nextEpoch().also { resetEpoch = it }
        }
      }

  fun finishExplicitReset(epoch: Long) {
    synchronized(lock) {
      if (resetEpoch == epoch) {
        resetEpoch = null
      }
    }
  }

  fun blocksNewStart(): Boolean =
      synchronized(lock) {
        lease != null || resetEpoch != null || activeStartEpochs.isNotEmpty()
      }

  fun beginStart(): Long? =
      synchronized(lock) {
        if (lease != null || resetEpoch != null || activeStartEpochs.isNotEmpty()) {
          null
        } else {
          nextEpoch().also(activeStartEpochs::add)
        }
      }

  fun finishStart(epoch: Long) {
    synchronized(lock) {
      activeStartEpochs.remove(epoch)
    }
  }

  private fun lead(
      attempt: CleanupAttempt.Lead<Resource>,
      timeoutMs: Long,
  ): TerminalLeaseCloseResult {
    val candidate = attempt.lease
    val expectedGeneration = candidate.ownership?.expectedNativeGeneration
    val stopResult =
        candidate.translator.stopAndAwait(
            candidate.ownership?.revision ?: candidate.policy?.revision,
            timeoutMs,
        )
    val performed =
        when {
          stopResult != TranslatorStopResult.SafeToClose ->
              TerminalLeaseCloseResult.StopFailed(candidate.epoch, stopResult)
          !candidate.translator.confirmsProcessReleased(candidate.ownership) ->
              TerminalLeaseCloseResult.ProcessOwnershipNotReleased(
                  candidate.epoch,
                  expectedGeneration,
              )
          !runCatching { closeResource(candidate.resource) }.getOrDefault(false) ->
              TerminalLeaseCloseResult.DescriptorCloseFailed(candidate.epoch)
          else ->
              TerminalLeaseCloseResult.Closed(candidate.epoch, expectedGeneration)
        }
    val completed =
        synchronized(lock) {
          val current = lease
          if (current?.epoch != candidate.epoch ||
              current.resource !== candidate.resource ||
              current.translator !== candidate.translator) {
            TerminalLeaseCloseResult.StaleCompletion(candidate.epoch)
          } else {
            if (performed is TerminalLeaseCloseResult.Closed) {
              lease = null
            } else {
              current.cleanup = null
              if (performed is TerminalLeaseCloseResult.DescriptorCloseFailed) {
                // close() can fail after the kernel already deactivated the
                // interface. Keep the object for bookkeeping, but never claim
                // that its capture still blocks direct traffic.
                current.captureValid = false
              }
            }
            performed
          }
        }
    attempt.future.complete(completed)
    return completed
  }

  private fun join(
      attempt: CleanupAttempt.Join,
      timeoutMs: Long,
  ): TerminalLeaseCloseResult =
      try {
        attempt.future.get(timeoutMs, TimeUnit.MILLISECONDS)
      } catch (_: TimeoutException) {
        TerminalLeaseCloseResult.JoinTimedOut(attempt.epoch)
      } catch (_: Exception) {
        TerminalLeaseCloseResult.StopFailed(
            attempt.epoch,
            TranslatorStopResult.UnexpectedReturnKeepBlocking(null),
        )
      }

  private fun nextEpoch(): Long {
    val epoch = epochCounter.getAndIncrement()
    check(epoch > 0 && epoch < Long.MAX_VALUE) {
      "The system-routing terminal ownership epoch is exhausted."
    }
    return epoch
  }
}
