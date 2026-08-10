package com.masqmobile

import android.content.SharedPreferences

enum class SystemRoutingMode(val wireName: String) {
  OFF("off"),
  WHOLE_DEVICE("wholeDevice"),
  SELECTED_APPS("selectedApps");

  companion object {
    fun fromWireName(value: String): SystemRoutingMode? =
        entries.firstOrNull { it.wireName == value }
  }
}

enum class SystemRoutingDiagnostic(val wireCode: String) {
  UNSUPPORTED_POLICY_SCHEMA("unsupported_policy_schema"),
  CORRUPT_OR_PARTIAL_POLICY("corrupt_or_partial_policy"),
  POLICY_READ_FAILED("policy_read_failed"),
  POLICY_COMMIT_INDETERMINATE("policy_commit_indeterminate"),
  POLICY_CLEAR_INDETERMINATE("policy_clear_indeterminate"),
  INVALID_START_MODE("invalid_start_mode"),
  INVALID_REVISION("invalid_revision"),
  REVISION_EXHAUSTED("revision_exhausted"),
  INVALID_PACKAGE_ID("invalid_package_id"),
  UNEXPECTED_PACKAGE_IDS("unexpected_package_ids"),
  EMPTY_SELECTED_APPS("empty_selected_apps"),
  MISSING_EXPLICIT_CONSENT("missing_explicit_consent"),
  VPN_INTERFACE_UNAVAILABLE("vpn_interface_unavailable"),
  TRANSLATOR_NOT_READY("translator_not_ready"),
  TRANSLATOR_STOP_TIMEOUT("translator_stop_timeout"),
  TRANSLATOR_RETURNED("translator_returned"),
  CORE_ROUTE_NOT_READY("core_route_not_ready"),
  POLICY_REVISION_CONFLICT("policy_revision_conflict"),
  PACKAGE_SCOPE_CHANGED("package_scope_changed"),
  PACKAGE_NOT_INSTALLED("package_not_installed"),
  OWN_PACKAGE_UNSUPPORTED("own_package_unsupported"),
  FAIL_CLOSED_UNSUPPORTED("fail_closed_unsupported"),
  ALWAYS_ON_UNSUPPORTED("always_on_unsupported"),
  LOCKDOWN_UNSUPPORTED("lockdown_unsupported"),
  NOTIFICATION_PERMISSION_REQUIRED("notification_permission_required"),
  TUNNEL_CLOSE_FAILED("tunnel_close_failed"),
  PERMISSION_REVOKED("permission_revoked"),
  NETWORK_UNAVAILABLE("network_unavailable"),
  INTERNAL_ERROR("internal_error"),
}

@ConsistentCopyVisibility
data class DesiredSystemRoutingPolicy internal constructor(
    val schemaVersion: Int,
    val revision: Long,
    val desiredMode: SystemRoutingMode,
    val selectedApps: List<String>,
    val explicitConsentTimestampMs: Long,
    val failClosedDesired: Boolean,
) {
  init {
    require(schemaVersion == SystemRoutingPolicyStore.CURRENT_SCHEMA_VERSION) {
      "The system-routing policy schema is unsupported."
    }
    require(revision > 0) {
      "The system-routing policy revision is invalid."
    }
    require(canonicalizeSystemRoutingPackageIds(selectedApps) == selectedApps) {
      "The system-routing package selection is not canonical."
    }
    when (desiredMode) {
      SystemRoutingMode.OFF ->
          require(
              selectedApps.isEmpty() &&
                  explicitConsentTimestampMs == 0L &&
                  !failClosedDesired) {
                "An off system-routing policy must contain no active routing data."
              }
      SystemRoutingMode.WHOLE_DEVICE ->
          require(selectedApps.isEmpty() && explicitConsentTimestampMs > 0) {
            "A whole-device policy requires consent and no package selection."
          }
      SystemRoutingMode.SELECTED_APPS ->
          require(selectedApps.isNotEmpty() && explicitConsentTimestampMs > 0) {
            "A selected-app policy requires consent and at least one package."
          }
    }
  }

  companion object {
    internal fun off(revision: Long): DesiredSystemRoutingPolicy =
        DesiredSystemRoutingPolicy(
            schemaVersion = SystemRoutingPolicyStore.CURRENT_SCHEMA_VERSION,
            revision = revision,
            desiredMode = SystemRoutingMode.OFF,
            selectedApps = emptyList(),
            explicitConsentTimestampMs = 0,
            failClosedDesired = false,
        )
  }
}

