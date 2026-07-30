package com.masqmobile

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class EntryNodeDiscoverySelectionTest {
  private val keyA = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"
  private val keyB = "AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE"
  private val keyC = "AgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgI"

  @Test
  fun selectsFreshBeforePreferredAndCache() {
    val selected =
        EntryNodeDiscoverySelection.select(
            chain = CHAIN,
            freshDescriptors =
                listOf(
                    descriptor(keyA, "8.8.8.8", "4100"),
                    descriptor(keyB, "9.9.9.9", "4200"),
                ),
            preferredDescriptors = listOf(descriptor(keyC, "1.1.1.1", "4300")),
            cachedDescriptors = listOf(descriptor(keyC, "4.2.2.2", "4400")),
        )

    assertEquals(listOf(keyA, keyB), selected.map(EntryNodeCandidate::publicKey))
  }

  @Test
  fun fillsFreshSelectionFromPreferredBeforeCache() {
    val selected =
        EntryNodeDiscoverySelection.select(
            chain = CHAIN,
            freshDescriptors = listOf(descriptor(keyA, "8.8.8.8", "4100")),
            preferredDescriptors = listOf(descriptor(keyB, "9.9.9.9", "4200")),
            cachedDescriptors = listOf(descriptor(keyC, "1.1.1.1", "4300")),
        )

    assertEquals(listOf(keyA, keyB), selected.map(EntryNodeCandidate::publicKey))
  }

  @Test
  fun requiresUniquePublicKeysAndHosts() {
    val selected =
        EntryNodeDiscoverySelection.select(
            chain = CHAIN,
            freshDescriptors =
                listOf(
                    descriptor(keyA, "8.8.8.8", "4100"),
                    descriptor(keyA, "9.9.9.9", "4200"),
                    descriptor(keyB, "8.8.8.8", "4300"),
                    descriptor(keyC, "1.1.1.1", "4400"),
                ),
            preferredDescriptors = emptyList(),
            cachedDescriptors = emptyList(),
        )

    assertEquals(listOf(keyA, keyC), selected.map(EntryNodeCandidate::publicKey))
    assertEquals(listOf("8.8.8.8", "1.1.1.1"), selected.map(EntryNodeCandidate::host))
  }

  @Test
  fun rejectedHostCollisionDoesNotPoisonItsOtherwiseUnusedPublicKey() {
    val selected =
        EntryNodeDiscoverySelection.select(
            chain = CHAIN,
            freshDescriptors =
                listOf(
                    descriptor(keyA, "8.8.8.8", "4100"),
                    descriptor(keyB, "8.8.8.8", "4200"),
                    descriptor(keyB, "9.9.9.9", "4300"),
                ),
            preferredDescriptors = emptyList(),
            cachedDescriptors = emptyList(),
        )

    assertEquals(listOf(keyA, keyB), selected.map(EntryNodeCandidate::publicKey))
    assertEquals(listOf("8.8.8.8", "9.9.9.9"), selected.map(EntryNodeCandidate::host))
  }

  @Test
  fun preservesOriginalDescriptorAndPassivelyRotatesOnePortPerGeneration() {
    val original = descriptor(keyA, "8.8.8.8", "4100/4200/4300/4400")
    val candidate = EntryNodeDiscoverySelection.parse(original, CHAIN)!!

    assertEquals(original, candidate.originalDescriptor)
    assertEquals(descriptor(keyA, "8.8.8.8", "4100"), candidate.singlePortDescriptor(0))
    assertEquals(descriptor(keyA, "8.8.8.8", "4200"), candidate.singlePortDescriptor(1))
    assertEquals(descriptor(keyA, "8.8.8.8", "4100"), candidate.singlePortDescriptor(4))
  }

  @Test
  fun restoresCachedPortVariantsWhenSavedPreferredNodesWereNarrowed() {
    val selected =
        EntryNodeDiscoverySelection.select(
            chain = CHAIN,
            freshDescriptors = emptyList(),
            preferredDescriptors =
                listOf(
                    descriptor(keyA, "8.8.8.8", "4200"),
                    descriptor(keyB, "9.9.9.9", "5200"),
                ),
            cachedDescriptors =
                listOf(
                    descriptor(keyA, "8.8.8.8", "4100/4200"),
                    descriptor(keyB, "9.9.9.9", "5100/5200"),
                ),
        )

    assertEquals(listOf(4100, 4200), selected[0].ports)
    assertEquals(listOf(5100, 5200), selected[1].ports)
    assertEquals(descriptor(keyA, "8.8.8.8", "4200"), selected[0].singlePortDescriptor(1))
    assertEquals(descriptor(keyA, "8.8.8.8", "4100"), selected[0].singlePortDescriptor(2))
    assertEquals(
        descriptor(keyA, "8.8.8.8", "4100/4200"),
        EntryNodeDiscoveryResult.fromSelection(selected, generation = 1).persistentDescriptors[0],
    )
  }

  @Test
  fun keepsFreshIdentityAndPortFirstWhileMergingMatchingCachedVariants() {
    val selected =
        EntryNodeDiscoverySelection.select(
            chain = CHAIN,
            freshDescriptors =
                listOf(
                    descriptor(keyA, "8.8.8.8", "4300"),
                    descriptor(keyB, "9.9.9.9", "5200"),
                ),
            preferredDescriptors = listOf(descriptor(keyC, "1.1.1.1", "6100")),
            cachedDescriptors =
                listOf(
                    descriptor(keyA, "8.8.8.8", "4100/4200"),
                    descriptor(keyC, "1.1.1.1", "6100/6200"),
                ),
        )

    assertEquals(listOf(keyA, keyB), selected.map(EntryNodeCandidate::publicKey))
    assertEquals(listOf(4300, 4100, 4200), selected[0].ports)
    assertEquals(descriptor(keyA, "8.8.8.8", "4300"), selected[0].singlePortDescriptor(0))
  }

  @Test
  fun separatesSinglePortRuntimeDescriptorsFromFullPersistentDescriptors() {
    val selected =
        listOf(
            EntryNodeDiscoverySelection.parse(
                descriptor(keyA, "8.8.8.8", "4100/4200"),
                CHAIN,
            )!!,
            EntryNodeDiscoverySelection.parse(
                descriptor(keyB, "9.9.9.9", "5100/5200"),
                CHAIN,
            )!!,
        )

    val result = EntryNodeDiscoveryResult.fromSelection(selected, generation = 1)

    assertEquals(
        listOf(
            descriptor(keyA, "8.8.8.8", "4200"),
            descriptor(keyB, "9.9.9.9", "5200"),
        ),
        result.runtimeDescriptors,
    )
    assertEquals(
        listOf(
            descriptor(keyA, "8.8.8.8", "4100/4200"),
            descriptor(keyB, "9.9.9.9", "5100/5200"),
        ),
        result.persistentDescriptors,
    )
  }

  @Test
  fun validatesChainCanonicalKeyAndPortBounds() {
    assertNull(EntryNodeDiscoverySelection.parse(descriptor(keyA, "8.8.8.8", "4100"), "Base"))
    assertNull(
        EntryNodeDiscoverySelection.parse(
            "masq://other-mainnet:$keyA@8.8.8.8:4100",
            CHAIN,
        ))
    assertNull(
        EntryNodeDiscoverySelection.parse(
            descriptor("short-key", "8.8.8.8", "4100"),
            CHAIN,
        ))
    assertNull(
        EntryNodeDiscoverySelection.parse(
            descriptor("BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB", "8.8.8.8", "4100"),
            CHAIN,
        ))
    assertNull(EntryNodeDiscoverySelection.parse(descriptor(keyA, "8.8.8.8", "1024"), CHAIN))
    assertNull(EntryNodeDiscoverySelection.parse(descriptor(keyA, "8.8.8.8", "65536"), CHAIN))
    assertNull(
        EntryNodeDiscoverySelection.parse(
            descriptor(keyA, "8.8.8.8", "4100/4200/4300/4400/4500"),
            CHAIN,
        ))
    assertNull(EntryNodeDiscoverySelection.parse(descriptor(keyA, "8.8.8.8", "04100"), CHAIN))
    assertTrue(EntryNodeDiscoverySelection.isCanonicalChain(CHAIN))
  }

  @Test
  fun acceptsOnlyPublicIpv4Literals() {
    listOf(
            "8.8.8.8",
            "1.1.1.1",
            "45.76.232.183",
            "223.255.255.254",
        )
        .forEach { host -> assertTrue(host, EntryNodeDiscoverySelection.isPublicIpv4Literal(host)) }

    listOf(
            "example.org",
            "::1",
            "0.1.2.3",
            "10.0.0.1",
            "100.64.0.1",
            "127.0.0.1",
            "169.254.1.1",
            "172.16.0.1",
            "192.0.0.9",
            "192.0.2.1",
            "192.88.99.1",
            "192.168.1.1",
            "198.18.0.1",
            "198.51.100.1",
            "203.0.113.1",
            "224.0.0.1",
            "240.0.0.1",
            "255.255.255.255",
            "008.8.8.8",
        )
        .forEach { host -> assertFalse(host, EntryNodeDiscoverySelection.isPublicIpv4Literal(host)) }
  }

  @Test
  fun refreshesDiscoveryForEveryStartThatIsNotAlreadyConnected() {
    listOf("", "unconfigured", "ready", "connecting", "paused", "error", "blocked")
        .forEach { phase -> assertTrue(phase, shouldDiscoverEntryNodesBeforeStart(phase)) }
    assertFalse(shouldDiscoverEntryNodesBeforeStart("connected"))
  }

  private fun descriptor(key: String, host: String, ports: String): String =
      "masq://$CHAIN:$key@$host:$ports"

  private companion object {
    const val CHAIN = "base-mainnet"
  }
}
