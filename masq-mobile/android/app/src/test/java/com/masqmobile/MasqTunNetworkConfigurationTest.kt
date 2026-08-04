package com.masqmobile

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class MasqTunNetworkConfigurationTest {
  @Test
  fun `tun captures both default routes so unsupported ipv6 cannot bypass protection`() {
    assertEquals(
        listOf(
            MasqTunPrefix("10.111.0.1", 32),
            MasqTunPrefix("fd00:111::1", 128),
        ),
        MasqTunNetworkConfiguration.addresses,
    )
    assertEquals(
        listOf(
            MasqTunPrefix("0.0.0.0", 0),
            MasqTunPrefix("::", 0),
        ),
        MasqTunNetworkConfiguration.routes,
    )
    assertTrue(MasqTunNetworkConfiguration.routes.contains(MasqTunPrefix("0.0.0.0", 0)))
    assertTrue(MasqTunNetworkConfiguration.routes.contains(MasqTunPrefix("::", 0)))
  }
}
