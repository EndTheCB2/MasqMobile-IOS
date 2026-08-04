package com.masqmobile

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class MasqSessionServiceTest {
  @Test
  fun moduleStartAdmissionCompletesExactlyOnce() {
    val gate = MasqModuleStartAdmissionGate()

    assertTrue(gate.complete(MasqModuleStartAdmissionDecision.ACCEPTED))
    assertFalse(gate.complete(MasqModuleStartAdmissionDecision.SERVICE_TAKEOVER))
    assertEquals(MasqModuleStartAdmissionDecision.ACCEPTED, gate.await(50L))
  }

  @Test
  fun serviceTakeoverAdmissionStopsTheModuleDiscoveryCaller() {
    val gate = MasqModuleStartAdmissionGate()

    assertTrue(gate.complete(MasqModuleStartAdmissionDecision.SERVICE_TAKEOVER))
    assertFalse(gate.await(50L).allowsModuleDiscovery())
    assertTrue(MasqModuleStartAdmissionDecision.ACCEPTED.allowsModuleDiscovery())
    assertTrue(MasqModuleStartAdmissionDecision.TIMED_OUT.allowsModuleDiscovery())
  }

  @Test
  fun missingAdmissionDecisionTimesOutForLiveness() {
    val gate = MasqModuleStartAdmissionGate()

    assertEquals(MasqModuleStartAdmissionDecision.TIMED_OUT, gate.await(5L))
  }

  @Test
  fun completedTakeoverAdmissionAvoidsASecondGenerationAdvance() {
    assertFalse(
        shouldInvalidateForegroundStartAfterServiceTakeover(
            admissionCompleted = true,
        ))
    assertTrue(
        shouldInvalidateForegroundStartAfterServiceTakeover(
            admissionCompleted = false,
        ))
  }

  @Test
  fun vpnPreflightEscalationIsExactDeduplicatedAndFailClosed() {
    val exact =
        MasqCoreRouteRestartEscalationScope(
            startGeneration = 41L,
            engineGeneration = 9L,
            networkEpoch = 7L,
        )

    assertEquals(
        MasqCoreRouteRestartEscalationDecision.SCHEDULE,
        masqCoreRouteRestartEscalationDecision(
            existing = null,
            requested = exact,
            currentStartGeneration = 41L,
            currentNetworkEpoch = 7L,
            networkAvailable = true,
        ),
    )
    assertEquals(
        MasqCoreRouteRestartEscalationDecision.DEDUPLICATE,
        masqCoreRouteRestartEscalationDecision(
            existing = exact,
            requested = exact,
            currentStartGeneration = 41L,
            currentNetworkEpoch = 7L,
            networkAvailable = true,
        ),
    )
    assertEquals(
        MasqCoreRouteRestartEscalationDecision.REJECT_STALE,
        masqCoreRouteRestartEscalationDecision(
            existing = null,
            requested = exact,
            currentStartGeneration = 42L,
            currentNetworkEpoch = 7L,
            networkAvailable = true,
        ),
    )
    assertEquals(
        MasqCoreRouteRestartEscalationDecision.REJECT_STALE,
        masqCoreRouteRestartEscalationDecision(
            existing = null,
            requested = exact,
            currentStartGeneration = 41L,
            currentNetworkEpoch = 8L,
            networkAvailable = true,
        ),
    )
    assertEquals(
        MasqCoreRouteRestartEscalationDecision.REJECT_STALE,
        masqCoreRouteRestartEscalationDecision(
            existing = null,
            requested = exact,
            currentStartGeneration = 41L,
            currentNetworkEpoch = 7L,
            networkAvailable = false,
        ),
    )
  }

  @Test
  fun vpnEscalationShutsDownOnlyTheExactStillHealthyEngine() {
    val healthy =
        MasqSessionCoreSnapshot(
            phase = "connected",
            connectedNeighbors = 1,
            routeStage = 2,
            proxyPort = 44_443,
            engineGeneration = 9L,
        )

    assertEquals(
        MasqCoreRouteRestartNativeAction.SHUTDOWN_EXACT_HEALTHY_ENGINE,
        masqCoreRouteRestartNativeAction(healthy, expectedEngineGeneration = 9L),
    )
    assertEquals(
        MasqCoreRouteRestartNativeAction.RECOVER_UNHEALTHY_ENGINE,
        masqCoreRouteRestartNativeAction(
            healthy.copy(phase = "connecting", routeStage = 1),
            expectedEngineGeneration = 9L,
        ),
    )
    assertEquals(
        MasqCoreRouteRestartNativeAction.IGNORE_SUPERSEDED_ENGINE,
        masqCoreRouteRestartNativeAction(
            healthy.copy(engineGeneration = 10L),
            expectedEngineGeneration = 9L,
        ),
    )
  }

  @Test
  fun preservesTheSafeCoreErrorCodeForRecoveryClassification() {
    val snapshot =
        masqSessionCoreSnapshot(
            """{"phase":"error","connectedNeighbors":0,"routeStage":0,"proxyPort":0,"engineGeneration":3,"lastError":"E_ENTRY_TCP_FAILED: redacted"}""")

    assertEquals("E_ENTRY_TCP_FAILED: redacted", snapshot?.lastError)
  }

  @Test
  fun parsesThePrivacySafeRouteProofGeneration() {
    val snapshot =
        masqSessionCoreSnapshot(
            """{"phase":"connected","connectedNeighbors":1,"routeStage":2,"proxyPort":44443,"engineGeneration":3,"routeProofGeneration":9}""")

    assertEquals(9L, snapshot?.routeProofGeneration)
  }

  @Test
  fun parsesOnlyTheStructuredRouteProofRefreshResult() {
    val snapshot =
        masqSessionCoreSnapshot(
            """{"phase":"connected","connectedNeighbors":1,"routeStage":2,"proxyPort":44443,"engineGeneration":3,"routeProofGeneration":9,"routeProofRefresh":{"attempted":true,"succeeded":false,"errorCode":"E_PRIVATE_ROUTE_REFRESH_FAILED"}}""")

    assertEquals(
        MasqRouteProofRefreshResult(
            attempted = true,
            succeeded = false,
            errorCode = "E_PRIVATE_ROUTE_REFRESH_FAILED",
        ),
        snapshot?.routeProofRefresh,
    )
    val unknownCode =
        masqSessionCoreSnapshot(
            """{"phase":"connected","connectedNeighbors":1,"routeStage":2,"proxyPort":44443,"engineGeneration":3,"routeProofRefresh":{"attempted":true,"succeeded":false,"errorCode":"E_PRIVATE_ROUTE_REFRESH_FAILED: raw detail"}}""")
    assertEquals(null, unknownCode?.routeProofRefresh?.errorCode)
  }

  @Test
  fun initialStageOneErrorIsClassifiedAsAShortRouteProofDeprioritization() {
    val snapshot =
        masqSessionCoreSnapshot(
            """{"phase":"error","connectedNeighbors":1,"routeStage":1,"proxyPort":44443,"engineGeneration":3,"lastError":"E_PRIVATE_ROUTE_TIMEOUT: redacted"}""")!!

    assertTrue(
        shouldDeprioritizeAttemptedEntryNodes(
            phase = snapshot.phase,
            engineGeneration = snapshot.engineGeneration,
            routeStage = snapshot.routeStage,
            lastError = snapshot.lastError,
        ))
  }

  @Test
  fun requiresAConnectedPeerAndRouteBeforeReportingAHealthySession() {
    val healthy =
        MasqSessionCoreSnapshot(
            "connected",
            connectedNeighbors = 1,
            routeStage = 2,
            proxyPort = 44_443,
            engineGeneration = 3,
        )
    val noPeer = healthy.copy(connectedNeighbors = 0)
    val noRoute = healthy.copy(routeStage = 0)
    val entryOnly = healthy.copy(phase = "connecting", routeStage = 1)
    val noProxy = healthy.copy(proxyPort = 0)
    val noEngine = healthy.copy(engineGeneration = 0)

    assertTrue(healthy.isHealthyConnectedSession())
    assertFalse(noPeer.isHealthyConnectedSession())
    assertFalse(noRoute.isHealthyConnectedSession())
    assertFalse(entryOnly.isHealthyConnectedSession())
    assertTrue(entryOnly.isEntryConnectedAwaitingRoute())
    assertFalse(noProxy.isHealthyConnectedSession())
    assertFalse(noEngine.isHealthyConnectedSession())
  }

  @Test
  fun lastErrorInvalidatesOtherwiseHealthyAndStageOneSnapshots() {
    val healthy =
        MasqSessionCoreSnapshot(
            phase = "connected",
            connectedNeighbors = 1,
            routeStage = 2,
            proxyPort = 44_443,
            engineGeneration = 3,
            lastError = "E_PRIVATE_ROUTE_TIMEOUT: stale",
        )
    val stageOne =
        healthy.copy(
            phase = "connecting",
            routeStage = 1,
            lastError = "E_ENTRY_NO_PROGRESS: stale",
        )

    assertFalse(healthy.isHealthyConnectedSession())
    assertFalse(stageOne.isEntryConnectedAwaitingRoute())
  }

  @Test
  fun routeDegradationIsNotReportedAsAHealthyBackgroundSession() {
    val validatedRoute =
        MasqSessionCoreSnapshot(
            phase = "connected",
            connectedNeighbors = 1,
            routeStage = 2,
            proxyPort = 44_443,
            engineGeneration = 3,
        )
    val degradedRoute = validatedRoute.copy(phase = "connecting", routeStage = 1)

    assertTrue(validatedRoute.isHealthyConnectedSession())
    assertFalse(degradedRoute.isHealthyConnectedSession())
    assertTrue(degradedRoute.isEntryConnectedAwaitingRoute())
  }

  @Test
  fun exactPreviouslyHealthyRouteOnTheSameNetworkUsesFastRebuild() {
    val degradedRoute =
        MasqSessionCoreSnapshot(
            phase = "connecting",
            connectedNeighbors = 1,
            routeStage = 1,
            proxyPort = 44_443,
            engineGeneration = 3,
        )
    val source = MasqSessionActiveRouteSource(networkId = 41L, engineGeneration = 3L)

    assertTrue(
        shouldFastRebuildPreviouslyHealthyRoute(
            snapshot = degradedRoute,
            activeRouteSource = source,
            currentNetworkId = 41L,
        ))
    assertTrue(
        shouldFastRebuildPreviouslyHealthyRoute(
            snapshot = degradedRoute.copy(connectedNeighbors = 0, routeStage = 0),
            activeRouteSource = source,
            currentNetworkId = 41L,
        ))
    assertFalse(
        shouldFastRebuildPreviouslyHealthyRoute(
            snapshot = degradedRoute.copy(engineGeneration = 4L),
            activeRouteSource = source,
            currentNetworkId = 41L,
        ))
    assertFalse(
        shouldFastRebuildPreviouslyHealthyRoute(
            snapshot = degradedRoute,
            activeRouteSource = source,
            currentNetworkId = 42L,
        ))
    assertFalse(
        shouldFastRebuildPreviouslyHealthyRoute(
            snapshot = degradedRoute.copy(phase = "connected", routeStage = 2),
            activeRouteSource = source,
            currentNetworkId = 41L,
        ))
    assertFalse(
        shouldFastRebuildPreviouslyHealthyRoute(
            snapshot = degradedRoute.copy(lastError = "E_PRIVATE_ROUTE_FAILED: redacted"),
            activeRouteSource = source,
            currentNetworkId = 41L,
        ))
    assertFalse(
        shouldFastRebuildPreviouslyHealthyRoute(
            snapshot = degradedRoute,
            activeRouteSource = null,
            currentNetworkId = 41L,
        ))
  }

  @Test
  fun packageReplacementRestoresOnlyDurableSystemRoutingSessions() {
    assertTrue(
        shouldRestoreMasqSessionAfterPackageReplacement(
            SystemRoutingPolicyLoadResult.Ready(
                DesiredSystemRoutingPolicy(
                    schemaVersion = SystemRoutingPolicyStore.CURRENT_SCHEMA_VERSION,
                    revision = 1L,
                    desiredMode = SystemRoutingMode.WHOLE_DEVICE,
                    selectedApps = emptyList(),
                    explicitConsentTimestampMs = 1L,
                    failClosedDesired = true,
                ))))
    assertFalse(
        shouldRestoreMasqSessionAfterPackageReplacement(
            SystemRoutingPolicyLoadResult.Missing))
    assertFalse(
        shouldRestoreMasqSessionAfterPackageReplacement(
            SystemRoutingPolicyLoadResult.ExplicitOff(DesiredSystemRoutingPolicy.off(2L))))
    assertFalse(
        shouldRestoreMasqSessionAfterPackageReplacement(
            SystemRoutingPolicyLoadResult.BlockRequired(
                SystemRoutingDiagnostic.CORRUPT_OR_PARTIAL_POLICY)))
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

  @Test
  fun schedulesAnIdleProofBeforeTheNativeFiveMinuteLeaseExpires() {
    val healthy =
        MasqSessionCoreSnapshot(
            phase = "connected",
            connectedNeighbors = 1,
            routeStage = 2,
            proxyPort = 44_443,
            engineGeneration = 3,
            routeProofGeneration = 1,
        )
    val first =
        MasqRouteProofRefreshSchedule().afterSnapshot(
            healthy,
            currentStartGeneration = 7L,
            nowElapsed = 1_000L,
            refreshAttempted = false,
        )

    assertFalse(
        first.isDue(
            1_000L + ROUTE_PROOF_REFRESH_INTERVAL_MILLIS - 1L,
            currentStartGeneration = 7L,
        ))
    assertTrue(
        first.isDue(
            1_000L + ROUTE_PROOF_REFRESH_INTERVAL_MILLIS,
            currentStartGeneration = 7L,
        ))
    assertFalse(
        first.isDue(
            1_000L + ROUTE_PROOF_REFRESH_INTERVAL_MILLIS,
            currentStartGeneration = 8L,
        ))
    assertTrue(ROUTE_PROOF_REFRESH_INTERVAL_MILLIS < 5 * 60_000L)
  }

  @Test
  fun allScheduledProofAttemptsFitInsideTheNativeReadinessLease() {
    val nativeReadinessLeaseMillis = 5 * 60_000L
    // One native probe deadline plus the bounded actor acknowledgement.
    val nativeSingleAttemptBudgetMillis = 12_750L
    val monitorJitterBudgetMillis = 5_000L
    val threeAttemptEscalationBudget =
        ROUTE_PROOF_REFRESH_INTERVAL_MILLIS +
            (3 * nativeSingleAttemptBudgetMillis) +
            ROUTE_PROOF_REFRESH_RETRY_INITIAL_MILLIS +
            (2 * ROUTE_PROOF_REFRESH_RETRY_INITIAL_MILLIS) +
            (4 * monitorJitterBudgetMillis)

    assertEquals(3 * 60_000L, ROUTE_PROOF_REFRESH_INTERVAL_MILLIS)
    assertTrue(threeAttemptEscalationBudget < nativeReadinessLeaseMillis)
  }

  @Test
  fun realRouteActivityPostponesTheIdleProofAndFailureClearsIt() {
    val healthy =
        MasqSessionCoreSnapshot(
            phase = "connected",
            connectedNeighbors = 1,
            routeStage = 2,
            proxyPort = 44_443,
            engineGeneration = 3,
            routeProofGeneration = 1,
        )
    val first =
        MasqRouteProofRefreshSchedule().afterSnapshot(healthy, 7L, 1_000L, false)
    val unchanged = first.afterSnapshot(healthy, 7L, 2_000L, false)
    val advanced =
        unchanged.afterSnapshot(
            healthy.copy(routeProofGeneration = 2),
            7L,
            3_000L,
            false,
        )

    assertEquals(first, unchanged)
    assertEquals(3_000L + ROUTE_PROOF_REFRESH_INTERVAL_MILLIS, advanced.deadlineElapsed)
    assertEquals(
        MasqRouteProofRefreshSchedule(),
        advanced.afterSnapshot(healthy.copy(routeStage = 1), 7L, 4_000L, false),
    )
  }

  @Test
  fun aSuccessfulScheduledRefreshMovesTheDeadlineEvenWithALegacyCounter() {
    val legacyHealthy =
        MasqSessionCoreSnapshot(
            phase = "connected",
            connectedNeighbors = 1,
            routeStage = 2,
            proxyPort = 44_443,
            engineGeneration = 3,
        )
    val first =
        MasqRouteProofRefreshSchedule().afterSnapshot(legacyHealthy, 7L, 1_000L, false)
    val refreshed =
        first.afterSnapshot(
            legacyHealthy.copy(
                routeProofRefresh =
                    MasqRouteProofRefreshResult(
                        attempted = true,
                        succeeded = true,
                        errorCode = null,
                    )),
            7L,
            1_000L + ROUTE_PROOF_REFRESH_INTERVAL_MILLIS,
            true,
        )

    assertEquals(
        1_000L + 2 * ROUTE_PROOF_REFRESH_INTERVAL_MILLIS,
        refreshed.deadlineElapsed,
    )
  }

  @Test
  fun routeProofCountersAdvanceOnlyInsideTheSameStartAndEngineGeneration() {
    val healthy =
        MasqSessionCoreSnapshot(
            phase = "connected",
            connectedNeighbors = 1,
            routeStage = 2,
            proxyPort = 44_443,
            engineGeneration = 3,
            routeProofGeneration = 9,
        )
    val first =
        MasqRouteProofRefreshSchedule().afterSnapshot(healthy, 7L, 1_000L, false)
    val decreased =
        first.afterSnapshot(healthy.copy(routeProofGeneration = 8), 7L, 2_000L, false)
    val newEngine =
        decreased.afterSnapshot(
            healthy.copy(engineGeneration = 4, routeProofGeneration = 1),
            7L,
            3_000L,
            false,
        )
    val newStart =
        newEngine.afterSnapshot(
            healthy.copy(engineGeneration = 4, routeProofGeneration = 1),
            8L,
            4_000L,
            false,
        )

    assertEquals(first, decreased)
    assertEquals(4L, newEngine.engineGeneration)
    assertEquals(1L, newEngine.observedGeneration)
    assertEquals(3_000L + ROUTE_PROOF_REFRESH_INTERVAL_MILLIS, newEngine.deadlineElapsed)
    assertEquals(8L, newStart.startGeneration)
    assertEquals(4_000L + ROUTE_PROOF_REFRESH_INTERVAL_MILLIS, newStart.deadlineElapsed)
  }

  @Test
  fun failedRouteProofRefreshUsesABoundedShortRetryWithoutRearmingTheLease() {
    val healthy =
        MasqSessionCoreSnapshot(
            phase = "connected",
            connectedNeighbors = 1,
            routeStage = 2,
            proxyPort = 44_443,
            engineGeneration = 3,
            routeProofGeneration = 1,
        )
    val failedRefresh =
        healthy.copy(
            routeProofRefresh =
                MasqRouteProofRefreshResult(
                    attempted = true,
                    succeeded = false,
                    errorCode = "E_PRIVATE_ROUTE_REFRESH_FAILED",
                ))
    val first =
        MasqRouteProofRefreshSchedule().afterSnapshot(healthy, 7L, 1_000L, false)
    val failedOnce = first.afterSnapshot(failedRefresh, 7L, 2_000L, true)
    val failedTwice = failedOnce.afterSnapshot(failedRefresh, 7L, 3_000L, true)
    val failedThrice = failedTwice.afterSnapshot(failedRefresh, 7L, 4_000L, true)
    val stillBounded = failedThrice.afterSnapshot(failedRefresh, 7L, 5_000L, true)
    val missingResult = first.afterSnapshot(null, 7L, 6_000L, true)
    val explicitRouteLoss =
        first.afterSnapshot(
            failedRefresh.copy(phase = "connecting", routeStage = 1),
            7L,
            7_000L,
            true,
        )
    val advancedDespiteFailedProbe =
        first.afterSnapshot(
            failedRefresh.copy(routeProofGeneration = 2),
            7L,
            8_000L,
            true,
        )

    assertEquals(2_000L + ROUTE_PROOF_REFRESH_RETRY_INITIAL_MILLIS, failedOnce.deadlineElapsed)
    assertEquals(3_000L + 30_000L, failedTwice.deadlineElapsed)
    assertEquals(4_000L + ROUTE_PROOF_REFRESH_RETRY_MAX_MILLIS, failedThrice.deadlineElapsed)
    assertEquals(5_000L + ROUTE_PROOF_REFRESH_RETRY_MAX_MILLIS, stillBounded.deadlineElapsed)
    assertEquals(6_000L + ROUTE_PROOF_REFRESH_RETRY_INITIAL_MILLIS, missingResult.deadlineElapsed)
    assertEquals(MasqRouteProofRefreshSchedule(), explicitRouteLoss)
    assertEquals(
        8_000L + ROUTE_PROOF_REFRESH_INTERVAL_MILLIS,
        advancedDespiteFailedProbe.deadlineElapsed,
    )
    assertEquals(0, advancedDespiteFailedProbe.consecutiveFailures)
    assertFalse(first.refreshSucceeded(failedRefresh, 7L))
  }

  @Test
  fun periodicRouteProofFailureEscalatesOnlyAfterThreeFailuresInTheSameScope() {
    val scoped =
        MasqRouteProofRefreshSchedule(
            startGeneration = 7L,
            engineGeneration = 3L,
            observedGeneration = 1L,
            deadlineElapsed = 20_000L,
        )
    fun action(
        schedule: MasqRouteProofRefreshSchedule,
        attempted: Boolean = true,
        forced: Boolean = false,
        succeeded: Boolean = false,
        nonMutatingFailure: Boolean = true,
        startGeneration: Long = 7L,
    ) =
        masqPeriodicRouteProofFailureAction(
            routeProofRefreshAttempted = attempted,
            forcedNetworkRouteProof = forced,
            refreshSucceeded = succeeded,
            nonMutatingRefreshFailure = nonMutatingFailure,
            schedule = schedule,
            currentStartGeneration = startGeneration,
        )

    assertEquals(
        MasqPeriodicRouteProofFailureAction.RETAIN_ROUTE,
        action(scoped.copy(consecutiveFailures = 1)),
    )
    assertEquals(
        MasqPeriodicRouteProofFailureAction.RETAIN_ROUTE,
        action(scoped.copy(consecutiveFailures = 2)),
    )
    assertEquals(
        MasqPeriodicRouteProofFailureAction.FAIL_CLOSED_RESTART,
        action(scoped.copy(consecutiveFailures = 3)),
    )
    assertEquals(
        MasqPeriodicRouteProofFailureAction.RETAIN_ROUTE,
        action(scoped.copy(consecutiveFailures = 3), forced = true),
    )
    assertEquals(
        MasqPeriodicRouteProofFailureAction.RETAIN_ROUTE,
        action(scoped.copy(consecutiveFailures = 3), succeeded = true),
    )
    assertEquals(
        MasqPeriodicRouteProofFailureAction.RETAIN_ROUTE,
        action(scoped.copy(consecutiveFailures = 3), attempted = false),
    )
    assertEquals(
        MasqPeriodicRouteProofFailureAction.RETAIN_ROUTE,
        action(scoped.copy(consecutiveFailures = 3), nonMutatingFailure = false),
    )
    assertEquals(
        MasqPeriodicRouteProofFailureAction.RETAIN_ROUTE,
        action(scoped.copy(consecutiveFailures = 3), startGeneration = 8L),
    )

    val healthyWithoutStructuredResult =
        MasqSessionCoreSnapshot(
            phase = "connected",
            connectedNeighbors = 1,
            routeStage = 2,
            proxyPort = 44_443,
            engineGeneration = 3L,
        )
    assertTrue(
        isNonMutatingRouteProofRefreshFailure(
            routeProofRefreshAttempted = true,
            refreshSucceeded = false,
            snapshot = healthyWithoutStructuredResult,
        ))
    assertTrue(
        isNonMutatingRouteProofRefreshFailure(
            routeProofRefreshAttempted = true,
            refreshSucceeded = false,
            snapshot = null,
        ))
    assertFalse(
        isNonMutatingRouteProofRefreshFailure(
            routeProofRefreshAttempted = true,
            refreshSucceeded = true,
            snapshot = healthyWithoutStructuredResult,
        ))
    assertFalse(
        isNonMutatingRouteProofRefreshFailure(
            routeProofRefreshAttempted = true,
            refreshSucceeded = false,
            snapshot = healthyWithoutStructuredResult.copy(phase = "connecting", routeStage = 1),
        ))
  }

  @Test
  fun periodicRouteProofRestartIsFencedToTheExactSessionStartEngineAndNetwork() {
    val scope =
        MasqPeriodicRouteProofRestartScope(
            sessionGeneration = 11L,
            startGeneration = 7L,
            engineGeneration = 3L,
            networkEpoch = 4L,
        )
    fun applies(
        sessionGeneration: Long = 11L,
        startGeneration: Long = 7L,
        networkEpoch: Long = 4L,
        networkAvailable: Boolean = true,
    ) =
        scope.applies(
            currentSessionGeneration = sessionGeneration,
            currentStartGeneration = startGeneration,
            currentNetworkEpoch = networkEpoch,
            networkAvailable = networkAvailable,
        )

    assertTrue(applies())
    assertFalse(applies(sessionGeneration = 12L))
    assertFalse(applies(startGeneration = 8L))
    assertFalse(applies(networkEpoch = 5L))
    assertFalse(applies(networkAvailable = false))

    val paused =
        MasqSessionCoreSnapshot(
            phase = "paused",
            connectedNeighbors = 0,
            routeStage = 0,
            engineGeneration = 3L,
        )
    assertTrue(isSuccessfulPeriodicRouteProofRestartSnapshot(paused, 3L))
    assertFalse(isSuccessfulPeriodicRouteProofRestartSnapshot(paused, 4L))
    assertFalse(
        isSuccessfulPeriodicRouteProofRestartSnapshot(
            paused.copy(phase = "connected", connectedNeighbors = 1, routeStage = 2),
            3L,
        ))
    assertFalse(isSuccessfulPeriodicRouteProofRestartSnapshot(null, 3L))
  }

  @Test
  fun discardsSessionSnapshotsWhenLifecycleEngineOrRefreshNetworkChanges() {
    val healthy =
        MasqSessionCoreSnapshot(
            phase = "connected",
            connectedNeighbors = 1,
            routeStage = 2,
            proxyPort = 44_443,
            engineGeneration = 3,
        )
    fun applies(
        currentSession: Long = 11L,
        completedStart: Long = 7L,
        currentStart: Long = 7L,
        refresh: Boolean = true,
        expectedEngine: Long = 3L,
        snapshot: MasqSessionCoreSnapshot? = healthy,
        network: Boolean = true,
        monitoredNetworkEpoch: Long = 4L,
        currentNetworkEpoch: Long = 4L,
    ): Boolean =
        shouldApplyMasqSessionSnapshot(
            monitoredSessionGeneration = 11L,
            currentSessionGeneration = currentSession,
            monitoredStartGeneration = 7L,
            completedStartGeneration = completedStart,
            currentStartGeneration = currentStart,
            refreshRouteProof = refresh,
            expectedRefreshEngineGeneration = expectedEngine,
            snapshot = snapshot,
            networkAvailable = network,
            monitoredNetworkEpoch = monitoredNetworkEpoch,
            currentNetworkEpoch = currentNetworkEpoch,
        )

    assertTrue(applies())
    assertFalse(applies(currentSession = 12L))
    assertFalse(applies(completedStart = 8L))
    assertFalse(applies(currentStart = 8L))
    assertFalse(applies(expectedEngine = 4L))
    assertFalse(applies(network = false))
    assertFalse(applies(monitoredNetworkEpoch = 3L))
    assertTrue(applies(snapshot = null))
    assertFalse(applies(refresh = false, network = false))
    assertTrue(
        applies(
            refresh = false,
            network = false,
            snapshot = healthy.copy(phase = "error", connectedNeighbors = 0, routeStage = 0),
        ))
  }

  @Test
  fun successfulForcedNetworkProofSchedulesTheNextBoundedRefresh() {
    val refreshed =
        MasqSessionCoreSnapshot(
            phase = "connected",
            connectedNeighbors = 1,
            routeStage = 2,
            proxyPort = 44_443,
            engineGeneration = 3,
            routeProofGeneration = 11,
            routeProofRefresh =
                MasqRouteProofRefreshResult(
                    attempted = true,
                    succeeded = true,
                    errorCode = null,
                ),
        )

    val schedule =
        scheduleAfterForcedNetworkProof(
            snapshot = refreshed,
            currentStartGeneration = 7L,
            nowElapsed = 20_000L,
        )

    assertEquals(7L, schedule.startGeneration)
    assertEquals(3L, schedule.engineGeneration)
    assertEquals(11L, schedule.observedGeneration)
    assertEquals(20_000L + ROUTE_PROOF_REFRESH_INTERVAL_MILLIS, schedule.deadlineElapsed)
    assertEquals(0, schedule.consecutiveFailures)
  }

  @Test
  fun startedRecoveryRetainsBackoffAndEnforcesGraceUntilHealthy() {
    val started =
        MasqSessionRecoveryBackoff(attempts = 1).afterStarted(nowElapsed = 10_000L)

    assertEquals(2, started.attempts)
    assertFalse(started.allowsAttempt(10_000L + RECOVERY_STARTED_GRACE_MILLIS - 1L))
    assertTrue(started.allowsAttempt(10_000L + RECOVERY_STARTED_GRACE_MILLIS))
    assertTrue(started.hasStageOneProofOpportunity())
    assertEquals(
        1L,
        started.stageOneProofDelayMillis(
            10_000L + STAGE_ONE_ROUTE_PROOF_SETTLE_MILLIS - 1L,
        ),
    )
    assertEquals(
        0L,
        started.stageOneProofDelayMillis(
            10_000L + STAGE_ONE_ROUTE_PROOF_SETTLE_MILLIS,
        ),
    )
    assertEquals(60_000L, started.nextDelayMillis())
    assertEquals(MasqSessionRecoveryBackoff(), started.afterHealthy())
  }

  @Test
  fun stageOneProofScopeRequiresTheExactCoreAndNetworkGeneration() {
    val scope =
        MasqStageOneProofScope(
            identity =
                MasqRecoveryAttemptIdentity(
                    startGeneration = 7L,
                    engineGeneration = 3L,
                ),
            networkEpoch = 11L,
        )

    assertTrue(scope.applies(currentStartGeneration = 7L, currentNetworkEpoch = 11L))
    assertFalse(scope.applies(currentStartGeneration = 8L, currentNetworkEpoch = 11L))
    assertFalse(scope.applies(currentStartGeneration = 7L, currentNetworkEpoch = 12L))
    // Once BackgroundRecovery has atomically claimed the next start generation,
    // only the Android underlay epoch remains external to its own generation fence.
    assertTrue(scope.appliesToNetwork(currentNetworkEpoch = 11L))
    assertFalse(scope.appliesToNetwork(currentNetworkEpoch = 12L))
  }

  @Test
  fun failedStageOneProofReleasesTheStartedGraceForBoundedRetry() {
    val started =
        MasqSessionRecoveryBackoff(attempts = 0).afterStarted(nowElapsed = 10_000L)
    val failed = started.afterStageOneProofFailed(nowElapsed = 15_000L)

    assertFalse(started.allowsAttempt(15_000L))
    assertTrue(failed.allowsAttempt(15_000L))
    assertFalse(failed.hasStageOneProofOpportunity())
    assertEquals(2, failed.attempts)
    assertEquals(2_000L, failed.routeRebuildRetryDelayMillis())
    val failedAgain = failed.afterStageOneProofFailed(nowElapsed = 20_000L)
    assertEquals(5_000L, failedAgain.routeRebuildRetryDelayMillis())
    val failedThird = failedAgain.afterStageOneProofFailed(nowElapsed = 30_000L)
    assertEquals(15_000L, failedThird.routeRebuildRetryDelayMillis())
    val failedFourth = failedThird.afterStageOneProofFailed(nowElapsed = 40_000L)
    assertEquals(30_000L, failedFourth.routeRebuildRetryDelayMillis())
    assertEquals(
        30_000L,
        failedFourth
            .afterStageOneProofFailed(nowElapsed = 50_000L)
            .routeRebuildRetryDelayMillis(),
    )
  }

  @Test
  fun recoveryAttemptIdentityMatchesBothNativeGenerations() {
    val expected = MasqRecoveryAttemptIdentity(startGeneration = 7L, engineGeneration = 3L)
    val snapshot =
        MasqSessionCoreSnapshot(
            phase = "connecting",
            connectedNeighbors = 1,
            routeStage = 1,
            proxyPort = 44_443,
            engineGeneration = 3L,
        )

    assertTrue(matchesMasqRecoveryAttemptIdentity(expected, 7L, snapshot))
    assertFalse(matchesMasqRecoveryAttemptIdentity(expected, 8L, snapshot))
    assertFalse(
        matchesMasqRecoveryAttemptIdentity(
            expected,
            7L,
            snapshot.copy(engineGeneration = 4L),
        ),
    )
  }

  @Test
  fun postProofSnapshotPreservesAStructuredEntryFailureForQuarantine() {
    val initial =
        MasqSessionCoreSnapshot(
            phase = "connecting",
            connectedNeighbors = 1,
            routeStage = 1,
            proxyPort = 44_443,
            engineGeneration = 3L,
        )
    val postProof =
        initial.copy(
            phase = "error",
            connectedNeighbors = 0,
            routeStage = 0,
            lastError = "E_ENTRY_TCP_FAILED: redacted diagnostic",
        )
    val freshest =
        freshestMasqRecoveryFailureSnapshot(
            initialSnapshot = initial,
            verification =
                MasqRouteVerificationOutcome(
                    result = MasqBackgroundRecoveryResult.FAILED,
                    snapshot = postProof,
                ),
        )

    assertTrue(
        shouldQuarantineAttemptedEntryNodes(
            phase = freshest.phase,
            engineGeneration = freshest.engineGeneration,
            routeStage = freshest.routeStage,
            lastError = freshest.lastError,
        ))
    assertEquals(
        initial,
        freshestMasqRecoveryFailureSnapshot(
            initial,
            MasqRouteVerificationOutcome(MasqBackgroundRecoveryResult.FAILED, null),
        ),
    )
  }

  @Test
  fun structuredTerminalEntrySignalReleasesOnlyTheGenericStartupGrace() {
    val started =
        MasqSessionRecoveryBackoff(attempts = 1).afterStarted(nowElapsed = 10_000L)
    val released = started.afterTerminalEntrySignal(nowElapsed = 42_000L)

    assertEquals(started.attempts, released.attempts)
    assertFalse(released.allowsAttempt(41_999L))
    assertTrue(released.allowsAttempt(42_000L))
  }

  @Test
  fun terminalEntryPairRotationUsesFastBoundedBackoff() {
    assertEquals(1_000L, MasqSessionRecoveryBackoff(attempts = 0).terminalEntryRetryDelayMillis())
    assertEquals(1_000L, MasqSessionRecoveryBackoff(attempts = 1).terminalEntryRetryDelayMillis())
    assertEquals(2_000L, MasqSessionRecoveryBackoff(attempts = 2).terminalEntryRetryDelayMillis())
    assertEquals(5_000L, MasqSessionRecoveryBackoff(attempts = 3).terminalEntryRetryDelayMillis())
    assertEquals(15_000L, MasqSessionRecoveryBackoff(attempts = 99).terminalEntryRetryDelayMillis())
  }

  @Test
  fun onlyPersistentStructuredStageZeroEntryFailuresTriggerImmediatePairRotation() {
    fun snapshot(
        phase: String = "connecting",
        routeStage: Int = 0,
        lastError: String?,
    ) =
        MasqSessionCoreSnapshot(
            phase = phase,
            connectedNeighbors = 0,
            routeStage = routeStage,
            proxyPort = 44_443,
            engineGeneration = 9L,
            lastError = lastError,
        )

    assertTrue(
        snapshot(lastError = "E_ENTRY_TCP_FAILED: fixed diagnostic")
            .hasTerminalEntryRecoverySignal())
    assertTrue(
        snapshot(phase = "error", lastError = "E_ENTRY_GOSSIP_PASS_LOOP: fixed diagnostic")
            .hasTerminalEntryRecoverySignal())
    assertFalse(
        snapshot(lastError = "E_ENTRY_NO_INBOUND_BYTES: transient diagnostic")
            .hasTerminalEntryRecoverySignal())
    assertFalse(
        snapshot(routeStage = 1, lastError = "E_ENTRY_NO_INBOUND_BYTES: stale")
            .hasTerminalEntryRecoverySignal())
    assertFalse(
        snapshot(lastError = "E_PRIVATE_ROUTE_TIMEOUT: fixed diagnostic")
            .hasTerminalEntryRecoverySignal())
    assertFalse(snapshot(lastError = null).hasTerminalEntryRecoverySignal())
    assertFalse(
        snapshot(lastError = "E_ENTRY_TCP_FAILED: stale")
            .copy(engineGeneration = 0L)
            .hasTerminalEntryRecoverySignal())
  }

  @Test
  fun companionWatchdogRestoresOnlyPersistedUserRequestedSessions() {
    assertEquals(
        MasqSessionEnsureDecision.NOT_DESIRED,
        masqSessionEnsureDecision(
            persistedDesired = false,
            activeInstanceLive = false,
            nowElapsed = 20_000L,
            lastDispatchElapsed = 0L,
        ),
    )
    assertEquals(
        MasqSessionEnsureDecision.ALREADY_RUNNING,
        masqSessionEnsureDecision(
            persistedDesired = true,
            activeInstanceLive = true,
            nowElapsed = 20_000L,
            lastDispatchElapsed = 0L,
        ),
    )
    assertEquals(
        MasqSessionEnsureDecision.DISPATCH_RESTORE,
        masqSessionEnsureDecision(
            persistedDesired = true,
            activeInstanceLive = false,
            nowElapsed = 20_000L,
            lastDispatchElapsed = 0L,
        ),
    )
  }

  @Test
  fun companionWatchdogThrottlesDuplicateDispatchButRetriesAfterDeadlineOrClockReset() {
    val lastDispatch = 20_000L

    assertEquals(
        MasqSessionEnsureDecision.RETRY_THROTTLED,
        masqSessionEnsureDecision(
            persistedDesired = true,
            activeInstanceLive = false,
            nowElapsed = lastDispatch + SESSION_RESTORE_DISPATCH_RETRY_MILLIS - 1L,
            lastDispatchElapsed = lastDispatch,
        ),
    )
    assertEquals(
        MasqSessionEnsureDecision.DISPATCH_RESTORE,
        masqSessionEnsureDecision(
            persistedDesired = true,
            activeInstanceLive = false,
            nowElapsed = lastDispatch + SESSION_RESTORE_DISPATCH_RETRY_MILLIS,
            lastDispatchElapsed = lastDispatch,
        ),
    )
    assertEquals(
        MasqSessionEnsureDecision.DISPATCH_RESTORE,
        masqSessionEnsureDecision(
            persistedDesired = true,
            activeInstanceLive = false,
            nowElapsed = 1_000L,
            lastDispatchElapsed = lastDispatch,
        ),
    )
  }

  @Test
  fun networkIdentityChangesAreRecoveryEventsEvenWhenBothNetworksAreValidated() {
    assertEquals(
        MasqSessionNetworkTransition.REPLACED,
        masqSessionNetworkTransition(
            previousAvailable = true,
            previousNetworkId = 41L,
            currentAvailable = true,
            currentNetworkId = 42L,
        ),
    )
    assertEquals(
        MasqSessionNetworkTransition.LOST,
        masqSessionNetworkTransition(true, 41L, false, null),
    )
    assertEquals(
        MasqSessionNetworkTransition.RESTORED,
        masqSessionNetworkTransition(false, null, true, 42L),
    )
    assertEquals(
        MasqSessionNetworkTransition.UNCHANGED,
        masqSessionNetworkTransition(true, 42L, true, 42L),
    )
    assertEquals(
        MasqSessionNetworkTransition.UNCHANGED,
        masqSessionNetworkTransition(false, null, false, null),
    )
  }

  @Test
  fun duplicateImplicitNetworkCallbacksCoalesceUntilOneConfirmedResample() {
    val coalescer = MasqSessionNetworkTransitionCoalescer()

    assertEquals(
        MasqSessionNetworkObservationAction.DEFER_NEW,
        coalescer.observe(
            previousNetworkId = 41L,
            observedNetworkId = null,
            explicitlyLostNetworkId = null,
            confirmed = false,
        ),
    )
    assertEquals(
        MasqSessionNetworkObservationAction.DEFER_EXISTING,
        coalescer.observe(
            previousNetworkId = 41L,
            observedNetworkId = null,
            explicitlyLostNetworkId = null,
            confirmed = false,
        ),
    )
    assertEquals(
        MasqSessionNetworkObservationAction.APPLY,
        coalescer.observe(
            previousNetworkId = 41L,
            observedNetworkId = null,
            explicitlyLostNetworkId = null,
            confirmed = true,
        ),
    )
  }

  @Test
  fun recoveredOriginalUnderlayCancelsAnImplicitTransitionWithoutChangingEpoch() {
    val coalescer = MasqSessionNetworkTransitionCoalescer()

    assertEquals(
        MasqSessionNetworkObservationAction.DEFER_NEW,
        coalescer.observe(41L, null, null, confirmed = false),
    )
    assertEquals(
        MasqSessionNetworkObservationAction.APPLY,
        coalescer.observe(41L, 41L, null, confirmed = false),
    )
    assertEquals(
        MasqSessionNetworkObservationAction.DEFER_NEW,
        coalescer.observe(41L, 42L, null, confirmed = false),
    )
  }

  @Test
  fun explicitTrackedUnderlayLossBypassesDebounceForFastRecovery() {
    val coalescer = MasqSessionNetworkTransitionCoalescer()

    assertEquals(
        MasqSessionNetworkObservationAction.APPLY,
        coalescer.observe(
            previousNetworkId = 41L,
            observedNetworkId = 42L,
            explicitlyLostNetworkId = 41L,
            confirmed = false,
        ),
    )
    assertEquals(
        MasqSessionNetworkObservationAction.APPLY,
        coalescer.observe(
            previousNetworkId = null,
            observedNetworkId = 42L,
            explicitlyLostNetworkId = null,
            confirmed = false,
        ),
    )
  }

  @Test
  fun changedImplicitReplacementRestartsTheBoundedConfirmationWindow() {
    val coalescer = MasqSessionNetworkTransitionCoalescer()

    assertEquals(
        MasqSessionNetworkObservationAction.DEFER_NEW,
        coalescer.observe(41L, 42L, null, confirmed = false),
    )
    assertEquals(
        MasqSessionNetworkObservationAction.DEFER_EXISTING,
        coalescer.observe(41L, 42L, null, confirmed = false),
    )
    assertEquals(
        MasqSessionNetworkObservationAction.DEFER_NEW,
        coalescer.observe(41L, 43L, null, confirmed = false),
    )
  }

  @Test
  fun unconfirmedCallbackCannotInvalidateAStageOneModuleOwnedAttempt() {
    val coalescer = MasqSessionNetworkTransitionCoalescer()
    val action = coalescer.observe(41L, null, null, confirmed = false)
    val networkEpoch = 7L

    assertEquals(MasqSessionNetworkObservationAction.DEFER_NEW, action)
    assertFalse(
        shouldInvalidateModuleOwnedConnectionAttemptForNetworkTransition(
            moduleOwnsAttempt = true,
            moduleStartGeneration = 12L,
            currentStartGeneration = 12L,
            moduleNetworkEpoch = networkEpoch,
            currentNetworkEpoch = networkEpoch,
        ),
    )

    val explicitLoss = coalescer.observe(41L, null, 41L, confirmed = false)
    assertEquals(MasqSessionNetworkObservationAction.APPLY, explicitLoss)
    assertTrue(
        shouldInvalidateModuleOwnedConnectionAttemptForNetworkTransition(
            moduleOwnsAttempt = true,
            moduleStartGeneration = 12L,
            currentStartGeneration = 12L,
            moduleNetworkEpoch = networkEpoch,
            currentNetworkEpoch = networkEpoch + 1L,
        ),
    )
  }

  @Test
  fun forcedNetworkProofIsConsumedOnlyAfterTheOldEngineStopsInTheSameEpoch() {
    assertTrue(
        shouldConsumeMasqForcedNetworkProofEpoch(
            proofRequiredEpoch = 7L,
            currentNetworkEpoch = 7L,
            stopSucceeded = true,
        ))
    assertFalse(
        shouldConsumeMasqForcedNetworkProofEpoch(
            proofRequiredEpoch = 7L,
            currentNetworkEpoch = 7L,
            stopSucceeded = false,
        ))
    assertEquals(
        MasqSessionNetworkRouteAction.STATUS,
        masqSessionNetworkRouteAction(
            proofRequiredEpoch = 0L,
            restartRequiredEpoch = 0L,
            currentNetworkEpoch = 7L,
        ),
    )
    assertEquals(
        MasqSessionNetworkRouteAction.RESTART,
        masqSessionNetworkRouteAction(
            proofRequiredEpoch = 0L,
            restartRequiredEpoch = 7L,
            currentNetworkEpoch = 7L,
        ),
    )
  }

  @Test
  fun delayedLossOfTheProofSourceUpgradesTheSameEpochToRestart() {
    assertTrue(
        shouldUpgradeMasqProofToRestartAfterSourceLoss(
            lostNetworkId = 41L,
            proofSourceNetworkId = 41L,
            proofRequiredEpoch = 8L,
            currentNetworkEpoch = 8L,
        ))
    assertFalse(
        shouldUpgradeMasqProofToRestartAfterSourceLoss(
            lostNetworkId = 42L,
            proofSourceNetworkId = 41L,
            proofRequiredEpoch = 8L,
            currentNetworkEpoch = 8L,
        ))
    assertFalse(
        shouldUpgradeMasqProofToRestartAfterSourceLoss(
            lostNetworkId = 41L,
            proofSourceNetworkId = 41L,
            proofRequiredEpoch = 8L,
            currentNetworkEpoch = 9L,
        ))
    assertTrue(
        shouldRestartAfterActiveRouteSourceLoss(
            lostNetworkId = 41L,
            activeRouteSource =
                MasqSessionActiveRouteSource(networkId = 41L, engineGeneration = 12L),
        ))
    assertFalse(
        shouldRestartAfterActiveRouteSourceLoss(
            lostNetworkId = 42L,
            activeRouteSource =
                MasqSessionActiveRouteSource(networkId = 41L, engineGeneration = 12L),
        ))
    assertFalse(
        shouldRestartAfterActiveRouteSourceLoss(
            lostNetworkId = 41L,
            activeRouteSource =
                MasqSessionActiveRouteSource(networkId = 41L, engineGeneration = 0L),
        ))
    assertFalse(
        shouldConsumeMasqForcedNetworkProofEpoch(
            proofRequiredEpoch = 7L,
            currentNetworkEpoch = 8L,
            stopSucceeded = true,
        ))
  }

  @Test
  fun validatedUnderlaySelectionNeverTreatsTheAppsOwnVpnAsANetworkHandover() {
    val candidates =
        listOf(
            MasqSessionNetworkCandidate(
                networkId = 91L,
                hasInternet = true,
                validated = true,
                notVpn = false,
            ),
            MasqSessionNetworkCandidate(
                networkId = 42L,
                hasInternet = true,
                validated = true,
                notVpn = true,
            ),
            MasqSessionNetworkCandidate(
                networkId = 43L,
                hasInternet = true,
                validated = false,
                notVpn = true,
            ),
        )

    assertEquals(
        42L,
        selectMasqValidatedUnderlayNetworkId(
            activeNetworkId = 91L,
            previousNetworkId = 42L,
            candidates = candidates,
        ),
    )
  }

  @Test
  fun validatedUnderlaySelectionDefensivelyRejectsVpnTransport() {
    assertEquals(
        42L,
        selectMasqValidatedUnderlayNetworkId(
            activeNetworkId = 91L,
            previousNetworkId = 42L,
            candidates =
                listOf(
                    MasqSessionNetworkCandidate(
                        networkId = 91L,
                        hasInternet = true,
                        validated = true,
                        notVpn = true,
                        vpnTransport = true,
                    ),
                    MasqSessionNetworkCandidate(42L, true, true, true),
                ),
        ),
    )
  }

  @Test
  fun validatedUnderlaySelectionPrefersActiveThenPreviousThenStableFallback() {
    val candidates =
        listOf(
            MasqSessionNetworkCandidate(44L, true, true, true),
            MasqSessionNetworkCandidate(42L, true, true, true),
        )

    assertEquals(
        44L,
        selectMasqValidatedUnderlayNetworkId(44L, 42L, candidates),
    )
    assertEquals(
        42L,
        selectMasqValidatedUnderlayNetworkId(90L, 42L, candidates),
    )
    assertEquals(
        42L,
        selectMasqValidatedUnderlayNetworkId(90L, 91L, candidates),
    )
    assertEquals(
        null,
        selectMasqValidatedUnderlayNetworkId(
            activeNetworkId = 90L,
            previousNetworkId = 91L,
            candidates =
                listOf(
                    MasqSessionNetworkCandidate(42L, true, false, true),
                    MasqSessionNetworkCandidate(43L, true, true, false),
                ),
        ),
    )
    assertEquals(
        44L,
        selectMasqValidatedUnderlayNetworkId(
            activeNetworkId = 90L,
            previousNetworkId = 91L,
            candidates =
                listOf(
                    MasqSessionNetworkCandidate(
                        networkId = 42L,
                        hasInternet = true,
                        validated = true,
                        notVpn = true,
                        matchesActiveVpnTransport = false,
                        transportPreference = 1,
                    ),
                    MasqSessionNetworkCandidate(
                        networkId = 44L,
                        hasInternet = true,
                        validated = true,
                        notVpn = true,
                        matchesActiveVpnTransport = true,
                        transportPreference = 2,
                    ),
                ),
        ),
    )
  }

  @Test
  fun moduleOwnedConnectionWindowPreventsCompetingBackgroundRecovery() {
    fun shouldDefer(
        nowElapsed: Long = 20_000L,
        moduleOwnsAttempt: Boolean = true,
        moduleStartGeneration: Long = 12L,
        currentStartGeneration: Long = 12L,
        moduleNetworkEpoch: Long = 7L,
        currentNetworkEpoch: Long = 7L,
        snapshotHealthy: Boolean = false,
    ) =
        shouldDeferRecoveryToModuleOwnedConnectionAttempt(
            moduleOwnsAttempt = moduleOwnsAttempt,
            moduleStartGeneration = moduleStartGeneration,
            currentStartGeneration = currentStartGeneration,
            moduleNetworkEpoch = moduleNetworkEpoch,
            currentNetworkEpoch = currentNetworkEpoch,
            nowElapsed = nowElapsed,
            moduleDeadlineElapsed = 30_000L,
            snapshotHealthy = snapshotHealthy,
        )

    assertTrue(shouldDefer())
    assertFalse(shouldDefer(nowElapsed = 30_000L))
    assertFalse(shouldDefer(moduleOwnsAttempt = false))
    assertFalse(shouldDefer(snapshotHealthy = true))
    assertFalse(shouldDefer(currentStartGeneration = 13L))
    assertFalse(shouldDefer(currentNetworkEpoch = 8L))
  }

  @Test
  fun moduleOwnershipAndNetworkHandoverHaveOneMutationOwnerInEitherOrdering() {
    // Module start first, then handover: the matching foreground generation is
    // invalidated as soon as the network epoch advances.
    assertTrue(
        shouldInvalidateModuleOwnedConnectionAttemptForNetworkTransition(
            moduleOwnsAttempt = true,
            moduleStartGeneration = 12L,
            currentStartGeneration = 12L,
            moduleNetworkEpoch = 7L,
            currentNetworkEpoch = 8L,
        ))
    assertFalse(
        shouldInvalidateModuleOwnedConnectionAttemptForNetworkTransition(
            moduleOwnsAttempt = true,
            moduleStartGeneration = 12L,
            currentStartGeneration = 13L,
            moduleNetworkEpoch = 7L,
            currentNetworkEpoch = 8L,
        ))

    // Handover first, then a posted module start: the pending proof/restart
    // remains service-owned instead of being reset by the later start event.
    assertFalse(
        shouldAcceptModuleOwnedConnectionAttempt(
            networkAvailable = true,
            pendingNetworkAction = MasqSessionNetworkRouteAction.PROVE,
        ))
    assertFalse(
        shouldAcceptModuleOwnedConnectionAttempt(
            networkAvailable = true,
            pendingNetworkAction = MasqSessionNetworkRouteAction.RESTART,
        ))
    assertFalse(
        shouldAcceptModuleOwnedConnectionAttempt(
            networkAvailable = false,
            pendingNetworkAction = MasqSessionNetworkRouteAction.STATUS,
        ))
    assertFalse(
        shouldAcceptModuleOwnedConnectionAttempt(
            networkAvailable = true,
            pendingNetworkAction = MasqSessionNetworkRouteAction.STATUS,
            pendingServiceRouteMutation = true,
        ))
    val stalePeriodicRestart =
        MasqPeriodicRouteProofRestartScope(
            sessionGeneration = 11L,
            startGeneration = 11L,
            engineGeneration = 3L,
            networkEpoch = 7L,
        )
    val stalePeriodicRestartApplies =
        stalePeriodicRestart.applies(
            currentSessionGeneration = 12L,
            currentStartGeneration = 12L,
            currentNetworkEpoch = 7L,
            networkAvailable = true,
        )
    assertFalse(stalePeriodicRestartApplies)
    assertTrue(
        shouldAcceptModuleOwnedConnectionAttempt(
            networkAvailable = true,
            pendingNetworkAction = MasqSessionNetworkRouteAction.STATUS,
            pendingServiceRouteMutation = stalePeriodicRestartApplies,
        ))
  }

  @Test
  fun networkHandoverInvalidationCarriesOneRetryReasonToTheStaleStart() {
    val generation =
        synchronized(MasqCoreLifecycle.lock) {
          MasqCoreLifecycle.startGeneration.incrementAndGet()
        }

    assertTrue(MasqCoreLifecycle.invalidateForNetworkHandover(generation))
    assertEquals(
        MasqStartInvalidationReason.NETWORK_HANDOVER,
        MasqCoreLifecycle.consumeInvalidationReason(generation),
    )
    assertEquals(null, MasqCoreLifecycle.consumeInvalidationReason(generation))
    assertFalse(MasqCoreLifecycle.invalidateForNetworkHandover(generation))
  }

  @Test
  fun explicitNetworkLossRestartsWhileTemporaryValidationLossAndReplacementUseProof() {
    assertEquals(
        MasqSessionNetworkRouteAction.RESTART,
        masqSessionNetworkRouteAction(
            proofRequiredEpoch = 0L,
            restartRequiredEpoch = 8L,
            currentNetworkEpoch = 8L,
        ),
    )
    assertEquals(
        MasqSessionNetworkRouteAction.PROVE,
        masqSessionNetworkRouteAction(
            proofRequiredEpoch = 8L,
            restartRequiredEpoch = 0L,
            currentNetworkEpoch = 8L,
        ),
    )
    assertEquals(
        MasqSessionNetworkRouteAction.RESTART,
        masqSessionNetworkRouteAction(
            proofRequiredEpoch = 8L,
            restartRequiredEpoch = 8L,
            currentNetworkEpoch = 8L,
        ),
    )
  }

  @Test
  fun staleOrUninitializedNetworkEpochsCannotMutateTheCurrentRoute() {
    assertEquals(
        MasqSessionNetworkRouteAction.STATUS,
        masqSessionNetworkRouteAction(
            proofRequiredEpoch = 7L,
            restartRequiredEpoch = 6L,
            currentNetworkEpoch = 8L,
        ),
    )
    assertEquals(
        MasqSessionNetworkRouteAction.STATUS,
        masqSessionNetworkRouteAction(
            proofRequiredEpoch = 0L,
            restartRequiredEpoch = 0L,
            currentNetworkEpoch = 0L,
        ),
    )
  }

  @Test
  fun recoveryDistinguishesAWarmPauseFromACompleteNetworkShutdown() {
    fun snapshot(phase: String) =
        MasqSessionCoreSnapshot(
            phase = phase,
            connectedNeighbors = if (phase == "connected") 1 else 0,
            routeStage = if (phase == "connected") 2 else 0,
            proxyPort = if (phase == "connected") 44_443 else 0,
            engineGeneration = 4L,
        )

    assertTrue(isSuccessfulNetworkRouteRestartSnapshot(snapshot("paused")))
    assertTrue(isSuccessfulNetworkRouteRestartSnapshot(snapshot("ready")))
    assertTrue(isSuccessfulNetworkRouteRestartSnapshot(snapshot("unconfigured")))
    assertFalse(isSuccessfulNetworkRouteRestartSnapshot(snapshot("error")))
    assertFalse(isSuccessfulNetworkRouteRestartSnapshot(snapshot("connected")))
    assertFalse(isSuccessfulNetworkRouteRestartSnapshot(null))

    assertFalse(isSuccessfulNetworkRouteShutdownSnapshot(snapshot("paused")))
    assertTrue(isSuccessfulNetworkRouteShutdownSnapshot(snapshot("ready")))
    assertTrue(isSuccessfulNetworkRouteShutdownSnapshot(snapshot("unconfigured")))
    assertFalse(isSuccessfulNetworkRouteShutdownSnapshot(snapshot("error")))
    assertFalse(isSuccessfulNetworkRouteShutdownSnapshot(snapshot("connected")))
    assertFalse(isSuccessfulNetworkRouteShutdownSnapshot(null))
  }

  @Test
  fun verifiesAnEntryRouteAtMostOncePerAttemptIdentity() {
    val gate = MasqRecoveryRouteVerificationGate()
    val first = MasqRecoveryAttemptIdentity(startGeneration = 7L, engineGeneration = 3L)
    val second = MasqRecoveryAttemptIdentity(startGeneration = 8L, engineGeneration = 4L)

    assertTrue(gate.claim(first))
    assertFalse(gate.claim(first))
    assertTrue(gate.claim(second))
    assertFalse(gate.claim(second))
  }
}
