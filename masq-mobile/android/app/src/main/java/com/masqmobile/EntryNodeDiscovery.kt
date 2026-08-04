package com.masqmobile

import android.content.Context
import android.util.Log
import java.io.IOException
import java.io.InterruptedIOException
import java.net.ConnectException
import java.net.InetSocketAddress
import java.net.ProtocolException
import java.net.Socket
import java.net.SocketException
import java.net.SocketTimeoutException
import java.net.UnknownHostException
import java.net.UnknownServiceException
import java.nio.charset.StandardCharsets
import java.util.UUID
import java.util.concurrent.Callable
import java.util.concurrent.ExecutorCompletionService
import java.util.concurrent.Executors
import java.util.concurrent.Future
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicInteger
import java.util.concurrent.locks.ReentrantLock
import javax.net.ssl.SSLException
import okhttp3.CacheControl
import okhttp3.ConnectionSpec
import okhttp3.HttpUrl
import okhttp3.HttpUrl.Companion.toHttpUrlOrNull
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.ResponseBody
import org.json.JSONArray

internal class EntryNodeDiscoveryCancelledException : RuntimeException()

internal class EntryNodeDiscoveryGate(
    private val pollIntervalMs: Long = 100L,
) {
  private val lock = ReentrantLock(true)

  init {
    require(pollIntervalMs > 0L)
  }

  fun <T> run(isCurrent: () -> Boolean, operation: () -> T): T {
    while (true) {
      ensureCurrent(isCurrent)
      val acquired =
          try {
            lock.tryLock(pollIntervalMs, TimeUnit.MILLISECONDS)
          } catch (_: InterruptedException) {
            Thread.currentThread().interrupt()
            throw EntryNodeDiscoveryCancelledException()
          }
      if (acquired) break
    }
    try {
      ensureCurrent(isCurrent)
      return operation()
    } finally {
      lock.unlock()
    }
  }

  fun ensureCurrent(isCurrent: () -> Boolean) {
    if (Thread.currentThread().isInterrupted || !isCurrent()) {
      throw EntryNodeDiscoveryCancelledException()
    }
  }
}

