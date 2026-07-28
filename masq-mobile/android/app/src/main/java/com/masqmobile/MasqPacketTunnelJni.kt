package com.masqmobile

internal object MasqPacketTunnelJni {
  const val START_STOPPED = 0
  const val START_FAILED = -1
  const val START_UNEXPECTED_CLEAN_RETURN = -2
  const val START_BUSY = -3
  const val START_STALE_COMPLETION = -4

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

  external fun nativeStateJson(): String
}
