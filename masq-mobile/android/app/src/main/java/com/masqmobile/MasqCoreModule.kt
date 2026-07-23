package com.masqmobile

import android.app.Activity
import android.content.Context
import android.content.Intent
import android.net.ConnectivityManager
import android.net.NetworkCapabilities
import android.net.VpnService
import android.content.pm.PackageManager
import android.webkit.CookieManager
import android.webkit.WebStorage
import androidx.webkit.ProxyConfig
import androidx.webkit.ProxyController
import androidx.webkit.WebViewFeature
import com.facebook.react.bridge.Promise
import com.facebook.react.bridge.ReactApplicationContext
import com.facebook.react.bridge.BaseActivityEventListener
import androidx.core.content.ContextCompat
import java.io.File
import java.util.ArrayDeque
import java.util.Locale
import java.util.concurrent.Executor
import java.util.concurrent.Executors
import java.util.concurrent.ScheduledFuture
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicLong
import java.util.concurrent.atomic.AtomicReference
import org.json.JSONArray
import org.json.JSONObject

class MasqCoreModule(reactContext: ReactApplicationContext) : NativeMasqCoreSpec(reactContext) {
  private val callbackExecutor = Executor { command -> reactContext.runOnUiQueueThread(command) }
  private val ioExecutor = Executors.newSingleThreadExecutor()
  private val stopAcknowledgementExecutor = Executors.newSingleThreadScheduledExecutor()
  private val preferences =
      reactContext.getSharedPreferences("masq-mobile-consumer", Context.MODE_PRIVATE)
  private val walletStore = SecureWalletStore(reactContext)
  private val entryNodeDiscovery = EntryNodeDiscovery(reactContext)
  private val restoreLock = Any()
  private val lifecycleLock = Any()
  private val browserRoutingLock = Any()
  private val browserRoutingQueue = ArrayDeque<BrowserRoutingRequest>()
  private val pendingTunnelStops = mutableMapOf<Long, PendingTunnelStop>()
  @Volatile private var restoreAttempted = false
  private var moduleInvalidated = false
  private var browserRoutingInFlight = false
  private var pendingTunnelRequest: PendingTunnelRequest? = null
  private val tunnelActivityListener =
      object : BaseActivityEventListener() {
        override fun onActivityResult(
            activity: Activity,
            requestCode: Int,
            resultCode: Int,
            data: Intent?,
        ) {
          if (requestCode != VPN_PERMISSION_REQUEST) return
          val request = pendingTunnelRequest ?: return
          pendingTunnelRequest = null
          if (resultCode != Activity.RESULT_OK) {
            request.promise.reject("E_VPN_PERMISSION", "Android VPN permission was not granted.")
            return
          }
          startSystemTunnel(request.mode, request.apps, request.proxyPort, request.promise)
        }
      }

  init {
    reactContext.addActivityEventListener(tunnelActivityListener)
  }

  override fun getStatus(promise: Promise) {
    restoreCoreIfNeeded()
    promise.resolve(statusJson())
  }

