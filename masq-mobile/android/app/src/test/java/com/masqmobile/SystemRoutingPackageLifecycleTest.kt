package com.masqmobile

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class SystemRoutingPackageLifecycleTest {
  @Test
  fun selectedScopeRebuildsForExactPackageAndSharedUidChangesOnly() {
    val policy = selectedPolicy("org.example.browser")
    val installedUids =
        mapOf(
            "org.example.browser" to 10_041,
            "org.example.other" to 10_099,
        )

    assertTrue(
        systemRoutingPackageChangeAffectsPolicy(
            policy,
            "org.example.browser",
            10_041,
            installedUids::get,
        ))
    assertTrue(
        systemRoutingPackageChangeAffectsPolicy(
            policy,
            "org.example.shared",
            10_041,
            installedUids::get,
        ))
    assertFalse(
        systemRoutingPackageChangeAffectsPolicy(
            policy,
            "org.example.other",
            10_099,
            installedUids::get,
        ))
  }

  @Test
  fun wholeDeviceScopeRebuildsWhenControlPlaneExclusionOrSharedUidChanges() {
    val policy = wholeDevicePolicy()
    val installedUids =
        mapOf(
            PUBLIC_MASQ_PACKAGE_ID to 10_101,
            DOGFOOD_MASQ_PACKAGE_ID to 10_102,
        )

    assertTrue(
        systemRoutingPackageChangeAffectsPolicy(
            policy,
            DOGFOOD_MASQ_PACKAGE_ID,
            null,
            installedUids::get,
        ))
    assertTrue(
        systemRoutingPackageChangeAffectsPolicy(
            policy,
            "org.example.shared",
            10_101,
            installedUids::get,
        ))
    assertFalse(
        systemRoutingPackageChangeAffectsPolicy(
            policy,
            "org.example.browser",
            10_200,
            installedUids::get,
        ))
  }

  @Test
  fun offPolicyIgnoresEveryPackageChange() {
    assertFalse(
        systemRoutingPackageChangeAffectsPolicy(
            DesiredSystemRoutingPolicy.off(7),
            PUBLIC_MASQ_PACKAGE_ID,
            10_101,
        ) { 10_101 })
  }

  @Test
  fun drainCoalescesWithoutDroppingChangeObservedDuringRebuild() {
    val drain = SystemRoutingPackageChangeDrain()

    assertTrue(drain.observe(retireMissingSelectedScope = false))
    assertFalse(drain.observe(retireMissingSelectedScope = true))
    val first = checkNotNull(drain.nextBatch())
    assertEquals(2L, first.epoch)
    assertTrue(first.retireMissingSelectedScope)

    assertFalse(drain.observe(retireMissingSelectedScope = false))
    assertTrue(drain.complete(first))
    val second = checkNotNull(drain.nextBatch())
    assertEquals(3L, second.epoch)
    assertFalse(second.retireMissingSelectedScope)
    assertFalse(drain.complete(second))

    assertTrue(drain.observe(retireMissingSelectedScope = false))
  }

  @Test
  fun permanentRemovalBitAppliesOnlyToItsPendingBatch() {
    val drain = SystemRoutingPackageChangeDrain()
    drain.observe(retireMissingSelectedScope = true)
    val removal = checkNotNull(drain.nextBatch())
    assertTrue(removal.retireMissingSelectedScope)
    assertFalse(drain.complete(removal))

    drain.observe(retireMissingSelectedScope = false)
    val replacement = checkNotNull(drain.nextBatch())
    assertFalse(replacement.retireMissingSelectedScope)
  }

  private fun wholeDevicePolicy() =
      DesiredSystemRoutingPolicy(
          schemaVersion = SystemRoutingPolicyStore.CURRENT_SCHEMA_VERSION,
          revision = 1,
          desiredMode = SystemRoutingMode.WHOLE_DEVICE,
          selectedApps = emptyList(),
          explicitConsentTimestampMs = 1,
          failClosedDesired = false,
      )

  private fun selectedPolicy(packageId: String) =
      DesiredSystemRoutingPolicy(
          schemaVersion = SystemRoutingPolicyStore.CURRENT_SCHEMA_VERSION,
          revision = 1,
          desiredMode = SystemRoutingMode.SELECTED_APPS,
          selectedApps = listOf(packageId),
          explicitConsentTimestampMs = 1,
          failClosedDesired = false,
      )
}
