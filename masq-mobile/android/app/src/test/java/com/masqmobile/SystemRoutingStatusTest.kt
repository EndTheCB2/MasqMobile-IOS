package com.masqmobile

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class SystemRoutingStatusTest {
  @Test
  fun healthyMatchingPolicyDerivesActiveMasqState() {
    val status = wholeDeviceStatus()

    assertTrue(status.active)
    assertEquals(SystemRoutingPhase.ACTIVE, status.phase)
    assertEquals(SystemRoutingTrafficDisposition.MASQ, status.trafficDisposition)
    assertFalse(status.trafficObserved)
  }

  @Test
  fun trafficObservationIsReportedOnlyForAHealthyCurrentRoute() {
    val observed = wholeDeviceStatus(trafficObserved = true)
    val staleObservation =
        wholeDeviceStatus(coreRouteReady = false, trafficObserved = true)

    assertTrue(observed.trafficObserved)
    assertFalse(staleObservation.trafficObserved)
  }

  @Test
  fun everyRequiredHealthBitParticipatesInActiveDerivation() {
    listOf(
            wholeDeviceStatus(supported = false),
            wholeDeviceStatus(tunPresent = false),
            wholeDeviceStatus(translatorReady = false),
            wholeDeviceStatus(coreRouteReady = false),
            wholeDeviceStatus(lastError = SystemRoutingDiagnostic.INTERNAL_ERROR),
        )
        .forEach { status ->
          assertFalse(status.active)
          assertEquals(SystemRoutingPhase.BLOCKED, status.phase)
        }
  }

  @Test
  fun selectedPolicyRequiresExactDesiredAndAppliedRevisionAndPackageSet() {
    val revisionMismatch =
        selectedStatus(
            desiredRevision = 8L,
            appliedRevision = 7L,
            desiredApps = listOf("com.example.a"),
            appliedApps = listOf("com.example.a"),
        )
    val selectionMismatch =
        selectedStatus(
            desiredApps = listOf("com.example.a", "com.example.b"),
            appliedApps = listOf("com.example.a"),
        )

    assertFalse(revisionMismatch.active)
    assertFalse(selectionMismatch.active)
    assertEquals(SystemRoutingPhase.BLOCKED, revisionMismatch.phase)
    assertEquals(SystemRoutingPhase.BLOCKED, selectionMismatch.phase)
    assertEquals(
        SystemRoutingTrafficDisposition.BLOCKED,
        revisionMismatch.trafficDisposition,
    )
  }

  @Test
  fun partiallyAppliedSelectionIsDirectRiskWithoutAndroidLockdown() {
    val status =
        selectedStatus(
            desiredApps = listOf("com.example.a", "com.example.b"),
            appliedApps = listOf("com.example.a"),
            alwaysOn = false,
            lockdown = false,
        )

    assertFalse(status.active)
    assertEquals(
        SystemRoutingTrafficDisposition.DIRECT_RISK,
        status.trafficDisposition,
    )
  }

  @Test
  fun explicitTransitionPreventsHealthyComponentsFromClaimingActive() {
    val status =
        wholeDeviceStatus(transition = SystemRoutingTransition.RECONNECTING)

    assertFalse(status.active)
    assertEquals(SystemRoutingPhase.RECONNECTING, status.phase)
    assertEquals(SystemRoutingTrafficDisposition.BLOCKED, status.trafficDisposition)
  }

  @Test
  fun missingBlockingMechanismDerivesDirectRiskInsteadOfBlocked() {
    val status =
        SystemRoutingStatus.derive(
            supported = true,
            desiredRevision = 8L,
            desiredMode = SystemRoutingMode.WHOLE_DEVICE,
            desiredSelectedApps = emptyList(),
            failClosedDesired = true,
            appliedRevision = null,
            appliedMode = SystemRoutingMode.OFF,
            appliedSelectedApps = emptyList(),
            transition = SystemRoutingTransition.BLOCKED,
            tunPresent = false,
            translatorReady = false,
            coreRouteReady = false,
            alwaysOn = false,
            lockdown = false,
        )

    assertFalse(status.active)
    assertEquals(SystemRoutingTrafficDisposition.DIRECT_RISK, status.trafficDisposition)
  }

  @Test
  fun serviceDestructionClearsAppliedScopeAndNeverLeavesStaleActiveStatus() {
    val desired =
        DesiredSystemRoutingPolicy(
            schemaVersion = SystemRoutingPolicyStore.CURRENT_SCHEMA_VERSION,
            revision = 9L,
            desiredMode = SystemRoutingMode.WHOLE_DEVICE,
            selectedApps = emptyList(),
            explicitConsentTimestampMs = 1234L,
            failClosedDesired = false,
        )

    val status =
        systemRoutingStatusAfterServiceDestroyed(
            load = SystemRoutingPolicyLoadResult.Ready(desired),
            supported = true,
            alwaysOn = false,
            lockdown = false,
        )

    assertFalse(status.active)
    assertFalse(status.tunPresent)
    assertEquals(null, status.appliedRevision)
    assertEquals(SystemRoutingMode.OFF, status.appliedMode)
    assertEquals(SystemRoutingPhase.BLOCKED, status.phase)
    assertEquals(SystemRoutingTrafficDisposition.DIRECT_RISK, status.trafficDisposition)
  }

  @Test
  fun unsafeServiceDestructionRetainsTunAsBlockedInsteadOfClaimingNoTun() {
    val desired =
        DesiredSystemRoutingPolicy(
            schemaVersion = SystemRoutingPolicyStore.CURRENT_SCHEMA_VERSION,
            revision = 9L,
            desiredMode = SystemRoutingMode.WHOLE_DEVICE,
            selectedApps = emptyList(),
            explicitConsentTimestampMs = 1234L,
            failClosedDesired = false,
        )

    val status =
        systemRoutingStatusAfterServiceDestroyed(
            load = SystemRoutingPolicyLoadResult.Ready(desired),
            supported = true,
            alwaysOn = false,
            lockdown = false,
            retainedAppliedPolicy = desired,
            tunPresent = true,
            diagnostic = SystemRoutingDiagnostic.TRANSLATOR_STOP_TIMEOUT,
        )

    assertFalse(status.active)
    assertTrue(status.tunPresent)
    assertEquals(desired.revision, status.appliedRevision)
    assertEquals(SystemRoutingPhase.BLOCKED, status.phase)
    assertEquals(SystemRoutingTrafficDisposition.BLOCKED, status.trafficDisposition)
    assertEquals(SystemRoutingDiagnostic.TRANSLATOR_STOP_TIMEOUT, status.lastError)
  }

  @Test
  fun revokedCaptureIsDirectRiskEvenWhileDescriptorCleanupRemainsOwned() {
    val desired =
        DesiredSystemRoutingPolicy(
            schemaVersion = SystemRoutingPolicyStore.CURRENT_SCHEMA_VERSION,
            revision = 10L,
            desiredMode = SystemRoutingMode.WHOLE_DEVICE,
            selectedApps = emptyList(),
            explicitConsentTimestampMs = 1234L,
            failClosedDesired = false,
        )

    val status =
        systemRoutingStatusAfterServiceDestroyed(
            load = SystemRoutingPolicyLoadResult.Ready(desired),
            supported = true,
            alwaysOn = false,
            lockdown = false,
            retainedAppliedPolicy = desired,
            tunPresent = true,
            captureValid = false,
            diagnostic = SystemRoutingDiagnostic.PERMISSION_REVOKED,
        )

    assertFalse(status.active)
    assertFalse(status.tunPresent)
    assertEquals(null, status.appliedRevision)
    assertEquals(SystemRoutingMode.OFF, status.appliedMode)
    assertEquals(SystemRoutingPhase.REVOKED, status.phase)
    assertEquals(SystemRoutingTrafficDisposition.DIRECT_RISK, status.trafficDisposition)
  }

  @Test
  fun revokedCallbackAfterAnExplicitOffStateNormalizesToIdle() {
    val status =
        SystemRoutingStatus.derive(
            supported = true,
            desiredRevision = null,
            desiredMode = SystemRoutingMode.OFF,
            desiredSelectedApps = emptyList(),
            failClosedDesired = false,
            appliedRevision = null,
            appliedMode = SystemRoutingMode.OFF,
            appliedSelectedApps = emptyList(),
            transition = SystemRoutingTransition.REVOKED,
            tunPresent = false,
            translatorReady = false,
            coreRouteReady = false,
            alwaysOn = false,
            lockdown = false,
            lastError = SystemRoutingDiagnostic.PERMISSION_REVOKED,
        )

    assertFalse(status.active)
    assertEquals(SystemRoutingPhase.OFF, status.phase)
    assertEquals(SystemRoutingTrafficDisposition.OFF, status.trafficDisposition)
    assertEquals(null, status.lastError)
  }

  @Test
  fun revokedOffStateDoesNotHideATunnelCleanupFailure() {
    val status =
        SystemRoutingStatus.derive(
            supported = true,
            desiredRevision = null,
            desiredMode = SystemRoutingMode.OFF,
            desiredSelectedApps = emptyList(),
            failClosedDesired = false,
            appliedRevision = null,
            appliedMode = SystemRoutingMode.OFF,
            appliedSelectedApps = emptyList(),
            transition = SystemRoutingTransition.REVOKED,
            tunPresent = false,
            translatorReady = false,
            coreRouteReady = false,
            alwaysOn = false,
            lockdown = false,
            lastError = SystemRoutingDiagnostic.TUNNEL_CLOSE_FAILED,
        )

    assertEquals(SystemRoutingPhase.REVOKED, status.phase)
    assertEquals(
        SystemRoutingDiagnostic.TUNNEL_CLOSE_FAILED,
        status.lastError,
    )
  }

  @Test
  fun selectedPackagesPreserveExactCaseAndRejectWhitespace() {
    val exactCase =
        selectedStatus(
            desiredApps = listOf("Com.Example.Video"),
            appliedApps = listOf("Com.Example.Video"),
        )

    assertTrue(exactCase.active)
    assertEquals(listOf("Com.Example.Video"), exactCase.desiredSelectedApps)
    try {
      selectedStatus(
          desiredApps = listOf(" Com.Example.Video"),
          appliedApps = listOf("Com.Example.Video"),
      )
      throw AssertionError("Expected a noncanonical package ID to be rejected.")
    } catch (_: IllegalArgumentException) {
      // Expected.
    }
  }

  @Test
  fun versionedStatusJsonMatchesLegacyCompatibleGoldenFixture() {
    val expected =
        requireNotNull(javaClass.getResource("/system-routing-status-v2.json"))
            .readText()
            .trim()

    assertEquals(
        expected,
        selectedStatus(
                desiredApps =
                    listOf("org.example.browser", "Com.Example.Video"),
                appliedApps =
            listOf("Com.Example.Video", "org.example.browser"),
                trafficObserved = true,
            )
            .toJson(),
    )
  }

  @Test
  fun everyCompositePhaseHasATypeScriptDecoderCompatibleLegacyPhase() {
    val legacyPhases = setOf("off", "starting", "active", "stopping", "blocked")

    assertTrue(
        SystemRoutingPhase.entries.all { phase ->
          phase.legacyWireName in legacyPhases
        })
  }

  private fun wholeDeviceStatus(
      supported: Boolean = true,
      transition: SystemRoutingTransition = SystemRoutingTransition.IDLE,
      tunPresent: Boolean = true,
      translatorReady: Boolean = true,
      coreRouteReady: Boolean = true,
      trafficObserved: Boolean = false,
      lastError: SystemRoutingDiagnostic? = null,
  ): SystemRoutingStatus =
      SystemRoutingStatus.derive(
          supported = supported,
          desiredRevision = 7L,
          desiredMode = SystemRoutingMode.WHOLE_DEVICE,
          desiredSelectedApps = emptyList(),
          failClosedDesired = true,
          appliedRevision = 7L,
          appliedMode = SystemRoutingMode.WHOLE_DEVICE,
          appliedSelectedApps = emptyList(),
          transition = transition,
          tunPresent = tunPresent,
          translatorReady = translatorReady,
          coreRouteReady = coreRouteReady,
          trafficObserved = trafficObserved,
          alwaysOn = true,
          lockdown = true,
          lastError = lastError,
      )

  private fun selectedStatus(
      desiredRevision: Long = 8L,
      appliedRevision: Long = 8L,
      desiredApps: Iterable<String> = listOf("com.example.browser"),
      appliedApps: Iterable<String> = listOf("com.example.browser"),
      alwaysOn: Boolean = true,
      lockdown: Boolean = true,
      trafficObserved: Boolean = false,
  ): SystemRoutingStatus =
      SystemRoutingStatus.derive(
          supported = true,
          desiredRevision = desiredRevision,
          desiredMode = SystemRoutingMode.SELECTED_APPS,
          desiredSelectedApps = desiredApps,
          failClosedDesired = true,
          appliedRevision = appliedRevision,
          appliedMode = SystemRoutingMode.SELECTED_APPS,
          appliedSelectedApps = appliedApps,
          transition = SystemRoutingTransition.IDLE,
          tunPresent = true,
          translatorReady = true,
          coreRouteReady = true,
          trafficObserved = trafficObserved,
          alwaysOn = alwaysOn,
          lockdown = lockdown,
      )
}
