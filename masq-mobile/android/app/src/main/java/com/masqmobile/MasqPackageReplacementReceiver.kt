package com.masqmobile

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent

/** Restores only the durable consumer-session intent after Android replaces this APK. */
class MasqPackageReplacementReceiver : BroadcastReceiver() {
  override fun onReceive(context: Context, intent: Intent?) {
    if (intent?.action != Intent.ACTION_MY_PACKAGE_REPLACED) return
    MasqSessionService.ensureRunningIfDesired(context.applicationContext)
  }
}