internal class EntryNodeDiscovery(
    context: Context,
    private val portProbe: EntryNodePortProbe = SocketEntryNodePortProbe,
    private val epochMillis: () -> Long = System::currentTimeMillis,
) {
  private val preferences =
      context.getSharedPreferences("masq-mobile-consumer", Context.MODE_PRIVATE)
  private val discoveryGeneration = AtomicInteger(0)
  // A process-local nonce prevents an app restart from reusing the same finder/CDN cache keys.
  // It is random public request metadata and is never persisted or logged.
  private val finderSessionNonce = UUID.randomUUID().toString()
  private val recentRouteFailures =
      RecentEntryNodeFailureTracker(
          retentionMs = ROUTE_FAILURE_DEPRIORITIZATION_MS,
          maximumIdentities = MAX_RECENT_FAILURE_IDENTITIES,
      )
  private val httpClient =
      OkHttpClient.Builder()
          .connectTimeout(NODE_FINDER_TIMEOUT_MS, TimeUnit.MILLISECONDS)
          .readTimeout(NODE_FINDER_TIMEOUT_MS, TimeUnit.MILLISECONDS)
          .writeTimeout(NODE_FINDER_TIMEOUT_MS, TimeUnit.MILLISECONDS)
          .callTimeout(NODE_FINDER_CALL_TIMEOUT_MS, TimeUnit.MILLISECONDS)
          .followRedirects(false)
          .followSslRedirects(false)
          .retryOnConnectionFailure(true)
          .connectionSpecs(listOf(ConnectionSpec.MODERN_TLS))
          .build()

  /**
   * Temporarily excludes nodes with an explicit MASQ entry transport/handshake failure.
   * Descriptors are public network metadata; no wallet, destination, IP history, or device data is
   * stored. The bounded quarantine is shared with background recovery and expires automatically.
   */
  fun recordConnectionFailure(chain: String, attemptedDescriptors: List<String>) {
    if (!EntryNodeDiscoverySelection.isCanonicalChain(chain)) return
    val now = epochMillis()
    val retained = loadQuarantined(chain, now).toMutableList()
    val replacements =
        attemptedDescriptors
            .mapNotNull { descriptor -> EntryNodeDiscoverySelection.parse(descriptor, chain) }
            .distinctBy(EntryNodeCandidate::identity)
            .take(MAX_QUARANTINED_CANDIDATES)
            .map { candidate ->
              QuarantinedEntryNode(
                  descriptor = candidate.persistentDescriptor(),
                  untilEpochMs = now + ENTRY_FAILURE_QUARANTINE_MS,
              )
            }
    if (replacements.isEmpty()) return
    val replacementIdentities =
        replacements
            .mapNotNull { entry ->
              EntryNodeDiscoverySelection.parse(entry.descriptor, chain)?.identity()
            }
            .toSet()
    retained.removeAll { entry ->
      EntryNodeDiscoverySelection.parse(entry.descriptor, chain)?.identity() in
          replacementIdentities
    }
    val quarantined = (replacements + retained).take(MAX_QUARANTINED_CANDIDATES)
    saveQuarantined(chain, quarantined)
    removeKnownGood(chain, replacementIdentities, now)
    safeDiagnostic("NF_QUARANTINE_UPDATED", "count" to quarantined.size)
  }

  /**
   * Remembers only the bounded public entry pair behind a fully validated route. The descriptors
   * contain public MASQ peer metadata, never wallet, destination, device, or browsing data.
   */
  fun recordKnownGoodRoute(
      chain: String,
      descriptors: List<String>,
      status: MasqSessionCoreSnapshot?,
  ) {
    if (!EntryNodeDiscoverySelection.isCanonicalChain(chain)) return
    if (status?.isHealthyConnectedSession() != true) return
    val now = epochMillis()
    val verified =
        descriptors
            .mapNotNull { descriptor -> EntryNodeDiscoverySelection.parse(descriptor, chain) }
            .distinctBy(EntryNodeCandidate::identity)
            .take(MAX_RUNTIME_ENTRY_NODES)
            .map { candidate ->
              KnownGoodEntryNode(
                  descriptor = candidate.persistentDescriptor(),
                  expiresAtEpochMs = now + KNOWN_GOOD_TTL_MS,
              )
            }
    if (verified.isEmpty()) return

    val existing = loadKnownGood(chain, now)
    val sameDescriptors =
        existing.map(KnownGoodEntryNode::descriptor) ==
            verified.map(KnownGoodEntryNode::descriptor)
    val refreshDue =
        existing.any { entry -> entry.expiresAtEpochMs <= now + KNOWN_GOOD_REFRESH_WINDOW_MS }
    if (sameDescriptors && !refreshDue) return

    saveKnownGood(chain, verified)
    safeDiagnostic("NF_KNOWN_GOOD_UPDATED", "count" to verified.size)
  }

  /**
   * Briefly moves a superseded stage-zero or failed stage-one pair behind fresh alternatives
   * without adding it to the persistent handshake quarantine. This is an in-memory, bounded
   * ranking hint: a small node pool remains usable, and no destination or device metadata is
   * stored.
   */
  fun recordRouteProofFailure(chain: String, attemptedDescriptors: List<String>) {
    if (!EntryNodeDiscoverySelection.isCanonicalChain(chain)) return
    val identities = attemptedEntryNodeIdentities(chain, attemptedDescriptors)
    if (identities.isEmpty()) return
    val accumulated = recentRouteFailures.record(chain, identities, epochMillis())
    safeDiagnostic("NF_ROUTE_PAIR_DEPRIORITIZED", "count" to accumulated.size)
  }

  fun discover(
      chain: String,
      preferredNodes: List<String>,
      isCurrent: () -> Boolean = { !Thread.currentThread().isInterrupted },
  ): EntryNodeDiscoveryResult =
      SHARED_DISCOVERY_GATE.run(isCurrent) {
        discoverWhileOwned(chain, preferredNodes, isCurrent)
      }

  private fun discoverWhileOwned(
      chain: String,
      preferredNodes: List<String>,
      isCurrent: () -> Boolean,
  ): EntryNodeDiscoveryResult {
    val generation = discoveryGeneration.getAndIncrement()
    safeDiagnostic("NF_DISCOVERY_START", "generation" to generation)
    SHARED_DISCOVERY_GATE.ensureCurrent(isCurrent)
    if (!EntryNodeDiscoverySelection.isCanonicalChain(chain)) {
      safeDiagnostic("NF_CHAIN_REJECTED")
      throw EntryNodeDiscoveryException(
          "NF_CHAIN_REJECTED: The MASQ chain identifier is invalid."
      )
    }
    val now = epochMillis()
    val recentlyAttemptedIdentities = recentRouteFailureIdentities(chain, now)
    val knownGoodDescriptors =
        loadKnownGood(chain, now).map(KnownGoodEntryNode::descriptor)
    val quarantinedDescriptors =
        loadQuarantined(chain, now).map(QuarantinedEntryNode::descriptor)
    val quarantinedIdentities =
        attemptedEntryNodeIdentities(chain, quarantinedDescriptors)

    // Previously verified entries are the only path that can avoid another node-finder round
    // trip. It remains fail-closed: at least two distinct public identities are TCP-probed again,
    // quarantine excludes them, and a recent stage-one route failure forces fresh alternatives
    // into this attempt. A third stale backup must not delay two reachable known-good entries.
    val knownGoodCandidates =
        excludeQuarantinedEntryNodes(
            chain = chain,
            candidates =
                EntryNodeDiscoverySelection.select(
                    chain = chain,
                    freshDescriptors = emptyList(),
                    preferredDescriptors = emptyList(),
                    cachedDescriptors = emptyList(),
                    knownGoodDescriptors = knownGoodDescriptors,
                    limit = MAX_KNOWN_GOOD_CANDIDATES,
                ),
            quarantinedDescriptors = quarantinedDescriptors,
            limit = MAX_KNOWN_GOOD_CANDIDATES,
        )
    val knownGoodIdentities = knownGoodCandidates.map(EntryNodeCandidate::identity).toSet()
    val probedKnownGood =
        prioritizeKnownGoodEntryNodes(
            probeCandidates(
                knownGoodCandidates,
                generation,
                maxIdentities = MAX_KNOWN_GOOD_CANDIDATES,
            ),
            knownGoodIdentities,
        )
    SHARED_DISCOVERY_GATE.ensureCurrent(isCurrent)
    val fastSelection =
        deprioritizeAttemptedEntryNodes(probedKnownGood, recentlyAttemptedIdentities)
            .take(MAX_RUNTIME_ENTRY_NODES)
    if (
            fastSelection.size >= MIN_REQUIRED_ENTRY_NODES &&
            fastSelection.none { entry ->
              entry.candidate.identity() in recentlyAttemptedIdentities
            }
    ) {
      val result =
          EntryNodeDiscoveryResult.fromReachability(
              selected = fastSelection,
              cache = probedKnownGood,
          ).let { verifiedResult ->
            verifiedResult.copy(
                cacheDescriptors =
                    mergeEntryNodeCache(
                        chain = chain,
                        preferredDescriptors = verifiedResult.cacheDescriptors,
                        cachedDescriptors = loadCached(chain),
                        limit = MAX_CACHED_CANDIDATES,
                    ))
          }
      SHARED_DISCOVERY_GATE.ensureCurrent(isCurrent)
      saveCached(chain, result.cacheDescriptors)
      safeDiagnostic(
          "NF_KNOWN_GOOD_OK",
          "selected" to fastSelection.size,
          "generation" to generation,
      )
      return result
    }

    val pool =
        Executors.newFixedThreadPool(
            nodeFinderRequestConcurrency(
                attempts = NODE_FINDER_ATTEMPTS,
                maximumConcurrency = NODE_FINDER_MAX_CONCURRENT_REQUESTS,
            ))
    try {
      val freshDescriptors = fetchFreshCandidates(pool, chain, generation)
      SHARED_DISCOVERY_GATE.ensureCurrent(isCurrent)
      val cachedDescriptors = loadCached(chain)
      val candidatePoolLimit =
          entryNodeProbeCandidatePoolLimit(
              maximumAlternativeIdentities = MAX_RANKING_CANDIDATES,
              alreadyProbedKnownGoodIdentityCount = knownGoodCandidates.size,
          )
      val probePhases =
          planEntryNodeProbePhases(
              chain = chain,
              candidates =
                  EntryNodeDiscoverySelection.select(
                      chain = chain,
                      freshDescriptors = freshDescriptors,
                      preferredDescriptors = preferredNodes,
                      cachedDescriptors = cachedDescriptors,
                      knownGoodDescriptors = knownGoodDescriptors,
                      limit = candidatePoolLimit + quarantinedDescriptors.size,
                  ),
              quarantinedDescriptors = quarantinedDescriptors,
              maximumHealthyIdentities = candidatePoolLimit,
              maximumQuarantinedStandbyIdentities =
                  MAX_QUARANTINED_STANDBY_PROBE_IDENTITIES,
              recoveryGeneration = generation,
          )
      val alreadyProbedIdentities = knownGoodCandidates.map(EntryNodeCandidate::identity).toSet()
      val additionalProbePlan =
          planAdditionalEntryNodeProbes(
              candidates = probePhases.healthyCandidates,
              alreadyProbedKnownGoodIdentities = alreadyProbedIdentities,
              reachableKnownGoodIdentities =
                  probedKnownGood.map { entry -> entry.candidate.identity() }.toSet(),
              maximumReachableIdentities = MAX_PROBED_ENTRY_IDENTITIES,
          )
      val additionalReachable =
          probeCandidates(
              additionalProbePlan.candidates,
              generation,
              maxIdentities = additionalProbePlan.maxIdentities,
              requiredReachableIdentities = MAX_RUNTIME_ENTRY_NODES,
          )
      SHARED_DISCOVERY_GATE.ensureCurrent(isCurrent)
      val reachableWithStandby =
          supplementWithQuarantinedEntryNodeStandbys(
              primaryReachable = probedKnownGood + additionalReachable,
              standbyCandidates = probePhases.quarantinedStandbyCandidates,
              requiredReachableIdentities = MAX_RUNTIME_ENTRY_NODES,
          ) { candidates, requiredReachableIdentities ->
            val standbyReachable =
                probeCandidates(
                    candidates = candidates,
                    generation = generation,
                    maxIdentities = MAX_QUARANTINED_STANDBY_PROBE_IDENTITIES,
                    requiredReachableIdentities = requiredReachableIdentities,
                )
            safeDiagnostic(
                "NF_PROBE_STANDBY",
                "candidates" to candidates.size,
                "reachable" to standbyReachable.size,
            )
            standbyReachable
          }
      val reachable =
          prioritizeEntryNodesForRecovery(
              reachable =
                  prioritizeKnownGoodEntryNodes(
                      reachableWithStandby,
                      knownGoodIdentities,
                  ),
              quarantinedIdentities = quarantinedIdentities,
              recentlyAttemptedIdentities = recentlyAttemptedIdentities,
              generation = generation,
          )
      // TCP reachability is only a transport hint. Let the native MASQ handshake filter up to
      // three peers in parallel instead of discarding a potentially compatible third peer.
      val selected = reachable.take(MAX_RUNTIME_ENTRY_NODES)
      SHARED_DISCOVERY_GATE.ensureCurrent(isCurrent)

      if (selected.size < MIN_REQUIRED_ENTRY_NODES) {
        safeDiagnostic(
            "NF_PROBE_INSUFFICIENT",
            "fresh" to freshDescriptors.size,
            "preferred" to preferredNodes.size,
            "cached" to cachedDescriptors.size,
            "candidates" to probePhases.healthyCandidates.size,
            "standby" to probePhases.quarantinedStandbyCandidates.size,
            "reachable" to reachable.size,
            "quarantined" to quarantinedDescriptors.size,
        )
        throw EntryNodeDiscoveryException(
            "NF_PROBE_INSUFFICIENT: MASQ could not find two reachable public entry nodes. " +
                "Retrying with fresh nodes."
        )
      }

      val result =
          EntryNodeDiscoveryResult.fromReachability(
              selected = selected,
              cache = reachable.take(MAX_CACHED_CANDIDATES),
          )
      SHARED_DISCOVERY_GATE.ensureCurrent(isCurrent)
      saveCached(chain, result.cacheDescriptors)
      safeDiagnostic(
          "NF_SELECTION_OK",
          "selected" to selected.size,
          "reachable" to reachable.size,
          "quarantined" to quarantinedDescriptors.size,
          "best_band" to selected.minOf { entry -> entryNodeProbeLatencyBand(entry.bestLatencyMs) },
          "worst_band" to selected.maxOf { entry -> entryNodeProbeLatencyBand(entry.bestLatencyMs) },
          "generation" to generation,
      )
      return result
    } finally {
      pool.shutdownNow()
    }
  }

  private fun fetchFreshCandidates(
      pool: java.util.concurrent.ExecutorService,
      chain: String,
      generation: Int,
  ): List<String> {
    val completion = ExecutorCompletionService<String?>(pool)
    val futures =
        (0 until NODE_FINDER_ATTEMPTS).map { attempt ->
          completion.submit(Callable { fetchCandidate(chain, generation, attempt) })
        }
    val deadline = System.nanoTime() + TimeUnit.MILLISECONDS.toNanos(NODE_FINDER_BUDGET_MS)
    val freshDescriptors = mutableListOf<String>()
    var completed = 0
    try {
      while (completed < futures.size) {
        val remaining = deadline - System.nanoTime()
        if (remaining <= 0) break
        val completedFuture = completion.poll(remaining, TimeUnit.NANOSECONDS) ?: break
        completed += 1
        runCatching { completedFuture.get() }.getOrNull()?.let(freshDescriptors::add)
        val uniqueFresh =
            EntryNodeDiscoverySelection.select(
                chain = chain,
                freshDescriptors = freshDescriptors,
                preferredDescriptors = emptyList(),
                cachedDescriptors = emptyList(),
                limit = TARGET_FRESH_CANDIDATES,
            )
        if (uniqueFresh.size >= TARGET_FRESH_CANDIDATES) break
      }
    } finally {
      futures.forEach { future ->
        if (!future.isDone) future.cancel(true)
      }
    }
    safeDiagnostic(
        "NF_FETCH_COMPLETE",
        "completed" to completed,
        "valid" to freshDescriptors.size,
    )
    return freshDescriptors
  }

  private fun probeCandidates(
      candidates: List<EntryNodeCandidate>,
      generation: Int,
      maxIdentities: Int = MAX_PROBED_ENTRY_IDENTITIES,
      requiredReachableIdentities: Int = MIN_REQUIRED_ENTRY_NODES,
  ): List<EntryNodeReachability> {
    if (maxIdentities <= 0 || requiredReachableIdentities <= 0) return emptyList()
    val plan = planEntryNodeProbes(candidates, maxIdentities, generation)
    if (plan.primaryTargets.isEmpty()) return emptyList()
    val maximumBatchSize = maxOf(plan.primaryTargets.size, plan.fallbackTargets.size)
    val pool =
        Executors.newFixedThreadPool(minOf(ENTRY_PROBE_MAX_CONCURRENCY, maximumBatchSize))
    val deadline = System.nanoTime() + TimeUnit.MILLISECONDS.toNanos(ENTRY_PROBE_BUDGET_MS)
    try {
      val primaryResults = probeTargets(plan.primaryTargets, pool, deadline)
      val primaryReachable = rankReachableEntryNodes(plan.candidates, primaryResults)
      safeDiagnostic(
          "NF_PROBE_PRIMARY",
          "identities" to plan.candidates.size,
          "targets" to plan.primaryTargets.size,
          "completed" to primaryResults.size,
          "reachable" to primaryReachable.size,
      )
      val slowTargets =
          planSlowEntryNodeProbeTargets(
              plan,
              primaryResults,
              requiredReachableIdentities = requiredReachableIdentities,
          )
      if (slowTargets.isEmpty()) return primaryReachable
      val slowDeadline =
          System.nanoTime() + TimeUnit.MILLISECONDS.toNanos(ENTRY_PROBE_SLOW_BUDGET_MS)
      val fallbackResults =
          probeTargets(
              slowTargets,
              pool,
              slowDeadline,
              connectTimeoutMs = ENTRY_PROBE_SLOW_CONNECT_TIMEOUT_MS,
          )
      val reachable =
          rankReachableEntryNodes(plan.candidates, primaryResults + fallbackResults)
      safeDiagnostic(
          "NF_PROBE_FALLBACK",
          "identities" to plan.candidates.size,
          "targets" to slowTargets.size,
          "completed" to fallbackResults.size,
          "reachable" to reachable.size,
      )
      return reachable
    } finally {
      pool.shutdownNow()
    }
  }

  private fun probeTargets(
      targets: List<EntryNodeProbeTarget>,
      pool: java.util.concurrent.ExecutorService,
      deadlineNanos: Long,
      connectTimeoutMs: Int = ENTRY_PROBE_CONNECT_TIMEOUT_MS,
  ): List<EntryNodeProbeResult> {
    if (targets.isEmpty() || System.nanoTime() >= deadlineNanos) return emptyList()
    val completion = ExecutorCompletionService<EntryNodeProbeResult>(pool)
    val futures = mutableListOf<Future<EntryNodeProbeResult>>()
    targets.forEach { target ->
      futures +=
          completion.submit(
              Callable {
                EntryNodeProbeResult(
                    publicKey = target.candidate.publicKey,
                    host = target.candidate.host,
                    port = target.port,
                    latencyMs =
                        portProbe.measure(
                            target.candidate.host,
                            target.port,
                            connectTimeoutMs,
                        ),
                )
              })
    }
    val results = mutableListOf<EntryNodeProbeResult>()
    try {
      var completedCount = 0
      while (completedCount < targets.size) {
        val remaining = deadlineNanos - System.nanoTime()
        if (remaining <= 0) break
        val completed = completion.poll(remaining, TimeUnit.NANOSECONDS) ?: break
        completedCount += 1
        runCatching { completed.get() }.getOrNull()?.let(results::add)
      }
    } finally {
      futures.forEach { future ->
        if (!future.isDone) future.cancel(true)
      }
    }
    return results
  }

  private fun fetchCandidate(chain: String, generation: Int, attempt: Int): String? {
    val baseUrl = validatedNodeFinderBaseUrl()
    if (baseUrl == null) {
      safeDiagnostic("NF_ENDPOINT_REJECTED")
      return null
    }
    val requestUrl =
        baseUrl
            .newBuilder()
            .addPathSegment("randomnode")
            .addPathSegment(chain)
            .addPathSegment(PUBLIC_SUBURB)
            .addQueryParameter(
                "refresh",
                "$finderSessionNonce-${generation.toUInt()}-$attempt",
            )
            .build()
    val request =
        Request.Builder()
            .url(requestUrl)
            .get()
            .cacheControl(CacheControl.FORCE_NETWORK)
            .header("Accept", "text/plain, application/json")
            .build()
    val startedAt = System.nanoTime()

    return try {
      httpClient.newCall(request).execute().use { response ->
        if (!response.isSuccessful) {
          safeDiagnostic(
              "NF_FETCH_HTTP",
              "attempt" to attempt,
              "status_class" to (response.code / 100),
          )
          return null
        }
        val body =
            response.body
                ?: run {
                  safeDiagnostic("NF_FETCH_EMPTY", "attempt" to attempt)
                  return null
                }
        val boundedBody = body.readBoundedUtf8()
        if (boundedBody == null) {
          safeDiagnostic("NF_FETCH_OVERSIZE", "attempt" to attempt)
          return null
        }
        val descriptor = normalizeNodeFinderDescriptor(boundedBody)
        if (EntryNodeDiscoverySelection.parse(descriptor, chain) == null) {
          safeDiagnostic("NF_FETCH_INVALID", "attempt" to attempt)
          return null
        }
        safeDiagnostic("NF_FETCH_OK", "attempt" to attempt)
        descriptor
      }
    } catch (error: Exception) {
      val elapsedMs =
          TimeUnit.NANOSECONDS
              .toMillis(System.nanoTime() - startedAt)
              .coerceIn(0, Int.MAX_VALUE.toLong())
              .toInt()
      safeDiagnostic(
          nodeFinderFailureCode(error),
          "attempt" to attempt,
          "elapsed_ms" to elapsedMs,
      )
      null
    }
  }

  private fun validatedNodeFinderBaseUrl(): HttpUrl? {
    val parsed =
        BuildConfig.MASQ_NODE_FINDER_URL.trim().trimEnd('/').toHttpUrlOrNull() ?: return null
    return parsed.takeIf { url ->
      url.isHttps &&
          url.username.isEmpty() &&
          url.password.isEmpty() &&
          url.query == null &&
          url.fragment == null
    }
  }

  private fun ResponseBody.readBoundedUtf8(): String? {
    val declaredLength = contentLength()
    if (declaredLength > MAX_RESPONSE_BYTES) return null

    val input = byteStream()
    val bytes = ByteArray(MAX_RESPONSE_BYTES + 1)
    var total = 0
    while (total < bytes.size) {
      val read = input.read(bytes, total, bytes.size - total)
      if (read < 0) break
      if (read == 0) return null
      total += read
    }
    if (total > MAX_RESPONSE_BYTES) return null
    return String(bytes, 0, total, StandardCharsets.UTF_8)
  }

  private fun loadCached(chain: String): List<String> {
    val key = "$CACHE_PREFIX.$chain"
    val serialized = preferences.getString(key, null) ?: return emptyList()
    val decoded =
        runCatching {
          val array = JSONArray(serialized)
          (0 until array.length()).mapNotNull { index ->
            array.optString(index).takeIf(String::isNotBlank)
          }
        }
        .getOrDefault(emptyList())
    val normalized =
        mergeEntryNodeCache(
            chain = chain,
            preferredDescriptors = emptyList(),
            cachedDescriptors = decoded,
            limit = MAX_CACHED_CANDIDATES,
        )
    val normalizedSerialized = JSONArray(normalized).toString()
    if (normalized.isEmpty()) {
      preferences.edit().remove(key).apply()
    } else if (normalizedSerialized != serialized) {
      preferences.edit().putString(key, normalizedSerialized).apply()
    }
    return normalized
  }

  private fun saveCached(chain: String, nodes: List<String>) {
    preferences.edit().putString("$CACHE_PREFIX.$chain", JSONArray(nodes).toString()).apply()
  }

  private fun loadKnownGood(chain: String, nowEpochMs: Long): List<KnownGoodEntryNode> {
    val key = "$KNOWN_GOOD_PREFIX.$chain"
    val serialized = preferences.getString(key, null) ?: return emptyList()
    val retained =
        decodeKnownGoodEntryNodes(
            serialized = serialized,
            chain = chain,
            nowEpochMs = nowEpochMs,
            maxCandidates = MAX_KNOWN_GOOD_CANDIDATES,
            maximumAcceptedFutureMs = MAX_ACCEPTED_KNOWN_GOOD_FUTURE_MS,
        )
    val normalized = encodeKnownGoodEntryNodes(retained)
    if (retained.isEmpty()) {
      preferences.edit().remove(key).apply()
    } else if (normalized != serialized) {
      preferences.edit().putString(key, normalized).apply()
    }
    return retained
  }

  private fun saveKnownGood(chain: String, nodes: List<KnownGoodEntryNode>) {
    val key = "$KNOWN_GOOD_PREFIX.$chain"
    if (nodes.isEmpty()) {
      preferences.edit().remove(key).apply()
    } else {
      preferences.edit().putString(key, encodeKnownGoodEntryNodes(nodes)).apply()
    }
  }

  private fun removeKnownGood(
      chain: String,
      identities: Set<EntryNodeIdentity>,
      nowEpochMs: Long,
  ) {
    if (identities.isEmpty()) return
    val retained =
        loadKnownGood(chain, nowEpochMs).filterNot { entry ->
          EntryNodeDiscoverySelection.parse(entry.descriptor, chain)?.identity() in identities
        }
    saveKnownGood(chain, retained)
  }

  private fun loadQuarantined(chain: String, nowEpochMs: Long): List<QuarantinedEntryNode> {
    val key = "$QUARANTINE_PREFIX.$chain"
    val serialized = preferences.getString(key, null) ?: return emptyList()
    val retained =
        runCatching {
              val array = JSONArray(serialized)
              (0 until array.length())
                  .mapNotNull { index ->
                    val item = array.optJSONObject(index) ?: return@mapNotNull null
                    val descriptor = item.optString("descriptor")
                    val until = item.optLong("untilEpochMs", 0L)
                    if (
                        EntryNodeDiscoverySelection.parse(descriptor, chain) == null ||
                            until <= nowEpochMs ||
                            until > nowEpochMs + MAX_ACCEPTED_QUARANTINE_AGE_MS
                    ) {
                      null
                    } else {
                      QuarantinedEntryNode(descriptor, until)
                    }
                  }
                  .distinctBy { entry ->
                    EntryNodeDiscoverySelection.parse(entry.descriptor, chain)?.identity()
                  }
                  .take(MAX_QUARANTINED_CANDIDATES)
            }
            .getOrDefault(emptyList())
    if (retained.isEmpty()) {
      preferences.edit().remove(key).apply()
    } else if (retained.size < runCatching { JSONArray(serialized).length() }.getOrDefault(0)) {
      saveQuarantined(chain, retained)
    }
    return retained
  }

  private fun saveQuarantined(chain: String, nodes: List<QuarantinedEntryNode>) {
    val array = JSONArray()
    nodes.forEach { entry ->
      array.put(
          org.json.JSONObject()
              .put("descriptor", entry.descriptor)
              .put("untilEpochMs", entry.untilEpochMs))
    }
    preferences
        .edit()
        .putString("$QUARANTINE_PREFIX.$chain", array.toString())
        .apply()
  }

  private fun recentRouteFailureIdentities(
      chain: String,
      nowEpochMs: Long,
  ): Set<EntryNodeIdentity> = recentRouteFailures.identities(chain, nowEpochMs)

  /**
   * Emits only bounded aggregate counters/latencies. String-valued metrics are intentionally not
   * accepted, so a device address, entry-node address, public key, or descriptor cannot be logged.
   */
  private fun safeDiagnostic(code: String, vararg metrics: Pair<String, Int>) {
    val safeCode = code.takeIf(NF_CODE_PATTERN::matches) ?: "NF_DIAGNOSTIC_REJECTED"
    val suffix =
        metrics.joinToString(
            separator = " ",
            prefix = if (metrics.isEmpty()) "" else " ",
        ) { (name, value) ->
          val safeName = name.takeIf(NF_METRIC_PATTERN::matches) ?: "invalid_metric"
          "$safeName=$value"
        }
    Log.i(LOG_TAG, "$safeCode$suffix")
  }

  private companion object {
    val SHARED_DISCOVERY_GATE = EntryNodeDiscoveryGate()
    const val LOG_TAG = "MasqNodeFinder"
    const val PUBLIC_SUBURB = "masqpublic1"
    const val NODE_FINDER_ATTEMPTS = 12
    const val NODE_FINDER_MAX_CONCURRENT_REQUESTS = 12
    const val NODE_FINDER_BUDGET_MS = 6_000L
    const val TARGET_FRESH_CANDIDATES = 8
    const val MIN_REQUIRED_ENTRY_NODES = 2
    const val MAX_RUNTIME_ENTRY_NODES = 3
    const val MAX_RANKING_CANDIDATES = 8
    const val MAX_PROBED_ENTRY_IDENTITIES = 8
    const val MAX_QUARANTINED_STANDBY_PROBE_IDENTITIES = 2
    const val MAX_CACHED_CANDIDATES = 10
    const val MAX_KNOWN_GOOD_CANDIDATES = MAX_RUNTIME_ENTRY_NODES
    const val MAX_QUARANTINED_CANDIDATES = 12
    const val MAX_RECENT_FAILURE_IDENTITIES = 12
    const val NODE_FINDER_TIMEOUT_MS = 3500L
    const val NODE_FINDER_CALL_TIMEOUT_MS = 4500L
    const val ENTRY_PROBE_CONNECT_TIMEOUT_MS = 900
    const val ENTRY_PROBE_BUDGET_MS = 2_500L
    const val ENTRY_PROBE_SLOW_CONNECT_TIMEOUT_MS = 2_500
    const val ENTRY_PROBE_SLOW_BUDGET_MS = 5_500L
    const val ENTRY_PROBE_MAX_CONCURRENCY = 8
    const val MAX_RESPONSE_BYTES = 1024
    const val CACHE_PREFIX = "masq-mobile-entry-nodes"
    const val KNOWN_GOOD_PREFIX = "masq-mobile-entry-known-good"
    // v2 intentionally drops quarantine written by older builds whose pair-level feedback could
    // exclude a viable peer. Only public descriptors were stored under the previous key.
    const val QUARANTINE_PREFIX = "masq-mobile-entry-quarantine-v2"
    const val KNOWN_GOOD_TTL_MS = 24 * 60 * 60_000L
    const val KNOWN_GOOD_REFRESH_WINDOW_MS = 12 * 60 * 60_000L
    const val MAX_ACCEPTED_KNOWN_GOOD_FUTURE_MS = 2 * KNOWN_GOOD_TTL_MS
    const val ENTRY_FAILURE_QUARANTINE_MS = 10 * 60_000L
    const val MAX_ACCEPTED_QUARANTINE_AGE_MS = 2 * ENTRY_FAILURE_QUARANTINE_MS
    const val ROUTE_FAILURE_DEPRIORITIZATION_MS = 2 * 60_000L
    val NF_CODE_PATTERN = Regex("NF_[A-Z_]+")
    val NF_METRIC_PATTERN = Regex("[a-z_]+")
  }
}

