package com.masqmobile

import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Intent
import android.net.VpnService
import android.os.ParcelFileDescriptor
import androidx.core.app.NotificationCompat
import java.util.concurrent.Executors
import org.json.JSONArray
import org.json.JSONObject

class MasqVpnService : VpnService() {
  private val tunnelExecutor = Executors.newSingleThreadExecutor()
  @Volatile private var tunnelDescriptor: ParcelFileDescriptor? = null
  @Volatile private var stopping = false

  override fun onCreate() {
    super.onCreate()
    val manager = getSystemService(NotificationManager::class.java)
    manager.createNotificationChannel(
        NotificationChannel(
            NOTIFICATION_CHANNEL,
            "MASQ private routing",
            NotificationManager.IMPORTANCE_LOW,
        ))
  }

  override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
    startForeground(NOTIFICATION_ID, notification("MASQ traffic protection is starting…"))
    if (intent?.action == ACTION_STOP) {
      val requestId = intent.getLongExtra(EXTRA_STOP_REQUEST_ID, NO_STOP_REQUEST)
      markStopping()
      stopTunnel(requestId.takeIf { it != NO_STOP_REQUEST })
      return START_NOT_STICKY
    }
    if (intent?.action != ACTION_START) {
      updateStatus(
          "blocked",
          tunnelDescriptor != null,
          currentMode,
          currentApps,
          "MASQ must be reopened to restore the system tunnel.",
      )
      updateNotification("MASQ traffic is blocked until the app reconnects.")
      return START_NOT_STICKY
    }

    val mode = intent.getStringExtra(EXTRA_MODE) ?: "off"
    val proxyPort = intent.getIntExtra(EXTRA_PROXY_PORT, 0)
    val selectedApps = parseApps(intent.getStringExtra(EXTRA_APPS) ?: "[]")
    if (!MasqPacketTunnelJni.isAvailable || proxyPort !in 1..65535 ||
        (mode != "wholeDevice" && mode != "selectedApps") ||
        (mode == "selectedApps" && selectedApps.isEmpty())) {
      updateStatus(
          "blocked",
          tunnelDescriptor != null,
          mode,
          selectedApps,
          "The MASQ packet tunnel is unavailable in this build.",
      )
      updateNotification("MASQ could not start device traffic protection.")
      return START_NOT_STICKY
    }
    if (tunnelDescriptor != null) {
      updateStatus("blocked", false, mode, selectedApps, "Turn off the current system tunnel before changing its scope.")
      return START_NOT_STICKY
    }

