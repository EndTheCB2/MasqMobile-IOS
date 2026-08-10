package com.masqmobile

internal data class TunnelStartStatusSnapshot(
    val active: Boolean,
    val routingPhase: String,
    val desiredRevision: Long,
    val appliedRevision: Long,
    val tunPresent: Boolean,
    val translatorReady: Boolean,
    val coreRouteReady: Boolean,
)

/**
 * Semantic authority returned to the VPN service after the bridge wins its
 * one-shot PendingTunnelStart settlement.
 *
 * Claiming the callback is not sufficient: the exact policy and core
 * generation must still be current, and every ACTIVE health bit must belong
 * to that same candidate.
 */
internal fun tunnelStartAcknowledgementIsSemanticallyAccepted(
    status: TunnelStartStatusSnapshot?,
    error: String?,
    expectedPolicyRevision: Long,
    expectedCoreGeneration: Long,
    currentCoreGeneration: Long,
): Boolean =
    error == null &&
        status != null &&
        expectedCoreGeneration == currentCoreGeneration &&
        status.active &&
        status.routingPhase == "active" &&
        status.desiredRevision == expectedPolicyRevision &&
        status.appliedRevision == expectedPolicyRevision &&
        status.tunPresent &&
        status.translatorReady &&
        status.coreRouteReady