internal fun nodeFinderAttemptBatches(
    attempts: Int,
    maxConcurrentRequests: Int,
): List<List<Int>> {
  require(attempts > 0)
  require(maxConcurrentRequests > 0)
  return (0 until attempts).chunked(maxConcurrentRequests)
}

internal fun nodeFinderRequestConcurrency(
    attempts: Int,
    maximumConcurrency: Int,
): Int {
  require(attempts > 0)
  require(maximumConcurrency > 0)
  return minOf(attempts, maximumConcurrency)
}

internal fun normalizeNodeFinderDescriptor(value: String): String {
  val trimmed = value.trim()
  // The production node-finder returns an unquoted text/plain descriptor. Feeding
  // `masq://...` unconditionally to JSONTokener makes Android treat the scheme
  // colon as a token delimiter and return only `masq`. Decode only an actual
  // JSON string; preserve plain text byte-for-byte apart from surrounding space.
  if (trimmed.length < 2 || trimmed.first() != '"' || trimmed.last() != '"') {
    return trimmed
  }
  return decodeNodeFinderJsonString(trimmed)?.trim() ?: trimmed
}

private fun decodeNodeFinderJsonString(value: String): String? {
  val decoded = StringBuilder(value.length - 2)
  var index = 1
  while (index < value.lastIndex) {
    val character = value[index++]
    if (character == '"' || character.code < 0x20) return null
    if (character != '\\') {
      decoded.append(character)
      continue
    }
    if (index >= value.lastIndex) return null
    when (val escaped = value[index++]) {
      '"', '\\', '/' -> decoded.append(escaped)
      'b' -> decoded.append('\b')
      'f' -> decoded.append('\u000C')
      'n' -> decoded.append('\n')
      'r' -> decoded.append('\r')
      't' -> decoded.append('\t')
      'u' -> {
        if (index + 4 > value.lastIndex) return null
        val codePoint = value.substring(index, index + 4).toIntOrNull(16) ?: return null
        decoded.append(codePoint.toChar())
        index += 4
      }
      else -> return null
    }
  }
  return decoded.toString()
}

