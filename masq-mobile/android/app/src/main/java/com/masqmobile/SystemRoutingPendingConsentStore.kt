package com.masqmobile

import android.content.SharedPreferences

/**
 * Minimal, short-lived continuation state for Android's system VPN consent activity.
 *
 * This store intentionally contains no wallet, seed, RPC, entry-node, or network-profile data.
 * Selected package IDs are required only to reconstruct the exact user-approved routing scope and
 * are kept in this app's private SharedPreferences, just like the durable routing policy that is
 * written after consent succeeds.
 */
internal data class PendingSystemRoutingConsent(
    val consentId: String,
    val mode: SystemRoutingMode,
    val selectedApps: List<String>,
    val expectedRevision: Long?,
    val reuseRevision: Long?,
    val createdAtEpochMs: Long,
) {
  init {
    require(consentId.matches(CONSENT_ID_PATTERN)) {
      "The pending system-routing consent ID is invalid."
    }
    require(mode == SystemRoutingMode.WHOLE_DEVICE || mode == SystemRoutingMode.SELECTED_APPS) {
      "A pending system-routing consent requires an active routing mode."
    }
    require(canonicalizeSystemRoutingPackageIds(selectedApps) == selectedApps) {
      "The pending system-routing package selection is not canonical."
    }
    require(
        (mode == SystemRoutingMode.SELECTED_APPS && selectedApps.isNotEmpty()) ||
            (mode == SystemRoutingMode.WHOLE_DEVICE && selectedApps.isEmpty())) {
          "The pending system-routing package selection does not match its mode."
        }
    require(expectedRevision == null || expectedRevision > 0) {
      "The pending expected routing revision is invalid."
    }
    require(reuseRevision == null || reuseRevision > 0) {
      "The pending reusable routing revision is invalid."
    }
    require(reuseRevision == null || reuseRevision == expectedRevision) {
      "A reusable routing revision must match the expected revision."
    }
    require(createdAtEpochMs > 0) {
      "The pending system-routing consent timestamp is invalid."
    }
  }

  private companion object {
    val CONSENT_ID_PATTERN = Regex("^[A-Za-z0-9-]{16,64}$")
  }
}

internal interface PendingSystemRoutingConsentStorage {
  fun readAll(): Map<String, *>

  fun replaceAll(values: Map<String, Any>): Boolean

  fun clearAll(): Boolean
}

internal class SharedPreferencesPendingSystemRoutingConsentStorage(
    preferences: SharedPreferences,
) : PendingSystemRoutingConsentStorage {
  private val preferences = preferences

  override fun readAll(): Map<String, *> = preferences.all.toMap()

  override fun replaceAll(values: Map<String, Any>): Boolean {
    val editor = preferences.edit().clear()
    values.forEach { (key, value) ->
      when (value) {
        is Int -> editor.putInt(key, value)
        is Long -> editor.putLong(key, value)
        is String -> editor.putString(key, value)
        is Set<*> -> {
          require(value.all { it is String }) {
            "Pending system-routing consent sets may only contain strings."
          }
          editor.putStringSet(key, value.filterIsInstance<String>().toSet())
        }
        else -> error("Unsupported pending system-routing consent value type.")
      }
    }
    return editor.commit()
  }

  override fun clearAll(): Boolean = preferences.edit().clear().commit()
}

