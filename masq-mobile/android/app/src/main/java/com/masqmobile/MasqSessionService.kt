package com.masqmobile

import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.content.pm.ServiceInfo
import android.net.ConnectivityManager
import android.net.Network
import android.net.NetworkCapabilities
import android.os.Build
import android.os.Handler
import android.os.IBinder
import android.os.Looper
import android.os.PowerManager
import android.os.SystemClock
import androidx.core.app.NotificationCompat
import androidx.core.app.ServiceCompat
import androidx.core.content.ContextCompat
import java.util.concurrent.Executors
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicLong
import java.util.concurrent.atomic.AtomicReference
import org.json.JSONObject

internal enum class MasqSessionNotificationState {
  CONNECTING,
  CONNECTED,
  ATTENTION,
}

internal data class MasqSessionCoreSnapshot(
    val phase: String,
    val connectedNeighbors: Int,
    val routeStage: Int,
)

internal fun masqSessionCoreSnapshot(statusJson: String): MasqSessionCoreSnapshot? =
    runCatching {
          JSONObject(statusJson).let { status ->
            MasqSessionCoreSnapshot(
                phase = status.optString("phase"),
                connectedNeighbors = status.optInt("connectedNeighbors", 0),
                routeStage = status.optInt("routeStage", 0),
            )
          }
        }
        .getOrNull()

internal fun MasqSessionCoreSnapshot.isHealthyConnectedSession(): Boolean =
    phase == "connected" && connectedNeighbors > 0 && routeStage > 0

internal fun masqSessionNotificationText(state: MasqSessionNotificationState): String =
    when (state) {
      MasqSessionNotificationState.CONNECTING ->
          "Building or restoring a private MASQ connection."
      MasqSessionNotificationState.CONNECTED ->
          "Private MASQ connection remains active while the screen is locked."
      MasqSessionNotificationState.ATTENTION ->
          "Waiting to restore the private MASQ connection."
    }

internal class MasqSessionIntentStore(context: Context) {
  private val applicationContext = context.applicationContext
  private val preferences =
      applicationContext.getSharedPreferences(PREFERENCES_NAME, Context.MODE_PRIVATE)

  fun isDesired(): Boolean = preferences.getBoolean(DESIRED_KEY, false)

  fun setDesired(desired: Boolean): Boolean =
      if (desired) {
        preferences.edit().putBoolean(DESIRED_KEY, true).commit()
      } else {
        preferences.edit().remove(DESIRED_KEY).commit()
      }

  fun clearDesiredFailClosed(): Boolean {
    if (setDesired(false) && !isDesired()) return true
    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.N) {
      if (applicationContext.deleteSharedPreferences(PREFERENCES_NAME)) return true
    }
    return !isDesired()
  }

  private companion object {
    const val PREFERENCES_NAME = "masq-mobile-background-session"
    const val DESIRED_KEY = "consumer-session-desired"
  }
}

/**
 * Keeps the in-process MASQ consumer engine alive during screen lock and app backgrounding.
 *
 * This service never creates a VPN interface and never changes whole-device or per-app routing.
 */
class MasqSessionService : Service() {
  private val mainHandler = Handler(Looper.getMainLooper())
  private val monitorInFlight = AtomicBoolean(false)
  private val recoveryEpoch = AtomicLong(0L)
  private val recoveryExecutor = Executors.newSingleThreadExecutor()
  private lateinit var intentStore: MasqSessionIntentStore
  private lateinit var recovery: MasqBackgroundSessionRecovery
  private lateinit var connectivityManager: ConnectivityManager
  private var generation = NO_GENERATION
  private var latestStartId = 0
  private var foregroundStarted = false
  private var destroyed = false
  private var stoppingExplicitly = false
  private var terminalObservations = 0
  private var recoveryAttempts = 0
  private var moduleStartupDeadlineElapsed = 0L
  private var connectingProgressDeadlineElapsed = 0L
  private var connectingNeighbors = 0
  private var connectingRouteStage = 0
  private var recoveryDelayRunnable: Runnable? = null
  private var recoveryRunningToken = NO_RECOVERY_TOKEN
  private var networkAvailable = false
  private var screenOff = false
  private var cpuRequired = false
  private var wakeLock: PowerManager.WakeLock? = null
  private var networkCallbackRegistered = false
  private var screenReceiverRegistered = false