internal fun nodeFinderFailureCode(error: Throwable): String {
  var cause: Throwable? = error
  var sawIoFailure = false
  while (cause != null) {
    when (cause) {
      is UnknownHostException -> return "NF_FETCH_DNS"
      is SSLException -> return "NF_FETCH_TLS"
      is SocketTimeoutException -> return "NF_FETCH_TIMEOUT"
      is InterruptedIOException -> return "NF_FETCH_INTERRUPTED"
      is ConnectException -> return "NF_FETCH_CONNECT"
      is UnknownServiceException, is ProtocolException -> return "NF_FETCH_PROTOCOL"
      is SocketException -> return "NF_FETCH_SOCKET"
      is SecurityException -> return "NF_FETCH_PERMISSION"
      is IOException -> sawIoFailure = true
    }
    if (cause.javaClass.name == "android.os.NetworkOnMainThreadException") {
      return "NF_FETCH_THREAD"
    }
    cause = cause.cause
  }
  return if (sawIoFailure) "NF_FETCH_IO" else "NF_FETCH_UNEXPECTED"
}

internal data class EntryNodeCandidate(
    val originalDescriptor: String,
    val chain: String,
    val publicKey: String,
    val host: String,
    val ports: List<Int>,
) {
  fun identity(): EntryNodeIdentity = EntryNodeIdentity(publicKey, host)

  fun persistentDescriptor(): String =
      "masq://$chain:$publicKey@$host:${ports.joinToString("/")}"

  fun singlePortDescriptor(generation: Int): String {
    val index = Math.floorMod(generation, ports.size)
    return "masq://$chain:$publicKey@$host:${ports[index]}"
  }
}