internal class SystemRoutingPendingConsentStore(
    private val storage: PendingSystemRoutingConsentStorage,
) {
  fun persist(consent: PendingSystemRoutingConsent): Boolean =
      synchronized(processLock) {
        val values = consent.toPreferences()
        val committed =
            runCatching { storage.replaceAll(values) }
                .getOrDefault(false)
        val readback = readStoredConsent()
        val exact = committed && readback == consent
        if (!exact) {
          clearUnconditionally()
        }
        exact
      }

  /** Returns only a current, fully valid continuation. Invalid or expired state is cleared. */
  fun load(nowEpochMs: Long): PendingSystemRoutingConsent? =
      synchronized(processLock) {
        val consent = readStoredConsent()
        if (consent == null) {
          if (safeReadAll().isNotEmpty()) clearUnconditionally()
          return@synchronized null
        }
        val age = nowEpochMs - consent.createdAtEpochMs
        if (age !in 0..MAX_AGE_MS) {
          clearUnconditionally()
          return@synchronized null
        }
        consent
      }

  /** Clears only the continuation identified by [consentId], preserving a newer request. */
  fun clear(consentId: String): Boolean =
      synchronized(processLock) {
        val current = readStoredConsent()
        if (current == null) {
          if (safeReadAll().isNotEmpty()) clearUnconditionally() else true
        } else if (current.consentId != consentId) {
          false
        } else {
          clearUnconditionally()
        }
      }

  fun clearAll(): Boolean = synchronized(processLock) { clearUnconditionally() }

  private fun PendingSystemRoutingConsent.toPreferences(): Map<String, Any> =
      mapOf(
          KEY_SCHEMA_VERSION to CURRENT_SCHEMA_VERSION,
          KEY_CONSENT_ID to consentId,
          KEY_MODE to mode.wireName,
          KEY_SELECTED_APPS to selectedApps.toSet(),
          KEY_EXPECTED_REVISION to (expectedRevision ?: NO_REVISION),
          KEY_REUSE_REVISION to (reuseRevision ?: NO_REVISION),
          KEY_CREATED_AT to createdAtEpochMs,
      )

  private fun readStoredConsent(): PendingSystemRoutingConsent? {
    val values = safeReadAll()
    if (values.isEmpty()) return null
    if (values.keys != REQUIRED_KEYS) return null
    if (values[KEY_SCHEMA_VERSION] != CURRENT_SCHEMA_VERSION) return null
    val consentId = values[KEY_CONSENT_ID] as? String ?: return null
    val mode =
        (values[KEY_MODE] as? String)?.let(SystemRoutingMode::fromWireName)
            ?: return null
    val rawApps = values[KEY_SELECTED_APPS] as? Set<*> ?: return null
    if (!rawApps.all { it is String }) return null
    val apps = canonicalizeSystemRoutingPackageIds(rawApps.filterIsInstance<String>()) ?: return null
    val expected = values[KEY_EXPECTED_REVISION] as? Long ?: return null
    val reuse = values[KEY_REUSE_REVISION] as? Long ?: return null
    val createdAt = values[KEY_CREATED_AT] as? Long ?: return null
    return runCatching {
          PendingSystemRoutingConsent(
              consentId = consentId,
              mode = mode,
              selectedApps = apps,
              expectedRevision = expected.takeIf { it != NO_REVISION },
              reuseRevision = reuse.takeIf { it != NO_REVISION },
              createdAtEpochMs = createdAt,
          )
        }
        .getOrNull()
  }

  private fun safeReadAll(): Map<String, *> =
      runCatching { storage.readAll() }.getOrDefault(emptyMap<String, Any>())

  private fun clearUnconditionally(): Boolean {
    runCatching { storage.clearAll() }
    return safeReadAll().isEmpty()
  }

  companion object {
    const val PREFERENCES_NAME = "masq-system-routing-pending-consent"
    const val MAX_AGE_MS = 10 * 60 * 1000L
    private const val CURRENT_SCHEMA_VERSION = 1
    private const val NO_REVISION = 0L
    private const val KEY_SCHEMA_VERSION = "schemaVersion"
    private const val KEY_CONSENT_ID = "consentId"
    private const val KEY_MODE = "mode"
    private const val KEY_SELECTED_APPS = "selectedApps"
    private const val KEY_EXPECTED_REVISION = "expectedRevision"
    private const val KEY_REUSE_REVISION = "reuseRevision"
    private const val KEY_CREATED_AT = "createdAtEpochMs"
    private val REQUIRED_KEYS =
        setOf(
            KEY_SCHEMA_VERSION,
            KEY_CONSENT_ID,
            KEY_MODE,
            KEY_SELECTED_APPS,
            KEY_EXPECTED_REVISION,
            KEY_REUSE_REVISION,
            KEY_CREATED_AT,
        )
    private val processLock = Any()
  }
}
