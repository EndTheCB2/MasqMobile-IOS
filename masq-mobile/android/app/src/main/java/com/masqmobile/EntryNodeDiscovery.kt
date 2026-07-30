package com.masqmobile

import android.content.Context
import android.util.Log
import java.nio.charset.StandardCharsets
import java.util.concurrent.Callable
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicInteger
import okhttp3.CacheControl
import okhttp3.ConnectionSpec
import okhttp3.HttpUrl
import okhttp3.HttpUrl.Companion.toHttpUrlOrNull
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.ResponseBody
import org.json.JSONArray
import org.json.JSONTokener

internal class EntryNodeDiscovery(context: Context) {
  private val preferences =
      context.getSharedPreferences("masq-mobile-consumer", Context.MODE_PRIVATE)
  private val discoveryGeneration = AtomicInteger(0)
  private val httpClient =
      OkHttpClient.Builder()
          .connectTimeout(NODE_FINDER_TIMEOUT_MS, TimeUnit.MILLISECONDS)
          .readTimeout(NODE_FINDER_TIMEOUT_MS, TimeUnit.MILLISECONDS)
          .writeTimeout(NODE_FINDER_TIMEOUT_MS, TimeUnit.MILLISECONDS)
          .callTimeout(NODE_FINDER_CALL_TIMEOUT_MS, TimeUnit.MILLISECONDS)
          .followRedirects(false)
          .followSslRedirects(false)
          .retryOnConnectionFailure(false)
          .connectionSpecs(listOf(ConnectionSpec.MODERN_TLS))
          .build()

  fun discover(chain: String, preferredNodes: List<String>): EntryNodeDiscoveryResult {
    val generation = discoveryGeneration.getAndIncrement()
    safeDiagnostic("NF_DISCOVERY_START", "generation" to generation)
    if (!EntryNodeDiscoverySelection.isCanonicalChain(chain)) {
      safeDiagnostic("NF_CHAIN_REJECTED")
      throw EntryNodeDiscoveryException(
          "NF_CHAIN_REJECTED: The MASQ chain identifier is invalid."
      )
    }

    val pool = Executors.newFixedThreadPool(NODE_FINDER_ATTEMPTS)
    try {
      val freshDescriptors =
          pool
              .invokeAll(
                  (0 until NODE_FINDER_ATTEMPTS).map { attempt ->
                    Callable { fetchCandidate(chain, generation, attempt) }
                  })
              .mapNotNull { future -> runCatching { future.get() }.getOrNull() }
      val cachedDescriptors = loadCached(chain)
      val selected =
          EntryNodeDiscoverySelection.select(
              chain = chain,
              freshDescriptors = freshDescriptors,
              preferredDescriptors = preferredNodes,
              cachedDescriptors = cachedDescriptors,
          )

      if (selected.size < REQUIRED_ENTRY_NODES) {
        safeDiagnostic(
            "NF_SELECTION_INSUFFICIENT",
            "fresh" to freshDescriptors.size,
            "preferred" to preferredNodes.size,
            "cached" to cachedDescriptors.size,
        )
        throw EntryNodeDiscoveryException(
            "NF_SELECTION_INSUFFICIENT: MASQ could not find two valid public entry nodes. " +
                "Retrying with fresh nodes."
        )
      }

      val result = EntryNodeDiscoveryResult.fromSelection(selected, generation)
      // Preserve every validated port variant. Only the passive, generation-selected
      // single-port descriptor is passed to the native core.
      saveCached(chain, result.persistentDescriptors)
      safeDiagnostic(
          "NF_SELECTION_OK",
          "selected" to selected.size,
          "generation" to generation,
      )
      return result
    } finally {
      pool.shutdownNow()
    }
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
            .addQueryParameter("refresh", "${generation.toUInt()}-$attempt")
            .build()
    val request =
        Request.Builder()
            .url(requestUrl)
            .get()
            .cacheControl(CacheControl.FORCE_NETWORK)
            .header("Accept", "text/plain, application/json")
            .build()

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
        val descriptor = normalizeDescriptor(boundedBody)
        if (EntryNodeDiscoverySelection.parse(descriptor, chain) == null) {
          safeDiagnostic("NF_FETCH_INVALID", "attempt" to attempt)
          return null
        }
        safeDiagnostic("NF_FETCH_OK", "attempt" to attempt)
        descriptor
      }
    } catch (_: Exception) {
      safeDiagnostic("NF_FETCH_IO", "attempt" to attempt)
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

  private fun normalizeDescriptor(value: String): String {
    val trimmed = value.trim()
    return runCatching { JSONTokener(trimmed).nextValue() }
        .getOrNull()
        .let { decoded -> if (decoded is String) decoded.trim() else trimmed }
  }

  private fun loadCached(chain: String): List<String> {
    val serialized = preferences.getString("$CACHE_PREFIX.$chain", null) ?: return emptyList()
    return runCatching {
          val array = JSONArray(serialized)
          (0 until array.length()).mapNotNull { index ->
            array.optString(index).takeIf(String::isNotBlank)
          }
        }
        .getOrDefault(emptyList())
  }

  private fun saveCached(chain: String, nodes: List<String>) {
    preferences.edit().putString("$CACHE_PREFIX.$chain", JSONArray(nodes).toString()).apply()
  }

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
    const val LOG_TAG = "MasqNodeFinder"
    const val PUBLIC_SUBURB = "masqpublic1"
    const val NODE_FINDER_ATTEMPTS = 6
    const val REQUIRED_ENTRY_NODES = 2
    const val NODE_FINDER_TIMEOUT_MS = 6000L
    const val NODE_FINDER_CALL_TIMEOUT_MS = 7000L
    const val MAX_RESPONSE_BYTES = 1024
    const val CACHE_PREFIX = "masq-mobile-entry-nodes"
    val NF_CODE_PATTERN = Regex("NF_[A-Z_]+")
    val NF_METRIC_PATTERN = Regex("[a-z_]+")
  }
}

internal data class EntryNodeCandidate(
    val originalDescriptor: String,
    val chain: String,
    val publicKey: String,
    val host: String,
    val ports: List<Int>,
) {
  fun persistentDescriptor(): String =
      "masq://$chain:$publicKey@$host:${ports.joinToString("/")}"

  fun singlePortDescriptor(generation: Int): String {
    val index = Math.floorMod(generation, ports.size)
    return "masq://$chain:$publicKey@$host:${ports[index]}"
  }
}

internal data class EntryNodeDiscoveryResult(
    val runtimeDescriptors: List<String>,
    val persistentDescriptors: List<String>,
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
  ): List<EntryNodeCandidate> {
    val freshCandidates = freshDescriptors.mapNotNull { descriptor -> parse(descriptor, chain) }
    val preferredCandidates =
        preferredDescriptors.mapNotNull { descriptor -> parse(descriptor, chain) }
    val cachedCandidates = cachedDescriptors.mapNotNull { descriptor -> parse(descriptor, chain) }
    val orderedCandidates = freshCandidates + preferredCandidates + cachedCandidates
    val selected = mutableListOf<EntryNodeCandidate>()
    val selectedKeys = mutableSetOf<String>()
    val selectedHosts = mutableSetOf<String>()
    orderedCandidates
        .forEachIndexed { index, candidate ->
          if (selected.size >= REQUIRED_ENTRY_NODES) return@forEachIndexed
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
                  preservePrimaryOrder = index < freshCandidates.size,
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