internal data class EntryNodeIdentity(
    val publicKey: String,
    val host: String,
)

private data class QuarantinedEntryNode(
    val descriptor: String,
    val untilEpochMs: Long,
)

internal data class KnownGoodEntryNode(
    val descriptor: String,
    val expiresAtEpochMs: Long,
)

internal fun decodeKnownGoodEntryNodes(
    serialized: String,
    chain: String,
    nowEpochMs: Long,
    maxCandidates: Int,
    maximumAcceptedFutureMs: Long,
): List<KnownGoodEntryNode> {
  require(maxCandidates > 0)
  require(maximumAcceptedFutureMs > 0)
  return runCatching {
        val array = JSONArray(serialized)
        (0 until array.length())
            .mapNotNull { index ->
              val item = array.optJSONObject(index) ?: return@mapNotNull null
              val descriptor = item.optString("descriptor")
              val expiresAt =
                  when (val rawExpiry = item.opt("expiresAtEpochMs")) {
                    is Int -> rawExpiry.toLong()
                    is Long -> rawExpiry
                    else -> return@mapNotNull null
                  }
              if (
                  EntryNodeDiscoverySelection.parse(descriptor, chain) == null ||
                      expiresAt <= nowEpochMs ||
                      expiresAt > nowEpochMs + maximumAcceptedFutureMs
              ) {
                null
              } else {
                KnownGoodEntryNode(descriptor, expiresAt)
              }
            }
            .distinctBy { entry ->
              EntryNodeDiscoverySelection.parse(entry.descriptor, chain)?.identity()
            }
            .take(maxCandidates)
      }
      .getOrDefault(emptyList())
}

internal fun encodeKnownGoodEntryNodes(nodes: List<KnownGoodEntryNode>): String {
  val array = JSONArray()
  nodes.forEach { entry ->
    array.put(
        org.json.JSONObject()
            .put("descriptor", entry.descriptor)
            .put("expiresAtEpochMs", entry.expiresAtEpochMs))
  }
  return array.toString()
}

private data class RecentEntryNodeFailureKey(
    val chain: String,
    val identity: EntryNodeIdentity,
)

/**
 * Keeps a small in-memory LRU of recently unsuccessful public entry identities. Each identity
 * expires relative to its own last failed attempt, so a later failure cannot indefinitely extend
 * older entries. Nothing is persisted and no wallet, destination, or device metadata is stored.
 */
internal class RecentEntryNodeFailureTracker(
    private val retentionMs: Long,
    private val maximumIdentities: Int,
) {
  private val lock = Any()
  private val expiresAtByIdentity = LinkedHashMap<RecentEntryNodeFailureKey, Long>()

  init {
    require(retentionMs > 0)
    require(maximumIdentities > 0)
  }

  fun record(
      chain: String,
      identities: Set<EntryNodeIdentity>,
      nowEpochMs: Long,
  ): Set<EntryNodeIdentity> =
      synchronized(lock) {
        pruneExpired(nowEpochMs)
        val expiresAt =
            if (nowEpochMs > Long.MAX_VALUE - retentionMs) {
              Long.MAX_VALUE
            } else {
              nowEpochMs + retentionMs
            }
        identities.forEach { identity ->
          val key = RecentEntryNodeFailureKey(chain, identity)
          expiresAtByIdentity.remove(key)
          expiresAtByIdentity[key] = expiresAt
        }
        while (expiresAtByIdentity.size > maximumIdentities) {
          val oldest = expiresAtByIdentity.entries.iterator()
          if (!oldest.hasNext()) break
          oldest.next()
          oldest.remove()
        }
        activeIdentities(chain)
      }

  fun identities(chain: String, nowEpochMs: Long): Set<EntryNodeIdentity> =
      synchronized(lock) {
        pruneExpired(nowEpochMs)
        activeIdentities(chain)
      }

  private fun pruneExpired(nowEpochMs: Long) {
    expiresAtByIdentity.entries.removeAll { entry -> entry.value <= nowEpochMs }
  }

  private fun activeIdentities(chain: String): Set<EntryNodeIdentity> =
      expiresAtByIdentity.keys
          .asSequence()
          .filter { key -> key.chain == chain }
          .map(RecentEntryNodeFailureKey::identity)
          .toCollection(linkedSetOf())
}

internal fun attemptedEntryNodeIdentities(
    chain: String,
    descriptors: List<String>,
): Set<EntryNodeIdentity> =
    descriptors
        .mapNotNull { descriptor -> EntryNodeDiscoverySelection.parse(descriptor, chain) }
        .map(EntryNodeCandidate::identity)
        .toSet()

/**
 * Keeps newly verified public entries first without discarding valid standby identities. Matching
 * descriptors are merged so alternate validated ports survive a fast known-good reconnect.
 */
internal fun mergeEntryNodeCache(
    chain: String,
    preferredDescriptors: List<String>,
    cachedDescriptors: List<String>,
    limit: Int,
): List<String> =
    EntryNodeDiscoverySelection.select(
            chain = chain,
            freshDescriptors = emptyList(),
            preferredDescriptors = emptyList(),
            cachedDescriptors = cachedDescriptors,
            knownGoodDescriptors = preferredDescriptors,
            limit = limit,
        )
        .map(EntryNodeCandidate::persistentDescriptor)

