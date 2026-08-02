package com.masqmobile

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class SystemRoutingCoreReadinessTest {
  @Test
  fun requiresPeerRouteProgressAndAValidProxyPort() {
    val ready = status()

    assertTrue(ready.ready)
    assertFalse(status(connectedNeighbors = 0).ready)
    assertFalse(status(routeStage = 0).ready)
    assertFalse(status(proxyPort = 0).ready)
    assertFalse(status(engineGeneration = 0).ready)
    assertFalse(status(phase = "connecting").ready)
  }

  @Test
  fun exactRouteRejectsAStaleProxyPort() {
    val ready = status()
    assertTrue(systemRoutingCoreRouteIsExact(ready, 44_443, 3))
    assertFalse(systemRoutingCoreRouteIsExact(ready, 44_444, 3))
    assertFalse(systemRoutingCoreRouteIsExact(ready, 44_443, 4))
  }

  private fun status(
      phase: String = "connected",
      connectedNeighbors: Int = 1,
      routeStage: Int = 1,
      proxyPort: Int = 44_443,
      engineGeneration: Long = 3,
  ): SystemRoutingCoreReadiness =
      systemRoutingCoreReadiness(
          phase,
          connectedNeighbors,
          routeStage,
          proxyPort,
          engineGeneration,
      )
}
