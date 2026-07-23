package com.masqmobile

internal object MasqCoreJni {
  val isAvailable: Boolean =
      try {
        System.loadLibrary("masq_mobile_core")
        true
      } catch (_: UnsatisfiedLinkError) {
        false
      } catch (_: SecurityException) {
        false
      }

  external fun nativeGetStatus(): String

  external fun nativeConfigure(configJson: String): String

  external fun nativeImportWallet(privateKey: String): String

  external fun nativeUpdateMinHops(minHops: Int): String

  external fun nativeStart(): String

  external fun nativeStop(): String

  external fun nativeShutdown(): String

  external fun nativeReset(): String

  external fun nativeResetNetworkProfile(): String

  external fun nativeRemoveWallet(): String

  external fun nativePreflightProxy(): String

  external fun nativeSetProxyEnabled(enabled: Boolean): String
}