internal fun excludeQuarantinedEntryNodes(
    chain: String,
    candidates: List<EntryNodeCandidate>,
    quarantinedDescriptors: List<String>,
    limit: Int,
    minimumCandidates: Int = 0,
    recoveryGeneration: Int = 0,
): List<EntryNodeCandidate> {
  require(limit > 0)
  require(minimumCandidates in 0..limit)
  val excluded =
      quarantinedDescriptors
          .mapNotNull { descriptor -> EntryNodeDiscoverySelection.parse(descriptor, chain) }
          .map(EntryNodeCandidate::identity)
          .toSet()
  val eligible = candidates.filterNot { candidate -> candidate.identity() in excluded }.take(limit)
  if (eligible.size >= minimumCandidates) return eligible

  // Quarantine is a ranking hint, not a 10-minute network lockout. If the public finder exposes
  // only the same small pool, retain a bounded set of quarantined identities as a last resort.
  // Keeping the full probe budget here is important: returning only the minimum pair makes every
  // retry contact that same pair until quarantine expires, even when more candidates are present.
  val eligibleIdentities = eligible.map(EntryNodeCandidate::identity).toSet()
  val lastResort =
      rotateQuarantinedProbeCandidates(
              candidates.filter { candidate ->
                candidate.identity() in excluded && candidate.identity() !in eligibleIdentities
              },
              generation = recoveryGeneration,
              runtimePairSize = maxOf(minimumCandidates, 1),
          )
          .take(limit - eligible.size)
  return (eligible + lastResort).take(limit)
}

internal data class EntryNodeProbePhases(
    val healthyCandidates: List<EntryNodeCandidate>,
    val quarantinedStandbyCandidates: List<EntryNodeCandidate>,
)

/**
 * Keeps quarantined public entries out of the normal probe phase without throwing them away.
 * A small deterministic standby window remains available when enough healthy-looking candidates
 * were discovered but fewer than two actually accept a TCP connection. Standbys are contacted
 * only by the second phase, and both lists are independently bounded.
 */
internal fun planEntryNodeProbePhases(
    chain: String,
    candidates: List<EntryNodeCandidate>,
    quarantinedDescriptors: List<String>,
    maximumHealthyIdentities: Int,
    maximumQuarantinedStandbyIdentities: Int,
    recoveryGeneration: Int,
): EntryNodeProbePhases {
  require(maximumHealthyIdentities > 0)
  require(maximumQuarantinedStandbyIdentities > 0)
  val quarantinedIdentities = attemptedEntryNodeIdentities(chain, quarantinedDescriptors)
  val selectedCandidates = candidates.distinctBy(EntryNodeCandidate::identity)
  val selectedPublicKeys = selectedCandidates.map(EntryNodeCandidate::publicKey).toSet()
  val selectedHosts = selectedCandidates.map(EntryNodeCandidate::host).toSet()
  val persistedQuarantinedCandidates =
      if (quarantinedDescriptors.isEmpty()) {
        emptyList()
      } else {
        EntryNodeDiscoverySelection.select(
                chain = chain,
                freshDescriptors = quarantinedDescriptors,
                preferredDescriptors = emptyList(),
                cachedDescriptors = emptyList(),
                limit = quarantinedDescriptors.size,
            )
            .filterNot { candidate ->
              candidate.publicKey in selectedPublicKeys || candidate.host in selectedHosts
            }
      }
  // Persisted quarantine is an explicit bounded source of standbys. It must not disappear merely
  // because the healthy finder/preference/cache ranking filled its own candidate limit.
  val uniqueCandidates = selectedCandidates + persistedQuarantinedCandidates
  val healthy =
      uniqueCandidates
          .filterNot { candidate -> candidate.identity() in quarantinedIdentities }
          .take(maximumHealthyIdentities)
  val healthyIdentities = healthy.map(EntryNodeCandidate::identity).toSet()
  val standbys =
      rotateQuarantinedProbeCandidates(
              uniqueCandidates.filter { candidate ->
                candidate.identity() in quarantinedIdentities &&
                    candidate.identity() !in healthyIdentities
              },
              generation = recoveryGeneration,
              runtimePairSize = maximumQuarantinedStandbyIdentities,
          )
          .take(maximumQuarantinedStandbyIdentities)
  return EntryNodeProbePhases(
      healthyCandidates = healthy,
      quarantinedStandbyCandidates = standbys,
  )
}

/**
 * Runs the bounded standby phase only for the missing number of reachable identities. The probe
 * callback receives that deficit so one successful standby does not trigger an unnecessary slow
 * retry merely because the standby list cannot itself contain a full pair.
 */
internal fun supplementWithQuarantinedEntryNodeStandbys(
    primaryReachable: List<EntryNodeReachability>,
    standbyCandidates: List<EntryNodeCandidate>,
    requiredReachableIdentities: Int,
    probeStandbys:
        (candidates: List<EntryNodeCandidate>, requiredReachableIdentities: Int) ->
            List<EntryNodeReachability>,
): List<EntryNodeReachability> {
  require(requiredReachableIdentities > 0)
  val primary = primaryReachable.distinctBy { entry -> entry.candidate.identity() }
  val missing = (requiredReachableIdentities - primary.size).coerceAtLeast(0)
  if (missing == 0 || standbyCandidates.isEmpty()) return primary

  val primaryIdentities = primary.map { entry -> entry.candidate.identity() }.toSet()
  val standbyReachable =
      probeStandbys(standbyCandidates, missing).filterNot { entry ->
        entry.candidate.identity() in primaryIdentities
      }
  return (primary + standbyReachable).distinctBy { entry -> entry.candidate.identity() }
}

private fun rotateQuarantinedProbeCandidates(
    candidates: List<EntryNodeCandidate>,
    generation: Int,
    runtimePairSize: Int,
): List<EntryNodeCandidate> {
  require(runtimePairSize > 0)
  if (candidates.size <= 1) return candidates
  val stable =
      candidates.sortedWith(
          compareBy<EntryNodeCandidate> { candidate -> candidate.publicKey }
              .thenBy { candidate -> candidate.host })
  val offset =
      Math.floorMod(
          generation.toLong(),
          stable.size.toLong(),
      ).toInt()
  return stable.drop(offset) + stable.take(offset)
}

internal fun interface EntryNodePortProbe {
  /** Returns TCP connect latency in milliseconds, or null when the port is unreachable. */
  fun measure(host: String, port: Int, timeoutMs: Int): Int?
}

internal object SocketEntryNodePortProbe : EntryNodePortProbe {
  override fun measure(host: String, port: Int, timeoutMs: Int): Int? {
    val startedAt = System.nanoTime()
    return try {
      Socket().use { socket ->
        socket.tcpNoDelay = true
        socket.connect(InetSocketAddress(host, port), timeoutMs)
      }
      TimeUnit.NANOSECONDS
          .toMillis(System.nanoTime() - startedAt)
          .coerceIn(1, Int.MAX_VALUE.toLong())
          .toInt()
    } catch (_: IOException) {
      null
    } catch (_: SecurityException) {
      null
    }
  }
}

internal data class EntryNodeProbeTarget(
    val candidate: EntryNodeCandidate,
    val port: Int,
)

internal data class EntryNodeProbePlan(
    val candidates: List<EntryNodeCandidate>,
    val primaryTargets: List<EntryNodeProbeTarget>,
    val fallbackTargets: List<EntryNodeProbeTarget>,
)

internal data class AdditionalEntryNodeProbePlan(
    val candidates: List<EntryNodeCandidate>,
    val maxIdentities: Int,
)

/**
 * Keeps room for the normal fresh-alternative pool after known-good identities receive their
 * separate fast probe. This changes only how many already-fetched candidates are retained; probe
 * deadlines and executor concurrency remain independently bounded.
 */
internal fun entryNodeProbeCandidatePoolLimit(
    maximumAlternativeIdentities: Int,
    alreadyProbedKnownGoodIdentityCount: Int,
): Int {
  require(maximumAlternativeIdentities > 0)
  require(alreadyProbedKnownGoodIdentityCount >= 0)
  return maximumAlternativeIdentities + alreadyProbedKnownGoodIdentityCount
}

/**
 * Charges the four-identity route-quality budget only for known-good identities that remain
 * reachable. Failed fast probes are not repeated, but they cannot displace fresh alternatives.
 */
internal fun planAdditionalEntryNodeProbes(
    candidates: List<EntryNodeCandidate>,
    alreadyProbedKnownGoodIdentities: Set<EntryNodeIdentity>,
    reachableKnownGoodIdentities: Set<EntryNodeIdentity>,
    maximumReachableIdentities: Int,
): AdditionalEntryNodeProbePlan {
  require(maximumReachableIdentities > 0)
  val reachableAlreadyProbedIdentities =
      reachableKnownGoodIdentities.intersect(alreadyProbedKnownGoodIdentities)
  return AdditionalEntryNodeProbePlan(
      candidates =
          candidates.filterNot { candidate ->
            candidate.identity() in alreadyProbedKnownGoodIdentities
          },
      maxIdentities =
          (maximumReachableIdentities - reachableAlreadyProbedIdentities.size).coerceAtLeast(0),
  )
}

/**
 * Bounds direct node contact to four identities. The common path tries exactly one advertised port
 * per identity; alternate ports are retained for a second pass only when the primary pass cannot
 * supply the two required entries.
 */
