package com.masqmobile

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class MasqControlPlanePackagesTest {
  @Test
  fun excludesBothPublicAndDogfoodMasqPackagesFromCapturedTraffic() {
    assertTrue(isMasqControlPlanePackage(PUBLIC_MASQ_PACKAGE_ID))
    assertTrue(isMasqControlPlanePackage(DOGFOOD_MASQ_PACKAGE_ID))
    assertFalse(isMasqControlPlanePackage("org.example.browser"))
  }

  @Test
  fun excludesInstalledSiblingButTreatsAnAbsentSiblingAsNonFatal() {
    assertTrue(
        installedMasqControlPlanePackages { true }
            .containsAll(
                listOf(PUBLIC_MASQ_PACKAGE_ID, DOGFOOD_MASQ_PACKAGE_ID),
            ))
    assertTrue(
        installedMasqControlPlanePackages {
              it == DOGFOOD_MASQ_PACKAGE_ID
            }
            .let { installed ->
              installed == listOf(DOGFOOD_MASQ_PACKAGE_ID)
            })
  }
}
