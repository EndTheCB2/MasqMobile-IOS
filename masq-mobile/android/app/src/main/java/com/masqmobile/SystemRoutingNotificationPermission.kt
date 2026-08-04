package com.masqmobile

internal const val ANDROID_POST_NOTIFICATIONS_API_LEVEL = 33

internal fun systemRoutingNotificationPermissionDiagnostic(
    sdkInt: Int,
    permissionGranted: Boolean,
    desiredMode: SystemRoutingMode,
): SystemRoutingDiagnostic? =
    if (desiredMode != SystemRoutingMode.OFF &&
        sdkInt >= ANDROID_POST_NOTIFICATIONS_API_LEVEL &&
        !permissionGranted) {
      SystemRoutingDiagnostic.NOTIFICATION_PERMISSION_REQUIRED
    } else {
      null
    }
