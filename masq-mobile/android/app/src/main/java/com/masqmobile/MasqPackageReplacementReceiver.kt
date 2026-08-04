package com.masqmobile

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent

internal fun shouldRestoreMasqSessionAfterPackageReplacement(
    policyLoad: SystemRoutingPolicyLoadResult,
): Boolean = policyLoad is SystemRoutingPolicyLoadResult.Ready

/**
 * Restores the consumer supervisor after replacement only for a durable VPN
 * policy. A private-browser-only session is restarted by the next explicit
 * connect action; eagerly racing discovery during APK replacement can consume
 * stale backoff state before the UI is ready.
 */
class MasqPackageReplacementReceiver : BroadcastReceiver() {
  override fun onReceive(context: Context, intent: Intent?) {
    if (intent?.action != Intent.ACTION_MY_PACKAGE_REPLACED) return
    val applicationContext = context.applicationContext
    val policyLoad =
        runCatching {
              SystemRoutingPolicyStore(
                      SharedPreferencesSystemRoutingPolicyStorage(
                          applicationContext.getSharedPreferences(
                              SystemRoutingPolicyStore.PREFERENCES_NAME,
                              Context.MODE_PRIVATE,
                          )))
                  .loadForServiceStart()
            }
            .getOrNull()
            ?: return
    if (!shouldRestoreMasqSessionAfterPackageReplacement(policyLoad)) return
    MasqSessionService.ensureRunningIfDesired(applicationContext)
  }
}
