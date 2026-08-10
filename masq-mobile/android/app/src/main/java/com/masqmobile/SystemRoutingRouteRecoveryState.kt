package com.masqmobile

/** Exact native-core route authority used to (re)bind the packet translator. */
internal data class SystemRoutingCoreRouteIdentity(
    val proxyPort: Int,
    val coreGeneration: Long,
    val engineGeneration: Long,
    val networkEpoch: Long,
) {
  init {
    require(proxyPort in 1..65535)
    require(coreGeneration > 0)
    require(engineGeneration > 0)
    require(networkEpoch > 0)
  }
}

internal data class SystemRoutingRouteLossAction(
    val lossEpoch: Long,
)

internal data class SystemRoutingRouteRecoveryAction(
    val lossEpoch: Long,
    val identity: SystemRoutingCoreRouteIdentity,
)

/**
 * Serializes route-loss and recovery intent before work is submitted to VpnService's control
 * executor.
 *
 * A loss epoch advances when an observation invalidates a proven route or there is captured/queued
 * work to block. Repeated unhealthy observations therefore do not create an unbounded stop queue.
 * A recovery is unique for the tuple (loss epoch, proxy port, core generation, engine generation,
 * Android network epoch), while a later loss makes an already-queued recovery stale. Callers
 * submit the returned actions while holding their own dispatch lock so executor order agrees with
 * the order observed here.
 */
internal class SystemRoutingRouteRecoveryState {
  private var lossEpoch = 0L
  private var pendingLoss: SystemRoutingRouteLossAction? = null
  private var provenRoute: SystemRoutingCoreRouteIdentity? = null
  private val pendingRecoveries = mutableSetOf<SystemRoutingRouteRecoveryAction>()
  private var failedRecoveryPreflight: SystemRoutingRouteRecoveryAction? = null

  fun observeRouteLoss(
      captureOrRecoveryNeedsBlocking: Boolean,
  ): SystemRoutingRouteLossAction? {
    val invalidatesProvenRoute = provenRoute != null
    provenRoute = null
    if (invalidatesProvenRoute ||
        (captureOrRecoveryNeedsBlocking && pendingLoss == null)) {
      check(lossEpoch < Long.MAX_VALUE) {
        "The MASQ system-routing route-loss epoch is exhausted."
      }
      lossEpoch += 1L
      failedRecoveryPreflight = null
    }
    if (!captureOrRecoveryNeedsBlocking || pendingLoss != null) return null
    return SystemRoutingRouteLossAction(lossEpoch).also { pendingLoss = it }
  }

  fun completeRouteLoss(action: SystemRoutingRouteLossAction) {
    if (pendingLoss == action) {
      pendingLoss = null
    }
  }

  fun observeProvenRoute(
      identity: SystemRoutingCoreRouteIdentity,
      routeAlreadyExact: Boolean,
  ): SystemRoutingRouteRecoveryAction? {
    provenRoute = identity
    if (routeAlreadyExact) return null
    val action = SystemRoutingRouteRecoveryAction(lossEpoch, identity)
    if (failedRecoveryPreflight?.identity != identity ||
        failedRecoveryPreflight?.lossEpoch != lossEpoch) {
      failedRecoveryPreflight = null
    }
    if (failedRecoveryPreflight == action) return null
    return if (pendingRecoveries.add(action)) action else null
  }

  fun recoveryIsCurrent(action: SystemRoutingRouteRecoveryAction): Boolean =
      action.lossEpoch == lossEpoch && provenRoute == action.identity

  fun completeRecovery(action: SystemRoutingRouteRecoveryAction) {
    pendingRecoveries.remove(action)
  }

  fun recordFailedRecoveryPreflight(action: SystemRoutingRouteRecoveryAction): Boolean {
    if (!recoveryIsCurrent(action)) return false
    failedRecoveryPreflight = action
    return true
  }

  fun hasPendingRecovery(): Boolean = pendingRecoveries.isNotEmpty()
}
