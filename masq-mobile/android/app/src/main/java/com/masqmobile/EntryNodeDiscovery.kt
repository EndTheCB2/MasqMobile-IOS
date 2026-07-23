package com.masqmobile

import android.content.Context
import java.net.HttpURLConnection
import java.net.InetSocketAddress
import java.net.Socket
import java.net.URI
import java.net.URL
import java.util.concurrent.Callable
import java.util.concurrent.Executors
import org.json.JSONArray
import org.json.JSONTokener

internal class EntryNodeDiscovery(context: Context) {
  private val preferences =
      context.getSharedPreferences("masq-mobile-consumer", Context.MODE_PRIVATE)

  fun discover(chain: String, preferredNodes: List<String>): List<String> {
    val candidates = linkedSetOf<String>()
    candidates.addAll(loadCached(chain).filter { descriptorParts(it, chain) != null })
    candidates.addAll(preferredNodes.filter { descriptorParts(it, chain) != null })

    val pool = Executors.newFixedThreadPool(NODE_FINDER_ATTEMPTS)
    try {
      val requests =
          (0 until NODE_FINDER_ATTEMPTS).map { attempt ->
            Callable {
              fetchCandidate(chain, attempt)?.takeIf { descriptorParts(it, chain) != null }
            }
          }
      pool.invokeAll(requests).forEach { future ->
        runCatching { future.get() }.getOrNull()?.let(candidates::add)
      }

      val reachabilityChecks =
          candidates.map { descriptor ->
            Callable { descriptor.takeIf { isReachable(it, chain) } }
          }
      val reachable =
          pool.invokeAll(reachabilityChecks)
              .mapNotNull { future -> runCatching { future.get() }.getOrNull() }
              .take(REQUIRED_ENTRY_NODES)
      if (reachable.size < REQUIRED_ENTRY_NODES) {
        throw EntryNodeDiscoveryException(
            "MASQ could not find two reachable entry nodes. Retrying with fresh nodes."
        )
      }
      saveCached(chain, reachable)
      return reachable
    } finally {
      pool.shutdownNow()
    }
  }

  private fun fetchCandidate(chain: String, attempt: Int): String? {
    val connection =
        URL(
                "${BuildConfig.MASQ_NODE_FINDER_URL}/randomnode/$chain/$PUBLIC_SUBURB" +
                    "?refresh=${System.nanoTime()}-$attempt"
            )
            .openConnection() as HttpURLConnection
    return try {
      connection.connectTimeout = NODE_FINDER_TIMEOUT_MS
      connection.readTimeout = NODE_FINDER_TIMEOUT_MS
      connection.requestMethod = "GET"
      connection.setRequestProperty("Accept", "text/plain")
      connection.setRequestProperty("Cache-Control", "no-cache")
      if (connection.responseCode !in 200..299) return null
      normalizeDescriptor(connection.inputStream.bufferedReader().use { it.readText() })
    } catch (_: Exception) {
      null
    } finally {
      connection.disconnect()
    }
  }

  private fun normalizeDescriptor(value: String): String {
    val trimmed = value.trim()
    return runCatching { JSONTokener(trimmed).nextValue() }
        .getOrNull()
        .let { decoded -> if (decoded is String) decoded.trim() else trimmed }
  }

  private fun isReachable(descriptor: String, chain: String): Boolean {
    val (_, host, port) = descriptorParts(descriptor, chain) ?: return false
    return try {
      Socket().use { socket ->
        socket.connect(InetSocketAddress(host, port), ENTRY_NODE_TIMEOUT_MS)
      }
      true
    } catch (_: Exception) {
      false
    }
  }

  private fun descriptorParts(descriptor: String, chain: String): Triple<String, String, Int>? {
    return try {
      val uri = URI(descriptor)
      val userInfo = uri.rawUserInfo ?: return null
      val separator = userInfo.indexOf(':')
      val descriptorChain = if (separator > 0) userInfo.substring(0, separator) else return null
      val publicKey = userInfo.substring(separator + 1)
      val host = uri.host ?: return null
      if (
          uri.scheme?.lowercase() != "masq" ||
              descriptorChain != chain ||
              publicKey.isBlank() ||
              uri.port !in 1..65535
      ) {
        return null
      }
      Triple(publicKey, host, uri.port)
    } catch (_: Exception) {
      null
    }
  }

  private fun loadCached(chain: String): List<String> {
    val serialized = preferences.getString("$CACHE_PREFIX.$chain", null) ?: return emptyList()
    return runCatching {
          val array = JSONArray(serialized)
          (0 until array.length()).mapNotNull { index -> array.optString(index).takeIf(String::isNotBlank) }
        }
        .getOrDefault(emptyList())
  }

  private fun saveCached(chain: String, nodes: List<String>) {
    preferences.edit().putString("$CACHE_PREFIX.$chain", JSONArray(nodes).toString()).apply()
  }

  private companion object {
    const val PUBLIC_SUBURB = "masqpublic1"
    const val NODE_FINDER_ATTEMPTS = 6
    const val REQUIRED_ENTRY_NODES = 2
    const val NODE_FINDER_TIMEOUT_MS = 6000
    const val ENTRY_NODE_TIMEOUT_MS = 2500
    const val CACHE_PREFIX = "masq-mobile-entry-nodes"
  }
}

internal class EntryNodeDiscoveryException(message: String) : Exception(message)
