package com.masqmobile

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class SystemRoutingStartAuthorityTest {
  @Test
  fun exactCandidateAndCoreGenerationAreSemanticallyAccepted() {
    assertTrue(
        tunnelStartAcknowledgementIsSemanticallyAccepted(
            status = exactStatus(),
            error = null,
            expectedPolicyRevision = 71,
            expectedCoreGeneration = 9,
            currentCoreGeneration = 9,
        ))
  }

  @Test
  fun coreGenerationChangeAfterServiceCandidateCreationRejectsAcknowledgement() {
    assertFalse(
        tunnelStartAcknowledgementIsSemanticallyAccepted(
            status = exactStatus(),
            error = null,
            expectedPolicyRevision = 71,
            expectedCoreGeneration = 9,
            currentCoreGeneration = 10,
        ))
  }

  @Test
  fun callbackClaimWithoutExactActiveHealthIsNotSemanticAcceptance() {
    assertFalse(
        tunnelStartAcknowledgementIsSemanticallyAccepted(
            status = exactStatus().copy(coreRouteReady = false),
            error = null,
            expectedPolicyRevision = 71,
            expectedCoreGeneration = 9,
            currentCoreGeneration = 9,
        ))
    assertFalse(
        tunnelStartAcknowledgementIsSemanticallyAccepted(
            status = exactStatus(),
            error = "expired",
            expectedPolicyRevision = 71,
            expectedCoreGeneration = 9,
            currentCoreGeneration = 9,
        ))
  }

  private fun exactStatus() =
      TunnelStartStatusSnapshot(
          active = true,
          routingPhase = "active",
          desiredRevision = 71,
          appliedRevision = 71,
          tunPresent = true,
          translatorReady = true,
          coreRouteReady = true,
      )
}
