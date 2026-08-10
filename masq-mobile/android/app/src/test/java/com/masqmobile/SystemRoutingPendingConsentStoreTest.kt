package com.masqmobile

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class SystemRoutingPendingConsentStoreTest {
  @Test
  fun approvedConsentResumesTheExactRequestedScopeAfterStoreRecreation() {
    val storage = FakePendingConsentStorage()
    val createdAt = 1_700_000_000_000L
    val requested =
        PendingSystemRoutingConsent(
            consentId = "12345678-1234-1234-1234-123456789abc",
            mode = SystemRoutingMode.SELECTED_APPS,
            selectedApps = listOf("Com.Example.Video", "org.example.browser"),
            expectedRevision = 8L,
            reuseRevision = null,
            createdAtEpochMs = createdAt,
        )

    assertTrue(SystemRoutingPendingConsentStore(storage).persist(requested))

    // A new store instance models a recreated React/native module or app process receiving the
    // Android VPN activity result. It can recover every field needed to continue the exact scope.
    val recreatedStore = SystemRoutingPendingConsentStore(storage)
    val resumed = recreatedStore.load(createdAt + 1_000L)
    assertEquals(requested, resumed)
    assertEquals(SystemRoutingMode.SELECTED_APPS, resumed?.mode)
    assertEquals(
        listOf("Com.Example.Video", "org.example.browser"),
        resumed?.selectedApps,
    )
    assertEquals(8L, resumed?.expectedRevision)
    assertNull(resumed?.reuseRevision)

    assertTrue(recreatedStore.clear(requested.consentId))
    assertNull(recreatedStore.load(createdAt + 2_000L))
  }

  @Test
  fun pendingStateContainsNoWalletSeedNodeOrCoreProfileDetails() {
    val storage = FakePendingConsentStorage()
    val consent = wholeDeviceConsent(createdAt = 2_000L)

    assertTrue(SystemRoutingPendingConsentStore(storage).persist(consent))

    assertEquals(
        setOf(
            "schemaVersion",
            "consentId",
            "mode",
            "selectedApps",
            "expectedRevision",
            "reuseRevision",
            "createdAtEpochMs",
        ),
        storage.values.keys,
    )
    val serialized = storage.values.toString().lowercase()
    listOf("wallet", "seed", "privatekey", "mnemonic", "rpc", "node", "neighbor", "chain")
        .forEach { forbidden -> assertFalse(serialized.contains(forbidden)) }
  }

  @Test
  fun cancellationClearsTheMatchingRequestWithoutClearingANewerOne() {
    val storage = FakePendingConsentStorage()
    val store = SystemRoutingPendingConsentStore(storage)
    val first = wholeDeviceConsent(createdAt = 3_000L)
    val newer =
        first.copy(
            consentId = "87654321-4321-4321-4321-cba987654321",
            createdAtEpochMs = 4_000L,
        )

    assertTrue(store.persist(first))
    assertTrue(store.clear(first.consentId))
    assertNull(store.load(3_500L))

    assertTrue(store.persist(newer))
    assertFalse(store.clear(first.consentId))
    assertEquals(newer, store.load(4_500L))
    assertTrue(store.clear(newer.consentId))
    assertTrue(storage.values.isEmpty())
  }

  @Test
  fun expiredAndCorruptContinuationsAreClearedInsteadOfResumed() {
    val expiredStorage = FakePendingConsentStorage()
    val expiredStore = SystemRoutingPendingConsentStore(expiredStorage)
    val createdAt = 5_000L
    assertTrue(expiredStore.persist(wholeDeviceConsent(createdAt)))

    assertNull(
        expiredStore.load(
            createdAt + SystemRoutingPendingConsentStore.MAX_AGE_MS + 1,
        ))
    assertTrue(expiredStorage.values.isEmpty())

    val corruptStorage =
        FakePendingConsentStorage(
            initialValues =
                mapOf(
                    "schemaVersion" to 1,
                    "consentId" to "12345678-1234-1234-1234-123456789abc",
                    "mode" to "wholeDevice",
                ))
    assertNull(SystemRoutingPendingConsentStore(corruptStorage).load(createdAt))
    assertTrue(corruptStorage.values.isEmpty())
  }

  private fun wholeDeviceConsent(createdAt: Long) =
      PendingSystemRoutingConsent(
          consentId = "12345678-1234-1234-1234-123456789abc",
          mode = SystemRoutingMode.WHOLE_DEVICE,
          selectedApps = emptyList(),
          expectedRevision = null,
          reuseRevision = null,
          createdAtEpochMs = createdAt,
      )
}

private class FakePendingConsentStorage(
    initialValues: Map<String, Any> = emptyMap(),
) : PendingSystemRoutingConsentStorage {
  var values: Map<String, Any> = initialValues.toMap()
    private set

  override fun readAll(): Map<String, *> = values.toMap()

  override fun replaceAll(values: Map<String, Any>): Boolean {
    this.values = values.toMap()
    return true
  }

  override fun clearAll(): Boolean {
    values = emptyMap()
    return true
  }
}