internal fun planEntryNodeProbes(
    candidates: List<EntryNodeCandidate>,
    maxIdentities: Int,
    generation: Int,
): EntryNodeProbePlan {
  require(maxIdentities > 0)
  val boundedCandidates = candidates.distinctBy(EntryNodeCandidate::identity).take(maxIdentities)
  val rotatedPorts =
      boundedCandidates.associateWith { candidate ->
        val primaryIndex = Math.floorMod(generation, candidate.ports.size)
        candidate.ports.drop(primaryIndex) + candidate.ports.take(primaryIndex)
      }
  return EntryNodeProbePlan(
      candidates = boundedCandidates,
      primaryTargets =
          boundedCandidates.map { candidate ->
            EntryNodeProbeTarget(candidate = candidate, port = rotatedPorts.getValue(candidate).first())
          },
      fallbackTargets =
          boundedCandidates.flatMap { candidate ->
            rotatedPorts.getValue(candidate).drop(1).map { port ->
              EntryNodeProbeTarget(candidate = candidate, port = port)
            }
          },
  )
}

/**
 * Builds one bounded mobile-radio retry pass. Only identities without a successful 900 ms primary
 * result are retried; their rotated primary and every advertised alternate port share a fresh slow
 * deadline. With four identities and four ports this is hard-bounded to sixteen tasks.
 */
internal fun planSlowEntryNodeProbeTargets(
    plan: EntryNodeProbePlan,
    primaryResults: List<EntryNodeProbeResult>,
    requiredReachableIdentities: Int,
): List<EntryNodeProbeTarget> {
  require(requiredReachableIdentities > 0)
  val matchedPrimaryResults =
      primaryResults.filter { result ->
        plan.primaryTargets.any { target ->
          result.publicKey == target.candidate.publicKey &&
              result.host == target.candidate.host &&
              result.port == target.port
        }
      }
  if (!entryNodeProbeFallbackRequired(matchedPrimaryResults, requiredReachableIdentities)) {
    return emptyList()
  }
  val successfulPrimaryIdentities =
      plan.primaryTargets
          .filter { target ->
            primaryResults.any { result ->
              result.publicKey == target.candidate.publicKey &&
                  result.host == target.candidate.host &&
                  result.port == target.port &&
                  result.latencyMs != null
            }
          }
          .map { target -> target.candidate.identity() }
          .toSet()
  return (plan.primaryTargets + plan.fallbackTargets)
      .filter { target -> target.candidate.identity() !in successfulPrimaryIdentities }
      .distinctBy { target -> target.candidate.identity() to target.port }
}

internal data class EntryNodeProbeResult(
    val publicKey: String,
    val host: String,
    val port: Int,
    val latencyMs: Int?,
)

internal fun entryNodeProbeFallbackRequired(
    primaryResults: List<EntryNodeProbeResult>,
    requiredReachableIdentities: Int,
): Boolean {
  require(requiredReachableIdentities > 0)
  val reachableIdentities =
      primaryResults
          .asSequence()
          .filter { result -> result.latencyMs != null }
          .map { result -> EntryNodeIdentity(result.publicKey, result.host) }
          .distinct()
          .count()
  return reachableIdentities < requiredReachableIdentities
}

internal data class EntryNodePortLatency(
    val port: Int,
    val latencyMs: Int,
)

internal fun entryNodeProbeLatencyBand(latencyMs: Int): Int =
    latencyMs.coerceAtLeast(0) / ENTRY_PROBE_LATENCY_BAND_MS

internal data class EntryNodeReachability(
    val candidate: EntryNodeCandidate,
    val reachablePorts: List<EntryNodePortLatency>,
) {
  init {
    require(reachablePorts.isNotEmpty())
  }

  val bestLatencyMs: Int
    get() = reachablePorts.first().latencyMs

  fun runtimeDescriptor(): String =
      "masq://${candidate.chain}:${candidate.publicKey}@${candidate.host}:${reachablePorts.first().port}"

  fun persistentDescriptor(): String {
    val orderedPorts = linkedSetOf<Int>()
    reachablePorts.forEach { result -> orderedPorts.add(result.port) }
    candidate.ports.forEach(orderedPorts::add)
    return "masq://${candidate.chain}:${candidate.publicKey}@${candidate.host}:" +
        orderedPorts.joinToString("/")
  }
}

internal fun deprioritizeAttemptedEntryNodes(
    reachable: List<EntryNodeReachability>,
    attemptedIdentities: Set<EntryNodeIdentity>,
): List<EntryNodeReachability> {
  if (attemptedIdentities.isEmpty()) return reachable
  val alternatives =
      reachable.filterNot { entry -> entry.candidate.identity() in attemptedIdentities }
  val attempted = reachable.filter { entry -> entry.candidate.identity() in attemptedIdentities }
  return alternatives + attempted
}

internal fun prioritizeKnownGoodEntryNodes(
    reachable: List<EntryNodeReachability>,
    knownGoodIdentities: Set<EntryNodeIdentity>,
): List<EntryNodeReachability> {
  if (knownGoodIdentities.isEmpty()) return reachable
  val knownGood = reachable.filter { entry -> entry.candidate.identity() in knownGoodIdentities }
  val other = reachable.filterNot { entry -> entry.candidate.identity() in knownGoodIdentities }
  return knownGood + other
}

/**
 * Preserves the latency-ranked healthy path while forcing deterministic diversity when recovery
 * has to reuse recently failed or quarantined public entries. Suspect groups are sorted only by
 * public entry identity and rotated by one position per discovery generation. This keeps the
 * behavior stable regardless of concurrent finder completion order, avoids repeatedly choosing
 * the same small pair, and stores or logs no additional metadata.
 */
internal fun prioritizeEntryNodesForRecovery(
    reachable: List<EntryNodeReachability>,
    quarantinedIdentities: Set<EntryNodeIdentity>,
    recentlyAttemptedIdentities: Set<EntryNodeIdentity>,
    generation: Int,
    runtimePairSize: Int = 2,
): List<EntryNodeReachability> {
  require(runtimePairSize > 0)
  if (reachable.size <= 1) return reachable

  val healthy = mutableListOf<EntryNodeReachability>()
  val recentOnly = mutableListOf<EntryNodeReachability>()
  val quarantinedOnly = mutableListOf<EntryNodeReachability>()
  val quarantinedAndRecent = mutableListOf<EntryNodeReachability>()
  reachable.forEach { entry ->
    val identity = entry.candidate.identity()
    val quarantined = identity in quarantinedIdentities
    val recent = identity in recentlyAttemptedIdentities
    when {
      quarantined && recent -> quarantinedAndRecent += entry
      quarantined -> quarantinedOnly += entry
      recent -> recentOnly += entry
      else -> healthy += entry
    }
  }

  return healthy +
      rotateSuspectEntryNodes(recentOnly, generation, runtimePairSize) +
      rotateSuspectEntryNodes(quarantinedOnly, generation, runtimePairSize) +
      rotateSuspectEntryNodes(quarantinedAndRecent, generation, runtimePairSize)
}

private fun rotateSuspectEntryNodes(
    entries: List<EntryNodeReachability>,
    generation: Int,
    runtimePairSize: Int,
): List<EntryNodeReachability> {
  require(runtimePairSize > 0)
  if (entries.size <= 1) return entries
  val stable =
      entries.sortedWith(
          compareBy<EntryNodeReachability> { entry -> entry.candidate.publicKey }
              .thenBy { entry -> entry.candidate.host })
  val offset =
      Math.floorMod(
          generation.toLong(),
          stable.size.toLong(),
      ).toInt()
  return stable.drop(offset) + stable.take(offset)
}

/**
 * Turns concurrently collected probe results into deterministic coarse-latency bands. Finder
 * order is preserved within a 100 ms band instead of fingerprinting or overfitting to exact TCP
 * timing. A stale, unreachable cached descriptor is omitted.
 */