  private val monitorRunnable =
      object : Runnable {
        override fun run() {
          if (destroyed || !isSessionDesired()) return
          if (!monitorInFlight.compareAndSet(false, true)) {
            mainHandler.postDelayed(this, STATUS_POLL_INTERVAL_MILLIS)
            return
          }
          MasqCoreLifecycle.executor.execute {
            val snapshot =
                if (!MasqCoreJni.isAvailable) {
                  null
                } else {
                  runCatching {
                        masqSessionCoreSnapshot(MasqCoreJni.nativeGetStatus())
                      }
                      .getOrNull()
                }
            mainHandler.post {
              monitorInFlight.set(false)
              if (!destroyed && isSessionDesired()) {
                applyCoreSnapshot(snapshot)
                mainHandler.postDelayed(this, STATUS_POLL_INTERVAL_MILLIS)
              }
            }
          }
        }
      }

  private val renewWakeLockRunnable =
      object : Runnable {
        override fun run() {
          if (destroyed || !shouldHoldWakeLock()) {
            releaseWakeLock()
            return
          }
          acquireTimedWakeLock(forceRenewal = true)
        }
      }

  private val screenReceiver =
      object : BroadcastReceiver() {
        override fun onReceive(context: Context?, intent: Intent?) {
          when (intent?.action) {
            Intent.ACTION_SCREEN_OFF -> screenOff = true
            Intent.ACTION_SCREEN_ON -> screenOff = false
            else -> return
          }
          refreshWakeLock()
        }
      }

  private val networkCallback =
      object : ConnectivityManager.NetworkCallback() {
        override fun onAvailable(network: Network) {
          mainHandler.post { refreshNetworkState() }
        }

        override fun onLost(network: Network) {
          mainHandler.post { refreshNetworkState() }
        }

        override fun onCapabilitiesChanged(
            network: Network,
            networkCapabilities: NetworkCapabilities,
        ) {
          mainHandler.post { refreshNetworkState() }
        }
      }

  override fun onCreate() {
    super.onCreate()
    synchronized(lifecycleAuthorityLock) {
      activeInstance.set(this)
      stoppingExplicitly = false
    }
    intentStore = MasqSessionIntentStore(this)
    recovery = MasqBackgroundSessionRecovery(this, ::isSessionDesired)
    connectivityManager =
        getSystemService(Context.CONNECTIVITY_SERVICE) as ConnectivityManager
    screenOff = !(getSystemService(Context.POWER_SERVICE) as PowerManager).isInteractive
    networkAvailable = currentNetworkAvailable()
    createNotificationChannel()
    registerLifecycleSignals()
  }

  override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
    synchronized(lifecycleAuthorityLock) {
      activeInstance.set(this)
      stoppingExplicitly = false
    }
    latestStartId = startId
    if (!intentStore.isDesired()) {
      desiredGeneration.set(NO_GENERATION)
      showForegroundState(MasqSessionNotificationState.ATTENTION)
      stopIfUndesired(startId)
      return START_NOT_STICKY
    }

