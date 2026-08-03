package com.masqmobile

internal fun systemRoutingPackageChangeAffectsPolicy(
    policy: DesiredSystemRoutingPolicy,
    packageId: String,
    changedUid: Int?,
    installedUid: (String) -> Int?,
): Boolean {
  val scopedPackages =
      when (policy.desiredMode) {
        SystemRoutingMode.OFF -> return false
        SystemRoutingMode.WHOLE_DEVICE -> MASQ_CONTROL_PLANE_PACKAGE_IDS
        SystemRoutingMode.SELECTED_APPS -> policy.selectedApps.toSet()
      }
  if (packageId in scopedPackages) return true
  if (changedUid == null || changedUid < 0) return false
  return scopedPackages.any { installedUid(it) == changedUid }
}

internal data class SystemRoutingPackageChangeBatch(
    val epoch: Long,
    val retireMissingSelectedScope: Boolean,
)

/**
 * Coalesces package broadcasts without dropping an event that arrives while a rebuild is running.
 *
 * The first observation tells the caller to dispatch one drain. Later observations advance the
 * epoch and are consumed by that same drain. A permanent removal bit is sticky only until the next
 * batch snapshot, so a replacement burst can retain its blocking TUN while an actual uninstall
 * retires a selected-app UID scope that Android may later reuse.
 */
internal class SystemRoutingPackageChangeDrain {
  private val lock = Any()
  private var epoch = 0L
  private var drainDispatched = false
  private var retireMissingSelectedScope = false

  fun observe(retireMissingSelectedScope: Boolean): Boolean =
      synchronized(lock) {
        check(epoch < Long.MAX_VALUE) {
          "The system-routing package-change epoch is exhausted."
        }
        epoch += 1L
        this.retireMissingSelectedScope =
            this.retireMissingSelectedScope || retireMissingSelectedScope
        if (drainDispatched) {
          false
        } else {
          drainDispatched = true
          true
        }
      }

  fun nextBatch(): SystemRoutingPackageChangeBatch? =
      synchronized(lock) {
        if (!drainDispatched) return@synchronized null
        SystemRoutingPackageChangeBatch(
                epoch = epoch,
                retireMissingSelectedScope = retireMissingSelectedScope,
            )
            .also { retireMissingSelectedScope = false }
      }

  /** Returns true when the existing drain must immediately process another batch. */
  fun complete(batch: SystemRoutingPackageChangeBatch): Boolean =
      synchronized(lock) {
        if (!drainDispatched) return@synchronized false
        if (epoch != batch.epoch) {
          true
        } else {
          drainDispatched = false
          false
        }
      }

  fun cancel() {
    synchronized(lock) {
      drainDispatched = false
      retireMissingSelectedScope = false
    }
  }
}
