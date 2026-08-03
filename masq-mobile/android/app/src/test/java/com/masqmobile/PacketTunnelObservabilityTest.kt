package com.masqmobile

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class PacketTunnelObservabilityTest {
  @Test
  fun runningSnapshotPreservesAddressFreeSessionMetricsForDeltaAnalysis() {
    val snapshot =
        parsePacketTunnelSnapshot(
            """
            {
              "state":"running",
              "generation":7,
              "lastResult":null,
              "trafficObserved":true,
              "sessionMetrics":{
                "sessionCapacity":256,
                "activeSessions":2,
                "peakSessions":4,
                "rejectedCapacity":1,
                "rejectedUdp":11,
                "rejectedIpv6":-3,
                "rejectedNon443Tcp":999999999999,
                "payloadTxBytes":321,
                "payloadRxBytes":654
              }
            }
            """.trimIndent())

    assertEquals(PacketTunnelNativeState.RUNNING, snapshot.state)
    assertEquals(7L, snapshot.generation)
    assertTrue(snapshot.trafficObserved)
    assertEquals(
        PacketTunnelSessionMetrics(
            sessionCapacity = 256,
            activeSessions = 2,
            peakSessions = 4,
            rejectedCapacity = 1,
            rejectedUdp = 11,
            rejectedIpv6 = 0,
            rejectedNon443Tcp = 999_999_999_999,
            payloadTxBytes = 321,
            payloadRxBytes = 654,
        ),
        snapshot.sessionMetrics,
    )
    val diagnostic = formatSafePacketTunnelDiagnostic(snapshot)
    assertTrue(diagnostic.contains("signal=payload_returned"))
    assertTrue(diagnostic.contains("rejected_non443_tcp=high"))
    assertTrue(diagnostic.contains("payload_tx=under_1k"))
    assertTrue(diagnostic.contains("payload_rx=under_1k"))
    assertFalse(diagnostic.contains("321"))
    assertFalse(diagnostic.contains("654"))
  }

  @Test
  fun nonRunningGenerationCannotReusePriorTrafficOrMetrics() {
    val snapshot =
        parsePacketTunnelSnapshot(
            """
            {
              "state":"starting",
              "generation":8,
              "lastResult":null,
              "trafficObserved":true,
              "sessionMetrics":{
                "sessionCapacity":256,
                "activeSessions":9,
                "peakSessions":9,
                "rejectedCapacity":9,
                "rejectedUdp":9,
                "rejectedIpv6":9,
                "rejectedNon443Tcp":9,
                "payloadTxBytes":9,
                "payloadRxBytes":9
              }
            }
            """.trimIndent())

    assertFalse(snapshot.trafficObserved)
    assertEquals(PacketTunnelSessionMetrics.EMPTY, snapshot.sessionMetrics)
  }

  @Test
  fun diagnosticUsesOnlyFixedCategoriesAndBoundedCounters() {
    val snapshot =
        PacketTunnelSnapshot(
            state = PacketTunnelNativeState.RUNNING,
            generation = Long.MAX_VALUE,
            lastResult = "endpoint=203.0.113.8",
            trafficObserved = false,
            sessionMetrics =
                PacketTunnelSessionMetrics(
                    sessionCapacity = 256,
                    activeSessions = 1,
                    peakSessions = 1,
                    rejectedCapacity = 0,
                    rejectedUdp = 0,
                    rejectedIpv6 = 0,
                    rejectedNon443Tcp = 0,
                    payloadTxBytes = 0,
                    payloadRxBytes = 0,
                ),
        )

    val diagnostic = formatSafePacketTunnelDiagnostic(snapshot)

    assertEquals(
        "TUN_STATUS generation=999999999 state=running result=unknown " +
            "signal=tcp443_active traffic=false capacity=high active=one peak=one " +
            "rejected_capacity=none rejected_udp=none rejected_ipv6=none " +
            "rejected_non443_tcp=none payload_tx=none payload_rx=none",
        diagnostic,
    )
    assertFalse(diagnostic.contains("203.0.113.8"))
    assertTrue(
        diagnostic.matches(
            Regex(
                "^TUN_STATUS generation=[0-9]+ " +
                    "state=(idle|starting|running|stopping|failed|unknown) " +
                    "result=(none|stopped|unexpected_clean_return|failed|unknown) " +
                    "signal=(idle|starting|stopping|ready|tcp443_active|tcp443_seen|" +
                    "payload_sent|payload_returned|policy_rejected|capacity_pressure|" +
                    "translator_failed) " +
                    "traffic=(true|false)( [a-z0-9_]+=[a-z0-9_]+)+$")))
  }

  @Test
  fun reporterDeduplicatesIdenticalGenerationScopedSnapshots() {
    val emitted = mutableListOf<String>()
    val reporter = SafePacketTunnelDiagnosticReporter(emitted::add)
    val initial =
        PacketTunnelSnapshot(
            state = PacketTunnelNativeState.RUNNING,
            generation = 3,
            lastResult = null,
        )

    reporter.record(initial)
    reporter.record(initial)
    reporter.record(
        initial.copy(
            sessionMetrics =
                PacketTunnelSessionMetrics.EMPTY.copy(
                    sessionCapacity = 256,
                    activeSessions = 1,
                    peakSessions = 1,
                )))

    assertEquals(2, emitted.size)
    assertTrue(emitted.first().contains("generation=3"))
    assertTrue(emitted.last().contains("signal=tcp443_active"))
  }

  @Test
  fun reporterDoesNotEmitExactPayloadDeltasInsideOnePrivacyBucket() {
    val emitted = mutableListOf<String>()
    val reporter = SafePacketTunnelDiagnosticReporter(emitted::add)
    val initial =
        PacketTunnelSnapshot(
            state = PacketTunnelNativeState.RUNNING,
            generation = 4,
            lastResult = null,
            trafficObserved = true,
            sessionMetrics =
                PacketTunnelSessionMetrics.EMPTY.copy(
                    sessionCapacity = 256,
                    payloadTxBytes = 2_000,
                    payloadRxBytes = 3_000,
                ),
        )

    reporter.record(initial)
    reporter.record(
        initial.copy(
            sessionMetrics =
                initial.sessionMetrics.copy(
                    payloadTxBytes = 40_000,
                    payloadRxBytes = 60_000,
                )))
    reporter.record(
        initial.copy(
            sessionMetrics =
                initial.sessionMetrics.copy(
                    payloadTxBytes = 70_000,
                    payloadRxBytes = 80_000,
                )))

    assertEquals(2, emitted.size)
    assertTrue(emitted.first().contains("payload_tx=under_64k"))
    assertTrue(emitted.last().contains("payload_tx=under_1m"))
    assertFalse(emitted.joinToString().contains("2000"))
    assertFalse(emitted.joinToString().contains("70000"))
  }
}
