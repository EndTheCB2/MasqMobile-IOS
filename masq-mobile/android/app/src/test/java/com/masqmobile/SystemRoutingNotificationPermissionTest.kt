package com.masqmobile

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class SystemRoutingNotificationPermissionTest {
  @Test
  fun android13StickyRecoveryIsRefusedWhenNotificationPermissionWasRevoked() {
    assertEquals(
        SystemRoutingDiagnostic.NOTIFICATION_PERMISSION_REQUIRED,
        systemRoutingNotificationPermissionDiagnostic(
            sdkInt = 33,
            permissionGranted = false,
            desiredMode = SystemRoutingMode.WHOLE_DEVICE,
        ))
  }

  @Test
  fun grantedOrPreAndroid13ActivationRemainsAvailable() {
    assertNull(
        systemRoutingNotificationPermissionDiagnostic(
            sdkInt = 33,
            permissionGranted = true,
            desiredMode = SystemRoutingMode.SELECTED_APPS,
        ))
    assertNull(
        systemRoutingNotificationPermissionDiagnostic(
            sdkInt = 32,
            permissionGranted = false,
            desiredMode = SystemRoutingMode.WHOLE_DEVICE,
        ))
  }

  @Test
  fun offCleanupNeverRequiresNotificationPermission() {
    assertNull(
        systemRoutingNotificationPermissionDiagnostic(
            sdkInt = 33,
            permissionGranted = false,
            desiredMode = SystemRoutingMode.OFF,
        ))
  }
}
