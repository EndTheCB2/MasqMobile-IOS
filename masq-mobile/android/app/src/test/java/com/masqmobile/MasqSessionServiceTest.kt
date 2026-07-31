package com.masqmobile

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class MasqSessionServiceTest {
  @Test
  fun requiresAConnectedPeerAndRouteBeforeReportingAHealthySession() {
    val healthy = MasqSessionCoreSnapshot("connected", connectedNeighbors = 1, routeStage = 1)
    val noPeer = MasqSessionCoreSnapshot("connected", connectedNeighbors = 0, routeStage = 1)
    val noRoute = MasqSessionCoreSnapshot("connected", connectedNeighbors = 1, routeStage = 0)

    assertTrue(healthy.isHealthyConnectedSession())
    assertFalse(noPeer.isHealthyConnectedSession())
    assertFalse(noRoute.isHealthyConnectedSession())
  }

  @Test
  fun notificationCopyIsEnglishAndContainsNoConnectionIdentifiers() {
    MasqSessionNotificationState.values().forEach { state ->
      val text = masqSessionNotificationText(state)
      assertFalse(text.contains("wallet", ignoreCase = true))
      assertFalse(text.contains("peer", ignoreCase = true))
      assertFalse(text.contains("node", ignoreCase = true))
      assertFalse(text.contains("address", ignoreCase = true))
      assertFalse(text.contains(Regex("\\b\\d{1,3}(?:\\.\\d{1,3}){3}\\b")))
    }
  }
}
