package com.masqmobile

internal object MasqPacketTunnelJni {
  val isAvailable: Boolean =
      try {
        System.loadLibrary("masq_packet_tunnel")
        true
      } catch (_: UnsatisfiedLinkError) {
        false
      } catch (_: SecurityException) {
        false
      }

  external fun nativeStart(tunFd: Int, proxyPort: Int, mtu: Int): Int

  external fun nativeStop(): Boolean
}
