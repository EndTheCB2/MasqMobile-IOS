package com.masqmobile

import java.io.InterruptedIOException
import java.net.SocketTimeoutException
import java.net.UnknownHostException
import java.util.concurrent.CountDownLatch
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class EntryNodeDiscoverySelectionTest {
  private val keyA = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"
  private val keyB = "AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE"
  private val keyC = "AgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgI"
  private val keyD = "AwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwM"
  private val keyE = "BAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQ"
  private val keyF = "BQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQU"

  @Test
  fun preservesAnUnquotedPlainTextNodeFinderDescriptor() {
    val raw = descriptor(keyA, "8.8.8.8", "4100")

    assertEquals(raw, normalizeNodeFinderDescriptor("\n  $raw  \r\n"))
  }

  @Test
  fun decodesAQuotedJsonNodeFinderDescriptor() {
    val raw = descriptor(keyA, "8.8.8.8", "4100")

    assertEquals(raw, normalizeNodeFinderDescriptor("\"$raw\""))
  }

  @Test
  fun classifiesNodeFinderFailuresWithoutLoggingTheirDetails() {
    assertEquals("NF_FETCH_DNS", nodeFinderFailureCode(UnknownHostException("private host")))
    assertEquals(
        "NF_FETCH_TIMEOUT",
        nodeFinderFailureCode(IllegalStateException("wrapper", SocketTimeoutException("private"))),
    )
    assertEquals(
        "NF_FETCH_INTERRUPTED",
        nodeFinderFailureCode(InterruptedIOException("private")),
    )
    assertEquals("NF_FETCH_UNEXPECTED", nodeFinderFailureCode(IllegalStateException("private")))
  }

  @Test
  fun startsAllTwelveBoundedNodeFinderAttemptsWithoutAQueuedSecondWave() {
    assertEquals(
        listOf((0 until 12).toList()),
        nodeFinderAttemptBatches(attempts = 12, maxConcurrentRequests = 12),
    )
    assertEquals(12, nodeFinderRequestConcurrency(attempts = 12, maximumConcurrency = 12))
    assertEquals(8, nodeFinderRequestConcurrency(attempts = 12, maximumConcurrency = 8))
    assertEquals(3, nodeFinderRequestConcurrency(attempts = 3, maximumConcurrency = 12))
  }

  @Test
  fun staleDiscoveryWaiterNeverOverlapsTheCurrentDiscoveryOwner() {
    val gate = EntryNodeDiscoveryGate(pollIntervalMs = 10L)
    val executor = Executors.newFixedThreadPool(2)
    val ownerEntered = CountDownLatch(1)
    val releaseOwner = CountDownLatch(1)
    val waiterPolled = CountDownLatch(1)
    val waiterCurrent = AtomicBoolean(true)
    val staleWaiterEntered = AtomicBoolean(false)

    try {
      val owner =
          executor.submit {
            gate.run({ true }) {
              ownerEntered.countDown()
              releaseOwner.await(2, TimeUnit.SECONDS)
            }
          }
      assertTrue(ownerEntered.await(1, TimeUnit.SECONDS))
      val waiter =
          executor.submit<Boolean> {
            try {
              gate.run(
                  isCurrent = {
                    waiterPolled.countDown()
                    waiterCurrent.get()
                  },
              ) {
                staleWaiterEntered.set(true)
                true
              }
            } catch (_: EntryNodeDiscoveryCancelledException) {
              false
            }
          }
      assertTrue(waiterPolled.await(1, TimeUnit.SECONDS))
      waiterCurrent.set(false)
      assertFalse(waiter.get(1, TimeUnit.SECONDS))
      assertFalse(staleWaiterEntered.get())
      releaseOwner.countDown()
      owner.get(1, TimeUnit.SECONDS)
    } finally {
      releaseOwner.countDown()
      executor.shutdownNow()
    }
  }

  @Test
  fun selectsFreshBeforePreferredAndCache() {
    val selected =
        EntryNodeDiscoverySelection.select(
            chain = CHAIN,
            freshDescriptors =
                listOf(
                    descriptor(keyA, "8.8.8.8", "4100"),
                    descriptor(keyB, "9.9.9.9", "4200"),
                ),
            preferredDescriptors = listOf(descriptor(keyC, "1.1.1.1", "4300")),
            cachedDescriptors = listOf(descriptor(keyC, "4.2.2.2", "4400")),
        )

    assertEquals(listOf(keyA, keyB), selected.map(EntryNodeCandidate::publicKey))
  }

  @Test
  fun preservesAThreeIdentityKnownGoodGroupBeforeFreshPreferredAndCache() {
    val selected =
        EntryNodeDiscoverySelection.select(
            chain = CHAIN,
            freshDescriptors =
                listOf(
                    descriptor(keyA, "8.8.8.8", "4100"),
                    descriptor(keyB, "9.9.9.9", "4200"),
                ),
            preferredDescriptors = listOf(descriptor(keyF, "8.26.56.26", "4600")),
            cachedDescriptors = listOf(descriptor(keyA, "8.8.4.4", "4600")),
            knownGoodDescriptors =
                listOf(
                    descriptor(keyC, "1.1.1.1", "4300"),
                    descriptor(keyD, "4.2.2.2", "4400"),
                    descriptor(keyE, "208.67.222.222", "4500"),
                ),
            limit = 5,
        )

    assertEquals(listOf(keyC, keyD, keyE, keyA, keyB), selected.map(EntryNodeCandidate::publicKey))
  }

  @Test
  fun fillsFreshSelectionFromPreferredBeforeCache() {
    val selected =
        EntryNodeDiscoverySelection.select(
            chain = CHAIN,
            freshDescriptors = listOf(descriptor(keyA, "8.8.8.8", "4100")),
            preferredDescriptors = listOf(descriptor(keyB, "9.9.9.9", "4200")),
            cachedDescriptors = listOf(descriptor(keyC, "1.1.1.1", "4300")),
        )

    assertEquals(listOf(keyA, keyB), selected.map(EntryNodeCandidate::publicKey))
  }

  @Test
  fun requiresUniquePublicKeysAndHosts() {
    val selected =
        EntryNodeDiscoverySelection.select(
            chain = CHAIN,
            freshDescriptors =
                listOf(
                    descriptor(keyA, "8.8.8.8", "4100"),
                    descriptor(keyA, "9.9.9.9", "4200"),
                    descriptor(keyB, "8.8.8.8", "4300"),
                    descriptor(keyC, "1.1.1.1", "4400"),
                ),
            preferredDescriptors = emptyList(),
            cachedDescriptors = emptyList(),
        )

    assertEquals(listOf(keyA, keyC), selected.map(EntryNodeCandidate::publicKey))
    assertEquals(listOf("8.8.8.8", "1.1.1.1"), selected.map(EntryNodeCandidate::host))
  }

  @Test
  fun rejectedHostCollisionDoesNotPoisonItsOtherwiseUnusedPublicKey() {
    val selected =
        EntryNodeDiscoverySelection.select(
            chain = CHAIN,
            freshDescriptors =
                listOf(
                    descriptor(keyA, "8.8.8.8", "4100"),
                    descriptor(keyB, "8.8.8.8", "4200"),
                    descriptor(keyB, "9.9.9.9", "4300"),
                ),
            preferredDescriptors = emptyList(),
            cachedDescriptors = emptyList(),
        )

    assertEquals(listOf(keyA, keyB), selected.map(EntryNodeCandidate::publicKey))
    assertEquals(listOf("8.8.8.8", "9.9.9.9"), selected.map(EntryNodeCandidate::host))
  }

  @Test
  fun preservesOriginalDescriptorAndPassivelyRotatesOnePortPerGeneration() {
    val original = descriptor(keyA, "8.8.8.8", "4100/4200/4300/4400")
    val candidate = EntryNodeDiscoverySelection.parse(original, CHAIN)!!

    assertEquals(original, candidate.originalDescriptor)
    assertEquals(descriptor(keyA, "8.8.8.8", "4100"), candidate.singlePortDescriptor(0))
    assertEquals(descriptor(keyA, "8.8.8.8", "4200"), candidate.singlePortDescriptor(1))
    assertEquals(descriptor(keyA, "8.8.8.8", "4100"), candidate.singlePortDescriptor(4))
  }

  @Test
  fun restoresCachedPortVariantsWhenSavedPreferredNodesWereNarrowed() {
    val selected =
        EntryNodeDiscoverySelection.select(
            chain = CHAIN,
            freshDescriptors = emptyList(),
            preferredDescriptors =
                listOf(
                    descriptor(keyA, "8.8.8.8", "4200"),
                    descriptor(keyB, "9.9.9.9", "5200"),
                ),
            cachedDescriptors =
                listOf(
                    descriptor(keyA, "8.8.8.8", "4100/4200"),
                    descriptor(keyB, "9.9.9.9", "5100/5200"),
                ),
        )

    assertEquals(listOf(4100, 4200), selected[0].ports)
    assertEquals(listOf(5100, 5200), selected[1].ports)
    assertEquals(descriptor(keyA, "8.8.8.8", "4200"), selected[0].singlePortDescriptor(1))
    assertEquals(descriptor(keyA, "8.8.8.8", "4100"), selected[0].singlePortDescriptor(2))
    assertEquals(
        descriptor(keyA, "8.8.8.8", "4100/4200"),
        EntryNodeDiscoveryResult.fromSelection(selected, generation = 1).persistentDescriptors[0],
    )
  }

  @Test
  fun keepsFreshIdentityAndPortFirstWhileMergingMatchingCachedVariants() {
    val selected =
        EntryNodeDiscoverySelection.select(
            chain = CHAIN,
            freshDescriptors =
                listOf(
                    descriptor(keyA, "8.8.8.8", "4300"),
                    descriptor(keyB, "9.9.9.9", "5200"),
                ),
            preferredDescriptors = listOf(descriptor(keyC, "1.1.1.1", "6100")),
            cachedDescriptors =
                listOf(
                    descriptor(keyA, "8.8.8.8", "4100/4200"),
                    descriptor(keyC, "1.1.1.1", "6100/6200"),
                ),
        )

    assertEquals(listOf(keyA, keyB), selected.map(EntryNodeCandidate::publicKey))
    assertEquals(listOf(4300, 4100, 4200), selected[0].ports)
    assertEquals(descriptor(keyA, "8.8.8.8", "4300"), selected[0].singlePortDescriptor(0))
  }

  @Test
  fun separatesSinglePortRuntimeDescriptorsFromFullPersistentDescriptors() {
    val selected =
        listOf(
            EntryNodeDiscoverySelection.parse(
                descriptor(keyA, "8.8.8.8", "4100/4200"),
                CHAIN,
            )!!,
            EntryNodeDiscoverySelection.parse(
                descriptor(keyB, "9.9.9.9", "5100/5200"),
                CHAIN,
            )!!,
        )

    val result = EntryNodeDiscoveryResult.fromSelection(selected, generation = 1)

    assertEquals(
        listOf(
            descriptor(keyA, "8.8.8.8", "4200"),
            descriptor(keyB, "9.9.9.9", "5200"),
        ),
        result.runtimeDescriptors,
    )
    assertEquals(
        listOf(
            descriptor(keyA, "8.8.8.8", "4100/4200"),
            descriptor(keyB, "9.9.9.9", "5100/5200"),
        ),
        result.persistentDescriptors,
    )
    assertEquals(result.persistentDescriptors, result.cacheDescriptors)
  }

  @Test
  fun canCollectStandbyCandidatesBeyondTheTwoRuntimeEntries() {
    val selected =
        EntryNodeDiscoverySelection.select(
            chain = CHAIN,
            freshDescriptors =
                listOf(
                    descriptor(keyA, "8.8.8.8", "4100"),
                    descriptor(keyB, "9.9.9.9", "4200"),
                    descriptor(keyC, "1.1.1.1", "4300"),
                    descriptor(keyD, "4.2.2.2", "4400"),
                ),
            preferredDescriptors = emptyList(),
            cachedDescriptors = emptyList(),
            limit = 4,
        )

    assertEquals(4, selected.size)
  }

  @Test
  fun unreachableKnownGoodEntriesDoNotDisplaceFourFreshProbeCandidates() {
    val knownGoodDescriptors =
        listOf(
            descriptor(keyA, "8.8.8.8", "4100"),
            descriptor(keyB, "9.9.9.9", "4200"),
        )
    val freshDescriptors =
        listOf(
            descriptor(keyC, "1.1.1.1", "4300"),
            descriptor(keyD, "4.2.2.2", "4400"),
            descriptor(keyE, "208.67.222.222", "4500"),
            descriptor(keyF, "8.26.56.26", "4600"),
        )
    val knownGoodCandidates =
        EntryNodeDiscoverySelection.select(
            chain = CHAIN,
            freshDescriptors = emptyList(),
            preferredDescriptors = emptyList(),
            cachedDescriptors = emptyList(),
            knownGoodDescriptors = knownGoodDescriptors,
            limit = 2,
        )
    val candidates =
        EntryNodeDiscoverySelection.select(
            chain = CHAIN,
            freshDescriptors = freshDescriptors,
            preferredDescriptors = emptyList(),
            cachedDescriptors = emptyList(),
            knownGoodDescriptors = knownGoodDescriptors,
            limit =
                entryNodeProbeCandidatePoolLimit(
                    maximumAlternativeIdentities = 4,
                    alreadyProbedKnownGoodIdentityCount = knownGoodCandidates.size,
                ),
        )
    val alreadyProbed = knownGoodCandidates.map(EntryNodeCandidate::identity).toSet()

    val additional =
        planAdditionalEntryNodeProbes(
            candidates = candidates,
            alreadyProbedKnownGoodIdentities = alreadyProbed,
            reachableKnownGoodIdentities = emptySet(),
            maximumReachableIdentities = 4,
        )
    val probePlan =
        planEntryNodeProbes(
            additional.candidates,
            maxIdentities = additional.maxIdentities,
            generation = 0,
        )

    assertEquals(listOf(keyA, keyB, keyC, keyD, keyE, keyF), candidates.map { it.publicKey })
    assertEquals(4, additional.maxIdentities)
    assertEquals(listOf(keyC, keyD, keyE, keyF), probePlan.candidates.map { it.publicKey })
    assertEquals(4, probePlan.primaryTargets.size)
  }

  @Test
  fun onlyReachableKnownGoodEntriesConsumeProbeSlotsAndRetainPreference() {
    val candidates =
        listOf(
                keyA to "8.8.8.8",
                keyB to "9.9.9.9",
                keyC to "1.1.1.1",
                keyD to "4.2.2.2",
                keyE to "208.67.222.222",
                keyF to "8.26.56.26",
            )
            .mapIndexed { index, (key, host) ->
              EntryNodeDiscoverySelection.parse(
                  descriptor(key, host, "${4100 + index}"),
                  CHAIN,
              )!!
            }
    val alreadyProbed = candidates.take(2).map(EntryNodeCandidate::identity).toSet()
    val reachableKnownGood =
        EntryNodeReachability(
            candidate = candidates.first(),
            reachablePorts = listOf(EntryNodePortLatency(port = 4100, latencyMs = 20)),
        )

    val additional =
        planAdditionalEntryNodeProbes(
            candidates = candidates,
            alreadyProbedKnownGoodIdentities = alreadyProbed,
            reachableKnownGoodIdentities = setOf(candidates.first().identity()),
            maximumReachableIdentities = 4,
        )
    val freshProbePlan =
        planEntryNodeProbes(
            additional.candidates,
            maxIdentities = additional.maxIdentities,
            generation = 0,
        )
    val freshReachable =
        freshProbePlan.candidates.mapIndexed { index, candidate ->
          EntryNodeReachability(
              candidate = candidate,
              reachablePorts =
                  listOf(EntryNodePortLatency(candidate.ports.first(), 30 + index)),
          )
        }

    assertEquals(3, additional.maxIdentities)
    assertEquals(listOf(keyC, keyD, keyE), freshProbePlan.candidates.map { it.publicKey })
    assertEquals(
        listOf(keyA, keyC, keyD, keyE),
        prioritizeKnownGoodEntryNodes(
                freshReachable + reachableKnownGood,
                alreadyProbed,
            )
            .map { entry -> entry.candidate.publicKey },
    )
  }

  @Test
  fun commonProbePathContactsOnePortOnAtMostFourIdentities() {
    val candidates =
        listOf(
                keyA to "8.8.8.8",
                keyB to "9.9.9.9",
                keyC to "1.1.1.1",
                keyD to "4.2.2.2",
                keyE to "208.67.222.222",
            )
            .mapIndexed { index, (key, host) ->
              EntryNodeDiscoverySelection.parse(
                  descriptor(key, host, "${4100 + index}/${4200 + index}/${4300 + index}"),
                  CHAIN,
              )!!
            }

    val plan = planEntryNodeProbes(candidates, maxIdentities = 4, generation = 0)

    assertEquals(listOf(keyA, keyB, keyC, keyD), plan.candidates.map { it.publicKey })
    assertEquals(4, plan.primaryTargets.size)
    assertEquals(
        plan.candidates.map { candidate -> candidate.ports.first() },
        plan.primaryTargets.map { target -> target.port },
    )
    assertFalse(plan.primaryTargets.any { target -> target.candidate.publicKey == keyE })
  }

  @Test
  fun fallbackPortsAreUsedOnlyWhenPrimaryHasFewerThanTwoReachableIdentities() {
    val candidates =
        listOf(
                keyA to "8.8.8.8",
                keyB to "9.9.9.9",
                keyC to "1.1.1.1",
                keyD to "4.2.2.2",
                keyE to "208.67.222.222",
            )
            .mapIndexed { index, (key, host) ->
              EntryNodeDiscoverySelection.parse(
                  descriptor(
                      key,
                      host,
                      "${4100 + index}/${4200 + index}/${4300 + index}/${4400 + index}",
                  ),
                  CHAIN,
              )!!
            }
    val plan = planEntryNodeProbes(candidates, maxIdentities = 4, generation = 0)
    val oneReachable =
        plan.primaryTargets.mapIndexed { index, target ->
          EntryNodeProbeResult(
              publicKey = target.candidate.publicKey,
              host = target.candidate.host,
              port = target.port,
              latencyMs = if (index == 0) 20 else null,
          )
        }
    val twoReachable =
        oneReachable.mapIndexed { index, result ->
          if (index == 1) result.copy(latencyMs = 30) else result
        }

    assertTrue(entryNodeProbeFallbackRequired(oneReachable, requiredReachableIdentities = 2))
    assertFalse(entryNodeProbeFallbackRequired(twoReachable, requiredReachableIdentities = 2))
    assertEquals(12, plan.fallbackTargets.size)
    assertTrue(
        plan.fallbackTargets.all { target ->
          target.candidate in plan.candidates && target.port != target.candidate.ports.first()
        })
    assertFalse(plan.fallbackTargets.any { target -> target.candidate.publicKey == keyE })
  }

  @Test
  fun twoFastPrimaryResultsKeepTheCommonPathFreeOfSlowRetryTargets() {
    val candidates =
        listOf(
                keyA to "8.8.8.8",
                keyB to "9.9.9.9",
                keyC to "1.1.1.1",
            )
            .mapIndexed { index, (key, host) ->
              EntryNodeDiscoverySelection.parse(
                  descriptor(key, host, "${4100 + index}/${5100 + index}"),
                  CHAIN,
              )!!
            }
    val plan = planEntryNodeProbes(candidates, maxIdentities = 4, generation = 0)
    val primaryResults =
        plan.primaryTargets.mapIndexed { index, target ->
          EntryNodeProbeResult(
              publicKey = target.candidate.publicKey,
              host = target.candidate.host,
              port = target.port,
              latencyMs = if (index < 2) 20 + index else null,
          )
        }

    assertFalse(entryNodeProbeFallbackRequired(primaryResults, 2))
    assertTrue(
        planSlowEntryNodeProbeTargets(plan, primaryResults, 2)
            .isEmpty())
  }

  @Test
  fun slowRetryOfALostPrimarySynCanRecoverTheSecondIdentity() {
    val candidates =
        listOf(
            EntryNodeDiscoverySelection.parse(
                descriptor(keyA, "8.8.8.8", "4100"),
                CHAIN,
            )!!,
            EntryNodeDiscoverySelection.parse(
                descriptor(keyB, "9.9.9.9", "4200"),
                CHAIN,
            )!!,
        )
    val plan = planEntryNodeProbes(candidates, maxIdentities = 4, generation = 0)
    val primaryResults =
        listOf(
            EntryNodeProbeResult(keyA, "8.8.8.8", 4100, 25),
            EntryNodeProbeResult(keyB, "9.9.9.9", 4200, null),
        )
    val slowTargets = planSlowEntryNodeProbeTargets(plan, primaryResults, 2)

    assertEquals(listOf(keyB), slowTargets.map { it.candidate.publicKey })
    assertEquals(listOf(4200), slowTargets.map(EntryNodeProbeTarget::port))

    val recovered =
        rankReachableEntryNodes(
            plan.candidates,
            primaryResults + EntryNodeProbeResult(keyB, "9.9.9.9", 4200, 1_250),
        )
    assertEquals(listOf(keyA, keyB), recovered.map { it.candidate.publicKey })
  }

  @Test
  fun slowRetryKeepsRotatedAlternatePortsAvailable() {
    val candidates =
        listOf(
            EntryNodeDiscoverySelection.parse(
                descriptor(keyA, "8.8.8.8", "4100"),
                CHAIN,
            )!!,
            EntryNodeDiscoverySelection.parse(
                descriptor(keyB, "9.9.9.9", "4200/5200/6200"),
                CHAIN,
            )!!,
        )
    val plan = planEntryNodeProbes(candidates, maxIdentities = 4, generation = 1)
    val primaryResults =
        listOf(
            EntryNodeProbeResult(keyA, "8.8.8.8", 4100, 20),
            EntryNodeProbeResult(keyB, "9.9.9.9", 5200, null),
        )
    val slowTargets = planSlowEntryNodeProbeTargets(plan, primaryResults, 2)

    assertEquals(listOf(5200, 6200, 4200), slowTargets.map(EntryNodeProbeTarget::port))
    val recovered =
        rankReachableEntryNodes(
            plan.candidates,
            primaryResults + EntryNodeProbeResult(keyB, "9.9.9.9", 6200, 1_100),
        )
    assertEquals(descriptor(keyB, "9.9.9.9", "6200"), recovered[1].runtimeDescriptor())
  }

  @Test
  fun slowRetryIsHardBoundedToFourIdentitiesAndTheirAdvertisedPorts() {
    val candidates =
        listOf(
                keyA to "8.8.8.8",
                keyB to "9.9.9.9",
                keyC to "1.1.1.1",
                keyD to "4.2.2.2",
                keyE to "208.67.222.222",
            )
            .mapIndexed { index, (key, host) ->
              EntryNodeDiscoverySelection.parse(
                  descriptor(
                      key,
                      host,
                      "${4100 + index}/${5100 + index}/${6100 + index}/${7100 + index}",
                  ),
                  CHAIN,
              )!!
            }
    val plan = planEntryNodeProbes(candidates, maxIdentities = 4, generation = 0)
    val primaryResults =
        plan.primaryTargets.map { target ->
          EntryNodeProbeResult(
              target.candidate.publicKey,
              target.candidate.host,
              target.port,
              null,
          )
        }

    val slowTargets = planSlowEntryNodeProbeTargets(plan, primaryResults, 2)

    assertEquals(4, slowTargets.map { it.candidate.identity() }.distinct().size)
    assertEquals(16, slowTargets.size)
    assertTrue(slowTargets.groupBy { it.candidate.identity() }.values.all { it.size == 4 })
    assertFalse(slowTargets.any { it.candidate.publicKey == keyE })
  }

  @Test
  fun rotatesThePrimaryPortByGenerationWithoutAddingProbeTargets() {
    val candidate =
        EntryNodeDiscoverySelection.parse(
            descriptor(keyA, "8.8.8.8", "4100/4200/4300/4400"),
            CHAIN,
        )!!

    val generationZero =
        planEntryNodeProbes(listOf(candidate), maxIdentities = 4, generation = 0)
    val generationOne =
        planEntryNodeProbes(listOf(candidate), maxIdentities = 4, generation = 1)
    val generationFour =
        planEntryNodeProbes(listOf(candidate), maxIdentities = 4, generation = 4)

    assertEquals(listOf(4100), generationZero.primaryTargets.map { it.port })
    assertEquals(listOf(4200), generationOne.primaryTargets.map { it.port })
    assertEquals(listOf(4100), generationFour.primaryTargets.map { it.port })
    assertEquals(listOf(4300, 4400, 4100), generationOne.fallbackTargets.map { it.port })
    assertEquals(4, generationOne.primaryTargets.size + generationOne.fallbackTargets.size)
  }

  @Test
  fun aFailedRoutePairMovesBehindReachableAlternativesWithoutBeingExcluded() {
    val candidates =
        listOf(
                keyA to "8.8.8.8",
                keyB to "9.9.9.9",
                keyC to "1.1.1.1",
                keyD to "4.2.2.2",
            )
            .mapIndexed { index, (key, host) ->
              EntryNodeDiscoverySelection.parse(
                  descriptor(key, host, "${4100 + index}"),
                  CHAIN,
              )!!
            }
    val reachable =
        candidates.mapIndexed { index, candidate ->
          EntryNodeReachability(
              candidate = candidate,
              reachablePorts = listOf(EntryNodePortLatency(candidate.ports.first(), 20 + index)),
          )
        }
    val attempted =
        attemptedEntryNodeIdentities(
            CHAIN,
            listOf(candidates[0].persistentDescriptor(), candidates[1].persistentDescriptor()),
        )

    val rotated = deprioritizeAttemptedEntryNodes(reachable, attempted)

    assertEquals(listOf(keyC, keyD, keyA, keyB), rotated.map { it.candidate.publicKey })
    assertEquals(4, rotated.size)
    assertEquals(
        listOf(keyA, keyB),
        deprioritizeAttemptedEntryNodes(reachable.take(2), attempted)
            .map { it.candidate.publicKey },
    )
  }

  @Test
  fun knownGoodPreferenceStillYieldsToARecentStageOneRouteFailure() {
    val candidates =
        listOf(
                keyA to "8.8.8.8",
                keyB to "9.9.9.9",
                keyC to "1.1.1.1",
            )
            .mapIndexed { index, (key, host) ->
              EntryNodeDiscoverySelection.parse(
                  descriptor(key, host, "${4100 + index}"),
                  CHAIN,
              )!!
            }
    val reachable =
        candidates.mapIndexed { index, candidate ->
          EntryNodeReachability(
              candidate = candidate,
              reachablePorts =
                  listOf(EntryNodePortLatency(candidate.ports.first(), 20 + index * 100)),
          )
        }
    val knownGood = setOf(candidates[1].identity(), candidates[2].identity())
    val recentFailure = setOf(candidates[1].identity())

    val preferred = prioritizeKnownGoodEntryNodes(reachable, knownGood)
    val recovered = deprioritizeAttemptedEntryNodes(preferred, recentFailure)

    assertEquals(listOf(keyB, keyC, keyA), preferred.map { it.candidate.publicKey })
    assertEquals(listOf(keyC, keyA, keyB), recovered.map { it.candidate.publicKey })
  }

  @Test
  fun recentFailedEntryIdentitiesAccumulateAndExpireIndependently() {
    val identities =
        listOf(
                keyA to "8.8.8.8",
                keyB to "9.9.9.9",
                keyC to "1.1.1.1",
                keyD to "4.2.2.2",
            )
            .map { (key, host) ->
              EntryNodeDiscoverySelection.parse(descriptor(key, host, "4100"), CHAIN)!!
                  .identity()
            }
    val tracker = RecentEntryNodeFailureTracker(retentionMs = 120_000, maximumIdentities = 8)

    assertEquals(
        setOf(identities[0], identities[1]),
        tracker.record(CHAIN, identities.take(2).toSet(), nowEpochMs = 1_000),
    )
    assertEquals(
        identities.toSet(),
        tracker.record(CHAIN, identities.drop(2).toSet(), nowEpochMs = 61_000),
    )
    assertEquals(
        setOf(identities[2], identities[3]),
        tracker.identities(CHAIN, nowEpochMs = 121_001),
    )
    assertTrue(tracker.identities(CHAIN, nowEpochMs = 181_000).isEmpty())
  }

  @Test
  fun recentFailedEntryIdentitiesEvictTheOldestIdentityAtTheBound() {
    val identities =
        listOf(
                keyA to "8.8.8.8",
                keyB to "9.9.9.9",
                keyC to "1.1.1.1",
                keyD to "4.2.2.2",
            )
            .map { (key, host) ->
              EntryNodeDiscoverySelection.parse(descriptor(key, host, "4100"), CHAIN)!!
                  .identity()
            }
    val tracker = RecentEntryNodeFailureTracker(retentionMs = 120_000, maximumIdentities = 3)

    tracker.record(CHAIN, identities.take(2).toSet(), nowEpochMs = 1_000)
    val accumulated = tracker.record(CHAIN, identities.drop(2).toSet(), nowEpochMs = 2_000)

    assertEquals(listOf(identities[1], identities[2], identities[3]), accumulated.toList())
  }

  @Test
  fun knownGoodStorageExpiresRejectsAndBoundsThreePublicDescriptorsDeterministically() {
    val now = 1_000_000L
    val validA = descriptor(keyA, "8.8.8.8", "4100/4200")
    val validB = descriptor(keyB, "9.9.9.9", "4300")
    val validC = descriptor(keyF, "8.26.56.26", "4700")
    val serialized =
        encodeKnownGoodEntryNodes(
            listOf(
                KnownGoodEntryNode(validA, now + 1_000),
                KnownGoodEntryNode(validA, now + 2_000),
                KnownGoodEntryNode(descriptor(keyC, "192.168.1.1", "4400"), now + 1_000),
                KnownGoodEntryNode(descriptor(keyD, "4.2.2.2", "4500"), now - 1),
                KnownGoodEntryNode(descriptor(keyE, "208.67.222.222", "4600"), now + 20_000),
                KnownGoodEntryNode(validB, now + 3_000),
                KnownGoodEntryNode(validC, now + 4_000),
            ))

    val retained =
        decodeKnownGoodEntryNodes(
            serialized = serialized,
            chain = CHAIN,
            nowEpochMs = now,
            maxCandidates = 3,
            maximumAcceptedFutureMs = 5_000,
        )

    assertEquals(listOf(validA, validB, validC), retained.map(KnownGoodEntryNode::descriptor))
    assertEquals(3, retained.size)
    assertEquals(retained, decodeKnownGoodEntryNodes(
        serialized = encodeKnownGoodEntryNodes(retained),
        chain = CHAIN,
        nowEpochMs = now,
        maxCandidates = 3,
        maximumAcceptedFutureMs = 5_000,
    ))
    assertTrue(
        decodeKnownGoodEntryNodes(
                serialized = "not-json",
                chain = CHAIN,
                nowEpochMs = now,
                maxCandidates = 3,
                maximumAcceptedFutureMs = 5_000,
            )
            .isEmpty())
    assertTrue(
        decodeKnownGoodEntryNodes(
                serialized =
                    org.json.JSONArray()
                        .put(
                            org.json.JSONObject()
                                .put("descriptor", validA)
                                .put("expiresAtEpochMs", "${now + 1_000}"))
                        .toString(),
                chain = CHAIN,
                nowEpochMs = now,
                maxCandidates = 3,
                maximumAcceptedFutureMs = 5_000,
            )
            .isEmpty())
  }

  @Test
  fun fastKnownGoodCacheMergeRetainsStandbysAndValidatedPortVariants() {
    val merged =
        mergeEntryNodeCache(
            chain = CHAIN,
            preferredDescriptors =
                listOf(
                    descriptor(keyA, "8.8.8.8", "4200/4100"),
                    descriptor(keyB, "9.9.9.9", "5200"),
                ),
            cachedDescriptors =
                listOf(
                    descriptor(keyC, "1.1.1.1", "6100"),
                    descriptor(keyD, "4.2.2.2", "6200"),
                    descriptor(keyA, "8.8.8.8", "4100/4300"),
                ),
            limit = 6,
        )

    assertEquals(listOf(keyA, keyB, keyC, keyD), merged.map { descriptor ->
      EntryNodeDiscoverySelection.parse(descriptor, CHAIN)!!.publicKey
    })
    assertEquals(
        listOf(4200, 4100, 4300),
        EntryNodeDiscoverySelection.parse(merged.first(), CHAIN)!!.ports,
    )
  }

  @Test
  fun cacheMergeDropsMalformedPrivateAndDuplicateDataBeforeApplyingItsBound() {
    val merged =
        mergeEntryNodeCache(
            chain = CHAIN,
            preferredDescriptors = emptyList(),
            cachedDescriptors =
                listOf(
                    "not-a-descriptor",
                    descriptor(keyA, "192.168.1.1", "4100"),
                    descriptor(keyC, "1.1.1.1", "4300"),
                    descriptor(keyC, "1.1.1.1", "4300"),
                    descriptor(keyC, "1.1.1.1", "4400"),
                    descriptor(keyD, "4.2.2.2", "4500"),
                    descriptor(keyE, "208.67.222.222", "4600"),
                ),
            limit = 2,
        )

    assertEquals(2, merged.size)
    assertEquals(listOf(keyC, keyD), merged.map { descriptor ->
      EntryNodeDiscoverySelection.parse(descriptor, CHAIN)!!.publicKey
    })
    assertEquals(
        listOf(4300, 4400),
        EntryNodeDiscoverySelection.parse(merged.first(), CHAIN)!!.ports,
    )
  }

  @Test
  fun quarantinedHandshakeFailuresDoNotOccupyTheNextRuntimeSelection() {
    val candidates =
        EntryNodeDiscoverySelection.select(
            chain = CHAIN,
            freshDescriptors =
                listOf(
                    descriptor(keyA, "8.8.8.8", "4100"),
                    descriptor(keyB, "9.9.9.9", "4200"),
                    descriptor(keyC, "1.1.1.1", "4300"),
                ),
            preferredDescriptors = emptyList(),
            cachedDescriptors = emptyList(),
            limit = 3,
        )

    val filtered =
        excludeQuarantinedEntryNodes(
            chain = CHAIN,
            candidates = candidates,
            quarantinedDescriptors =
                listOf(descriptor(keyA, "8.8.8.8", "4100/4400")),
            limit = 2,
        )

    assertEquals(listOf(keyB, keyC), filtered.map(EntryNodeCandidate::publicKey))
  }

  @Test
  fun aSmallFullyQuarantinedPoolRemainsAvailableAsALastResort() {
    val candidates =
        listOf(
            EntryNodeDiscoverySelection.parse(
                descriptor(keyA, "8.8.8.8", "4100"),
                CHAIN,
            )!!,
            EntryNodeDiscoverySelection.parse(
                descriptor(keyB, "9.9.9.9", "4200"),
                CHAIN,
            )!!,
        )

    val selected =
        excludeQuarantinedEntryNodes(
            chain = CHAIN,
            candidates = candidates,
            quarantinedDescriptors = candidates.map(EntryNodeCandidate::persistentDescriptor),
            limit = 4,
            minimumCandidates = 2,
        )

    assertEquals(listOf(keyA, keyB), selected.map(EntryNodeCandidate::publicKey))
  }

  @Test
  fun quarantineFallbackRetainsABoundedProbePoolAfterHealthyCandidates() {
    val candidates =
        listOf(
            EntryNodeDiscoverySelection.parse(
                descriptor(keyA, "8.8.8.8", "4100"),
                CHAIN,
            )!!,
            EntryNodeDiscoverySelection.parse(
                descriptor(keyB, "9.9.9.9", "4200"),
                CHAIN,
            )!!,
            EntryNodeDiscoverySelection.parse(
                descriptor(keyC, "1.1.1.1", "4300"),
                CHAIN,
            )!!,
        )
    val selected =
        excludeQuarantinedEntryNodes(
            chain = CHAIN,
            candidates = candidates,
            quarantinedDescriptors =
                listOf(candidates[0].persistentDescriptor(), candidates[1].persistentDescriptor()),
            limit = 4,
            minimumCandidates = 2,
        )

    assertEquals(listOf(keyC, keyA, keyB), selected.map(EntryNodeCandidate::publicKey))
  }

  @Test
  fun reachableQuarantinedStandbyCompletesAOneOfTwoEligibleProbeFailure() {
    val candidates =
        listOf(
                keyA to "8.8.8.8",
                keyB to "9.9.9.9",
                keyC to "1.1.1.1",
            )
            .mapIndexed { index, (key, host) ->
              EntryNodeDiscoverySelection.parse(
                  descriptor(key, host, "${4100 + index}"),
                  CHAIN,
              )!!
            }
    val quarantinedStandby =
        EntryNodeDiscoverySelection.parse(
            descriptor(keyC, "1.1.1.1", "4102"),
            CHAIN,
        )!!
    val phases =
        planEntryNodeProbePhases(
            chain = CHAIN,
            // The normal ranking returned exactly two eligible candidates. The persisted
            // quarantined identity must still be retained as a separate second-phase standby.
            candidates = candidates.take(2),
            quarantinedDescriptors = listOf(quarantinedStandby.persistentDescriptor()),
            maximumHealthyIdentities = 8,
            maximumQuarantinedStandbyIdentities = 2,
            recoveryGeneration = 0,
        )
    // A is reachable and B is not; an absent result represents the failed TCP probe.
    val primaryReachable =
        listOf(
            EntryNodeReachability(
                candidate = candidates[0],
                reachablePorts = listOf(EntryNodePortLatency(port = 4100, latencyMs = 20)),
            ))
    var standbyProbeCount = 0

    val reachable =
        supplementWithQuarantinedEntryNodeStandbys(
            primaryReachable = primaryReachable,
            standbyCandidates = phases.quarantinedStandbyCandidates,
            requiredReachableIdentities = 2,
        ) { standbys, requiredReachableIdentities ->
          standbyProbeCount += 1
          assertEquals(1, requiredReachableIdentities)
          assertEquals(listOf(keyC), standbys.map(EntryNodeCandidate::publicKey))
          listOf(
              EntryNodeReachability(
                  candidate = standbys.single(),
                  reachablePorts =
                      listOf(EntryNodePortLatency(port = 4102, latencyMs = 30)),
              ))
        }

    assertEquals(listOf(keyA, keyB), phases.healthyCandidates.map { it.publicKey })
    assertEquals(1, standbyProbeCount)
    assertEquals(listOf(keyA, keyC), reachable.map { it.candidate.publicKey })
  }

  @Test
  fun fullyQuarantinedProbeWindowAdvancesOneIdentityBeforeApplyingItsBound() {
    val candidates =
        listOf(
                keyA to "8.8.8.8",
                keyB to "9.9.9.9",
                keyC to "1.1.1.1",
                keyD to "4.2.2.2",
                keyE to "208.67.222.222",
                keyF to "8.26.56.26",
            )
            .map { (key, host) ->
              EntryNodeDiscoverySelection.parse(descriptor(key, host, "4100"), CHAIN)!!
            }
    val quarantine = candidates.map(EntryNodeCandidate::persistentDescriptor)

    val generationZero =
        excludeQuarantinedEntryNodes(
            chain = CHAIN,
            candidates = candidates.reversed(),
            quarantinedDescriptors = quarantine,
            limit = 4,
            minimumCandidates = 2,
            recoveryGeneration = 0,
        )
    val generationOne =
        excludeQuarantinedEntryNodes(
            chain = CHAIN,
            candidates = candidates.reversed(),
            quarantinedDescriptors = quarantine,
            limit = 4,
            minimumCandidates = 2,
            recoveryGeneration = 1,
        )
    val generationTwo =
        excludeQuarantinedEntryNodes(
            chain = CHAIN,
            candidates = candidates.reversed(),
            quarantinedDescriptors = quarantine,
            limit = 4,
            minimumCandidates = 2,
            recoveryGeneration = 2,
        )

    assertEquals(listOf(keyA, keyB, keyC, keyD), generationZero.map { it.publicKey })
    assertEquals(listOf(keyB, keyC, keyD, keyE), generationOne.map { it.publicKey })
    assertEquals(listOf(keyC, keyD, keyE, keyF), generationTwo.map { it.publicKey })
  }

  @Test
  fun quarantinedCandidatesNeverDisplaceAHealthyReachableCandidate() {
    val candidates =
        listOf(
                keyA to "8.8.8.8",
                keyB to "9.9.9.9",
                keyC to "1.1.1.1",
                keyD to "4.2.2.2",
            )
            .mapIndexed { index, (key, host) ->
              EntryNodeDiscoverySelection.parse(
                  descriptor(key, host, "${4100 + index}"),
                  CHAIN,
              )!!
            }
    val reachable =
        candidates.mapIndexed { index, candidate ->
          EntryNodeReachability(
              candidate = candidate,
              reachablePorts =
                  listOf(EntryNodePortLatency(candidate.ports.first(), 10 + index)),
          )
        }
    val quarantined = candidates.take(3).map(EntryNodeCandidate::identity).toSet()

    val ranked =
        prioritizeEntryNodesForRecovery(
            reachable = reachable,
            quarantinedIdentities = quarantined,
            recentlyAttemptedIdentities = emptySet(),
            generation = 3,
        )

    assertEquals(keyD, ranked.first().candidate.publicKey)
    assertEquals(setOf(keyA, keyB, keyC), ranked.drop(1).map { it.candidate.publicKey }.toSet())
  }

  @Test
  fun rotatesACompletelySuspectPoolByOneIdentityPerRetry() {
    val candidates =
        listOf(
                keyA to "8.8.8.8",
                keyB to "9.9.9.9",
                keyC to "1.1.1.1",
                keyD to "4.2.2.2",
                keyE to "208.67.222.222",
                keyF to "8.26.56.26",
            )
            .mapIndexed { index, (key, host) ->
              EntryNodeDiscoverySelection.parse(
                  descriptor(key, host, "${4100 + index}"),
                  CHAIN,
              )!!
            }
    // Deliberately reverse the concurrent-probe result order. Recovery ordering must depend only
    // on public identity and generation, not completion timing or exact latency.
    val reachable =
        candidates.reversed().mapIndexed { index, candidate ->
          EntryNodeReachability(
              candidate = candidate,
              reachablePorts =
                  listOf(EntryNodePortLatency(candidate.ports.first(), 10 + index * 100)),
          )
        }
    val suspect = candidates.map(EntryNodeCandidate::identity).toSet()

    val pairs =
        (0..3).map { generation ->
          prioritizeEntryNodesForRecovery(
                  reachable = reachable,
                  quarantinedIdentities = suspect,
                  recentlyAttemptedIdentities = suspect,
                  generation = generation,
              )
              .take(2)
              .map { entry -> entry.candidate.publicKey }
        }

    assertEquals(listOf(keyA, keyB), pairs[0])
    assertEquals(listOf(keyB, keyC), pairs[1])
    assertEquals(listOf(keyC, keyD), pairs[2])
    assertEquals(listOf(keyD, keyE), pairs[3])
    assertEquals(1, pairs[0].intersect(pairs[1].toSet()).size)
    assertEquals(1, pairs[1].intersect(pairs[2].toSet()).size)
  }

  @Test
  fun keepsAHealthyEntryFirstWhileRotatingTwoSuspectAlternatives() {
    val candidates =
        listOf(
                keyA to "8.8.8.8",
                keyB to "9.9.9.9",
                keyC to "1.1.1.1",
            )
            .mapIndexed { index, (key, host) ->
              EntryNodeDiscoverySelection.parse(
                  descriptor(key, host, "${4100 + index}"),
                  CHAIN,
              )!!
            }
    val reachable =
        candidates.map { candidate ->
          EntryNodeReachability(
              candidate = candidate,
              reachablePorts = listOf(EntryNodePortLatency(candidate.ports.first(), 20)),
          )
        }
    val quarantined = candidates.drop(1).map(EntryNodeCandidate::identity).toSet()

    val selections =
        (0..1).map { generation ->
          prioritizeEntryNodesForRecovery(
                  reachable = reachable,
                  quarantinedIdentities = quarantined,
                  recentlyAttemptedIdentities = emptySet(),
                  generation = generation,
              )
              .take(2)
              .map { entry -> entry.candidate.publicKey }
        }

    assertEquals(listOf(keyA, keyB), selections[0])
    assertEquals(listOf(keyA, keyC), selections[1])
  }

  @Test
  fun sameNetworkRecoveryAdvancesThroughUntriedQuarantinedStandbysBeforeTheFailedPair() {
    val candidates =
        listOf(
                keyA to "8.8.8.8",
                keyB to "9.9.9.9",
                keyC to "1.1.1.1",
                keyD to "4.2.2.2",
                keyE to "208.67.222.222",
                keyF to "8.26.56.26",
            )
            .mapIndexed { index, (key, host) ->
              EntryNodeDiscoverySelection.parse(
                  descriptor(key, host, "${4100 + index}"),
                  CHAIN,
              )!!
            }
    val reachable =
        candidates.map { candidate ->
          EntryNodeReachability(
              candidate = candidate,
              reachablePorts = listOf(EntryNodePortLatency(candidate.ports.first(), 20)),
          )
        }
    val quarantined = candidates.map(EntryNodeCandidate::identity).toSet()
    val failedPair = candidates.take(2).map(EntryNodeCandidate::identity).toSet()

    val firstRecoveryPair =
        prioritizeEntryNodesForRecovery(
                reachable = reachable,
                quarantinedIdentities = quarantined,
                recentlyAttemptedIdentities = failedPair,
                generation = 4,
            )
            .take(2)
            .map { entry -> entry.candidate.publicKey }
    val nextRecoveryPair =
        prioritizeEntryNodesForRecovery(
                reachable = reachable,
                quarantinedIdentities = quarantined,
                recentlyAttemptedIdentities = failedPair,
                generation = 5,
            )
            .take(2)
            .map { entry -> entry.candidate.publicKey }

    assertEquals(listOf(keyC, keyD), firstRecoveryPair)
    assertEquals(listOf(keyD, keyE), nextRecoveryPair)
    assertTrue(firstRecoveryPair.none { key -> key == keyA || key == keyB })
    assertTrue(nextRecoveryPair.none { key -> key == keyA || key == keyB })
  }

  @Test(expected = IllegalArgumentException::class)
  fun quarantineFilteringRejectsAnUnboundedZeroLimit() {
    excludeQuarantinedEntryNodes(
        chain = CHAIN,
        candidates = emptyList(),
        quarantinedDescriptors = emptyList(),
        limit = 0,
    )
  }

  @Test
  fun ranksOnlyReachableEntriesByCoarseLatencyBandAndCandidateOrder() {
    val nodeA =
        EntryNodeDiscoverySelection.parse(
            descriptor(keyA, "8.8.8.8", "4100/4200/4300"),
            CHAIN,
        )!!
    val nodeB =
        EntryNodeDiscoverySelection.parse(
            descriptor(keyB, "9.9.9.9", "5100/5200"),
            CHAIN,
        )!!
    val nodeC =
        EntryNodeDiscoverySelection.parse(descriptor(keyC, "1.1.1.1", "6100"), CHAIN)!!

    val ranked =
        rankReachableEntryNodes(
            candidates = listOf(nodeA, nodeB, nodeC),
            results =
                listOf(
                    EntryNodeProbeResult(keyA, "8.8.8.8", 4100, 80),
                    EntryNodeProbeResult(keyA, "8.8.8.8", 4200, 25),
                    EntryNodeProbeResult(keyA, "8.8.8.8", 4300, null),
                    EntryNodeProbeResult(keyB, "9.9.9.9", 5100, 15),
                    EntryNodeProbeResult(keyB, "9.9.9.9", 5200, 35),
                    EntryNodeProbeResult(keyC, "1.1.1.1", 6100, null),
                ),
        )

    assertEquals(listOf(keyA, keyB), ranked.map { result -> result.candidate.publicKey })
    assertEquals(descriptor(keyA, "8.8.8.8", "4100"), ranked[0].runtimeDescriptor())
    assertEquals(descriptor(keyB, "9.9.9.9", "5100"), ranked[1].runtimeDescriptor())
    assertEquals(descriptor(keyA, "8.8.8.8", "4100/4200/4300"), ranked[0].persistentDescriptor())
  }

  @Test
  fun aClearlyFasterLatencyBandWinsWhileTiesStayDeterministic() {
    val nodes =
        listOf(
            EntryNodeDiscoverySelection.parse(descriptor(keyA, "8.8.8.8", "4100"), CHAIN)!!,
            EntryNodeDiscoverySelection.parse(descriptor(keyB, "9.9.9.9", "4200"), CHAIN)!!,
            EntryNodeDiscoverySelection.parse(descriptor(keyC, "1.1.1.1", "4300"), CHAIN)!!,
        )

    val ranked =
        rankReachableEntryNodes(
            candidates = nodes,
            results =
                listOf(
                    EntryNodeProbeResult(keyA, "8.8.8.8", 4100, 190),
                    EntryNodeProbeResult(keyB, "9.9.9.9", 4200, 110),
                    EntryNodeProbeResult(keyC, "1.1.1.1", 4300, 90),
                ),
        )

    assertEquals(listOf(keyC, keyA, keyB), ranked.map { it.candidate.publicKey })
    assertEquals(0, entryNodeProbeLatencyBand(ranked[0].bestLatencyMs))
    assertEquals(1, entryNodeProbeLatencyBand(ranked[1].bestLatencyMs))
    assertEquals(1, entryNodeProbeLatencyBand(ranked[2].bestLatencyMs))
  }

  @Test
  fun configuresUpToThreeReachableEntriesAndKeepsAdditionalCacheStandbys() {
    val nodes =
        listOf(
            EntryNodeDiscoverySelection.parse(descriptor(keyA, "8.8.8.8", "4100"), CHAIN)!!,
            EntryNodeDiscoverySelection.parse(descriptor(keyB, "9.9.9.9", "4200"), CHAIN)!!,
            EntryNodeDiscoverySelection.parse(descriptor(keyC, "1.1.1.1", "4300"), CHAIN)!!,
            EntryNodeDiscoverySelection.parse(descriptor(keyD, "4.2.2.2", "4400"), CHAIN)!!,
        )
    val reachable =
        nodes.mapIndexed { index, candidate ->
          EntryNodeReachability(
              candidate = candidate,
              reachablePorts =
                  listOf(EntryNodePortLatency(candidate.ports.first(), 10 + index)),
          )
        }

    val result =
        EntryNodeDiscoveryResult.fromReachability(
            selected = reachable.take(3),
            cache = reachable,
        )

    assertEquals(3, result.runtimeDescriptors.size)
    assertEquals(3, result.persistentDescriptors.size)
    assertEquals(4, result.cacheDescriptors.size)
  }

  @Test
  fun validatesChainCanonicalKeyAndPortBounds() {
    assertNull(EntryNodeDiscoverySelection.parse(descriptor(keyA, "8.8.8.8", "4100"), "Base"))
    assertNull(
        EntryNodeDiscoverySelection.parse(
            "masq://other-mainnet:$keyA@8.8.8.8:4100",
            CHAIN,
        ))
    assertNull(
        EntryNodeDiscoverySelection.parse(
            descriptor("short-key", "8.8.8.8", "4100"),
            CHAIN,
        ))
    assertNull(
        EntryNodeDiscoverySelection.parse(
            descriptor("BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB", "8.8.8.8", "4100"),
            CHAIN,
        ))
    assertNull(EntryNodeDiscoverySelection.parse(descriptor(keyA, "8.8.8.8", "1024"), CHAIN))
    assertNull(EntryNodeDiscoverySelection.parse(descriptor(keyA, "8.8.8.8", "65536"), CHAIN))
    assertNull(
        EntryNodeDiscoverySelection.parse(
            descriptor(keyA, "8.8.8.8", "4100/4200/4300/4400/4500"),
            CHAIN,
        ))
    assertNull(EntryNodeDiscoverySelection.parse(descriptor(keyA, "8.8.8.8", "04100"), CHAIN))
    assertTrue(EntryNodeDiscoverySelection.isCanonicalChain(CHAIN))
  }

  @Test
  fun acceptsOnlyPublicIpv4Literals() {
    listOf(
            "8.8.8.8",
            "1.1.1.1",
            "45.76.232.183",
            "223.255.255.254",
        )
        .forEach { host -> assertTrue(host, EntryNodeDiscoverySelection.isPublicIpv4Literal(host)) }

    listOf(
            "example.org",
            "::1",
            "0.1.2.3",
            "10.0.0.1",
            "100.64.0.1",
            "127.0.0.1",
            "169.254.1.1",
            "172.16.0.1",
            "192.0.0.9",
            "192.0.2.1",
            "192.88.99.1",
            "192.168.1.1",
            "198.18.0.1",
            "198.51.100.1",
            "203.0.113.1",
            "224.0.0.1",
            "240.0.0.1",
            "255.255.255.255",
            "008.8.8.8",
        )
        .forEach { host -> assertFalse(host, EntryNodeDiscoverySelection.isPublicIpv4Literal(host)) }
  }

  @Test
  fun refreshesDiscoveryForEveryStartThatIsNotAlreadyConnected() {
    listOf("", "unconfigured", "ready", "connecting", "paused", "error", "blocked")
        .forEach { phase -> assertTrue(phase, shouldDiscoverEntryNodesBeforeStart(phase)) }
    assertFalse(shouldDiscoverEntryNodesBeforeStart("connected"))
  }

  @Test
  fun persistentlyQuarantinesOnlyAggregateEntryTransportOrHandshakeFailures() {
    listOf(
            "E_ENTRY_TCP_FAILED",
            "E_ENTRY_GOSSIP_PASS_LOOP",
            "E_ENTRY_NO_PROGRESS",
        )
        .forEach { code ->
          assertTrue(
              code,
              shouldQuarantineAttemptedEntryNodes(
                  phase = "error",
                  engineGeneration = 4,
                  routeStage = 0,
                  lastError = "$code: redacted diagnostic",
              ),
          )
        }

    assertTrue(
        shouldQuarantineAttemptedEntryNodes(
            phase = "connecting",
            engineGeneration = 4,
            routeStage = 0,
            lastError = "E_ENTRY_NO_PROGRESS: redacted diagnostic",
        ))
  }

  @Test
  fun transientEntryEvidenceIsDeprioritizedWithoutPersistentQuarantine() {
    listOf(
            "E_ENTRY_TCP_WAITING_GOSSIP",
            "E_ENTRY_GOSSIP_TIMEOUT",
            "E_ENTRY_DEBUT_NOT_WRITTEN",
            "E_ENTRY_NO_INBOUND_BYTES",
            "E_ENTRY_INBOUND_NOT_ACCEPTED",
            "E_ENTRY_GOSSIP_NOT_PROMOTED",
        )
        .forEach { code ->
          val lastError = "$code: redacted diagnostic"
          assertFalse(
              code,
              shouldQuarantineAttemptedEntryNodes(
                  phase = "error",
                  engineGeneration = 4,
                  routeStage = 0,
                  lastError = lastError,
              ),
          )
          assertTrue(
              code,
              shouldDeprioritizeAttemptedEntryNodes(
                  phase = "error",
                  engineGeneration = 4,
                  routeStage = 0,
                  lastError = lastError,
              ),
          )
        }
  }

  @Test
  fun doesNotQuarantineExitProofDiscoveryWalletOrUnclassifiedFailures() {
    listOf(
            null,
            "E_ENTRY_NODE_DISCOVERY: no candidates",
            "E_PRIVATE_ROUTE_FAILED: exit proof failed",
            "E_PRIVATE_ROUTE_TIMEOUT: exit proof timed out",
            "E_PROXY_TLS_FAILED: TLS failed",
            "E_WALLET_LOCKED: wallet needs attention",
            "E_CORE_EARLY_EXIT: core stopped",
            "unclassified failure",
        )
        .forEach { lastError ->
          assertFalse(
              lastError,
              shouldQuarantineAttemptedEntryNodes(
                  phase = "error",
                  engineGeneration = 4,
                  routeStage = 0,
                  lastError = lastError,
              ),
          )
        }
  }

  @Test
  fun doesNotQuarantineAProvenEntryOrAnUnstartedGeneration() {
    assertFalse(
        shouldQuarantineAttemptedEntryNodes(
            phase = "ready",
            engineGeneration = 0,
            routeStage = 0,
            lastError = "E_ENTRY_TCP_FAILED: redacted diagnostic",
        ))
    assertFalse(
        shouldQuarantineAttemptedEntryNodes(
            phase = "connected",
            engineGeneration = 4,
            routeStage = 2,
            lastError = "E_ENTRY_NO_PROGRESS: stale diagnostic",
        ))
    assertFalse(
        shouldQuarantineAttemptedEntryNodes(
            phase = "error",
            engineGeneration = 4,
            routeStage = 1,
            lastError = "E_ENTRY_NO_PROGRESS: stale diagnostic",
        ))
  }

  @Test
  fun deprioritizesAProvenStageOnePrivateRouteFailure() {
    assertTrue(
        shouldDeprioritizeAttemptedEntryNodes(
            phase = "error",
            engineGeneration = 4,
            routeStage = 1,
            lastError = "The MASQ exit route did not complete a TLS handshake.",
        ))
    assertTrue(
        shouldDeprioritizeAttemptedEntryNodes(
            phase = "error",
            engineGeneration = 4,
            routeStage = 1,
            lastError = "E_PRIVATE_ROUTE_TIMEOUT: redacted",
        ))
    assertFalse(
        shouldDeprioritizeAttemptedEntryNodes(
            phase = "error",
            engineGeneration = 4,
            routeStage = 0,
            lastError = "E_PRIVATE_ROUTE_FAILED: redacted",
        ))
    assertFalse(
        shouldDeprioritizeAttemptedEntryNodes(
            phase = "error",
            engineGeneration = 4,
            routeStage = 1,
            lastError = "E_WALLET_LOCKED: redacted",
        ))
    assertFalse(
        shouldDeprioritizeAttemptedEntryNodes(
            phase = "connecting",
            engineGeneration = 4,
            routeStage = 1,
            lastError = null,
        ))
  }

  @Test
  fun transientlyDeprioritizesOnlyAGenuineSupersededStageZeroAttempt() {
    assertTrue(
        shouldDeprioritizeAttemptedEntryNodes(
            phase = "connecting",
            engineGeneration = 4,
            routeStage = 0,
            lastError = null,
        ))
    assertFalse(
        shouldQuarantineAttemptedEntryNodes(
            phase = "connecting",
            engineGeneration = 4,
            routeStage = 0,
            lastError = null,
        ))

    listOf<Triple<String, Long, String?>>(
            Triple("connecting", 0L, null),
            Triple("ready", 4L, null),
            Triple("connected", 4L, null),
            Triple("connecting", 4L, "E_WALLET_LOCKED: redacted"),
        )
        .forEach { (phase, generation, lastError) ->
          assertFalse(
              shouldDeprioritizeAttemptedEntryNodes(
                  phase = phase,
                  engineGeneration = generation,
                  routeStage = 0,
                  lastError = lastError,
              ))
        }
  }

  private fun descriptor(key: String, host: String, ports: String): String =
      "masq://$CHAIN:$key@$host:$ports"

  private companion object {
    const val CHAIN = "base-mainnet"
  }
}