sealed interface SystemRoutingPolicyWriteResult {
  val mayStart: Boolean

  data class Stored(val policy: DesiredSystemRoutingPolicy) : SystemRoutingPolicyWriteResult {
    override val mayStart: Boolean
      get() = policy.desiredMode != SystemRoutingMode.OFF
  }

  data class Rejected(val reason: SystemRoutingDiagnostic) : SystemRoutingPolicyWriteResult {
    override val mayStart = false
  }

  data class Conflict(val actualRevision: Long?) : SystemRoutingPolicyWriteResult {
    override val mayStart = false
  }

  data class BlockRequired(val reason: SystemRoutingDiagnostic) :
      SystemRoutingPolicyWriteResult {
    override val mayStart = false
  }

  data class IndeterminateCommit(
      val observedRevision: Long?,
      val reason: SystemRoutingDiagnostic =
          SystemRoutingDiagnostic.POLICY_COMMIT_INDETERMINATE,
  ) :
      SystemRoutingPolicyWriteResult {
    override val mayStart = false
  }
}

sealed interface SystemRoutingPolicyLoadResult {
  val mayStart: Boolean
  val blockRequired: Boolean

  data object Missing : SystemRoutingPolicyLoadResult {
    override val mayStart = false
    override val blockRequired = false
  }

  data class ExplicitOff(val policy: DesiredSystemRoutingPolicy) :
      SystemRoutingPolicyLoadResult {
    override val mayStart = false
    override val blockRequired = false
  }

  data class Ready(val policy: DesiredSystemRoutingPolicy) : SystemRoutingPolicyLoadResult {
    override val mayStart = true
    override val blockRequired = false
  }

  data class BlockRequired(val reason: SystemRoutingDiagnostic) :
      SystemRoutingPolicyLoadResult {
    override val mayStart = false
    override val blockRequired = true
  }
}

sealed interface SystemRoutingPolicyClearResult {
  data object Cleared : SystemRoutingPolicyClearResult

  data class IndeterminateClear(val reason: SystemRoutingDiagnostic) :
      SystemRoutingPolicyClearResult
}

interface SystemRoutingPolicyStorage {
  fun readAll(): Map<String, *>

  fun replaceAll(values: Map<String, Any>): Boolean

  fun clearAll(): Boolean
}

class SharedPreferencesSystemRoutingPolicyStorage(
    private val preferences: SharedPreferences,
) : SystemRoutingPolicyStorage {
  override fun readAll(): Map<String, *> = preferences.all.toMap()

  override fun replaceAll(values: Map<String, Any>): Boolean {
    val editor = preferences.edit().clear()
    values.forEach { (key, value) ->
      when (value) {
        is Boolean -> editor.putBoolean(key, value)
        is Int -> editor.putInt(key, value)
        is Long -> editor.putLong(key, value)
        is String -> editor.putString(key, value)
        is Set<*> -> {
          require(value.all { it is String }) {
            "System-routing policy sets may only contain strings."
          }
          editor.putStringSet(key, value.filterIsInstance<String>().toSet())
        }
        else -> error("Unsupported system-routing policy value type.")
      }
    }
    return editor.commit()
  }

  override fun clearAll(): Boolean = preferences.edit().clear().commit()
}

