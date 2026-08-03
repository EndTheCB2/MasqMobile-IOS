package com.masqmobile

import android.util.Log
import java.util.concurrent.Callable
import java.util.concurrent.ExecutorService
import java.util.concurrent.Future
import java.util.concurrent.TimeUnit
import java.util.concurrent.TimeoutException
import java.util.IdentityHashMap
import java.util.concurrent.atomic.AtomicLong
import java.util.concurrent.atomic.AtomicReference
import org.json.JSONObject

internal enum class PacketTunnelNativeState {
  IDLE,
  STARTING,
  RUNNING,
  STOPPING,
  FAILED,
  UNKNOWN,
}

internal data class PacketTunnelSnapshot(
    val state: PacketTunnelNativeState,
    val generation: Long?,
    val lastResult: String?,
    val trafficObserved: Boolean = false,
    val sessionMetrics: PacketTunnelSessionMetrics = PacketTunnelSessionMetrics.EMPTY,
)

internal data class PacketTunnelSessionMetrics(
    val sessionCapacity: Long,
    val activeSessions: Long,
    val peakSessions: Long,
    val rejectedCapacity: Long,
    val rejectedUdp: Long,
    val rejectedIpv6: Long,
    val rejectedNon443Tcp: Long,
    val payloadTxBytes: Long,
    val payloadRxBytes: Long,
) {
  companion object {
    val EMPTY =
        PacketTunnelSessionMetrics(
            sessionCapacity = 0,
            activeSessions = 0,
            peakSessions = 0,
            rejectedCapacity = 0,
            rejectedUdp = 0,
            rejectedIpv6 = 0,
            rejectedNon443Tcp = 0,
            payloadTxBytes = 0,
            payloadRxBytes = 0,
        )
  }
}

internal fun parsePacketTunnelSnapshot(serialized: String): PacketTunnelSnapshot {
  val value = JSONObject(serialized)
  val state =
      when (value.optString("state")) {
        "idle" -> PacketTunnelNativeState.IDLE
        "starting" -> PacketTunnelNativeState.STARTING
        "running" -> PacketTunnelNativeState.RUNNING
        "stopping" -> PacketTunnelNativeState.STOPPING
        "failed" -> PacketTunnelNativeState.FAILED
        else -> PacketTunnelNativeState.UNKNOWN
      }
  val generation = value.optLong("generation").takeIf { it > 0 }
  // The native counters are reset inside the exact tun2proxy run before its
  // lifecycle reaches RUNNING. Ignoring them in every other lifecycle state
  // prevents a narrow STARTING/FAILED race from attributing the preceding
  // generation's final counters to its successor.
  val metrics =
      if (state == PacketTunnelNativeState.RUNNING && generation != null) {
        value.optJSONObject("sessionMetrics")?.let(::parsePacketTunnelSessionMetrics)
            ?: PacketTunnelSessionMetrics.EMPTY
      } else {
        PacketTunnelSessionMetrics.EMPTY
      }
  return PacketTunnelSnapshot(
      state = state,
      generation = generation,
      lastResult = value.optString("lastResult").takeIf(String::isNotBlank),
      trafficObserved =
          state == PacketTunnelNativeState.RUNNING &&
              generation != null &&
              value.optBoolean("trafficObserved", false),
      sessionMetrics = metrics,
  )
}

private fun parsePacketTunnelSessionMetrics(value: JSONObject) =
    PacketTunnelSessionMetrics(
        sessionCapacity = value.safeAggregateCounter("sessionCapacity"),
        activeSessions = value.safeAggregateCounter("activeSessions"),
        peakSessions = value.safeAggregateCounter("peakSessions"),
        rejectedCapacity = value.safeAggregateCounter("rejectedCapacity"),
        rejectedUdp = value.safeAggregateCounter("rejectedUdp"),
        rejectedIpv6 = value.safeAggregateCounter("rejectedIpv6"),
        rejectedNon443Tcp = value.safeAggregateCounter("rejectedNon443Tcp"),
        payloadTxBytes = value.safeAggregateCounter("payloadTxBytes"),
        payloadRxBytes = value.safeAggregateCounter("payloadRxBytes"),
    )

private fun JSONObject.safeAggregateCounter(name: String): Long =
    runCatching { getLong(name) }.getOrDefault(0L).coerceAtLeast(0L)

