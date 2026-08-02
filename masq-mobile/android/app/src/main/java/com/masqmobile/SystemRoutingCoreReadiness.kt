package com.masqmobile

import org.json.JSONObject

internal data class SystemRoutingCoreReadiness(
    val ready: Boolean,
    val proxyPort: Int,
    val engineGeneration: Long,
)

internal fun systemRoutingCoreReadiness(status: JSONObject): SystemRoutingCoreReadiness {
  return systemRoutingCoreReadiness(
      phase = status.optString("phase"),
      connectedNeighbors = status.optInt("connectedNeighbors", 0),
      routeStage = status.optInt("routeStage", 0),
      proxyPort = status.optInt("proxyPort", 0),
      engineGeneration = status.optLong("engineGeneration", 0L),
  )
}

internal fun systemRoutingCoreReadiness(
    phase: String,
    connectedNeighbors: Int,
    routeStage: Int,
    proxyPort: Int,
    engineGeneration: Long,
): SystemRoutingCoreReadiness {
  return SystemRoutingCoreReadiness(
      ready =
          phase == "connected" &&
              connectedNeighbors > 0 &&
              routeStage > 0 &&
              proxyPort in 1..65535 &&
              engineGeneration > 0,
      proxyPort = proxyPort,
      engineGeneration = engineGeneration,
  )
}

internal fun systemRoutingCoreRouteIsExact(
    status: JSONObject,
    expectedProxyPort: Int,
    expectedEngineGeneration: Long,
): Boolean {
  return systemRoutingCoreRouteIsExact(
      systemRoutingCoreReadiness(status),
      expectedProxyPort,
      expectedEngineGeneration,
  )
}

internal fun systemRoutingCoreRouteIsExact(
    readiness: SystemRoutingCoreReadiness,
    expectedProxyPort: Int,
    expectedEngineGeneration: Long,
): Boolean {
  return readiness.ready &&
      readiness.proxyPort == expectedProxyPort &&
      readiness.engineGeneration == expectedEngineGeneration
}