internal fun rankReachableEntryNodes(
    candidates: List<EntryNodeCandidate>,
    results: List<EntryNodeProbeResult>,
): List<EntryNodeReachability> {
  val candidateOrder =
      candidates.withIndex().associate { indexed ->
        (indexed.value.publicKey to indexed.value.host) to indexed.index
      }
  return candidates
      .mapNotNull { candidate ->
        val originalPortOrder = candidate.ports.withIndex().associate { it.value to it.index }
        val reachablePorts =
            results
                .asSequence()
                .filter { result ->
                  result.publicKey == candidate.publicKey &&
                      result.host == candidate.host &&
                      result.port in candidate.ports &&
                      result.latencyMs != null
                }
                .groupBy(EntryNodeProbeResult::port)
                .map { (port, portResults) ->
                  EntryNodePortLatency(
                      port = port,
                      latencyMs = portResults.minOf { result -> result.latencyMs!! },
                  )
                }
                .sortedWith(
                    compareBy<EntryNodePortLatency> { result ->
                          entryNodeProbeLatencyBand(result.latencyMs)
                        }
                        .thenBy { result -> originalPortOrder[result.port] ?: Int.MAX_VALUE })
        reachablePorts.takeIf(List<EntryNodePortLatency>::isNotEmpty)?.let { ports ->
          EntryNodeReachability(candidate = candidate, reachablePorts = ports)
        }
      }
      .sortedWith(
          compareBy<EntryNodeReachability> { result ->
                entryNodeProbeLatencyBand(result.bestLatencyMs)
              }
              .thenBy { result ->
                candidateOrder[result.candidate.publicKey to result.candidate.host] ?: Int.MAX_VALUE
              })
}

private const val ENTRY_PROBE_LATENCY_BAND_MS = 100

internal data class EntryNodeDiscoveryResult(
    val runtimeDescriptors: List<String>,
    val persistentDescriptors: List<String>,
    val cacheDescriptors: List<String>,
) {
  companion object {
    fun fromSelection(
        selected: List<EntryNodeCandidate>,
        generation: Int,
    ): EntryNodeDiscoveryResult =
        EntryNodeDiscoveryResult(
            runtimeDescriptors =
                selected.map { candidate -> candidate.singlePortDescriptor(generation) },
            persistentDescriptors =
                selected.map(EntryNodeCandidate::persistentDescriptor),
            cacheDescriptors = selected.map(EntryNodeCandidate::persistentDescriptor),
        )

    fun fromReachability(
        selected: List<EntryNodeReachability>,
        cache: List<EntryNodeReachability>,
    ): EntryNodeDiscoveryResult =
        EntryNodeDiscoveryResult(
            runtimeDescriptors = selected.map(EntryNodeReachability::runtimeDescriptor),
            persistentDescriptors = selected.map(EntryNodeReachability::persistentDescriptor),
            cacheDescriptors = cache.map(EntryNodeReachability::persistentDescriptor),
        )
  }
}

internal object EntryNodeDiscoverySelection {
  private val chainPattern = Regex("[a-z0-9][a-z0-9-]{0,63}")
  private val publicKeyPattern = Regex("[A-Za-z0-9_-]{43}")
  private val canonicalPublicKeyTail =
      setOf('A', 'E', 'I', 'M', 'Q', 'U', 'Y', 'c', 'g', 'k', 'o', 's', 'w', '0', '4', '8')
  private val descriptorPattern =
      Regex("^masq://([^:]+):([^@]+)@([^:]+):([0-9]+(?:/[0-9]+){0,3})$")

  fun isCanonicalChain(chain: String): Boolean = chainPattern.matches(chain)

  fun parse(value: String, expectedChain: String): EntryNodeCandidate? {
    if (!isCanonicalChain(expectedChain)) return null
    val descriptor = value.trim()
    val match = descriptorPattern.matchEntire(descriptor) ?: return null
    val (chain, publicKey, host, rawPorts) = match.destructured
    if (
        chain != expectedChain ||
            !isCanonicalChain(chain) ||
            !isCanonicalPublicKey(publicKey) ||
            !isPublicIpv4Literal(host)
    ) {
      return null
    }
    val portStrings = rawPorts.split('/')
    if (portStrings.size !in 1..MAX_PORTS || portStrings.any(::hasAmbiguousLeadingZero)) {
      return null
    }
    val ports = portStrings.mapNotNull(String::toIntOrNull)
    if (
        ports.size != portStrings.size ||
            ports.any { port -> port !in MIN_ENTRY_PORT..MAX_ENTRY_PORT }
    ) {
      return null
    }
    return EntryNodeCandidate(
        originalDescriptor = descriptor,
        chain = chain,
        publicKey = publicKey,
        host = host,
        ports = ports,
    )
  }

  fun select(
      chain: String,
      freshDescriptors: List<String>,
      preferredDescriptors: List<String>,
      cachedDescriptors: List<String>,
      knownGoodDescriptors: List<String> = emptyList(),
      limit: Int = REQUIRED_ENTRY_NODES,
  ): List<EntryNodeCandidate> {
    require(limit > 0)
    val knownGoodCandidates =
        knownGoodDescriptors.mapNotNull { descriptor -> parse(descriptor, chain) }
    val freshCandidates = freshDescriptors.mapNotNull { descriptor -> parse(descriptor, chain) }
    val preferredCandidates =
        preferredDescriptors.mapNotNull { descriptor -> parse(descriptor, chain) }
    val cachedCandidates = cachedDescriptors.mapNotNull { descriptor -> parse(descriptor, chain) }
    val orderedCandidates =
        knownGoodCandidates + freshCandidates + preferredCandidates + cachedCandidates
    val selected = mutableListOf<EntryNodeCandidate>()
    val selectedKeys = mutableSetOf<String>()
    val selectedHosts = mutableSetOf<String>()
    orderedCandidates
        .forEachIndexed { index, candidate ->
          if (selected.size >= limit) return@forEachIndexed
          if (
              candidate.publicKey in selectedKeys ||
                  candidate.host in selectedHosts
          ) {
            return@forEachIndexed
          }
          val matchingIdentity =
              orderedCandidates.filter { other ->
                other.publicKey == candidate.publicKey && other.host == candidate.host
              }
          val mergedCandidate =
              mergePortVariants(
                  primary = candidate,
                  matchingIdentity = matchingIdentity,
                  preservePrimaryOrder =
                      index < knownGoodCandidates.size + freshCandidates.size,
              )
          selectedKeys.add(candidate.publicKey)
          selectedHosts.add(candidate.host)
          selected.add(mergedCandidate)
        }
    return selected
  }

  private fun mergePortVariants(
      primary: EntryNodeCandidate,
      matchingIdentity: List<EntryNodeCandidate>,
      preservePrimaryOrder: Boolean,
  ): EntryNodeCandidate {
    val widest = matchingIdentity.maxByOrNull { candidate -> candidate.ports.size } ?: primary
    val orderedSources =
        if (preservePrimaryOrder) {
          listOf(primary, widest) + matchingIdentity
        } else {
          listOf(widest, primary) + matchingIdentity
        }
    val mergedPorts = linkedSetOf<Int>()
    orderedSources.forEach { source ->
      source.ports.forEach { port ->
        if (mergedPorts.size < MAX_PORTS) mergedPorts.add(port)
      }
    }
    return primary.copy(ports = mergedPorts.toList())
  }

  fun isPublicIpv4Literal(host: String): Boolean {
    val octetStrings = host.split('.')
    if (
        octetStrings.size != 4 ||
            octetStrings.any { octet ->
              octet.isEmpty() ||
                  octet.any { character -> character !in '0'..'9' } ||
                  hasAmbiguousLeadingZero(octet)
            }
    ) {
      return false
    }
    val octets = octetStrings.mapNotNull(String::toIntOrNull)
    if (octets.size != 4 || octets.any { it !in 0..255 }) return false

    val first = octets[0]
    val second = octets[1]
    val third = octets[2]
    return when {
      first == 0 -> false // "This network"
      first == 10 -> false // Private
      first == 100 && second in 64..127 -> false // Shared address space
      first == 127 -> false // Loopback
      first == 169 && second == 254 -> false // Link-local
      first == 172 && second in 16..31 -> false // Private
      first == 192 && second == 0 && third == 0 -> false // IETF protocol assignments
      first == 192 && second == 0 && third == 2 -> false // Documentation
      first == 192 && second == 88 && third == 99 -> false // Deprecated relay anycast
      first == 192 && second == 168 -> false // Private
      first == 198 && second in 18..19 -> false // Benchmarking
      first == 198 && second == 51 && third == 100 -> false // Documentation
      first == 203 && second == 0 && third == 113 -> false // Documentation
      first in 224..239 -> false // Multicast
      first >= 240 -> false // Reserved and limited broadcast
      else -> true
    }
  }

  private fun isCanonicalPublicKey(publicKey: String): Boolean {
    // A 32-byte unpadded base64url value is 43 characters. Its final character contains
    // four data bits followed by two zero padding bits, so its alphabet index is divisible by four.
    return publicKeyPattern.matches(publicKey) && publicKey.last() in canonicalPublicKeyTail
  }

  private fun hasAmbiguousLeadingZero(value: String): Boolean =
      value.length > 1 && value.startsWith('0')

  private const val REQUIRED_ENTRY_NODES = 2
  private const val MAX_PORTS = 4
  private const val MIN_ENTRY_PORT = 1025
  private const val MAX_ENTRY_PORT = 65535
}

internal class EntryNodeDiscoveryException(message: String) : Exception(message)