private fun Long.safeDiagnosticCountBucket(): String =
    when (coerceAtLeast(0L)) {
      0L -> "none"
      1L -> "one"
      in 2L..4L -> "few"
      in 5L..16L -> "several"
      in 17L..64L -> "many"
      else -> "high"
    }

private fun Long.safeDiagnosticByteBucket(): String =
    when (coerceAtLeast(0L)) {
      0L -> "none"
      in 1L..1_023L -> "under_1k"
      in 1_024L..65_535L -> "under_64k"
      in 65_536L..1_048_575L -> "under_1m"
      in 1_048_576L..16_777_215L -> "under_16m"
      else -> "high"
    }

internal fun formatSafePacketTunnelDiagnostic(snapshot: PacketTunnelSnapshot): String {
  val metrics = snapshot.sessionMetrics
  val state =
      when (snapshot.state) {
        PacketTunnelNativeState.IDLE -> "idle"
        PacketTunnelNativeState.STARTING -> "starting"
        PacketTunnelNativeState.RUNNING -> "running"
        PacketTunnelNativeState.STOPPING -> "stopping"
        PacketTunnelNativeState.FAILED -> "failed"
        PacketTunnelNativeState.UNKNOWN -> "unknown"
      }
  val result =
      when (snapshot.lastResult) {
        null -> "none"
        "stopped" -> "stopped"
        "unexpectedCleanReturn" -> "unexpected_clean_return"
        "failed" -> "failed"
        else -> "unknown"
      }
  val signal =
      when {
        snapshot.state == PacketTunnelNativeState.FAILED -> "translator_failed"
        snapshot.state == PacketTunnelNativeState.STOPPING -> "stopping"
        snapshot.state == PacketTunnelNativeState.STARTING -> "starting"
        snapshot.state != PacketTunnelNativeState.RUNNING -> "idle"
        metrics.payloadRxBytes > 0 -> "payload_returned"
        metrics.payloadTxBytes > 0 || snapshot.trafficObserved -> "payload_sent"
        metrics.rejectedCapacity > 0 -> "capacity_pressure"
        metrics.activeSessions > 0 -> "tcp443_active"
        metrics.peakSessions > 0 -> "tcp443_seen"
        metrics.rejectedUdp > 0 ||
            metrics.rejectedIpv6 > 0 ||
            metrics.rejectedNon443Tcp > 0 -> "policy_rejected"
        else -> "ready"
      }
  return "TUN_STATUS generation=${snapshot.generation?.coerceIn(1L, MAX_SAFE_TUNNEL_COUNTER) ?: 0} " +
      "state=$state result=$result signal=$signal traffic=${snapshot.trafficObserved} " +
      "capacity=${metrics.sessionCapacity.safeDiagnosticCountBucket()} " +
      "active=${metrics.activeSessions.safeDiagnosticCountBucket()} " +
      "peak=${metrics.peakSessions.safeDiagnosticCountBucket()} " +
      "rejected_capacity=${metrics.rejectedCapacity.safeDiagnosticCountBucket()} " +
      "rejected_udp=${metrics.rejectedUdp.safeDiagnosticCountBucket()} " +
      "rejected_ipv6=${metrics.rejectedIpv6.safeDiagnosticCountBucket()} " +
      "rejected_non443_tcp=${metrics.rejectedNon443Tcp.safeDiagnosticCountBucket()} " +
      "payload_tx=${metrics.payloadTxBytes.safeDiagnosticByteBucket()} " +
      "payload_rx=${metrics.payloadRxBytes.safeDiagnosticByteBucket()}"
}

internal class SafePacketTunnelDiagnosticReporter(
    private val emit: (String) -> Unit,
) {
  private val lastDiagnostic = AtomicReference<String?>()

  fun record(snapshot: PacketTunnelSnapshot) {
    val diagnostic = formatSafePacketTunnelDiagnostic(snapshot)
    if (lastDiagnostic.getAndSet(diagnostic) != diagnostic) {
      emit(diagnostic)
    }
  }
}

internal interface PacketTunnelNativeApi {
  fun start(tunFd: Int, proxyPort: Int, mtu: Int): Int

  fun requestStop(): Boolean

  fun snapshot(): PacketTunnelSnapshot
}

internal object JniPacketTunnelNativeApi : PacketTunnelNativeApi {
  private val diagnosticReporter =
      SafePacketTunnelDiagnosticReporter { diagnostic ->
        Log.i(PACKET_TUNNEL_DIAGNOSTIC_TAG, diagnostic)
      }