class SystemRoutingPolicyStore(
    private val storage: SystemRoutingPolicyStorage,
) {
  fun persistBeforeStart(
      expectedRevision: Long?,
      desiredMode: SystemRoutingMode,
      packageIds: Iterable<String>,
      explicitConsentTimestampMs: Long,
      failClosedDesired: Boolean,
  ): SystemRoutingPolicyWriteResult =
      synchronized(processLock) {
        if (desiredMode == SystemRoutingMode.OFF) {
          return@synchronized rejected(SystemRoutingDiagnostic.INVALID_START_MODE)
        }
        if (expectedRevision != null && expectedRevision <= 0) {
          return@synchronized rejected(SystemRoutingDiagnostic.INVALID_REVISION)
        }
        if (explicitConsentTimestampMs <= 0) {
          return@synchronized rejected(SystemRoutingDiagnostic.MISSING_EXPLICIT_CONSENT)
        }
        val canonicalApps =
            canonicalizeSystemRoutingPackageIds(packageIds)
                ?: return@synchronized rejected(SystemRoutingDiagnostic.INVALID_PACKAGE_ID)
        if (desiredMode == SystemRoutingMode.WHOLE_DEVICE && canonicalApps.isNotEmpty()) {
          return@synchronized rejected(SystemRoutingDiagnostic.UNEXPECTED_PACKAGE_IDS)
        }
        if (desiredMode == SystemRoutingMode.SELECTED_APPS && canonicalApps.isEmpty()) {
          return@synchronized rejected(SystemRoutingDiagnostic.EMPTY_SELECTED_APPS)
        }

        val currentState = readStoredState()
        val currentPolicy =
            when (currentState) {
              StoredPolicyState.Missing -> null
              is StoredPolicyState.Valid -> currentState.policy
              is StoredPolicyState.Unsafe ->
                  return@synchronized SystemRoutingPolicyWriteResult.BlockRequired(
                      currentState.reason)
            }
        if (currentPolicy?.revision != expectedRevision) {
          return@synchronized SystemRoutingPolicyWriteResult.Conflict(currentPolicy?.revision)
        }
        val nextRevision =
            nextRevision(currentPolicy?.revision)
                ?: return@synchronized rejected(SystemRoutingDiagnostic.REVISION_EXHAUSTED)
        commitAndVerify(
            DesiredSystemRoutingPolicy(
                schemaVersion = CURRENT_SCHEMA_VERSION,
                revision = nextRevision,
                desiredMode = desiredMode,
                selectedApps = canonicalApps,
                explicitConsentTimestampMs = explicitConsentTimestampMs,
                failClosedDesired = failClosedDesired,
            ))
      }

  fun persistOff(expectedRevision: Long?): SystemRoutingPolicyWriteResult =
      synchronized(processLock) {
        if (expectedRevision != null && expectedRevision <= 0) {
          return@synchronized rejected(SystemRoutingDiagnostic.INVALID_REVISION)
        }
        val state = readStoredState()
        val current =
            when (state) {
              StoredPolicyState.Missing -> null
              is StoredPolicyState.Valid -> state.policy
              is StoredPolicyState.Unsafe ->
                  return@synchronized SystemRoutingPolicyWriteResult.BlockRequired(state.reason)
            }
        if (current?.revision != expectedRevision) {
          return@synchronized SystemRoutingPolicyWriteResult.Conflict(current?.revision)
        }
        val nextRevision =
            nextRevision(current?.revision)
                ?: return@synchronized rejected(SystemRoutingDiagnostic.REVISION_EXHAUSTED)
        commitAndVerify(DesiredSystemRoutingPolicy.off(nextRevision))
      }

  fun loadForServiceStart(): SystemRoutingPolicyLoadResult =
      synchronized(processLock) {
        when (val stored = readStoredState()) {
          StoredPolicyState.Missing -> SystemRoutingPolicyLoadResult.Missing
          is StoredPolicyState.Unsafe ->
              SystemRoutingPolicyLoadResult.BlockRequired(stored.reason)
          is StoredPolicyState.Valid ->
              if (stored.policy.desiredMode == SystemRoutingMode.OFF) {
                SystemRoutingPolicyLoadResult.ExplicitOff(stored.policy)
              } else {
                SystemRoutingPolicyLoadResult.Ready(stored.policy)
              }
        }
      }

  fun clearAfterExplicitReset(): SystemRoutingPolicyClearResult =
      synchronized(processLock) {
        val commitSucceeded =
            try {
              storage.clearAll()
            } catch (_: RuntimeException) {
              false
            }
        val readbackIsEmpty =
            try {
              storage.readAll().isEmpty()
            } catch (_: RuntimeException) {
              false
            }
        if (commitSucceeded && readbackIsEmpty) {
          SystemRoutingPolicyClearResult.Cleared
        } else {
          SystemRoutingPolicyClearResult.IndeterminateClear(
              SystemRoutingDiagnostic.POLICY_CLEAR_INDETERMINATE)
        }
      }

  private fun commitAndVerify(
      policy: DesiredSystemRoutingPolicy,
  ): SystemRoutingPolicyWriteResult {
    val commitSucceeded =
        try {
          storage.replaceAll(policy.toPreferences())
        } catch (_: RuntimeException) {
          false
        }
    val readback = readStoredState()
    return if (
        commitSucceeded &&
            readback is StoredPolicyState.Valid &&
            readback.policy == policy
    ) {
      SystemRoutingPolicyWriteResult.Stored(policy)
    } else {
      SystemRoutingPolicyWriteResult.IndeterminateCommit(
          (readback as? StoredPolicyState.Valid)?.policy?.revision)
    }
  }

  private fun DesiredSystemRoutingPolicy.toPreferences(): Map<String, Any> =
      mapOf(
          KEY_SCHEMA_VERSION to schemaVersion,
          KEY_REVISION to revision,
          KEY_DESIRED_MODE to desiredMode.wireName,
          KEY_SELECTED_APPS to selectedApps.toSet(),
          KEY_CONSENT_TIMESTAMP to explicitConsentTimestampMs,
          KEY_FAIL_CLOSED_DESIRED to failClosedDesired,
      )

  private fun readStoredState(): StoredPolicyState {
    val values =
        try {
          storage.readAll()
        } catch (_: RuntimeException) {
          return StoredPolicyState.Unsafe(SystemRoutingDiagnostic.POLICY_READ_FAILED)
        }
    if (values.isEmpty()) return StoredPolicyState.Missing
    val schemaVersion = values[KEY_SCHEMA_VERSION] as? Int ?: return corruptPolicy()
    if (schemaVersion != CURRENT_SCHEMA_VERSION) {
      return StoredPolicyState.Unsafe(
          SystemRoutingDiagnostic.UNSUPPORTED_POLICY_SCHEMA)
    }
    val revision = values[KEY_REVISION] as? Long ?: return corruptPolicy()
    if (revision <= 0) return corruptPolicy()
    val mode =
        (values[KEY_DESIRED_MODE] as? String)?.let(SystemRoutingMode::fromWireName)
            ?: return corruptPolicy()
    val rawApps = values[KEY_SELECTED_APPS] as? Set<*> ?: return corruptPolicy()
    if (!rawApps.all { it is String }) return corruptPolicy()
    val apps =
        canonicalizeSystemRoutingPackageIds(rawApps.filterIsInstance<String>())
            ?: return corruptPolicy()
    val consent = values[KEY_CONSENT_TIMESTAMP] as? Long ?: return corruptPolicy()
    val failClosed = values[KEY_FAIL_CLOSED_DESIRED] as? Boolean ?: return corruptPolicy()
    val policy =
        when (mode) {
          SystemRoutingMode.OFF -> {
            if (apps.isNotEmpty() || consent != 0L || failClosed) return corruptPolicy()
            DesiredSystemRoutingPolicy.off(revision)
          }
          SystemRoutingMode.WHOLE_DEVICE -> {
            if (apps.isNotEmpty() || consent <= 0) return corruptPolicy()
            DesiredSystemRoutingPolicy(
                schemaVersion,
                revision,
                mode,
                emptyList(),
                consent,
                failClosed,
            )
          }
          SystemRoutingMode.SELECTED_APPS -> {
            if (apps.isEmpty() || consent <= 0) return corruptPolicy()
            DesiredSystemRoutingPolicy(schemaVersion, revision, mode, apps, consent, failClosed)
          }
        }
    return StoredPolicyState.Valid(policy)
  }

  private fun corruptPolicy() =
      StoredPolicyState.Unsafe(SystemRoutingDiagnostic.CORRUPT_OR_PARTIAL_POLICY)

  private fun rejected(reason: SystemRoutingDiagnostic) =
      SystemRoutingPolicyWriteResult.Rejected(reason)

  private fun nextRevision(current: Long?): Long? =
      when {
        current == null -> 1
        current == Long.MAX_VALUE -> null
        else -> current + 1
      }

  private sealed interface StoredPolicyState {
    data object Missing : StoredPolicyState

    data class Valid(val policy: DesiredSystemRoutingPolicy) : StoredPolicyState

    data class Unsafe(val reason: SystemRoutingDiagnostic) : StoredPolicyState
  }

  companion object {
    const val CURRENT_SCHEMA_VERSION = 1
    const val PREFERENCES_NAME = "masq-system-routing-policy"

    private const val KEY_SCHEMA_VERSION = "schemaVersion"
    private const val KEY_REVISION = "revision"
    private const val KEY_DESIRED_MODE = "desiredMode"
    private const val KEY_SELECTED_APPS = "selectedApps"
    private const val KEY_CONSENT_TIMESTAMP = "explicitConsentTimestampMs"
    private const val KEY_FAIL_CLOSED_DESIRED = "failClosedDesired"
    private val processLock = Any()
  }
}

private val ANDROID_PACKAGE_ID =
    Regex("^[A-Za-z][A-Za-z0-9_]*(?:\\.[A-Za-z][A-Za-z0-9_]*)+$")

internal fun canonicalizeSystemRoutingPackageIds(packageIds: Iterable<String>): List<String>? {
  val values = packageIds.toList()
  if (values.any { it != it.trim() || !ANDROID_PACKAGE_ID.matches(it) }) return null
  return values.distinct().sorted()
}