    try {
      val builder =
          Builder()
              .setSession("MASQ private route")
              .setMtu(TUNNEL_MTU)
              .addAddress("10.111.0.1", 32)
              .addAddress("fd00:111::1", 128)
              .addRoute("0.0.0.0", 0)
              .addRoute("::", 0)
              .addDnsServer("10.111.0.2")
              .setBlocking(false)
      if (mode == "wholeDevice") {
        // The embedded Node and TUN translator share this UID. Excluding it prevents a routing
        // loop; all other apps remain captured by the VPN interface.
        builder.addDisallowedApplication(packageName)
      } else {
        selectedApps.forEach(builder::addAllowedApplication)
      }
      val configureIntent = Intent(this, MainActivity::class.java)
      builder.setConfigureIntent(
          PendingIntent.getActivity(
              this,
              0,
              configureIntent,
              PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
          ))
      val descriptor = builder.establish()
          ?: throw IllegalStateException("Android did not create the VPN interface.")
      tunnelDescriptor = descriptor
      stopping = false
      updateStatus("active", true, mode, selectedApps, null)
      updateNotification(
          if (mode == "wholeDevice") "Device traffic is protected by MASQ."
          else "${selectedApps.size} selected app${if (selectedApps.size == 1) " is" else "s are"} protected by MASQ.")

      tunnelExecutor.execute {
        val result = MasqPacketTunnelJni.nativeStart(descriptor.fd, proxyPort, TUNNEL_MTU)
        if (!stopping && result != 0) {
          // Keep the VPN descriptor open: packets remain blocked instead of falling back direct.
          updateStatus(
              "blocked",
              true,
              mode,
              selectedApps,
              "The MASQ packet translator stopped. Traffic remains blocked.",
          )
          updateNotification("MASQ traffic is blocked. Reopen the app to reconnect.")
        }
      }
    } catch (error: Exception) {
      updateStatus(
          "blocked",
          tunnelDescriptor != null,
          mode,
          selectedApps,
          error.message ?: "The Android VPN could not start.",
      )
      updateNotification("MASQ could not start device traffic protection.")
    }
    return START_NOT_STICKY
  }

  override fun onDestroy() {
    stopTunnel(stopService = false)
    tunnelExecutor.shutdownNow()
    super.onDestroy()
  }

  private fun stopTunnel(
      requestId: Long? = null,
      stopService: Boolean = true,
  ) {
    stopping = true
    val nativeStopError =
        runCatching {
          if (MasqPacketTunnelJni.isAvailable) {
            MasqPacketTunnelJni.nativeStop()
          }
          Unit
        }.exceptionOrNull()
    if (nativeStopError != null) {
      val message = "The MASQ packet translator could not be stopped."
      markStopFailed(message)
      runCatching {
        updateNotification("MASQ traffic remains blocked because shutdown failed.")
      }
      requestId?.let { acknowledgeStop(it, null, message) }
      return
    }

    val descriptor = tunnelDescriptor
    val descriptorCloseError = runCatching { descriptor?.close() }.exceptionOrNull()
    if (descriptorCloseError != null) {
      val message = "The Android VPN interface could not be closed."
      markStopFailed(message)
      runCatching {
        updateNotification("MASQ traffic remains blocked because shutdown failed.")
      }
      requestId?.let { acknowledgeStop(it, null, message) }
      return
    }

    tunnelDescriptor = null
    updateStatus("off", false, "off", emptyList(), null)
    val status = statusJson()
    requestId?.let { acknowledgeStop(it, status, null) }
    if (stopService) {
      stopForeground(STOP_FOREGROUND_REMOVE)
      stopSelf()
    }
  }

  private fun notification(message: String) =
      NotificationCompat.Builder(this, NOTIFICATION_CHANNEL)
          .setSmallIcon(R.mipmap.ic_launcher)
          .setContentTitle("MASQ private routing")
          .setContentText(message)
          .setContentIntent(
              PendingIntent.getActivity(
                  this,
                  0,
                  Intent(this, MainActivity::class.java),
                  PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
              ))
          .setOngoing(true)
          .setOnlyAlertOnce(true)
          .build()

  private fun updateNotification(message: String) {
    getSystemService(NotificationManager::class.java)
        .notify(NOTIFICATION_ID, notification(message))
  }

  companion object {
    const val ACTION_START = "com.masqmobile.START_SYSTEM_TUNNEL"
    const val ACTION_STOP = "com.masqmobile.STOP_SYSTEM_TUNNEL"
    const val EXTRA_MODE = "mode"
    const val EXTRA_PROXY_PORT = "proxyPort"
    const val EXTRA_APPS = "apps"
    const val EXTRA_STOP_REQUEST_ID = "stopRequestId"
    private const val NOTIFICATION_CHANNEL = "masq-system-tunnel"
    private const val NOTIFICATION_ID = 4107
    private const val TUNNEL_MTU = 1500
    private const val NO_STOP_REQUEST = -1L
    private val statusLock = Any()
    private val stopAcknowledgementLock = Any()
    private val stopAcknowledgements =
        mutableMapOf<Long, (StopAcknowledgement) -> Unit>()
    private var currentPhase = "off"
    private var currentActive = false
    private var currentMode = "off"
    private var currentApps: List<String> = emptyList()
    private var currentError: String? = null

    fun markStarting(mode: String, apps: List<String>): String {
      updateStatus("starting", false, mode, apps, null)
      return statusJson()
    }

    fun markOff(): String {
      updateStatus("off", false, "off", emptyList(), null)
      return statusJson()
    }

    fun markStopping(): String {
      synchronized(statusLock) {
        currentPhase = "stopping"
        currentActive = true
        currentError = null
      }
      return statusJson()
    }

    fun markStopFailed(message: String): String {
      synchronized(statusLock) {
        currentPhase = "blocked"
        currentActive = true
        currentError = message
      }
      return statusJson()
    }

    fun registerStopAcknowledgement(
        requestId: Long,
        callback: (StopAcknowledgement) -> Unit,
    ) {
      synchronized(stopAcknowledgementLock) {
        check(!stopAcknowledgements.containsKey(requestId)) {
          "A MASQ tunnel stop acknowledgement is already registered."
        }
        stopAcknowledgements[requestId] = callback
      }
    }

    fun cancelStopAcknowledgement(requestId: Long) {
      synchronized(stopAcknowledgementLock) {
        stopAcknowledgements.remove(requestId)
      }
    }

    fun statusJson(): String =
        synchronized(statusLock) {
          JSONObject()
              .put("supported", MasqPacketTunnelJni.isAvailable)
              .put("active", currentActive)
              .put("mode", currentMode)
              .put("phase", currentPhase)
              .put("selectedApps", JSONArray(currentApps))
              .put("lastError", currentError ?: JSONObject.NULL)
              .toString()
        }

    private fun updateStatus(
        phase: String,
        active: Boolean,
        mode: String,
        apps: List<String>,
        error: String?,
    ) {
      synchronized(statusLock) {
        currentPhase = phase
        currentActive = active
        currentMode = mode
        currentApps = apps
        currentError = error
      }
    }

    private fun acknowledgeStop(
        requestId: Long,
        status: String?,
        error: String?,
    ) {
      val callback =
          synchronized(stopAcknowledgementLock) {
            stopAcknowledgements.remove(requestId)
          }
      runCatching {
        callback?.invoke(StopAcknowledgement(status, error))
      }
    }

    private fun parseApps(serialized: String): List<String> =
        runCatching {
          val values = JSONArray(serialized)
          (0 until values.length())
              .mapNotNull { index -> values.optString(index).takeIf(String::isNotBlank) }
              .distinct()
        }.getOrDefault(emptyList())

    data class StopAcknowledgement(
        val status: String?,
        val error: String?,
    )
  }
}