  override fun start(tunFd: Int, proxyPort: Int, mtu: Int): Int =
      MasqPacketTunnelJni.nativeStart(tunFd, proxyPort, mtu)

  override fun requestStop(): Boolean = MasqPacketTunnelJni.nativeStop()

  override fun snapshot(): PacketTunnelSnapshot {
    val snapshot = parsePacketTunnelSnapshot(MasqPacketTunnelJni.nativeStateJson())
    diagnosticReporter.record(snapshot)
    return snapshot
  }
}

private const val PACKET_TUNNEL_DIAGNOSTIC_TAG = "MasqTunnelStatus"
private const val MAX_SAFE_TUNNEL_COUNTER = 999_999_999L

internal sealed interface TranslatorStartResult {
  data object Started : TranslatorStartResult

  data object Idempotent : TranslatorStartResult

  data class RevisionConflict(val ownedRevision: Long) : TranslatorStartResult

  data class AlreadyReturned(val nativeResult: Int?) : TranslatorStartResult

  data object SubmissionFailed : TranslatorStartResult

  data object ConfigurationConflict : TranslatorStartResult

  data object NativeBusy : TranslatorStartResult
}

internal sealed interface TranslatorReadiness {
  data object Ready : TranslatorReadiness

  data object TimedOut : TranslatorReadiness

  data class RevisionConflict(val ownedRevision: Long?) : TranslatorReadiness

  data class Returned(val nativeResult: Int?) : TranslatorReadiness

  data object NativeStateUnavailable : TranslatorReadiness

  data object WrongNativeGeneration : TranslatorReadiness
}

internal sealed interface TranslatorStopResult {
  data object SafeToClose : TranslatorStopResult

  data object TimedOutKeepBlocking : TranslatorStopResult

  data object StopWasNotAcceptedKeepBlocking : TranslatorStopResult

  data class UnexpectedReturnKeepBlocking(val nativeResult: Int?) : TranslatorStopResult

  data object NativeStateNotIdleKeepBlocking : TranslatorStopResult
}

internal data class TranslatorOwnership(
    val revision: Long,
    val expectedNativeGeneration: Long,
    val runAttemptEpoch: Long,
)

/**
 * Owns the exact Future that is using a TUN file descriptor.
 *
 * An Android service must not close that descriptor until [stopAndAwait] returns
 * [TranslatorStopResult.SafeToClose]. Failed and unexpected returns deliberately remain owned:
 * the service can keep the already-established TUN as a blocker instead of allowing direct
 * fallback. A later explicit OFF, RESET, or service-handoff cleanup may release an exact
 * completed run whose matching native generation is terminal FAILED; the return callback itself
 * never releases ownership.
 */
