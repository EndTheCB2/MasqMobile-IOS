package com.masqmobile

enum class SystemRoutingTransition {
  IDLE,
  REQUESTING_PERMISSION,
  STARTING_BLOCKING,
  RECONNECTING,
  BLOCKED,
  STOPPING,
  REVOKED,
}

enum class SystemRoutingPhase(
    val wireName: String,
    val legacyWireName: String,
) {
  OFF("off", "off"),
  REQUESTING_PERMISSION("requestingPermission", "starting"),
  STARTING_BLOCKING("startingBlocking", "starting"),
  RECONNECTING("reconnecting", "starting"),
  ACTIVE("active", "active"),
  BLOCKED("blocked", "blocked"),
  STOPPING("stopping", "stopping"),
  REVOKED("revoked", "blocked"),
}

enum class SystemRoutingTrafficDisposition(val wireName: String) {
  MASQ("masq"),
  BLOCKED("blocked"),
  DIRECT_RISK("directRisk"),
  OFF("off"),
}

class SystemRoutingStatus private constructor(
    val supported: Boolean,
    val desiredRevision: Long?,
    val desiredMode: SystemRoutingMode,
    val desiredSelectedApps: List<String>,
    val failClosedDesired: Boolean,
    val appliedRevision: Long?,
    val appliedMode: SystemRoutingMode,
    val appliedSelectedApps: List<String>,
    val phase: SystemRoutingPhase,
    val active: Boolean,
    val trafficDisposition: SystemRoutingTrafficDisposition,
    val tunPresent: Boolean,
    val translatorReady: Boolean,
    val coreRouteReady: Boolean,
    val trafficObserved: Boolean,
    val alwaysOn: Boolean,
    val lockdown: Boolean,
    val lastError: SystemRoutingDiagnostic?,
) {
  fun toJson(): String =
      buildString {
        append('{')
        append("\"schemaVersion\":").append(STATUS_SCHEMA_VERSION)
        append(",\"supported\":").append(supported)
        append(",\"active\":").append(active)
        append(",\"mode\":").append(jsonString(desiredMode.wireName))
        append(",\"phase\":").append(jsonString(phase.legacyWireName))
        append(",\"selectedApps\":")
        appendJsonArray(desiredSelectedApps)
        append(",\"lastError\":")
        appendNullableDiagnostic(lastError)
        append(",\"routingPhase\":").append(jsonString(phase.wireName))
        append(",\"trafficDisposition\":")
        append(jsonString(trafficDisposition.wireName))
        append(",\"desiredRevision\":")
        appendNullableLong(desiredRevision)
        append(",\"desiredMode\":").append(jsonString(desiredMode.wireName))
        append(",\"desiredSelectedApps\":")
        appendJsonArray(desiredSelectedApps)
        append(",\"failClosedDesired\":").append(failClosedDesired)
        append(",\"appliedRevision\":")
        appendNullableLong(appliedRevision)
        append(",\"appliedMode\":").append(jsonString(appliedMode.wireName))
        append(",\"appliedSelectedApps\":")
        appendJsonArray(appliedSelectedApps)
        append(",\"tunPresent\":").append(tunPresent)
        append(",\"translatorReady\":").append(translatorReady)
        append(",\"coreRouteReady\":").append(coreRouteReady)
        append(",\"trafficObserved\":").append(trafficObserved)
        append(",\"alwaysOn\":").append(alwaysOn)
        append(",\"lockdown\":").append(lockdown)
        append('}')
      }

  companion object {
    const val STATUS_SCHEMA_VERSION = 2

    fun derive(
        supported: Boolean,
        desiredRevision: Long?,
        desiredMode: SystemRoutingMode,
        desiredSelectedApps: Iterable<String>,
        failClosedDesired: Boolean,
        appliedRevision: Long?,
        appliedMode: SystemRoutingMode,
        appliedSelectedApps: Iterable<String>,
        transition: SystemRoutingTransition,
        tunPresent: Boolean,
        translatorReady: Boolean,
        coreRouteReady: Boolean,
        trafficObserved: Boolean = false,
        alwaysOn: Boolean,
        lockdown: Boolean,
        lastError: SystemRoutingDiagnostic? = null,
    ): SystemRoutingStatus {
      val desiredApps =
          validatePolicyIdentity(
              revision = desiredRevision,
              mode = desiredMode,
              selectedApps = desiredSelectedApps,
              label = "desired",
          )
      val appliedApps =
          validatePolicyIdentity(
              revision = appliedRevision,
              mode = appliedMode,
              selectedApps = appliedSelectedApps,
              label = "applied",
          )
      require(desiredMode != SystemRoutingMode.OFF || !failClosedDesired) {
        "An off system-routing policy cannot request fail-closed routing."
      }

      val policiesMatch =
          desiredRevision != null &&
              desiredRevision == appliedRevision &&
              desiredMode != SystemRoutingMode.OFF &&
              desiredMode == appliedMode &&
              desiredApps == appliedApps
      val routeHealthy =
          supported &&
              policiesMatch &&
              tunPresent &&
              translatorReady &&
              coreRouteReady &&
              lastError == null
      val phase =
          when (transition) {
            SystemRoutingTransition.REQUESTING_PERMISSION ->
                SystemRoutingPhase.REQUESTING_PERMISSION
            SystemRoutingTransition.STARTING_BLOCKING ->
                SystemRoutingPhase.STARTING_BLOCKING
            SystemRoutingTransition.RECONNECTING -> SystemRoutingPhase.RECONNECTING
            SystemRoutingTransition.BLOCKED -> SystemRoutingPhase.BLOCKED
            SystemRoutingTransition.STOPPING -> SystemRoutingPhase.STOPPING
            SystemRoutingTransition.REVOKED -> SystemRoutingPhase.REVOKED
            SystemRoutingTransition.IDLE ->
                when {
                  routeHealthy -> SystemRoutingPhase.ACTIVE
                  desiredMode == SystemRoutingMode.OFF &&
                      appliedMode == SystemRoutingMode.OFF &&
                      !tunPresent -> SystemRoutingPhase.OFF
                  else -> SystemRoutingPhase.BLOCKED
                }
          }
      val active = phase == SystemRoutingPhase.ACTIVE
      val desiredScopeCaptured =
          tunPresent &&
              when (desiredMode) {
                SystemRoutingMode.OFF -> false
                SystemRoutingMode.WHOLE_DEVICE ->
                    appliedMode == SystemRoutingMode.WHOLE_DEVICE
                SystemRoutingMode.SELECTED_APPS ->
                    appliedMode == SystemRoutingMode.WHOLE_DEVICE ||
                        (appliedMode == SystemRoutingMode.SELECTED_APPS &&
                            appliedApps.containsAll(desiredApps))
              }
      val disposition =
          when {
            active -> SystemRoutingTrafficDisposition.MASQ
            desiredMode == SystemRoutingMode.OFF && tunPresent ->
                SystemRoutingTrafficDisposition.BLOCKED
            desiredMode == SystemRoutingMode.OFF -> SystemRoutingTrafficDisposition.OFF
            desiredScopeCaptured || (alwaysOn && lockdown) ->
                SystemRoutingTrafficDisposition.BLOCKED
            desiredMode != SystemRoutingMode.OFF ->
                SystemRoutingTrafficDisposition.DIRECT_RISK
            else -> error("Unreachable system-routing disposition.")
          }
      return SystemRoutingStatus(
          supported = supported,
          desiredRevision = desiredRevision,
          desiredMode = desiredMode,
          desiredSelectedApps = desiredApps,
          failClosedDesired = failClosedDesired,
          appliedRevision = appliedRevision,
          appliedMode = appliedMode,
          appliedSelectedApps = appliedApps,
          phase = phase,
          active = active,
          trafficDisposition = disposition,
          tunPresent = tunPresent,
          translatorReady = translatorReady,
          coreRouteReady = coreRouteReady,
          trafficObserved = routeHealthy && trafficObserved,
          alwaysOn = alwaysOn,
          lockdown = lockdown,
          lastError = lastError,
      )
    }

    private fun validatePolicyIdentity(
        revision: Long?,
        mode: SystemRoutingMode,
        selectedApps: Iterable<String>,
        label: String,
    ): List<String> {
      require(revision == null || revision > 0) {
        "The $label system-routing revision is invalid."
      }
      require(mode == SystemRoutingMode.OFF || revision != null) {
        "The $label active system-routing policy requires a revision."
      }
      val canonicalApps =
          requireNotNull(canonicalizeSystemRoutingPackageIds(selectedApps)) {
            "The $label system-routing policy contains an invalid package ID."
          }
      require(
          (mode == SystemRoutingMode.SELECTED_APPS && canonicalApps.isNotEmpty()) ||
              (mode != SystemRoutingMode.SELECTED_APPS && canonicalApps.isEmpty())) {
        "The $label system-routing package selection does not match its mode."
      }
      return canonicalApps
    }
  }
}

