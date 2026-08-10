package com.masqmobile

import java.util.concurrent.CountDownLatch
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class SystemRoutingPolicyStoreTest {
  @Test
  fun persistsExactCaseCanonicalSelectionBeforeAuthorizingStart() {
    val storage = FakePolicyStorage()
    val store = SystemRoutingPolicyStore(storage)

    val result =
        store.persistBeforeStart(
            expectedRevision = null,
            desiredMode = SystemRoutingMode.SELECTED_APPS,
            packageIds =
                listOf(
                    "org.example.browser",
                    "Com.Example.Video",
                    "Com.Example.Video",
                ),
            explicitConsentTimestampMs = 1234L,
            failClosedDesired = true,
        )

    assertTrue(result is SystemRoutingPolicyWriteResult.Stored)
    val policy = (result as SystemRoutingPolicyWriteResult.Stored).policy
    assertTrue(result.mayStart)
    assertEquals(1L, policy.revision)
    assertEquals(
        listOf("Com.Example.Video", "org.example.browser"),
        policy.selectedApps,
    )
    assertEquals(
        SystemRoutingPolicyLoadResult.Ready(policy),
        store.loadForServiceStart(),
    )
  }

  @Test
  fun nonCanonicalWhitespaceIsRejectedWithoutChangingAndroidIdentity() {
    val storage = FakePolicyStorage()

    val result =
        SystemRoutingPolicyStore(storage)
            .persistBeforeStart(
                expectedRevision = null,
                desiredMode = SystemRoutingMode.SELECTED_APPS,
                packageIds = listOf(" Com.Example.Video"),
                explicitConsentTimestampMs = 1234L,
                failClosedDesired = true,
            )

    assertEquals(
        SystemRoutingDiagnostic.INVALID_PACKAGE_ID,
        (result as SystemRoutingPolicyWriteResult.Rejected).reason,
    )
    assertFalse(result.mayStart)
    assertEquals(0, storage.replaceCalls)
  }

  @Test
  fun corruptAndUnsupportedPreferencesRequireBlockingRecovery() {
    val partial =
        FakePolicyStorage(
            initialValues =
                mapOf(
                    "schemaVersion" to SystemRoutingPolicyStore.CURRENT_SCHEMA_VERSION,
                    "revision" to 7L,
                    "desiredMode" to "selectedApps",
                ))
    val unsupported =
        FakePolicyStorage(
            initialValues =
                mapOf<String, Any>(
                    "schemaVersion" to 99,
                    "revision" to 8L,
                ))

    val partialResult = SystemRoutingPolicyStore(partial).loadForServiceStart()
    val unsupportedResult =
        SystemRoutingPolicyStore(unsupported).loadForServiceStart()

    assertEquals(
        SystemRoutingDiagnostic.CORRUPT_OR_PARTIAL_POLICY,
        (partialResult as SystemRoutingPolicyLoadResult.BlockRequired).reason,
    )
    assertTrue(partialResult.blockRequired)
    assertFalse(partialResult.mayStart)
    assertEquals(
        SystemRoutingDiagnostic.UNSUPPORTED_POLICY_SCHEMA,
        (unsupportedResult as SystemRoutingPolicyLoadResult.BlockRequired).reason,
    )
    assertTrue(unsupportedResult.blockRequired)
  }

  @Test
  fun corruptAndFuturePoliciesRecoverOnlyThroughOneExplicitClear() {
    val storages =
        listOf(
            FakePolicyStorage(
                initialValues =
                    mapOf(
                        "schemaVersion" to SystemRoutingPolicyStore.CURRENT_SCHEMA_VERSION,
                        "revision" to 7L,
                        "desiredMode" to "wholeDevice",
                    )),
            FakePolicyStorage(
                initialValues =
                    mapOf<String, Any>(
                        "schemaVersion" to SystemRoutingPolicyStore.CURRENT_SCHEMA_VERSION + 1,
                        "revision" to 8L,
                    )),
        )

    storages.forEach { storage ->
      val store = SystemRoutingPolicyStore(storage)
      assertTrue(store.loadForServiceStart() is SystemRoutingPolicyLoadResult.BlockRequired)

      assertEquals(SystemRoutingPolicyClearResult.Cleared, store.clearAfterExplicitReset())

      assertEquals(1, storage.clearCalls)
      assertEquals(0, storage.replaceCalls)
      assertEquals(SystemRoutingPolicyLoadResult.Missing, store.loadForServiceStart())
    }
  }

  @Test
  fun emptySelectedListIsRejectedBeforeReadingOrWritingStorage() {
    val storage = FakePolicyStorage()

    val result =
        SystemRoutingPolicyStore(storage)
            .persistBeforeStart(
                expectedRevision = null,
                desiredMode = SystemRoutingMode.SELECTED_APPS,
                packageIds = emptyList(),
                explicitConsentTimestampMs = 1234L,
                failClosedDesired = true,
            )

    assertEquals(
        SystemRoutingDiagnostic.EMPTY_SELECTED_APPS,
        (result as SystemRoutingPolicyWriteResult.Rejected).reason,
    )
    assertEquals(0, storage.readCalls)
    assertEquals(0, storage.replaceCalls)
  }

  @Test
  fun indeterminateCommitReadbackNeverAuthorizesStartEvenWhenNewValuesAppear() {
    val storage =
        FakePolicyStorage(replaceBehavior = StorageBehavior.APPLY_THEN_FALSE)

    val result =
        SystemRoutingPolicyStore(storage)
            .persistBeforeStart(
                expectedRevision = null,
                desiredMode = SystemRoutingMode.WHOLE_DEVICE,
                packageIds = emptyList(),
                explicitConsentTimestampMs = 1234L,
                failClosedDesired = true,
            )

    assertEquals(
        SystemRoutingPolicyWriteResult.IndeterminateCommit(observedRevision = 1L),
        result,
    )
    assertFalse(result.mayStart)
    assertFalse(storage.values.isEmpty())
    assertTrue(
        SystemRoutingPolicyStore(storage).loadForServiceStart()
            is SystemRoutingPolicyLoadResult.Ready)
  }

  @Test
  fun storageExceptionAfterMutationIsAlsoIndeterminateAndCannotStart() {
    val storage =
        FakePolicyStorage(replaceBehavior = StorageBehavior.APPLY_THEN_THROW)

    val result =
        SystemRoutingPolicyStore(storage)
            .persistBeforeStart(
                expectedRevision = null,
                desiredMode = SystemRoutingMode.WHOLE_DEVICE,
                packageIds = emptyList(),
                explicitConsentTimestampMs = 1234L,
                failClosedDesired = false,
            )

    assertEquals(
        SystemRoutingPolicyWriteResult.IndeterminateCommit(observedRevision = 1L),
        result,
    )
    assertFalse(result.mayStart)
  }

  @Test
  fun expectedRevisionPreventsStaleStartAndStopMutations() {
    val storage = FakePolicyStorage()
    val store = SystemRoutingPolicyStore(storage)
    val first =
        store.persistBeforeStart(
            expectedRevision = null,
            desiredMode = SystemRoutingMode.WHOLE_DEVICE,
            packageIds = emptyList(),
            explicitConsentTimestampMs = 1234L,
            failClosedDesired = true,
        ) as SystemRoutingPolicyWriteResult.Stored

    val staleStart =
        store.persistBeforeStart(
            expectedRevision = null,
            desiredMode = SystemRoutingMode.SELECTED_APPS,
            packageIds = listOf("com.example.browser"),
            explicitConsentTimestampMs = 1235L,
            failClosedDesired = true,
        )
    assertEquals(SystemRoutingPolicyWriteResult.Conflict(1L), staleStart)

    val second =
        store.persistBeforeStart(
            expectedRevision = first.policy.revision,
            desiredMode = SystemRoutingMode.SELECTED_APPS,
            packageIds = listOf("com.example.browser"),
            explicitConsentTimestampMs = 1235L,
            failClosedDesired = true,
        ) as SystemRoutingPolicyWriteResult.Stored
    assertEquals(2L, second.policy.revision)
    assertEquals(
        SystemRoutingPolicyWriteResult.Conflict(2L),
        store.persistOff(expectedRevision = first.policy.revision),
    )
    assertEquals(2, storage.replaceCalls)
  }

  @Test
  fun explicitOffSnapshotDiffersFromExplicitFullReset() {
    val storage = FakePolicyStorage()
    val store = SystemRoutingPolicyStore(storage)

    val off = store.persistOff(expectedRevision = null)
    assertTrue(off is SystemRoutingPolicyWriteResult.Stored)
    assertFalse(off.mayStart)
    assertTrue(
        store.loadForServiceStart() is SystemRoutingPolicyLoadResult.ExplicitOff)

    assertEquals(SystemRoutingPolicyClearResult.Cleared, store.clearAfterExplicitReset())
    assertEquals(
        SystemRoutingPolicyLoadResult.Missing,
        store.loadForServiceStart(),
    )
  }

  @Test
  fun indeterminateClearDoesNotReportAConfirmedReset() {
    val storage =
        FakePolicyStorage(clearBehavior = StorageBehavior.APPLY_THEN_FALSE)
    val store = SystemRoutingPolicyStore(storage)
    store.persistOff(expectedRevision = null)

    val result = store.clearAfterExplicitReset()

    assertEquals(
        SystemRoutingPolicyClearResult.IndeterminateClear(
            SystemRoutingDiagnostic.POLICY_CLEAR_INDETERMINATE),
        result,
    )
    assertTrue(storage.values.isEmpty())
  }

  @Test
  fun separateStoreInstancesCannotBothCommitTheSameExpectedRevision() {
    val storage = FakePolicyStorage()
    val firstStore = SystemRoutingPolicyStore(storage)
    val secondStore = SystemRoutingPolicyStore(storage)
    val ready = CountDownLatch(2)
    val start = CountDownLatch(1)
    val executor = Executors.newFixedThreadPool(2)
    val operations =
        listOf(firstStore, secondStore).map { store ->
          executor.submit<SystemRoutingPolicyWriteResult> {
            ready.countDown()
            start.await()
            store.persistBeforeStart(
                expectedRevision = null,
                desiredMode = SystemRoutingMode.WHOLE_DEVICE,
                packageIds = emptyList(),
                explicitConsentTimestampMs = 1234L,
                failClosedDesired = false,
            )
          }
        }
    assertTrue(ready.await(1, TimeUnit.SECONDS))
    start.countDown()

    val results = operations.map { it.get(1, TimeUnit.SECONDS) }
    executor.shutdownNow()

    assertEquals(1, results.count { it is SystemRoutingPolicyWriteResult.Stored })
    assertEquals(
        1,
        results.count {
          it == SystemRoutingPolicyWriteResult.Conflict(actualRevision = 1L)
        },
    )
    assertEquals(1, storage.replaceCalls)
  }
}