internal class SystemRoutingTranslator(
    private val nativeApi: PacketTunnelNativeApi,
    private val executor: ExecutorService,
    private val nanoTime: () -> Long = System::nanoTime,
    private val sleep: (Long) -> Unit = Thread::sleep,
) {
  private data class OwnedRun(
      val revision: Long,
      val tunFd: Int,
      val proxyPort: Int,
      val mtu: Int,
      val baselineSnapshot: PacketTunnelSnapshot,
      val expectedNativeGeneration: Long,
      val runAttemptEpoch: Long,
      val future: Future<Int>,
  )

  private data class TerminalReleaseProof(
      val ownership: TranslatorOwnership,
      val terminalSnapshot: PacketTunnelSnapshot,
  )

  private val lock = Any()
  private var ownedRun: OwnedRun? = null
  @Volatile private var terminalReleaseProof: TerminalReleaseProof? = null

  fun start(
      revision: Long,
      tunFd: Int,
      proxyPort: Int,
      mtu: Int,
      onReturn:
          (
              revision: Long,
              nativeGeneration: Long,
              runAttemptEpoch: Long,
              nativeResult: Int?,
          ) -> Unit,
  ): TranslatorStartResult {
    require(revision > 0)
    lateinit var future: Future<Int>
    synchronized(lock) {
      val current = ownedRun
      if (current != null) {
        if (current.revision != revision) {
          return TranslatorStartResult.RevisionConflict(current.revision)
        }
        if (current.tunFd != tunFd ||
            current.proxyPort != proxyPort ||
            current.mtu != mtu) {
          return TranslatorStartResult.ConfigurationConflict
        }
        return if (current.future.isDone) {
          TranslatorStartResult.AlreadyReturned(readCompletedResult(current.future))
        } else {
          TranslatorStartResult.Idempotent
        }
      }
      if (!claimProcessNativeOwnership()) {
        return TranslatorStartResult.NativeBusy
      }
      val baseline =
          runCatching { nativeApi.snapshot() }.getOrElse {
            releaseProcessNativeOwnership()
            return TranslatorStartResult.SubmissionFailed
          }
      if (baseline.state == PacketTunnelNativeState.STARTING ||
          baseline.state == PacketTunnelNativeState.RUNNING ||
          baseline.state == PacketTunnelNativeState.STOPPING) {
        releaseProcessNativeOwnership()
        return TranslatorStartResult.NativeBusy
      }
      if (baseline.state != PacketTunnelNativeState.IDLE &&
          baseline.state != PacketTunnelNativeState.FAILED) {
        releaseProcessNativeOwnership()
        return TranslatorStartResult.SubmissionFailed
      }
      val expectedNativeGeneration =
          (baseline.generation ?: 0L)
              .takeIf { it < Long.MAX_VALUE }
              ?.plus(1L)
              ?: run {
                releaseProcessNativeOwnership()
                return TranslatorStartResult.SubmissionFailed
              }
      val runAttemptEpoch =
          RUN_ATTEMPT_COUNTER.getAndUpdate { current ->
            if (current == Long.MAX_VALUE) Long.MAX_VALUE else current + 1
          }
      if (runAttemptEpoch <= 0 || runAttemptEpoch == Long.MAX_VALUE) {
        releaseProcessNativeOwnership()
        return TranslatorStartResult.SubmissionFailed
      }
      future =
          try {
            executor.submit(
                Callable {
                  val result =
                      runCatching { nativeApi.start(tunFd, proxyPort, mtu) }
                          .getOrElse { MasqPacketTunnelJni.START_FAILED }
                  // Status notification is best-effort and must not turn an
                  // exact native result into an exceptional Future.
                  runCatching {
                    onReturn(
                        revision,
                        expectedNativeGeneration,
                        runAttemptEpoch,
                        result,
                    )
                  }
                  result
                })
          } catch (_: RuntimeException) {
            releaseProcessNativeOwnership()
            return TranslatorStartResult.SubmissionFailed
          }
      ownedRun =
          OwnedRun(
              revision,
              tunFd,
              proxyPort,
              mtu,
              baseline,
              expectedNativeGeneration,
              runAttemptEpoch,
              future,
          )
    }
    return TranslatorStartResult.Started
  }

  fun awaitReadiness(
      revision: Long,
      timeoutMs: Long,
      pollIntervalMs: Long,
  ): TranslatorReadiness {
    require(timeoutMs >= 0)
    require(pollIntervalMs > 0)
    val deadline = nanoTime() + TimeUnit.MILLISECONDS.toNanos(timeoutMs)
    while (true) {
      val run =
          synchronized(lock) {
            ownedRun
          }
      if (run == null || run.revision != revision) {
        return TranslatorReadiness.RevisionConflict(run?.revision)
      }
      if (run.future.isDone) {
        return TranslatorReadiness.Returned(readCompletedResult(run.future))
      }
      val snapshot =
          runCatching { nativeApi.snapshot() }.getOrElse {
            return TranslatorReadiness.NativeStateUnavailable
          }
      if (snapshot.generation != null &&
          snapshot.generation > run.expectedNativeGeneration) {
        return TranslatorReadiness.WrongNativeGeneration
      }
      if (snapshot.state == PacketTunnelNativeState.RUNNING && !run.future.isDone) {
        return if (snapshot.generation == run.expectedNativeGeneration) {
          TranslatorReadiness.Ready
        } else {
          TranslatorReadiness.WrongNativeGeneration
        }
      }
      val remainingNanos = deadline - nanoTime()
      if (remainingNanos <= 0) return TranslatorReadiness.TimedOut
      val sleepMs =
          minOf(
              pollIntervalMs,
              maxOf(1L, TimeUnit.NANOSECONDS.toMillis(remainingNanos)),
          )
      if (runCatching { sleep(sleepMs) }.isFailure) {
        return TranslatorReadiness.NativeStateUnavailable
      }
    }
  }

  /**
   * Requests a prompt native stop without waiting for the exact run to return.
   *
   * Package-scope broadcasts use this before their serialized rebuild reaches the control
   * executor. The existing TUN stays open as a blocker, and [stopAndAwait] remains the only method
   * allowed to release ownership or authorize descriptor replacement.
   */
  fun requestStopWithoutRelease(): Boolean {
    val run =
        synchronized(lock) {
          ownedRun?.takeUnless { it.future.isDone }
        } ?: return false
    return runCatching { nativeApi.requestStop() }.getOrDefault(false)
  }

  fun stopAndAwait(revision: Long?, timeoutMs: Long): TranslatorStopResult {
    require(timeoutMs >= 0)
    val deadline = nanoTime() + TimeUnit.MILLISECONDS.toNanos(timeoutMs)
    val run =
        synchronized(lock) {
          ownedRun
        }
    if (run == null) {
      return if (unownedTerminalSnapshot() != null) {
        TranslatorStopResult.SafeToClose
      } else {
        TranslatorStopResult.NativeStateNotIdleKeepBlocking
      }
    }
    if (revision != null && run.revision != revision) {
      return TranslatorStopResult.NativeStateNotIdleKeepBlocking
    }
    var stopAccepted = false
    while (!stopAccepted && !run.future.isDone) {
      stopAccepted = runCatching { nativeApi.requestStop() }.getOrDefault(false)
      if (stopAccepted) break
      val remainingNanos = deadline - nanoTime()
      if (remainingNanos <= 0) {
        return TranslatorStopResult.StopWasNotAcceptedKeepBlocking
      }
      if (
          runCatching {
                sleep(
                    minOf(
                        STOP_POLL_INTERVAL_MS,
                        maxOf(1L, TimeUnit.NANOSECONDS.toMillis(remainingNanos)),
                    ))
              }
              .isFailure
      ) {
        return TranslatorStopResult.StopWasNotAcceptedKeepBlocking
      }
    }
    val remainingNanos = maxOf(0L, deadline - nanoTime())
    val result =
        try {
          run.future.get(remainingNanos, TimeUnit.NANOSECONDS)
        } catch (_: TimeoutException) {
          return TranslatorStopResult.TimedOutKeepBlocking
        } catch (_: Exception) {
          return TranslatorStopResult.UnexpectedReturnKeepBlocking(null)
        }
    val finalSnapshot =
        runCatching { nativeApi.snapshot() }.getOrNull()
            ?: return TranslatorStopResult.NativeStateNotIdleKeepBlocking
    val exactCompletedTerminal =
        when (result) {
          MasqPacketTunnelJni.START_STOPPED ->
              finalSnapshot.state == PacketTunnelNativeState.IDLE &&
                  finalSnapshot.generation == run.expectedNativeGeneration
          MasqPacketTunnelJni.START_FAILED,
          MasqPacketTunnelJni.START_UNEXPECTED_CLEAN_RETURN ->
              // This path is reached only by an explicit cleanup caller after
              // Future.get proved that the exact native run returned. FAILED
              // means tun2proxy has dropped its TUN device; the retained PFD
              // may now be closed without allowing an automatic fallback.
              finalSnapshot.state == PacketTunnelNativeState.FAILED &&
                  finalSnapshot.generation == run.expectedNativeGeneration
          else -> false
        }
    val failedBeforeNativeBegin =
        result == MasqPacketTunnelJni.START_FAILED &&
            (finalSnapshot.state == PacketTunnelNativeState.IDLE ||
                finalSnapshot.state == PacketTunnelNativeState.FAILED) &&
            finalSnapshot.state == run.baselineSnapshot.state &&
            finalSnapshot.generation == run.baselineSnapshot.generation
    if (!exactCompletedTerminal && !failedBeforeNativeBegin) {
      if (result != MasqPacketTunnelJni.START_STOPPED) {
        return TranslatorStopResult.UnexpectedReturnKeepBlocking(result)
      }
      return TranslatorStopResult.NativeStateNotIdleKeepBlocking
    }
    synchronized(lock) {
      if (ownedRun?.future === run.future) {
        ownedRun = null
        terminalReleaseProof =
            TerminalReleaseProof(
                ownership =
                    TranslatorOwnership(
                        revision = run.revision,
                        expectedNativeGeneration = run.expectedNativeGeneration,
                        runAttemptEpoch = run.runAttemptEpoch,
                    ),
                terminalSnapshot = finalSnapshot,
            )
        releaseProcessNativeOwnership()
      }
    }
    return TranslatorStopResult.SafeToClose
  }

  /**
   * Returns a snapshot only when the exact Future, policy revision, and native
   * lifecycle generation still agree. Callers must use this proof instead of a
   * bare native RUNNING bit before publishing translator readiness or traffic.
   */
  fun exactRunningSnapshot(revision: Long): PacketTunnelSnapshot? {
    val run =
        synchronized(lock) {
          ownedRun
        }
    if (run == null || run.revision != revision || run.future.isDone) return null
    val snapshot = runCatching { nativeApi.snapshot() }.getOrNull() ?: return null
    if (snapshot.state != PacketTunnelNativeState.RUNNING ||
        snapshot.generation != run.expectedNativeGeneration) {
      return null
    }
    return synchronized(lock) {
      val current = ownedRun
      snapshot.takeIf {
        current?.future === run.future &&
            current.revision == revision &&
            current.expectedNativeGeneration == snapshot.generation &&
            !run.future.isDone
      }
    }
  }

  fun isRunning(revision: Long): Boolean = exactRunningSnapshot(revision) != null

  fun hasOwnedRun(): Boolean = synchronized(lock) { ownedRun != null }

  fun ownership(): TranslatorOwnership? =
      synchronized(lock) {
        ownedRun?.let {
          TranslatorOwnership(
              revision = it.revision,
              expectedNativeGeneration = it.expectedNativeGeneration,
              runAttemptEpoch = it.runAttemptEpoch,
          )
        }
      }

  fun owns(ownership: TranslatorOwnership): Boolean =
      synchronized(lock) {
        ownedRun?.let {
          it.revision == ownership.revision &&
              it.expectedNativeGeneration == ownership.expectedNativeGeneration &&
              it.runAttemptEpoch == ownership.runAttemptEpoch
        } == true
      }

  fun confirmsProcessReleased(expectedOwnership: TranslatorOwnership?): Boolean =
      synchronized(PROCESS_OWNER_LOCK) {
        if (PROCESS_NATIVE_OWNERS.containsKey(nativeApi)) {
          return@synchronized false
        }
        val currentSnapshot =
            runCatching { nativeApi.snapshot() }.getOrNull()
                ?: return@synchronized false
        val proof = terminalReleaseProof
        if (expectedOwnership != null) {
          return@synchronized proof?.ownership == expectedOwnership &&
              proof.terminalSnapshot == currentSnapshot
        }
        currentSnapshot.state == PacketTunnelNativeState.IDLE ||
            currentSnapshot.state == PacketTunnelNativeState.FAILED
      }

  private fun readCompletedResult(future: Future<Int>): Int? =
      runCatching { future.get() }.getOrNull()

  private fun claimProcessNativeOwnership(): Boolean =
      synchronized(PROCESS_OWNER_LOCK) {
        val owner = PROCESS_NATIVE_OWNERS[nativeApi]
        if (owner != null) {
          false
        } else {
          PROCESS_NATIVE_OWNERS[nativeApi] = this
          true
        }
      }

  private fun releaseProcessNativeOwnership() {
    synchronized(PROCESS_OWNER_LOCK) {
      if (PROCESS_NATIVE_OWNERS[nativeApi] === this) {
        PROCESS_NATIVE_OWNERS.remove(nativeApi)
      }
    }
  }

  private fun unownedTerminalSnapshot(): PacketTunnelSnapshot? =
      synchronized(PROCESS_OWNER_LOCK) {
        if (PROCESS_NATIVE_OWNERS.containsKey(nativeApi)) {
          return@synchronized null
        }
        runCatching { nativeApi.snapshot() }
            .getOrNull()
            ?.takeIf {
              it.state == PacketTunnelNativeState.IDLE ||
                  it.state == PacketTunnelNativeState.FAILED
            }
      }

  private companion object {
    const val STOP_POLL_INTERVAL_MS = 10L
    val PROCESS_OWNER_LOCK = Any()
    val PROCESS_NATIVE_OWNERS =
        IdentityHashMap<PacketTunnelNativeApi, SystemRoutingTranslator>()
    val RUN_ATTEMPT_COUNTER = AtomicLong(1L)
  }
}