    val requestedGeneration =
        intent?.getLongExtra(EXTRA_GENERATION, NO_GENERATION) ?: NO_GENERATION
    when {
      intent?.action == ACTION_KEEP_SESSION && requestedGeneration > NO_GENERATION -> {
        val newestGeneration =
            desiredGeneration.get().takeIf { it > NO_GENERATION }
                ?: requestedGeneration.also(desiredGeneration::set)
        beginModuleOwnedGeneration(newestGeneration)
      }
      intent == null -> {
        val recoveryGeneration =
            desiredGeneration.updateAndGet { current ->
              if (current > NO_GENERATION) current else RECOVERY_GENERATION
            }
        beginRestoredGeneration(recoveryGeneration)
      }
      else -> {
        synchronized(lifecycleAuthorityLock) {
          desiredGeneration.set(NO_GENERATION)
          intentStore.clearDesiredFailClosed()
        }
        showForegroundState(MasqSessionNotificationState.ATTENTION)
        stopIfUndesired(startId)
        return START_NOT_STICKY
      }
    }
    return START_STICKY
  }

  override fun onBind(intent: Intent?): IBinder? = null

  override fun onDestroy() {
    destroyed = true
    synchronized(lifecycleAuthorityLock) {
      activeInstance.compareAndSet(this, null)
    }
    cancelRecovery()
    mainHandler.removeCallbacks(monitorRunnable)
    mainHandler.removeCallbacks(renewWakeLockRunnable)
    releaseWakeLock()
    if (screenReceiverRegistered) {
      runCatching { unregisterReceiver(screenReceiver) }
      screenReceiverRegistered = false
    }
    if (networkCallbackRegistered) {
      runCatching { connectivityManager.unregisterNetworkCallback(networkCallback) }
      networkCallbackRegistered = false
    }
    recoveryExecutor.shutdownNow()
    if (foregroundStarted) {
      stopForeground(STOP_FOREGROUND_REMOVE)
      foregroundStarted = false
    }
    super.onDestroy()
  }

  private fun beginModuleOwnedGeneration(nextGeneration: Long) {
    cancelRecovery()
    generation = nextGeneration
    moduleStartupDeadlineElapsed =
        SystemClock.elapsedRealtime() + MODULE_STARTUP_GRACE_MILLIS
    resetConnectingWatchdog()
    terminalObservations = 0
    recoveryAttempts = 0
    cpuRequired = true
    showForegroundState(MasqSessionNotificationState.CONNECTING)
    refreshWakeLock()
    scheduleMonitor()
  }

  private fun beginRestoredGeneration(nextGeneration: Long) {
    cancelRecovery()
    generation = nextGeneration
    moduleStartupDeadlineElapsed = 0L
    resetConnectingWatchdog()
    terminalObservations = 0
    cpuRequired = false
    showForegroundState(MasqSessionNotificationState.CONNECTING)
    refreshWakeLock()
    scheduleMonitor()
    requestRecovery(delayMillis = 0L)
  }

  private fun adoptGeneration(nextGeneration: Long): Boolean {
    synchronized(lifecycleAuthorityLock) {
      if (destroyed || stoppingExplicitly) return false
      recoveryEpoch.incrementAndGet()
    }
    mainHandler.post {
      if (!destroyed && isSessionDesired() && desiredGeneration.get() == nextGeneration) {
        beginModuleOwnedGeneration(nextGeneration)
      }
    }
    return true
  }

  private fun applyCoreSnapshot(snapshot: MasqSessionCoreSnapshot?) {
    val now = SystemClock.elapsedRealtime()
    when {
      snapshot?.isHealthyConnectedSession() == true -> {
        moduleStartupDeadlineElapsed = 0L
        connectingProgressDeadlineElapsed = 0L
        terminalObservations = 0
        recoveryAttempts = 0
        cpuRequired = true
        updateNotification(MasqSessionNotificationState.CONNECTED)
        refreshWakeLock()
      }
      snapshot?.phase == "connecting" && connectingIsMakingProgress(snapshot, now) -> {
        terminalObservations = 0
        cpuRequired = true
        updateNotification(MasqSessionNotificationState.CONNECTING)
        refreshWakeLock()
      }
      else -> {
        val unhealthyConnected = snapshot?.phase == "connected"
        if (!unhealthyConnected && now < moduleStartupDeadlineElapsed) {
          return
        }
        terminalObservations += 1
        if (terminalObservations >= TERMINAL_OBSERVATIONS_BEFORE_RECOVERY) {
          cpuRequired = false
          updateNotification(MasqSessionNotificationState.ATTENTION)
          refreshWakeLock()
          requestRecovery(nextRecoveryDelayMillis())
        }
      }
    }
  }

  private fun resetConnectingWatchdog() {
    connectingNeighbors = 0
    connectingRouteStage = 0
    connectingProgressDeadlineElapsed =
        SystemClock.elapsedRealtime() + CONNECTING_PROGRESS_TIMEOUT_MILLIS
  }

  private fun connectingIsMakingProgress(
      snapshot: MasqSessionCoreSnapshot,
      now: Long,
  ): Boolean {
    if (
        snapshot.connectedNeighbors > connectingNeighbors ||
            snapshot.routeStage > connectingRouteStage
    ) {
      connectingNeighbors = snapshot.connectedNeighbors
      connectingRouteStage = snapshot.routeStage
      connectingProgressDeadlineElapsed = now + CONNECTING_PROGRESS_TIMEOUT_MILLIS
    }
    return now < connectingProgressDeadlineElapsed
  }

  private fun requestRecovery(delayMillis: Long) {
    if (
        destroyed ||
            !isSessionDesired() ||
            !networkAvailable ||
            SystemClock.elapsedRealtime() < moduleStartupDeadlineElapsed
    ) {
      return
    }
    // A Handler delay cannot wake a sleeping device by itself. Keep this bounded
    // lease through the backoff (at most five minutes) so the scheduled recovery
    // can actually begin while the screen remains locked. Refresh it even when
    // this call deduplicates against an already scheduled or running recovery.
    cpuRequired = true
    refreshWakeLock()
    if (recoveryRunningToken != NO_RECOVERY_TOKEN) return
    if (recoveryDelayRunnable != null) {
      if (delayMillis > 0L) return
      cancelRecovery()
    }

    val token = recoveryEpoch.incrementAndGet()
    val delayedRecovery =
        Runnable {
          recoveryDelayRunnable = null
          if (!isRecoveryCurrent(token) || !networkAvailable) return@Runnable
          recoveryRunningToken = token
          cpuRequired = true
          updateNotification(MasqSessionNotificationState.CONNECTING)
          refreshWakeLock()
          recoveryExecutor.execute {
            val result = recovery.recover { isRecoveryCurrent(token) }
            mainHandler.post {
              if (recoveryRunningToken == token) {
                recoveryRunningToken = NO_RECOVERY_TOKEN
              }
              if (!isRecoveryCurrent(token) || !networkAvailable) return@post
              when (result) {
                MasqBackgroundRecoveryResult.ACTIVE,
                MasqBackgroundRecoveryResult.STARTED -> {
                  recoveryAttempts = 0
                  terminalObservations = 0
                  resetConnectingWatchdog()
                  cpuRequired = true
                  refreshWakeLock()
                }
                MasqBackgroundRecoveryResult.CANCELLED -> {
                  cpuRequired = false
                  refreshWakeLock()
                }
                MasqBackgroundRecoveryResult.FAILED -> {
                  recoveryAttempts =
                      (recoveryAttempts + 1).coerceAtMost(MAX_RECOVERY_ATTEMPTS)
                  cpuRequired = false
                  updateNotification(MasqSessionNotificationState.ATTENTION)
                  refreshWakeLock()
                  requestRecovery(nextRecoveryDelayMillis())
                }
              }
            }
          }
        }
    recoveryDelayRunnable = delayedRecovery
    mainHandler.postDelayed(delayedRecovery, delayMillis)
  }

  private fun cancelRecovery() {
    recoveryEpoch.incrementAndGet()
    recoveryDelayRunnable?.let(mainHandler::removeCallbacks)
    recoveryDelayRunnable = null
    recoveryRunningToken = NO_RECOVERY_TOKEN
  }

  private fun isRecoveryCurrent(token: Long): Boolean =
      !destroyed && token == recoveryEpoch.get() && isSessionDesired()

  private fun nextRecoveryDelayMillis(): Long =
      RECOVERY_BACKOFF_MILLIS[
          recoveryAttempts.coerceIn(0, RECOVERY_BACKOFF_MILLIS.lastIndex)]

  private fun refreshNetworkState() {
    val wasAvailable = networkAvailable
    networkAvailable = currentNetworkAvailable()
    if (!networkAvailable) {
      cancelRecovery()
      cpuRequired = false
      updateNotification(MasqSessionNotificationState.ATTENTION)
    } else if (!wasAvailable && isSessionDesired()) {
      cancelRecovery()
      requestRecovery(delayMillis = 0L)
    }
    refreshWakeLock()
  }

  private fun currentNetworkAvailable(): Boolean {
    val activeNetwork = connectivityManager.activeNetwork ?: return false
    val capabilities =
        connectivityManager.getNetworkCapabilities(activeNetwork) ?: return false
    return capabilities.hasCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET) &&
        capabilities.hasCapability(NetworkCapabilities.NET_CAPABILITY_VALIDATED)
  }

  private fun scheduleMonitor() {
    mainHandler.removeCallbacks(monitorRunnable)
    mainHandler.post(monitorRunnable)
  }

  private fun showForegroundState(state: MasqSessionNotificationState) {
    if (foregroundStarted) {
      updateNotification(state)
      return
    }
    val serviceType =
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
          ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE
        } else {
          0
        }
    ServiceCompat.startForeground(
        this,
        NOTIFICATION_ID,
        notification(state),
        serviceType,
    )
    foregroundStarted = true
  }

  private fun updateNotification(state: MasqSessionNotificationState) {
    if (!foregroundStarted || destroyed) return
    getSystemService(NotificationManager::class.java)
        .notify(NOTIFICATION_ID, notification(state))
  }

  private fun notification(state: MasqSessionNotificationState) =
      NotificationCompat.Builder(this, NOTIFICATION_CHANNEL)
          .setSmallIcon(R.mipmap.ic_launcher)
          .setContentTitle("MASQ private connection")
          .setContentText(masqSessionNotificationText(state))
          .setStyle(
              NotificationCompat.BigTextStyle()
                  .bigText(masqSessionNotificationText(state)))
          .setContentIntent(
              PendingIntent.getActivity(
                  this,
                  NOTIFICATION_REQUEST_CODE,
                  Intent(this, MainActivity::class.java),
                  PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
              ))
          .setCategory(NotificationCompat.CATEGORY_SERVICE)
          .setOngoing(true)
          .setOnlyAlertOnce(true)
          .setShowWhen(false)
          .build()

  private fun createNotificationChannel() {
    if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) return
    getSystemService(NotificationManager::class.java)
        .createNotificationChannel(
            NotificationChannel(
                NOTIFICATION_CHANNEL,
                "MASQ private connection",
                NotificationManager.IMPORTANCE_LOW,
            ).apply {
              description =
                  "Shows when the user-requested MASQ consumer connection is active."
              setShowBadge(false)
            })
  }

  private fun registerLifecycleSignals() {
    runCatching {
          ContextCompat.registerReceiver(
              this,
              screenReceiver,
              IntentFilter().apply {
                addAction(Intent.ACTION_SCREEN_OFF)
                addAction(Intent.ACTION_SCREEN_ON)
              },
              ContextCompat.RECEIVER_NOT_EXPORTED,
          )
        }
        .onSuccess { screenReceiverRegistered = true }
    runCatching { connectivityManager.registerDefaultNetworkCallback(networkCallback) }
        .onSuccess { networkCallbackRegistered = true }
  }

  private fun shouldHoldWakeLock(): Boolean =
      isSessionDesired() && screenOff && networkAvailable && cpuRequired

  private fun refreshWakeLock() {
    if (shouldHoldWakeLock()) {
      acquireTimedWakeLock(forceRenewal = false)
    } else {
      mainHandler.removeCallbacks(renewWakeLockRunnable)
      releaseWakeLock()
    }
  }

  private fun acquireTimedWakeLock(forceRenewal: Boolean) {
    val lock =
        wakeLock
            ?: (getSystemService(Context.POWER_SERVICE) as PowerManager)
                .newWakeLock(
                    PowerManager.PARTIAL_WAKE_LOCK,
                    WAKE_LOCK_TAG,
                )
                .apply {
                  setReferenceCounted(false)
                  wakeLock = this
                }
    if (forceRenewal && lock.isHeld) {
      runCatching { lock.release() }
    }
    val scheduleRenewal = forceRenewal || !lock.isHeld
    if (!lock.isHeld) {
      lock.acquire(WAKE_LOCK_TIMEOUT_MILLIS)
    }
    if (scheduleRenewal) {
      mainHandler.removeCallbacks(renewWakeLockRunnable)
      mainHandler.postDelayed(renewWakeLockRunnable, WAKE_LOCK_RENEWAL_MILLIS)
    }
  }

  private fun releaseWakeLock() {
    wakeLock?.takeIf { it.isHeld }?.let { lock ->
      runCatching { lock.release() }
    }
    wakeLock = null
  }

  private fun isSessionDesired(): Boolean =
      desiredGeneration.get() > NO_GENERATION && intentStore.isDesired()

  private fun requestExplicitStop() {
    mainHandler.post {
      if (!destroyed && !isSessionDesired()) {
        stopIfUndesired(latestStartId)
      }
    }
  }

  private fun stopIfUndesired(startId: Int) {
    synchronized(lifecycleAuthorityLock) {
      if (desiredGeneration.get() > NO_GENERATION) return
      stoppingExplicitly = true
      activeInstance.compareAndSet(this, null)
    }
    cancelRecovery()
    generation = NO_GENERATION
    moduleStartupDeadlineElapsed = 0L
    cpuRequired = false
    mainHandler.removeCallbacks(monitorRunnable)
    mainHandler.removeCallbacks(renewWakeLockRunnable)
    releaseWakeLock()
    if (foregroundStarted) {
      stopForeground(STOP_FOREGROUND_REMOVE)
      foregroundStarted = false
    }
    if (startId > 0) {
      stopSelfResult(startId)
    } else {
      stopSelf()
    }
  }

  companion object {
    private const val ACTION_KEEP_SESSION =
        "com.masqmobile.action.KEEP_CONSUMER_SESSION"
    private const val EXTRA_GENERATION = "masq_session_generation"
    private const val NOTIFICATION_CHANNEL = "masq-private-connection"
    private const val NOTIFICATION_ID = 901
    private const val NOTIFICATION_REQUEST_CODE = 901
    private const val NO_GENERATION = 0L
    private const val NO_RECOVERY_TOKEN = 0L
    private const val RECOVERY_GENERATION = 1L
    private const val STATUS_POLL_INTERVAL_MILLIS = 5_000L
    private const val MODULE_STARTUP_GRACE_MILLIS = 90_000L
    private const val CONNECTING_PROGRESS_TIMEOUT_MILLIS = 90_000L
    private const val TERMINAL_OBSERVATIONS_BEFORE_RECOVERY = 3
    private const val MAX_RECOVERY_ATTEMPTS = 3
    private const val WAKE_LOCK_TIMEOUT_MILLIS = 10 * 60_000L
    private const val WAKE_LOCK_RENEWAL_MILLIS = 9 * 60_000L
    private const val WAKE_LOCK_TAG = "com.masqmobile:consumer-session"
    private val RECOVERY_BACKOFF_MILLIS =
        longArrayOf(5_000L, 15_000L, 60_000L, 5 * 60_000L)
    private val desiredGeneration = AtomicLong(NO_GENERATION)
    private val activeInstance = AtomicReference<MasqSessionService?>(null)
    private val lifecycleAuthorityLock = Any()

    fun start(context: Context): Long {
      val intentStore = MasqSessionIntentStore(context)
      val generation: Long
      var adoptedByActiveService = false
      synchronized(lifecycleAuthorityLock) {
        if (!intentStore.setDesired(true)) {
          throw IllegalStateException(
              "Android could not persist the requested MASQ background session.",
          )
        }
        activeInstance.get()?.recoveryEpoch?.incrementAndGet()
        generation =
            synchronized(MasqCoreLifecycle.lock) {
              MasqCoreLifecycle.startGeneration.incrementAndGet()
            }
        desiredGeneration.set(generation)
        if (activeInstance.get()?.adoptGeneration(generation) == true) {
          adoptedByActiveService = true
        }
      }
      if (adoptedByActiveService) return generation
      val intent =
          Intent(context, MasqSessionService::class.java)
              .setAction(ACTION_KEEP_SESSION)
              .putExtra(EXTRA_GENERATION, generation)
      try {
        ContextCompat.startForegroundService(context, intent)
      } catch (error: RuntimeException) {
        var cleanupSucceeded = true
        synchronized(lifecycleAuthorityLock) {
          if (desiredGeneration.compareAndSet(generation, NO_GENERATION)) {
            cleanupSucceeded = intentStore.clearDesiredFailClosed()
            context.stopService(Intent(context, MasqSessionService::class.java))
          }
        }
        if (!cleanupSucceeded) {
          throw IllegalStateException(
              "Android could not clear the failed MASQ background-session request.",
              error,
          )
        }
        throw error
      }
      return generation
    }

    fun stop(context: Context): Boolean {
      val intentStore = MasqSessionIntentStore(context)
      val instance: MasqSessionService?
      val persisted: Boolean
      synchronized(lifecycleAuthorityLock) {
        desiredGeneration.set(NO_GENERATION)
        persisted = intentStore.clearDesiredFailClosed()
        instance = activeInstance.get()
        instance?.let { service ->
          service.stoppingExplicitly = true
          service.recoveryEpoch.incrementAndGet()
          activeInstance.compareAndSet(service, null)
        }
        context.stopService(Intent(context, MasqSessionService::class.java))
      }
      if (instance != null) {
        instance.requestExplicitStop()
      }
      return persisted
    }
  }
}