private enum class StorageBehavior {
  SUCCEED,
  APPLY_THEN_FALSE,
  APPLY_THEN_THROW,
}

private class FakePolicyStorage(
    initialValues: Map<String, Any> = emptyMap(),
    private val replaceBehavior: StorageBehavior = StorageBehavior.SUCCEED,
    private val clearBehavior: StorageBehavior = StorageBehavior.SUCCEED,
) : SystemRoutingPolicyStorage {
  var values: Map<String, Any> = initialValues.toMap()
    private set

  var readCalls = 0
    private set

  var replaceCalls = 0
    private set

  var clearCalls = 0
    private set

  override fun readAll(): Map<String, *> {
    readCalls += 1
    return values.toMap()
  }

  override fun replaceAll(values: Map<String, Any>): Boolean {
    replaceCalls += 1
    this.values = values.toMap()
    return when (replaceBehavior) {
      StorageBehavior.SUCCEED -> true
      StorageBehavior.APPLY_THEN_FALSE -> false
      StorageBehavior.APPLY_THEN_THROW -> error("injected commit failure")
    }
  }

  override fun clearAll(): Boolean {
    clearCalls += 1
    values = emptyMap()
    return when (clearBehavior) {
      StorageBehavior.SUCCEED -> true
      StorageBehavior.APPLY_THEN_FALSE -> false
      StorageBehavior.APPLY_THEN_THROW -> error("injected clear failure")
    }
  }
}
