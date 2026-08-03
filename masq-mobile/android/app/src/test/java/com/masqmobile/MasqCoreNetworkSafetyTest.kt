package com.masqmobile

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class MasqCoreNetworkSafetyTest {
  @Test
  fun `internet capability is unavailable until Android validates the network`() {
    val tracker = AndroidNetworkStatusTracker()

    val unvalidated =
        tracker.observe(
            observation(
                identity = 11L,
                hasInternet = true,
                validated = false,
            ))
    val validated =
        tracker.observe(
            observation(
                identity = 11L,
                hasInternet = true,
                validated = true,
            ))

    assertFalse(unvalidated.available)
    assertTrue(validated.available)
    assertEquals(1L, unvalidated.generation)
    assertEquals(2L, validated.generation)
  }

  @Test
  fun `unchanged polls keep one generation and validated handovers advance it`() {
    val tracker = AndroidNetworkStatusTracker()
    val first = tracker.observe(observation(identity = 21L))
    val unchanged = tracker.observe(observation(identity = 21L))
    val handover = tracker.observe(observation(identity = 22L))

    assertEquals(1L, first.generation)
    assertEquals(first, unchanged)
    assertEquals(2L, handover.generation)
    assertTrue(handover.available)
    assertEquals("wifi", handover.interfaceName)
  }

  @Test
  fun `public network snapshot contains only coarse privacy-safe state`() {
    val snapshot =
        AndroidNetworkStatusTracker()
            .observe(
                AndroidNetworkObservation(
                    opaqueNetworkIdentity = 9_876_543_210L,
                    hasInternetCapability = true,
                    isValidated = true,
                    interfaceName = "cellular",
                    expensive = true,
                ))

    assertEquals(
        AndroidNetworkStatusSnapshot(
            available = true,
            interfaceName = "cellular",
            expensive = true,
            constrained = false,
            generation = 1L,
        ),
        snapshot,
    )
    assertFalse(snapshot.toString().contains("9876543210"))
  }

  @Test
  fun `core termination retains the supervisor for active or unsafe routing policies`() {
    assertTrue(
        mayStopSessionSupervisorForCoreTermination(SystemRoutingPolicyLoadResult.Missing))
    assertTrue(
        mayStopSessionSupervisorForCoreTermination(
            SystemRoutingPolicyLoadResult.ExplicitOff(DesiredSystemRoutingPolicy.off(1L))))
    assertFalse(
        mayStopSessionSupervisorForCoreTermination(
            SystemRoutingPolicyLoadResult.Ready(
                DesiredSystemRoutingPolicy(
                    schemaVersion = SystemRoutingPolicyStore.CURRENT_SCHEMA_VERSION,
                    revision = 2L,
                    desiredMode = SystemRoutingMode.WHOLE_DEVICE,
                    selectedApps = emptyList(),
                    explicitConsentTimestampMs = 1L,
                    failClosedDesired = true,
                ))))
    assertFalse(
        mayStopSessionSupervisorForCoreTermination(
            SystemRoutingPolicyLoadResult.BlockRequired(
                SystemRoutingDiagnostic.CORRUPT_OR_PARTIAL_POLICY)))
  }

  @Test
  fun `network reset requires policy and observed tunnel state to be exactly off`() {
    val off = SystemRoutingPolicyLoadResult.ExplicitOff(DesiredSystemRoutingPolicy.off(3L))
    val active =
        SystemRoutingPolicyLoadResult.Ready(
            DesiredSystemRoutingPolicy(
                schemaVersion = SystemRoutingPolicyStore.CURRENT_SCHEMA_VERSION,
                revision = 4L,
                desiredMode = SystemRoutingMode.SELECTED_APPS,
                selectedApps = listOf("org.example.browser"),
                explicitConsentTimestampMs = 1L,
                failClosedDesired = true,
            ))

    assertTrue(
        isSystemRoutingConfirmedOff(
            policy = off,
            active = false,
            tunPresent = false,
            routingPhase = "off",
        ))
    assertFalse(
        isSystemRoutingConfirmedOff(
            policy = active,
            active = false,
            tunPresent = false,
            routingPhase = "off",
        ))
    assertFalse(
        isSystemRoutingConfirmedOff(
            policy = off,
            active = false,
            tunPresent = true,
            routingPhase = "blocked",
        ))
  }

  private fun observation(
      identity: Long,
      hasInternet: Boolean = true,
      validated: Boolean = true,
  ): AndroidNetworkObservation =
      AndroidNetworkObservation(
          opaqueNetworkIdentity = identity,
          hasInternetCapability = hasInternet,
          isValidated = validated,
          interfaceName = "wifi",
          expensive = false,
      )
}