internal fun systemRoutingStatusAfterServiceDestroyed(
    load: SystemRoutingPolicyLoadResult,
    supported: Boolean,
    alwaysOn: Boolean,
    lockdown: Boolean,
    retainedAppliedPolicy: DesiredSystemRoutingPolicy? = null,
    tunPresent: Boolean = false,
    captureValid: Boolean = true,
    diagnostic: SystemRoutingDiagnostic? = null,
): SystemRoutingStatus {
  val desired =
      when (load) {
        is SystemRoutingPolicyLoadResult.Ready -> load.policy
        is SystemRoutingPolicyLoadResult.ExplicitOff -> load.policy
        else -> null
      }
  val effectiveTunPresent = tunPresent && captureValid
  val effectiveAppliedPolicy =
      retainedAppliedPolicy.takeIf { effectiveTunPresent }
  return SystemRoutingStatus.derive(
      supported = supported,
      desiredRevision = desired?.revision,
      desiredMode = desired?.desiredMode ?: SystemRoutingMode.OFF,
      desiredSelectedApps = desired?.selectedApps ?: emptyList(),
      failClosedDesired = desired?.failClosedDesired ?: false,
      appliedRevision = effectiveAppliedPolicy?.revision,
      appliedMode = effectiveAppliedPolicy?.desiredMode ?: SystemRoutingMode.OFF,
      appliedSelectedApps = effectiveAppliedPolicy?.selectedApps ?: emptyList(),
      transition =
          if (!captureValid) {
            SystemRoutingTransition.REVOKED
          } else if (load is SystemRoutingPolicyLoadResult.Ready ||
              load is SystemRoutingPolicyLoadResult.BlockRequired ||
              effectiveTunPresent) {
            SystemRoutingTransition.BLOCKED
          } else {
            SystemRoutingTransition.IDLE
          },
      tunPresent = effectiveTunPresent,
      translatorReady = false,
      coreRouteReady = false,
      alwaysOn = alwaysOn,
      lockdown = lockdown,
      lastError =
          diagnostic
              ?: (load as? SystemRoutingPolicyLoadResult.BlockRequired)?.reason,
  )
}

private fun StringBuilder.appendJsonArray(values: List<String>) {
  append('[')
  values.forEachIndexed { index, value ->
    if (index > 0) append(',')
    append(jsonString(value))
  }
  append(']')
}

private fun StringBuilder.appendNullableDiagnostic(value: SystemRoutingDiagnostic?) {
  if (value == null) append("null") else append(jsonString(value.wireCode))
}

private fun StringBuilder.appendNullableLong(value: Long?) {
  if (value == null) append("null") else append(value)
}

private fun jsonString(value: String): String =
    buildString {
      append('"')
      value.forEach { character ->
        when (character) {
          '"' -> append("\\\"")
          '\\' -> append("\\\\")
          '\b' -> append("\\b")
          '\u000C' -> append("\\f")
          '\n' -> append("\\n")
          '\r' -> append("\\r")
          '\t' -> append("\\t")
          else -> {
            if (character.code < 0x20) {
              append("\\u")
              append(character.code.toString(16).padStart(4, '0'))
            } else {
              append(character)
            }
          }
        }
      }
      append('"')
    }
