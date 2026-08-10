package com.masqmobile

import java.util.ArrayDeque
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class BrowserRoutingQueueTest {
  @Test
  fun ordinaryRequestsRemainFifo() {
    val queue = ArrayDeque<String>()

    assertTrue(enqueueBrowserRoutingRequest(queue, "masq", prioritizeBlocked = false).isEmpty())
    assertTrue(enqueueBrowserRoutingRequest(queue, "direct", prioritizeBlocked = false).isEmpty())

    assertEquals(listOf("masq", "direct"), queue.toList())
  }

  @Test
  fun blockedRequestSupersedesEveryPendingMutationAndBecomesTheOnlyBarrier() {
    val queue = ArrayDeque(listOf("masq", "direct", "masq"))

    val superseded =
        enqueueBrowserRoutingRequest(queue, "blocked", prioritizeBlocked = true)

    assertEquals(listOf("masq", "direct", "masq"), superseded)
    assertEquals(listOf("blocked"), queue.toList())
  }

  @Test
  fun aNewBlockedBarrierCoalescesAnOlderPendingBarrier() {
    val queue = ArrayDeque(listOf("blocked"))

    val superseded =
        enqueueBrowserRoutingRequest(queue, "blocked-new", prioritizeBlocked = true)

    assertEquals(listOf("blocked"), superseded)
    assertEquals(listOf("blocked-new"), queue.toList())
  }

  @Test
  fun lateCallbackCannotReleaseOrOverwriteTheCurrentProxyMutation() {
    val fence = BrowserProxyCallbackFence()
    val firstTicket = requireNotNull(fence.begin())

    assertNull(fence.begin())
    assertEquals(BrowserProxyCallbackCompletion.CURRENT, fence.complete(firstTicket))

    val secondTicket = requireNotNull(fence.begin())
    assertEquals(BrowserProxyCallbackCompletion.STALE, fence.complete(firstTicket))
    assertTrue(fence.hasActiveMutation())
    assertEquals(BrowserProxyCallbackCompletion.CURRENT, fence.complete(secondTicket))
    assertFalse(fence.hasActiveMutation())
  }

  @Test
  fun timeoutRetainsFenceUntilTheMatchingLateCallbackArrives() {
    val fence = BrowserProxyCallbackFence()
    val ticket = requireNotNull(fence.begin())

    assertTrue(fence.markActiveTimedOut())
    assertNull(fence.begin())
    assertEquals(
        BrowserProxyCallbackCompletion.CURRENT_AFTER_TIMEOUT,
        fence.complete(ticket),
    )
    assertFalse(fence.hasActiveMutation())
    assertTrue(fence.begin() != null)
  }
}
