package com.masqmobile

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class MasqSessionServiceTest {
  @Test
  fun requiresAConnectedPeerAndRouteBeforeReportingAHealthySession() {
    val healthy =
        MasqSessionCoreSnapshot(
            "connected",
            connectedNeighbors = 1,
            routeStage = 1,
            proxyPort = 44_443,
            engineGeneration = 3,
        )
    val noPeer = healthy.copy(connectedNeighbors = 0)
    val noRoute = healthy.copy(routeStage = 0)
    val noProxy = healthy.copy(proxyPort = 0)
    val noEngine = healthy.copy(engineGeneration = 0)

    assertTrue(healthy.isHealthyConnectedSession())
    assertFalse(noPeer.isHealthyConnectedSession())
    assertFalse(noRoute.isHealthyConnectedSession())
    assertFalse(noProxy.isHealthyConnectedSession())
    assertFalse(noEngine.isHealthyConnectedSession())
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