  override fun getNetworkStatus(promise: Promise) {
    val manager = reactApplicationContext.getSystemService(Context.CONNECTIVITY_SERVICE)
        as ConnectivityManager
    val capabilities = manager.getNetworkCapabilities(manager.activeNetwork)
    val interfaceName = when {
      capabilities?.hasTransport(NetworkCapabilities.TRANSPORT_WIFI) == true -> "wifi"
      capabilities?.hasTransport(NetworkCapabilities.TRANSPORT_CELLULAR) == true -> "cellular"
      capabilities?.hasTransport(NetworkCapabilities.TRANSPORT_ETHERNET) == true -> "wired"
      capabilities != null -> "other"
      else -> "unknown"
    }
    promise.resolve(
        JSONObject()
            .put("available", capabilities?.hasCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET) == true)
            .put("interface", interfaceName)
            .put("expensive", capabilities?.hasCapability(NetworkCapabilities.NET_CAPABILITY_NOT_METERED) != true)
            .put("constrained", false)
            .put("generation", System.currentTimeMillis() / 2000)
            .toString())
  }

  override fun getNodeFinderUrl(promise: Promise) {
    promise.resolve(BuildConfig.MASQ_NODE_FINDER_URL)
  }

  override fun prepareBrowserProtection(promise: Promise) {
    try {
      promise.resolve(
          browserProtectionJson(
              blockAdsAndTrackers =
                  preferences.getBoolean(BLOCK_ADS_AND_TRACKERS_KEY, true),
              blockCrossSiteCookies =
                  preferences.getBoolean(BLOCK_CROSS_SITE_COOKIES_KEY, true),
              hideCookieBanners =
                  preferences.getBoolean(HIDE_COOKIE_BANNERS_KEY, true),
              rejectOptionalCookies =
                  preferences.getBoolean(REJECT_OPTIONAL_COOKIES_KEY, false),
              youtubeBestEffort =
                  preferences.getBoolean(YOUTUBE_BEST_EFFORT_KEY, false),
          ))
    } catch (error: ClassCastException) {
      promise.reject(
          "E_BROWSER_PROTECTION_STORAGE",
          "The saved Android browser protection preferences are invalid.",
          error,
      )
    }
  }

  override fun setBrowserProtection(configJson: String, promise: Promise) {
    val config =
        try {
          JSONObject(configJson)
        } catch (error: Exception) {
          promise.reject(
              "E_BROWSER_PROTECTION_CONFIG",
              "The Android browser protection preferences are invalid.",
              error,
          )
          return
        }
    val fields = config.keys().asSequence().toSet()
    if (fields != BROWSER_PROTECTION_FIELDS) {
      promise.reject(
          "E_BROWSER_PROTECTION_CONFIG",
          "The Android browser protection preferences must contain exactly the supported fields.",
      )
      return
    }
    if (BROWSER_PROTECTION_FIELDS.any { field -> config.opt(field) !is Boolean }) {
      promise.reject(
          "E_BROWSER_PROTECTION_CONFIG",
          "Every Android browser protection preference must be a boolean.",
      )
      return
    }

    val blockAdsAndTrackers = config.getBoolean("blockAdsAndTrackers")
    val blockCrossSiteCookies = config.getBoolean("blockCrossSiteCookies")
    val hideCookieBanners = config.getBoolean("hideCookieBanners")
    val rejectOptionalCookies = config.getBoolean("rejectOptionalCookies")
    val youtubeBestEffort = config.getBoolean("youtubeBestEffort")
    if (youtubeBestEffort) {
      promise.reject(
          "E_BROWSER_PROTECTION_UNAVAILABLE",
          "YouTube-specific best-effort protection is unavailable on Android.",
      )
      return
    }

    val saved =
        preferences
            .edit()
            .putBoolean(BLOCK_ADS_AND_TRACKERS_KEY, blockAdsAndTrackers)
            .putBoolean(BLOCK_CROSS_SITE_COOKIES_KEY, blockCrossSiteCookies)
            .putBoolean(HIDE_COOKIE_BANNERS_KEY, hideCookieBanners)
            .putBoolean(REJECT_OPTIONAL_COOKIES_KEY, rejectOptionalCookies)
            .putBoolean(YOUTUBE_BEST_EFFORT_KEY, youtubeBestEffort)
            .commit()
    if (!saved) {
      promise.reject(
          "E_BROWSER_PROTECTION_STORAGE",
          "Android could not save the browser protection preferences.",
      )
      return
    }
    promise.resolve(
        browserProtectionJson(
            blockAdsAndTrackers,
            blockCrossSiteCookies,
            hideCookieBanners,
            rejectOptionalCookies,
            youtubeBestEffort,
        ))
  }

  override fun getSavedConfiguration(promise: Promise) {
    val saved = preferences.getString(SAVED_CONFIG_KEY, null)
    if (saved == null) {
      promise.resolve("null")
      return
    }
    val migrated = JSONObject(saved)
        .put("configVersion", 2)
        .apply {
          if (!has("minHops")) put("minHops", 1)
          if (!has("exitCountry")) put("exitCountry", JSONObject.NULL)
          if (!has("exitCountryFallback")) put("exitCountryFallback", true)
        }
        .toString()
    preferences.edit().putString(SAVED_CONFIG_KEY, migrated).apply()
    promise.resolve(migrated)
  }

  override fun configure(configJson: String, promise: Promise) {
    restoreAttempted = true
    ifCoreAvailable(promise) {
      val result = MasqCoreJni.nativeConfigure(prepareNativeConfig(configJson))
      if (statusSucceeded(result)) {
        preferences.edit().putString(SAVED_CONFIG_KEY, migrateConfig(configJson)).apply()
      }
      result
    }
  }

  override fun importWallet(privateKey: String, promise: Promise) {
    if (!MasqCoreJni.isAvailable) {
      promise.reject("E_CORE_UNAVAILABLE", "The native MASQ core is missing from this build.")
      return
    }
    try {
      val result = MasqCoreJni.nativeImportWallet(privateKey)
      if (statusSucceeded(result)) {
        try {
          walletStore.save(privateKey)
        } catch (error: Exception) {
          runCatching { walletStore.delete() }
          MasqCoreJni.nativeRemoveWallet()
          promise.reject(
              "E_KEYSTORE",
              "The consumer wallet could not be saved in secure Android storage.",
              error,
          )
          return
        }
      }
      promise.resolve(result)
    } catch (error: RuntimeException) {
      promise.reject("E_CORE_WALLET", "The MASQ core rejected the wallet.", error)
    }
  }

  override fun updateMinHops(minHops: Double, promise: Promise) {
    if (minHops < 1 || minHops > 6 || minHops % 1.0 != 0.0) {
      promise.reject("E_MIN_HOPS", "Choose between one and six MASQ hops.")
      return
    }
    val saved = preferences.getString(SAVED_CONFIG_KEY, null)
    if (saved == null) {
      promise.reject("E_SAVED_CONFIG", "The saved MASQ network profile is invalid.")
      return
    }
    restoreCoreIfNeeded()
    ifCoreAvailable(promise) {
      val hops = minHops.toInt()
      val result = MasqCoreJni.nativeUpdateMinHops(hops)
      if (statusSucceeded(result)) {
        preferences
            .edit()
            .putString(SAVED_CONFIG_KEY, JSONObject(saved).put("minHops", hops).toString())
            .apply()
      }
      result
    }
  }

  override fun start(promise: Promise) {
    restoreCoreIfNeeded()
    if (!MasqCoreJni.isAvailable) {
      promise.reject("E_CORE_UNAVAILABLE", "The native MASQ core is missing from this build.")
      return
    }
    ioExecutor.execute {
      try {
        val currentStatus = JSONObject(MasqCoreJni.nativeGetStatus())
        if (currentStatus.optString("phase") != "ready") {
          promise.resolve(MasqCoreJni.nativeStart())
          return@execute
        }

        val saved = preferences.getString(SAVED_CONFIG_KEY, null)
            ?: throw IllegalStateException("The saved MASQ network profile is invalid.")
        val config = JSONObject(saved)
        val chain = config.optString("chain")
        if (chain.isBlank()) {
          throw IllegalStateException("The saved MASQ network profile is invalid.")
        }
        val preferredNodes =
            config.optJSONArray("neighbors")?.let { nodes ->
              (0 until nodes.length()).mapNotNull { index ->
                nodes.optString(index).takeIf(String::isNotBlank)
              }
            } ?: emptyList()
        val refreshedNodes = entryNodeDiscovery.discover(chain, preferredNodes)
        config.put("neighbors", JSONArray(refreshedNodes))
        val refreshedConfig = migrateConfig(config.toString())
        val configureResult =
            MasqCoreJni.nativeConfigure(prepareNativeConfig(refreshedConfig))
        if (!statusSucceeded(configureResult)) {
          throw IllegalStateException("The MASQ core rejected the refreshed entry nodes.")
        }
        preferences.edit().putString(SAVED_CONFIG_KEY, refreshedConfig).apply()
        promise.resolve(MasqCoreJni.nativeStart())
      } catch (error: EntryNodeDiscoveryException) {
        promise.reject("E_ENTRY_NODE_DISCOVERY", error.message, error)
      } catch (error: Exception) {
        promise.reject("E_CORE_START", error.message ?: "The MASQ core could not start.", error)
      }
    }
  }

  override fun stop(promise: Promise) {
    if (!MasqCoreJni.isAvailable) {
      promise.resolve(statusJson())
      return
    }
    try {
      promise.resolve(MasqCoreJni.nativeStop())
    } catch (error: RuntimeException) {
      promise.reject("E_CORE_STOP", "The MASQ core could not be stopped.", error)
    }
  }

  override fun shutdown(promise: Promise) {
    if (!MasqCoreJni.isAvailable) {
      promise.resolve(statusJson())
      return
    }
    ioExecutor.execute {
      try {
        promise.resolve(MasqCoreJni.nativeShutdown())
      } catch (error: RuntimeException) {
        promise.reject(
            "E_CORE_SHUTDOWN",
            "The MASQ peer connection could not be shut down.",
            error,
        )
      }
    }
  }

  override fun reset(promise: Promise) {
    try {
      walletStore.delete()
    } catch (error: Exception) {
      promise.reject(
          "E_KEYSTORE_DELETE",
          "The saved consumer wallet could not be removed from secure Android storage.",
          error,
      )
      return
    }
    preferences.edit().remove(SAVED_CONFIG_KEY).commit()
    restoreAttempted = true
    if (!MasqCoreJni.isAvailable) {
      promise.resolve(statusJson())
      return
    }
    try {
      promise.resolve(MasqCoreJni.nativeReset())
    } catch (error: RuntimeException) {
      promise.reject("E_CORE_RESET", "The MASQ core could not be reset.", error)
    }
  }

  override fun resetNetworkProfile(promise: Promise) {
    preferences.edit().remove(SAVED_CONFIG_KEY).apply()
    restoreAttempted = true
    ifCoreAvailable(promise) { MasqCoreJni.nativeResetNetworkProfile() }
  }

  override fun removeWallet(promise: Promise) {
    try {
      walletStore.delete()
    } catch (error: Exception) {
      promise.reject(
          "E_KEYSTORE_DELETE",
          "The saved consumer wallet could not be removed from secure Android storage.",
          error,
      )
      return
    }
    restoreAttempted = true
    ifCoreAvailable(promise) { MasqCoreJni.nativeRemoveWallet() }
  }

  override fun preflightBrowserProxy(promise: Promise) {
    ifCoreAvailable(promise) { MasqCoreJni.nativePreflightProxy() }
  }

  override fun getSystemTunnelStatus(promise: Promise) {
    promise.resolve(MasqVpnService.statusJson())
  }

  @Suppress("DEPRECATION")
  override fun getRoutableApps(promise: Promise) {
    try {
      val launcherIntent =
          Intent(Intent.ACTION_MAIN).apply { addCategory(Intent.CATEGORY_LAUNCHER) }
      val packageManager = reactApplicationContext.packageManager
      val apps =
          packageManager
              .queryIntentActivities(launcherIntent, PackageManager.MATCH_ALL)
              .mapNotNull { info ->
                val packageName = info.activityInfo?.packageName ?: return@mapNotNull null
                if (packageName == reactApplicationContext.packageName) return@mapNotNull null
                JSONObject()
                    .put("id", packageName)
                    .put("label", info.loadLabel(packageManager).toString().ifBlank { packageName })
              }
              .distinctBy { it.getString("id") }
              .sortedBy { it.getString("label").lowercase(Locale.getDefault()) }
      promise.resolve(JSONArray(apps).toString())
    } catch (error: Exception) {
      promise.reject("E_APP_LIST", "Android could not load the routable app list.", error)
    }
  }

  override fun setSystemTunnel(mode: String, appIdsJson: String, promise: Promise) {
    if (mode == "off") {
      stopSystemTunnel(promise)
      return
    }
    if (mode != "wholeDevice" && mode != "selectedApps") {
      promise.reject("E_VPN_MODE", "Choose a valid MASQ traffic scope.")
      return
    }
    if (!MasqPacketTunnelJni.isAvailable) {
      promise.reject("E_VPN_UNAVAILABLE", "The native MASQ packet tunnel is missing from this build.")
      return
    }
    val status = JSONObject(MasqVpnService.statusJson())
    if (status.optString("phase") != "off") {
      promise.reject("E_VPN_ACTIVE", "Turn off the current system tunnel before changing its scope.")
      return
    }
    val apps =
        runCatching {
          val values = JSONArray(appIdsJson)
          (0 until values.length())
              .mapNotNull { index -> values.optString(index).takeIf(String::isNotBlank) }
              .filterNot { it == reactApplicationContext.packageName }
              .distinct()
        }.getOrElse {
          promise.reject("E_VPN_APPS", "The selected Android app list is invalid.")
          return
        }
    if (mode == "selectedApps" && apps.isEmpty()) {
      promise.reject("E_VPN_APPS", "Choose at least one app to protect.")
      return
    }

    restoreCoreIfNeeded()
    val coreStatus =
        runCatching { JSONObject(MasqCoreJni.nativeGetStatus()) }.getOrElse {
          promise.reject("E_CORE_STATUS", "The MASQ core status is unavailable.", it)
          return
        }
    val proxyPort = coreStatus.optInt("proxyPort", 0)
    if (coreStatus.optString("phase") != "connected" || proxyPort !in 1..65535) {
      promise.reject("E_NOT_CONNECTED", "Connect a valid MASQ route before enabling system traffic.")
      return
    }

    val permissionIntent = VpnService.prepare(reactApplicationContext)
    if (permissionIntent == null) {
      startSystemTunnel(mode, apps, proxyPort, promise)
      return
    }
    val activity = reactApplicationContext.getCurrentActivity()
    if (activity == null || pendingTunnelRequest != null) {
      promise.reject("E_VPN_ACTIVITY", "Open MASQ before approving Android VPN access.")
      return
    }
    pendingTunnelRequest = PendingTunnelRequest(mode, apps, proxyPort, promise)
    try {
      activity.startActivityForResult(permissionIntent, VPN_PERMISSION_REQUEST)
    } catch (error: Exception) {
      pendingTunnelRequest = null
      promise.reject("E_VPN_PERMISSION", "Android could not open the VPN permission dialog.", error)
    }
  }

  private fun stopSystemTunnel(promise: Promise) {
    val requestId = STOP_REQUEST_COUNTER.getAndIncrement()
    val operation =
        PendingTunnelStop(
            requestId = requestId,
            promise = promise,
        )
    val timeoutMessage =
        "Android did not confirm that the MASQ system tunnel stopped."
    val timeout =
        Runnable {
          completeTunnelStop(operation) {
            MasqVpnService.markStopFailed(timeoutMessage)
            promise.reject("E_VPN_STOP_TIMEOUT", timeoutMessage)
          }
        }

    synchronized(lifecycleLock) {
      if (moduleInvalidated) {
        promise.reject(
            "E_MODULE_INVALIDATED",
            "The MASQ native module is shutting down.",
        )
        return
      }
      pendingTunnelStops[requestId] = operation

      MasqVpnService.markStopping()
      try {
        MasqVpnService.registerStopAcknowledgement(requestId) { acknowledgement ->
          completeTunnelStop(operation) {
            val error = acknowledgement.error
            val status = acknowledgement.status
            if (error != null || status == null) {
              val message = error ?: "The MASQ system tunnel stop response was invalid."
              MasqVpnService.markStopFailed(message)
              promise.reject("E_VPN_STOP", message)
            } else {
              promise.resolve(status)
            }
          }
        }
      } catch (error: RuntimeException) {
        completeTunnelStop(operation) {
          val message =
              "Android could not register the MASQ tunnel stop acknowledgement."
          MasqVpnService.markStopFailed(message)
          promise.reject("E_VPN_STOP", message, error)
        }
        return
      }

      try {
        operation.timeoutFuture.set(
            stopAcknowledgementExecutor.schedule(
                timeout,
                STOP_TUNNEL_TIMEOUT_MS,
                TimeUnit.MILLISECONDS,
            ),
        )
      } catch (error: RuntimeException) {
        completeTunnelStop(operation) {
          val message = "Android could not monitor the MASQ tunnel shutdown."
          MasqVpnService.markStopFailed(message)
          promise.reject("E_VPN_STOP", message, error)
        }
        return
      }

      val intent =
          Intent(reactApplicationContext, MasqVpnService::class.java)
              .setAction(MasqVpnService.ACTION_STOP)
              .putExtra(MasqVpnService.EXTRA_STOP_REQUEST_ID, requestId)
      try {
        val dispatched = reactApplicationContext.startService(intent)
        if (dispatched == null) {
          throw IllegalStateException("Android did not dispatch the MASQ tunnel stop request.")
        }
      } catch (error: Exception) {
        completeTunnelStop(operation) {
          val message = "Android could not dispatch the MASQ tunnel stop request."
          MasqVpnService.markStopFailed(message)
          promise.reject("E_VPN_STOP_DISPATCH", message, error)
        }
      }
    }
  }

  private fun completeTunnelStop(
      operation: PendingTunnelStop,
      settlement: () -> Unit,
  ): Boolean =
      synchronized(lifecycleLock) {
        if (!operation.completed.compareAndSet(false, true)) {
          return@synchronized false
        }
        pendingTunnelStops.remove(operation.requestId)
        operation.timeoutFuture.getAndSet(null)?.cancel(false)
        MasqVpnService.cancelStopAcknowledgement(operation.requestId)
        settlement()
        true
      }

  override fun invalidate() {
    val abandonedStops =
        synchronized(lifecycleLock) {
          val pending = mutableListOf<PendingTunnelStop>()
          if (!moduleInvalidated) {
            moduleInvalidated = true
            pendingTunnelStops.values.toList().forEach { operation ->
              if (operation.completed.compareAndSet(false, true)) {
                operation.timeoutFuture.getAndSet(null)?.cancel(false)
                MasqVpnService.cancelStopAcknowledgement(operation.requestId)
                pending.add(operation)
              }
            }
            pendingTunnelStops.clear()
          }
          pending
        }
    stopAcknowledgementExecutor.shutdownNow()
    ioExecutor.shutdownNow()
    abandonedStops.forEach { operation ->
      runCatching {
        operation.promise.reject(
            "E_MODULE_INVALIDATED",
            "The MASQ native module shut down before the system tunnel stop was confirmed.",
        )
      }
    }
    super.invalidate()
  }

  override fun setBrowserRoutingMode(mode: String, promise: Promise) {
    if (mode != "blocked" && mode != "masq" && mode != "direct") {
      promise.reject(
          "E_BROWSER_ROUTING_MODE",
          "Choose a valid browser routing mode.",
      )
      return
    }
    if (!WebViewFeature.isFeatureSupported(WebViewFeature.PROXY_OVERRIDE)) {
      promise.reject("E_PROXY_UNSUPPORTED", "This Android WebView does not support proxy override.")
      return
    }

    val request = BrowserRoutingRequest(mode, promise)
    val next =
        synchronized(browserRoutingLock) {
          browserRoutingQueue.addLast(request)
          if (browserRoutingInFlight) {
            null
          } else {
            browserRoutingInFlight = true
            browserRoutingQueue.removeFirst()
          }
        }
    next?.let(::applyBrowserRoutingMode)
  }

  private fun applyBrowserRoutingMode(request: BrowserRoutingRequest) {
    when (request.mode) {
      "blocked" -> applyBlockedBrowserRouting(request)
      "masq" -> applyMasqBrowserRouting(request)
      "direct" -> applyDirectBrowserRouting(request)
      else ->
          finishBrowserRoutingWithError(
              request,
              "E_BROWSER_ROUTING_MODE",
              "Choose a valid browser routing mode.",
          )
    }
  }

  private fun applyBlockedBrowserRouting(request: BrowserRoutingRequest) {
    installBlockedBrowserState(
        onReady = { finishBrowserRouting(request, "blocked") },
        onError = { error ->
          finishBrowserRoutingWithError(
              request,
              "E_PROXY_BLOCK",
              "Browser traffic could not be isolated.",
              error,
          )
        },
    )
  }

  private fun installBlockedBrowserState(
      onReady: () -> Unit,
      onError: (Throwable) -> Unit,
  ) {
    try {
      val config =
          ProxyConfig.Builder()
              .addProxyRule(BLOCKED_BROWSER_PROXY)
              // Intentionally no addDirect(): blocked must never fail open.
              .build()
      ProxyController.getInstance().setProxyOverride(config, callbackExecutor) {
        syncCoreBrowserProxy(false)
        clearBrowserWebsiteData(onReady, onError)
      }
    } catch (error: RuntimeException) {
      onError(error)
    }
  }

  private fun applyMasqBrowserRouting(request: BrowserRoutingRequest) {
    installBlockedBrowserState(
        onReady = { applyMasqBrowserRoutingAfterBlock(request) },
        onError = { error ->
          finishBrowserRoutingWithError(
              request,
              "E_PROXY_BLOCK",
              "Browser traffic could not be isolated before enabling MASQ.",
              error,
          )
        },
    )
  }

  private fun applyMasqBrowserRoutingAfterBlock(request: BrowserRoutingRequest) {
    if (!MasqCoreJni.isAvailable) {
      finishBrowserRoutingWithError(
          request,
          "E_CORE_UNAVAILABLE",
          "The native MASQ core is missing from this build.",
      )
      return
    }

    try {
      val status = JSONObject(MasqCoreJni.nativeGetStatus())
      if (status.optString("phase") != "connected") {
        finishBrowserRoutingWithError(
            request,
            "E_NOT_CONNECTED",
            "Build a MASQ route first.",
        )
        return
      }
      val proxyPort = status.optInt("proxyPort", 0)
      if (proxyPort !in 1..65535) {
        finishBrowserRoutingWithError(
            request,
            "E_PROXY_PORT",
            "The MASQ core returned an invalid proxy port.",
        )
        return
      }

      val config =
          ProxyConfig.Builder()
              .addProxyRule("http://127.0.0.1:$proxyPort")
              // Intentionally no addDirect(): failure must never bypass MASQ.
              .build()
      ProxyController.getInstance().setProxyOverride(config, callbackExecutor) {
        try {
          MasqCoreJni.nativeSetProxyEnabled(true)
          finishBrowserRouting(request, "masq")
        } catch (error: RuntimeException) {
          failBrowserRoutingClosed(
              request,
              "E_PROXY_STATE",
              "The MASQ core could not confirm the proxy.",
              error,
          )
        }
      }
    } catch (error: RuntimeException) {
      finishBrowserRoutingWithError(
          request,
          "E_PROXY_APPLY",
          "The local MASQ proxy could not be configured.",
          error,
      )
    }
  }

  private fun applyDirectBrowserRouting(request: BrowserRoutingRequest) {
    val tunnelStatus =
        runCatching { JSONObject(MasqVpnService.statusJson()) }.getOrElse {
          finishBrowserRoutingWithError(
              request,
              "E_VPN_STATUS",
              "The MASQ system tunnel status is unavailable.",
              it,
          )
          return
        }
    val tunnelPhase = tunnelStatus.optString("phase")
    val tunnelActive = tunnelStatus.optBoolean("active", false)
    if (tunnelPhase != "off" || tunnelActive) {
      finishBrowserRoutingWithError(
          request,
          "E_VPN_ACTIVE",
          "Turn off MASQ system routing before browsing directly.",
      )
      return
    }

    try {
      ProxyController.getInstance().clearProxyOverride(callbackExecutor) {
        try {
          if (MasqCoreJni.isAvailable) {
            MasqCoreJni.nativeSetProxyEnabled(false)
          }
          finishBrowserRouting(request, "direct")
        } catch (error: RuntimeException) {
          failBrowserRoutingClosed(
              request,
              "E_PROXY_STATE",
              "The MASQ core could not confirm direct browser routing.",
              error,
          )
        }
      }
    } catch (error: RuntimeException) {
      failBrowserRoutingClosed(
          request,
          "E_PROXY_CLEAR",
          "Direct browser routing could not be configured.",
          error,
      )
    }
  }

  private fun failBrowserRoutingClosed(
      request: BrowserRoutingRequest,
      code: String,
      message: String,
      cause: Throwable,
  ) {
    installBlockedBrowserState(
        onReady = {
          finishBrowserRoutingWithError(request, code, message, cause)
        },
        onError = {
          finishBrowserRoutingWithError(request, code, message, cause)
        },
    )
  }

  private fun clearBrowserWebsiteData(
      onComplete: () -> Unit,
      onError: (Throwable) -> Unit,
  ) {
    try {
      WebStorage.getInstance().deleteAllData()
      val cookieManager = CookieManager.getInstance()
      cookieManager.removeAllCookies {
        try {
          cookieManager.flush()
        } catch (error: RuntimeException) {
          onError(error)
          return@removeAllCookies
        }
        onComplete()
      }
    } catch (error: RuntimeException) {
      onError(error)
    }
  }

  private fun syncCoreBrowserProxy(enabled: Boolean) {
    if (!MasqCoreJni.isAvailable) return
    try {
      MasqCoreJni.nativeSetProxyEnabled(enabled)
    } catch (_: RuntimeException) {
      // WebView routing remains authoritative and blocked on cleanup paths.
    }
  }

  private fun finishBrowserRouting(request: BrowserRoutingRequest, mode: String) {
    request.promise.resolve(mode)
    startNextBrowserRoutingRequest()
  }

  private fun finishBrowserRoutingWithError(
      request: BrowserRoutingRequest,
      code: String,
      message: String,
      cause: Throwable? = null,
  ) {
    if (cause == null) {
      request.promise.reject(code, message)
    } else {
      request.promise.reject(code, message, cause)
    }
    startNextBrowserRoutingRequest()
  }

  private fun startNextBrowserRoutingRequest() {
    val next =
        synchronized(browserRoutingLock) {
          if (browserRoutingQueue.isEmpty()) {
            browserRoutingInFlight = false
            null
          } else {
            browserRoutingQueue.removeFirst()
          }
        }
    next?.let(::applyBrowserRoutingMode)
  }

  private data class BrowserRoutingRequest(
      val mode: String,
      val promise: Promise,
  )

  private fun startSystemTunnel(
      mode: String,
      apps: List<String>,
      proxyPort: Int,
      promise: Promise,
  ) {
    try {
      val status = MasqVpnService.markStarting(mode, apps)
      val intent =
          Intent(reactApplicationContext, MasqVpnService::class.java)
              .setAction(MasqVpnService.ACTION_START)
              .putExtra(MasqVpnService.EXTRA_MODE, mode)
              .putExtra(MasqVpnService.EXTRA_PROXY_PORT, proxyPort)
              .putExtra(MasqVpnService.EXTRA_APPS, JSONArray(apps).toString())
      ContextCompat.startForegroundService(reactApplicationContext, intent)
      promise.resolve(status)
    } catch (error: Exception) {
      MasqVpnService.markOff()
      promise.reject("E_VPN_START", "Android could not start MASQ system routing.", error)
    }
  }

  private fun ifCoreAvailable(promise: Promise, operation: () -> String) {
    if (!MasqCoreJni.isAvailable) {
      promise.reject("E_CORE_UNAVAILABLE", "The native MASQ core is missing from this build.")
      return
    }
    try {
      promise.resolve(operation())
    } catch (error: RuntimeException) {
      promise.reject("E_CORE", "The MASQ core rejected the request.", error)
    }
  }

  private fun restoreCoreIfNeeded() {
    if (restoreAttempted || !MasqCoreJni.isAvailable) return
    synchronized(restoreLock) {
      if (restoreAttempted) return
      restoreAttempted = true
      runCatching {
        val current = JSONObject(MasqCoreJni.nativeGetStatus())
        val savedConfig = preferences.getString(SAVED_CONFIG_KEY, null)
        if (current.isNull("chain") && savedConfig != null) {
          val result = MasqCoreJni.nativeConfigure(prepareNativeConfig(savedConfig))
          if (!statusSucceeded(result)) return@runCatching
        }
        val savedWallet = walletStore.load()
        if (current.isNull("walletAddress") && savedWallet != null) {
          MasqCoreJni.nativeImportWallet(savedWallet)
        }
      }
    }
  }

  private fun prepareNativeConfig(configJson: String): String {
    val dataDirectory = File(reactApplicationContext.noBackupFilesDir, "masq-node")
    if (!dataDirectory.exists() && !dataDirectory.mkdirs()) {
      throw IllegalStateException("The MASQ data directory could not be created.")
    }
    return JSONObject(configJson)
        .apply {
          remove("configVersion")
          put("dataDirectory", dataDirectory.absolutePath)
        }
        .toString()
  }

  private fun migrateConfig(configJson: String): String =
      JSONObject(configJson)
          .apply {
            put("configVersion", 2)
            if (!has("minHops")) put("minHops", 1)
            if (!has("exitCountry")) put("exitCountry", JSONObject.NULL)
            if (!has("exitCountryFallback")) put("exitCountryFallback", true)
          }
          .toString()

  private fun statusSucceeded(statusJson: String): Boolean =
      JSONObject(statusJson).optString("phase") != "error"

  private fun browserProtectionJson(
      blockAdsAndTrackers: Boolean,
      blockCrossSiteCookies: Boolean,
      hideCookieBanners: Boolean,
      rejectOptionalCookies: Boolean,
      youtubeBestEffort: Boolean,
  ): String =
      JSONObject()
          .put("blockAdsAndTrackers", blockAdsAndTrackers)
          .put("blockCrossSiteCookies", blockCrossSiteCookies)
          .put("hideCookieBanners", hideCookieBanners)
          .put("rejectOptionalCookies", rejectOptionalCookies)
          .put("youtubeBestEffort", youtubeBestEffort)
          // Android currently relies on WebView's generic page injection and cookie policy.
          .put("nativeRequestBlocking", false)
          .put("youtubeBestEffortAvailable", false)
          .toString()

  private fun statusJson(): String {
    if (MasqCoreJni.isAvailable) {
      return try {
        MasqCoreJni.nativeGetStatus()
      } catch (_: RuntimeException) {
        unavailableStatus("The native MASQ core is not responding.")
      }
    }
    return unavailableStatus("The native MASQ core is missing from this build.")
  }

  private fun unavailableStatus(reason: String): String =
      JSONObject()
          .put("phase", "blocked")
          .put("engineAvailable", false)
          .put("proxyEnabled", false)
          .put("proxyPort", JSONObject.NULL)
          .put("chain", JSONObject.NULL)
          .put("walletAddress", JSONObject.NULL)
          .put("connectedNeighbors", 0)
          .put("routeStage", 0)
          .put("routeHops", 0)
          .put("minHops", 1)
          .put("exitCountry", JSONObject.NULL)
            .put("exitCountryFallback", true)
            .put("availableExitCountries", JSONArray())
          .put("bytesUp", 0)
          .put("bytesDown", 0)
          .put("lastError", reason)
          .toString()

  companion object {
    const val NAME = NativeMasqCoreSpec.NAME
    private const val SAVED_CONFIG_KEY = "saved-consumer-config"
    private const val BLOCK_ADS_AND_TRACKERS_KEY =
        "browser-protection.block-ads-and-trackers"
    private const val BLOCK_CROSS_SITE_COOKIES_KEY =
        "browser-protection.block-cross-site-cookies"
    private const val HIDE_COOKIE_BANNERS_KEY =
        "browser-protection.hide-cookie-banners"
    private const val REJECT_OPTIONAL_COOKIES_KEY =
        "browser-protection.reject-optional-cookies"
    private const val YOUTUBE_BEST_EFFORT_KEY =
        "browser-protection.youtube-best-effort"
    private const val BLOCKED_BROWSER_PROXY = "http://127.0.0.1:1"
    private const val STOP_TUNNEL_TIMEOUT_MS = 10_000L
    private val STOP_REQUEST_COUNTER = AtomicLong(1L)
    private val BROWSER_PROTECTION_FIELDS =
        setOf(
            "blockAdsAndTrackers",
            "blockCrossSiteCookies",
            "hideCookieBanners",
            "rejectOptionalCookies",
            "youtubeBestEffort",
        )
    private const val VPN_PERMISSION_REQUEST = 4108
  }

  private data class PendingTunnelRequest(
      val mode: String,
      val apps: List<String>,
      val proxyPort: Int,
      val promise: Promise,
  )

  private data class PendingTunnelStop(
      val requestId: Long,
      val promise: Promise,
      val completed: AtomicBoolean = AtomicBoolean(false),
      val timeoutFuture: AtomicReference<ScheduledFuture<*>?> = AtomicReference(),
  )
}
