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
import androidx.webkit.Profile
import androidx.webkit.ProfileStore
import androidx.webkit.ProxyConfig
import androidx.webkit.ProxyController
import androidx.webkit.WebViewFeature
import com.facebook.react.bridge.Promise
import com.facebook.react.bridge.ReactApplicationContext
import com.facebook.react.bridge.BaseActivityEventListener
import androidx.core.content.ContextCompat
import java.io.File
import java.nio.charset.StandardCharsets
import java.security.MessageDigest
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

internal object MasqCoreLifecycle {
  val lock = Any()
  val startGeneration = AtomicLong(0L)
  val executor = Executors.newSingleThreadExecutor()
}

class MasqCoreModule(reactContext: ReactApplicationContext) : NativeMasqCoreSpec(reactContext) {
  private val callbackExecutor = Executor { command -> reactContext.runOnUiQueueThread(command) }
  private val discoveryExecutor = Executors.newSingleThreadExecutor()
  private val stopAcknowledgementExecutor = Executors.newSingleThreadScheduledExecutor()
  private val preferences =
      reactContext.getSharedPreferences("masq-mobile-consumer", Context.MODE_PRIVATE)
  private val systemRoutingPolicyStore =
      SystemRoutingPolicyStore(
          SharedPreferencesSystemRoutingPolicyStorage(
              reactContext.getSharedPreferences(
                  SystemRoutingPolicyStore.PREFERENCES_NAME,
                  Context.MODE_PRIVATE,
              )))
  private val walletStore = SecureWalletStore(reactContext)
  private val entryNodeDiscovery = EntryNodeDiscovery(reactContext)
  private val restoreLock = Any()
  private val lifecycleLock = Any()
  private val pendingTunnelStarts = mutableMapOf<Long, PendingTunnelStart>()
  private val pendingTunnelStops = mutableMapOf<Long, PendingTunnelStop>()
  private val pendingTunnelResets = mutableMapOf<Long, PendingTunnelReset>()
  @Volatile private var restoreAttempted = false
  @Volatile private var moduleInvalidated = false
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
          val request =
              synchronized(lifecycleLock) {
                if (moduleInvalidated) {
                  null
                } else {
                  pendingTunnelRequest.also { pendingTunnelRequest = null }
                }
              } ?: return
          if (resultCode != Activity.RESULT_OK) {
            request.promise.reject("E_VPN_PERMISSION", "Android VPN permission was not granted.")
            return
          }
          persistAndStartSystemTunnel(request)
        }
      }

  init {
    reactContext.addActivityEventListener(tunnelActivityListener)
  }

  override fun getStatus(promise: Promise) {
    MasqCoreLifecycle.executor.execute {
      try {
        restoreCoreIfNeeded()
        promise.resolve(statusJson())
      } catch (error: SecureWalletStore.UnreadableException) {
        promise.reject(
            "E_WALLET_STORAGE_UNREADABLE",
            "The encrypted consumer wallet could not be read safely. Unlock the device and retry without resetting or re-importing the wallet.",
            error,
        )
      } catch (error: Exception) {
        promise.reject(
            "E_CORE_RESTORE",
            "The saved MASQ state could not be restored safely.",
            error,
        )
      }
    }
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
                  preferences.getBoolean(HIDE_COOKIE_BANNERS_KEY, false),
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

  override fun getBrowserSiteSettings(mode: String, hostname: String, promise: Promise) {
    val site = validateBrowserSite(mode, hostname, promise) ?: return
    try {
      promise.resolve(selectBrowserSite(site.mode, site.hostname))
    } catch (error: RuntimeException) {
      promise.reject(
          "E_BROWSER_PROFILE",
          "Android could not prepare the isolated website profile.",
          error,
      )
    }
  }

  override fun setBrowserSiteSettings(
      mode: String,
      hostname: String,
      rememberSignIn: Boolean,
      protectionDisabled: Boolean,
      promise: Promise,
  ) {
    val site = validateBrowserSite(mode, hostname, promise) ?: return
    val persistentSupported = browserProfilesSupported()
    if (rememberSignIn && !persistentSupported) {
      promise.reject(
          "E_BROWSER_PROFILE_UNSUPPORTED",
          "This Android WebView cannot isolate remembered website sessions.",
      )
      return
    }
    val prefix = browserSitePreferencePrefix(site.mode, site.hostname)
    val saved =
        preferences
            .edit()
            .putBoolean("${prefix}remember-sign-in", rememberSignIn)
            .putBoolean("${prefix}protection-disabled", protectionDisabled)
            .commit()
    if (!saved) {
      promise.reject(
          "E_BROWSER_SITE_STORAGE",
          "Android could not save the website privacy settings.",
      )
      return
    }
    try {
      promise.resolve(selectBrowserSite(site.mode, site.hostname))
    } catch (error: RuntimeException) {
      promise.reject(
          "E_BROWSER_PROFILE",
          "Android could not switch the isolated website profile.",
          error,
      )
    }
  }

  override fun clearBrowserSiteData(mode: String, hostname: String, promise: Promise) {
    val site = validateBrowserSite(mode, hostname, promise) ?: return
    val prefix = browserSitePreferencePrefix(site.mode, site.hostname)
    val wasRemembered =
        browserProfilesSupported() &&
            preferences.getBoolean("${prefix}remember-sign-in", false)
    val profileToDelete =
        if (wasRemembered) {
          persistentBrowserProfileName(site.mode, site.hostname)
        } else {
          temporaryBrowserProfileName(site.mode)
        }
    val saved =
        preferences
            .edit()
            .remove("${prefix}remember-sign-in")
            .remove("${prefix}protection-disabled")
            .commit()
    if (!saved) {
      promise.reject(
          "E_BROWSER_SITE_STORAGE",
          "Android could not remove the website privacy settings.",
      )
      return
    }
    callbackExecutor.execute {
      try {
        selectTemporaryBrowserProfile(site.mode)
        if (browserProfilesSupported()) {
          ProfileStore.getInstance().deleteProfile(profileToDelete)
          promise.resolve(browserSiteSettingsJson(site.mode, site.hostname))
        } else {
          WebStorage.getInstance().deleteAllData()
          val cookieManager = CookieManager.getInstance()
          cookieManager.removeAllCookies {
            try {
              cookieManager.flush()
              promise.resolve(browserSiteSettingsJson(site.mode, site.hostname))
            } catch (error: RuntimeException) {
              promise.reject(
                  "E_BROWSER_SITE_DELETE",
                  "Android could not remove the temporary website data.",
                  error,
              )
            }
          }
        }
      } catch (error: RuntimeException) {
        promise.reject(
            "E_BROWSER_SITE_DELETE",
            "Android could not remove the remembered website data.",
            error,
        )
      }
    }
  }

  override fun clearRememberedBrowserData(promise: Promise) {
    callbackExecutor.execute {
      try {
        clearRememberedBrowserStorage(clearProtectionExceptions = false)
        promise.resolve("ok")
      } catch (error: RuntimeException) {
        promise.reject(
            "E_BROWSER_SITE_DELETE",
            "Android could not clear the remembered website sessions.",
            error,
        )
      }
    }
  }

  override fun getSavedConfiguration(promise: Promise) {
    MasqCoreLifecycle.executor.execute {
      val saved =
          try {
            preferences.getString(SAVED_CONFIG_KEY, null)
          } catch (error: Exception) {
            promise.reject(
                "E_SAVED_CONFIG_INVALID",
                SAVED_CONFIG_INVALID_MESSAGE,
                error,
            )
            return@execute
          }
      if (saved == null) {
        promise.resolve("null")
        return@execute
      }
      try {
        val migrated = migrateConfig(saved)
        val migrationCommitted =
            preferences.edit().putString(SAVED_CONFIG_KEY, migrated).commit()
        if (!migrationCommitted) {
          throw IllegalStateException("The saved MASQ profile migration could not be committed.")
        }
        promise.resolve(migrated)
      } catch (error: Exception) {
        promise.reject(
            "E_SAVED_CONFIG_INVALID",
            SAVED_CONFIG_INVALID_MESSAGE,
            error,
        )
      }
    }
  }

  override fun configure(configJson: String, promise: Promise) {
    invalidatePendingStarts()
    MasqCoreLifecycle.executor.execute {
      restoreAttempted = true
      ifCoreAvailable(promise) {
        val result = MasqCoreJni.nativeConfigure(prepareNativeConfig(configJson))
        if (statusSucceeded(result)) {
          val committed =
              preferences
                  .edit()
                  .putString(SAVED_CONFIG_KEY, migrateConfig(configJson))
                  .commit()
          if (!committed) {
            throw IllegalStateException("The saved MASQ profile could not be committed.")
          }
        }
        result
      }
    }
  }

  override fun importWallet(privateKey: String, promise: Promise) {
    invalidatePendingStarts()
    MasqCoreLifecycle.executor.execute {
      if (!MasqCoreJni.isAvailable) {
        promise.reject("E_CORE_UNAVAILABLE", "The native MASQ core is missing from this build.")
        return@execute
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
            return@execute
          }
        }
        promise.resolve(result)
      } catch (error: RuntimeException) {
        promise.reject("E_CORE_WALLET", "The MASQ core rejected the wallet.", error)
      }
    }
  }

  override fun updateMinHops(minHops: Double, promise: Promise) {
    if (minHops < 1 || minHops > 6 || minHops % 1.0 != 0.0) {
      promise.reject("E_MIN_HOPS", "Choose between one and six MASQ hops.")
      return
    }
    invalidatePendingStarts()
    MasqCoreLifecycle.executor.execute {
      try {
        val saved = preferences.getString(SAVED_CONFIG_KEY, null)
        if (saved == null) {
          promise.reject("E_SAVED_CONFIG", "The saved MASQ network profile is invalid.")
          return@execute
        }
        restoreCoreIfNeeded()
        ifCoreAvailable(promise) {
          val hops = minHops.toInt()
          val result = MasqCoreJni.nativeUpdateMinHops(hops)
          if (statusSucceeded(result)) {
            val committed =
                preferences
                    .edit()
                    .putString(
                        SAVED_CONFIG_KEY,
                        JSONObject(saved).put("minHops", hops).toString(),
                    )
                    .commit()
            if (!committed) {
              throw IllegalStateException("The saved MASQ profile could not be committed.")
            }
          }
          result
        }
      } catch (error: Exception) {
        promise.reject(
            "E_CORE_RESTORE",
            "The saved MASQ state could not be restored safely.",
            error,
        )
      }
    }
  }

  override fun start(promise: Promise) {
    val generation =
        synchronized(MasqCoreLifecycle.lock) {
          MasqCoreLifecycle.startGeneration.incrementAndGet()
        }
    if (!MasqCoreJni.isAvailable) {
      promise.reject("E_CORE_UNAVAILABLE", "The native MASQ core is missing from this build.")
      return
    }
    MasqCoreLifecycle.executor.execute {
      try {
        requireCurrentStart(generation)
        restoreCoreIfNeeded()
        requireCurrentStart(generation)
        val currentStatus = JSONObject(MasqCoreJni.nativeGetStatus())
        requireCurrentStart(generation)
        if (currentStatus.optString("phase") != "ready") {
          synchronized(MasqCoreLifecycle.lock) {
            requireCurrentStart(generation)
            val started = MasqCoreJni.nativeStart()
            requireCurrentStart(generation)
            resolveStart(generation, promise, started)
          }
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
        requireCurrentStart(generation)
        discoveryExecutor.execute {
          completeStartAfterDiscovery(
              generation,
              config,
              chain,
              preferredNodes,
              promise,
          )
        }
      } catch (_: StaleStartException) {
        rejectStart(generation, promise, "E_CORE_START_CANCELLED", START_CANCELLED_MESSAGE)
      } catch (error: EntryNodeDiscoveryException) {
        rejectStart(
            generation,
            promise,
            "E_ENTRY_NODE_DISCOVERY",
            error.message ?: "MASQ could not find reachable entry nodes.",
            error,
        )
      } catch (error: Exception) {
        rejectStart(
            generation,
            promise,
            "E_CORE_START",
            error.message ?: "The MASQ core could not start.",
            error,
        )
      }
    }
  }

  private fun completeStartAfterDiscovery(
      generation: Long,
      config: JSONObject,
      chain: String,
      preferredNodes: List<String>,
      promise: Promise,
  ) {
    try {
      val refreshedNodes = entryNodeDiscovery.discover(chain, preferredNodes)
      requireCurrentStart(generation)
      config.put("neighbors", JSONArray(refreshedNodes))
      val refreshedConfig = migrateConfig(config.toString())
      requireCurrentStart(generation)
      MasqCoreLifecycle.executor.execute {
        try {
          synchronized(MasqCoreLifecycle.lock) {
            requireCurrentStart(generation)
            val configureResult =
                MasqCoreJni.nativeConfigure(prepareNativeConfig(refreshedConfig))
            requireCurrentStart(generation)
            if (!statusSucceeded(configureResult)) {
              throw IllegalStateException(
                  "The MASQ core rejected the refreshed entry nodes.",
              )
            }
            val started = MasqCoreJni.nativeStart()
            requireCurrentStart(generation)
            resolveStart(generation, promise, started, refreshedConfig)
          }
        } catch (_: StaleStartException) {
          rejectStart(
              generation,
              promise,
              "E_CORE_START_CANCELLED",
              START_CANCELLED_MESSAGE,
          )
        } catch (error: Exception) {
          rejectStart(
              generation,
              promise,
              "E_CORE_START",
              error.message ?: "The MASQ core could not start.",
              error,
          )
        }
      }
    } catch (_: StaleStartException) {
      rejectStart(
          generation,
          promise,
          "E_CORE_START_CANCELLED",
          START_CANCELLED_MESSAGE,
      )
    } catch (error: EntryNodeDiscoveryException) {
      rejectStart(
          generation,
          promise,
          "E_ENTRY_NODE_DISCOVERY",
          error.message ?: "MASQ could not find reachable entry nodes.",
          error,
      )
    } catch (error: Exception) {
      rejectStart(
          generation,
          promise,
          "E_CORE_START",
          error.message ?: "The MASQ core could not start.",
          error,
      )
    }
  }

  override fun stop(promise: Promise) {
    invalidatePendingStarts()
    if (!MasqCoreJni.isAvailable) {
      promise.resolve(statusJson())
      return
    }
    MasqCoreLifecycle.executor.execute {
      try {
        promise.resolve(MasqCoreJni.nativeStop())
      } catch (error: RuntimeException) {
        promise.reject("E_CORE_STOP", "The MASQ core could not be stopped.", error)
      }
    }
  }

  override fun shutdown(promise: Promise) {
    invalidatePendingStarts()
    if (!MasqCoreJni.isAvailable) {
      promise.resolve(statusJson())
      return
    }
    MasqCoreLifecycle.executor.execute {
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

  private fun invalidatePendingStarts() {
    synchronized(MasqCoreLifecycle.lock) {
      MasqCoreLifecycle.startGeneration.incrementAndGet()
    }
  }

  private fun requireCurrentStart(generation: Long) {
    if (MasqCoreLifecycle.startGeneration.get() != generation) {
      throw StaleStartException()
    }
  }

  private fun resolveStart(
      generation: Long,
      promise: Promise,
      status: String,
      refreshedConfig: String? = null,
  ) {
    synchronized(MasqCoreLifecycle.lock) {
      if (MasqCoreLifecycle.startGeneration.get() != generation) {
        promise.reject("E_CORE_START_CANCELLED", START_CANCELLED_MESSAGE)
      } else {
        if (refreshedConfig != null) {
          val committed =
              preferences.edit().putString(SAVED_CONFIG_KEY, refreshedConfig).commit()
          if (!committed) {
            runCatching { MasqCoreJni.nativeStop() }
            throw IllegalStateException("The refreshed MASQ profile could not be committed.")
          }
        }
        promise.resolve(status)
      }
    }
  }

  private fun rejectStart(
      generation: Long,
      promise: Promise,
      code: String,
      message: String,
      error: Throwable? = null,
  ) {
    synchronized(MasqCoreLifecycle.lock) {
      if (MasqCoreLifecycle.startGeneration.get() != generation) {
        promise.reject("E_CORE_START_CANCELLED", START_CANCELLED_MESSAGE)
      } else if (error == null) {
        promise.reject(code, message)
      } else {
        promise.reject(code, message, error)
      }
    }
  }

  override fun reset(promise: Promise) {
    if (!MasqCoreJni.isAvailable) {
      promise.reject(
          "E_CORE_UNAVAILABLE",
          "The native MASQ core is missing from this build.",
      )
      return
    }
    invalidatePendingStarts()
    resetSystemTunnelForFullReset(promise)
  }

  private fun finishFullReset(promise: Promise) {
    MasqCoreLifecycle.executor.execute {
      try {
        val resetStatusJson = MasqCoreJni.nativeReset()
        if (!isExactFullResetStatus(JSONObject(resetStatusJson))) {
          promise.reject(
              "E_CORE_RESET",
              "The MASQ core did not confirm a complete reset.",
          )
          return@execute
        }
        walletStore.delete()
        if (!preferences.edit().remove(SAVED_CONFIG_KEY).commit()) {
          throw IllegalStateException("The saved MASQ profile could not be removed.")
        }
        restoreAttempted = true
        val finalStatusJson = MasqCoreJni.nativeGetStatus()
        if (!isExactFullResetStatus(JSONObject(finalStatusJson))) {
          promise.reject(
              "E_CORE_RESET",
              "The MASQ core did not retain the complete reset state.",
          )
          return@execute
        }
        callbackExecutor.execute {
          try {
            clearRememberedBrowserStorage(clearProtectionExceptions = true)
            promise.resolve(finalStatusJson)
          } catch (error: Exception) {
            promise.reject(
                "E_BROWSER_SITE_DELETE",
                "Remembered browser data could not be removed during the full reset.",
                error,
            )
          }
        }
      } catch (error: Exception) {
        promise.reject(
            "E_CORE_RESET",
            "The saved wallet, network profile, or MASQ core could not be reset.",
            error,
        )
      }
    }
  }

  private fun resetSystemTunnelForFullReset(promise: Promise) {
    val requestId = TUNNEL_REQUEST_COUNTER.getAndIncrement()
    val operation = PendingTunnelReset(requestId = requestId, promise = promise)
    val timeout =
        Runnable {
          completeTunnelReset(operation) {
            promise.reject(
                "E_VPN_RESET_TIMEOUT",
                "Android did not confirm that the MASQ system tunnel reset safely.",
            )
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
      pendingTunnelResets[requestId] = operation
      try {
        MasqVpnService.registerResetAcknowledgement(requestId) { acknowledgement ->
          completeTunnelReset(operation) {
            val error = acknowledgement.error
            val serialized = acknowledgement.status
            if (error != null || serialized == null) {
              promise.reject(
                  "E_VPN_RESET",
                  error ?: "The MASQ system tunnel reset response was invalid.",
              )
              return@completeTunnelReset
            }
            val status = runCatching { JSONObject(serialized) }.getOrNull()
            val resetConfirmed =
                status != null &&
                    !status.optBoolean("active", true) &&
                    !status.optBoolean("tunPresent", true) &&
                    status.optString("routingPhase") == "off" &&
                    status.isNull("desiredRevision") &&
                    status.isNull("appliedRevision")
            if (resetConfirmed) {
              finishFullReset(promise)
            } else {
              promise.reject(
                  "E_VPN_RESET",
                  "Android returned an unconfirmed MASQ system-routing reset state.",
              )
            }
          }
        }
      } catch (error: RuntimeException) {
        completeTunnelReset(operation) {
          promise.reject(
              "E_VPN_RESET",
              "Android could not register the MASQ system-routing reset acknowledgement.",
              error,
          )
        }
        return
      }

      try {
        operation.timeoutFuture.set(
            stopAcknowledgementExecutor.schedule(
                timeout,
                RESET_TUNNEL_TIMEOUT_MS,
                TimeUnit.MILLISECONDS,
            ))
      } catch (error: RuntimeException) {
        completeTunnelReset(operation) {
          promise.reject(
              "E_VPN_RESET",
              "Android could not monitor the MASQ system-routing reset.",
              error,
          )
        }
        return
      }

      val intent =
          Intent(reactApplicationContext, MasqVpnService::class.java)
              .setAction(MasqVpnService.ACTION_RESET)
              .putExtra(MasqVpnService.EXTRA_COMMAND_REQUEST_ID, requestId)
      try {
        ContextCompat.startForegroundService(reactApplicationContext, intent)
      } catch (error: Exception) {
        completeTunnelReset(operation) {
          promise.reject(
              "E_VPN_RESET_DISPATCH",
              "Android could not dispatch the MASQ system-routing reset.",
              error,
          )
        }
      }
    }
  }

  override fun resetNetworkProfile(promise: Promise) {
    invalidatePendingStarts()
    if (!MasqCoreJni.isAvailable) {
      promise.reject("E_CORE_UNAVAILABLE", "The native MASQ core is missing from this build.")
      return
    }
    MasqCoreLifecycle.executor.execute {
      val statusBeforeReset =
          try {
            JSONObject(MasqCoreJni.nativeGetStatus())
          } catch (error: Exception) {
            rejectNetworkProfileReset(promise, error)
            return@execute
          }
      val walletAddressBeforeReset =
          try {
            nullableStatusString(statusBeforeReset, "walletAddress")
          } catch (error: Exception) {
            rejectNetworkProfileReset(promise, error)
            return@execute
          }
      val walletReadBeforeReset =
          try {
            walletStore.readForPreservation()
          } catch (error: Exception) {
            rejectWalletPreservation(promise, error)
            return@execute
          }
      val savedWalletBeforeReset =
          when (walletReadBeforeReset) {
            SecureWalletStore.PreservationRead.Absent -> null
            is SecureWalletStore.PreservationRead.Readable ->
                walletReadBeforeReset.secret
            SecureWalletStore.PreservationRead.Unreadable -> {
              rejectWalletPreservation(promise)
              return@execute
            }
          }
      if (walletAddressBeforeReset != null && savedWalletBeforeReset == null) {
        rejectWalletPreservation(promise)
        return@execute
      }

      restoreAttempted = true

      val resetStatus =
          try {
            JSONObject(MasqCoreJni.nativeResetNetworkProfile())
          } catch (error: Exception) {
            rejectNetworkProfileReset(promise, error)
            return@execute
          }
      if (!isExactNetworkProfileResetStatus(resetStatus)) {
        rejectNetworkProfileReset(promise)
        return@execute
      }

      val walletReadAfterReset =
          try {
            walletStore.readForPreservation()
          } catch (error: Exception) {
            rejectWalletPreservation(promise, error)
            return@execute
          }
      val savedWalletAfterReset =
          when (walletReadAfterReset) {
            SecureWalletStore.PreservationRead.Absent -> null
            is SecureWalletStore.PreservationRead.Readable ->
                walletReadAfterReset.secret
            SecureWalletStore.PreservationRead.Unreadable -> {
              rejectWalletPreservation(promise)
              return@execute
            }
          }
      if (!walletSecretsMatch(savedWalletBeforeReset, savedWalletAfterReset)) {
        rejectWalletPreservation(promise)
        return@execute
      }

      var importedWalletAddress: String? = null
      if (savedWalletAfterReset != null) {
        val importStatus =
            try {
              JSONObject(MasqCoreJni.nativeImportWallet(savedWalletAfterReset))
            } catch (error: Exception) {
              rejectWalletPreservation(promise, error)
              return@execute
            }
        if (!isExactNetworkProfileResetStatus(importStatus)) {
          rejectWalletPreservation(promise)
          return@execute
        }
        importedWalletAddress =
            try {
              nullableStatusString(importStatus, "walletAddress")
            } catch (error: Exception) {
              rejectWalletPreservation(promise, error)
              return@execute
            }
        if (importedWalletAddress == null) {
          rejectWalletPreservation(promise)
          return@execute
        }
      }

      val finalStatusJson: String
      val finalStatus: JSONObject
      try {
        finalStatusJson = MasqCoreJni.nativeGetStatus()
        finalStatus = JSONObject(finalStatusJson)
      } catch (error: Exception) {
        rejectNetworkProfileReset(promise, error)
        return@execute
      }
      val finalWalletAddress =
          try {
            nullableStatusString(finalStatus, "walletAddress")
          } catch (error: Exception) {
            rejectWalletPreservation(promise, error)
            return@execute
          }
      val walletPresenceMatches =
          (savedWalletAfterReset == null && finalWalletAddress == null) ||
              (savedWalletAfterReset != null && finalWalletAddress != null)
      if (!walletPresenceMatches ||
          (importedWalletAddress != null &&
              finalWalletAddress != importedWalletAddress) ||
          (walletAddressBeforeReset != null &&
              finalWalletAddress != walletAddressBeforeReset)) {
        rejectWalletPreservation(promise)
        return@execute
      }
      if (!isExactNetworkProfileResetStatus(finalStatus)) {
        rejectNetworkProfileReset(promise)
        return@execute
      }
      val profileRemoved =
          try {
            preferences.edit().remove(SAVED_CONFIG_KEY).commit()
          } catch (error: Exception) {
            rejectNetworkProfileReset(promise, error)
            return@execute
          }
      if (!profileRemoved) {
        rejectNetworkProfileReset(promise)
        return@execute
      }
      promise.resolve(finalStatusJson)
    }
  }

  override fun removeWallet(promise: Promise) {
    invalidatePendingStarts()
    MasqCoreLifecycle.executor.execute {
      if (!MasqCoreJni.isAvailable) {
        promise.reject(
            "E_CORE_UNAVAILABLE",
            "The native MASQ core is missing from this build.",
        )
        return@execute
      }
      val removalStatusJson =
          try {
            MasqCoreJni.nativeRemoveWallet()
          } catch (error: Exception) {
            promise.reject(
                "E_CORE_WALLET_REMOVE",
                "The MASQ core could not remove the consumer wallet.",
                error,
            )
            return@execute
          }
      if (
          !isExactWalletRemovalStatus(JSONObject(removalStatusJson))
      ) {
        promise.reject(
              "E_CORE_WALLET_REMOVE",
              "The MASQ core did not confirm wallet removal.",
        )
        return@execute
      }
      try {
        walletStore.delete()
        restoreAttempted = true
        val finalStatusJson = MasqCoreJni.nativeGetStatus()
        if (!isExactWalletRemovalStatus(JSONObject(finalStatusJson))) {
          promise.reject(
              "E_CORE_WALLET_REMOVE",
              "The MASQ core did not retain the wallet-removed state.",
          )
          return@execute
        }
        promise.resolve(finalStatusJson)
      } catch (error: Exception) {
        promise.reject(
            "E_KEYSTORE_DELETE",
            "The saved consumer wallet could not be removed from secure Android storage.",
            error,
        )
      }
    }
  }

  override fun preflightBrowserProxy(promise: Promise) {
    MasqCoreLifecycle.executor.execute {
      ifCoreAvailable(promise) { MasqCoreJni.nativePreflightProxy() }
    }
  }

  override fun getSystemTunnelStatus(promise: Promise) {
    MasqCoreLifecycle.executor.execute {
      val load = systemRoutingPolicyStore.loadForServiceStart()
      val coreStatus =
          if (MasqCoreJni.isAvailable) {
            runCatching {
                  JSONObject(MasqCoreJni.nativeGetStatus())
                }
                .getOrNull()
          } else {
            null
          }
      promise.resolve(
          MasqVpnService.publishCoreRouteHealth(
              load = load,
              coreConnected = coreStatus?.optString("phase") == "connected",
              proxyPort = coreStatus?.optInt("proxyPort", 0) ?: 0,
          ))
    }
  }

  @Suppress("DEPRECATION")
  override fun getRoutableApps(promise: Promise) {
    if (!BuildConfig.MASQ_SYSTEM_TUNNEL_ENABLED) {
      promise.resolve("[]")
      return
    }
    try {
      val launcherIntent =
          Intent(Intent.ACTION_MAIN).apply { addCategory(Intent.CATEGORY_LAUNCHER) }
      val packageManager = reactApplicationContext.packageManager
      val apps =
          packageManager
              .queryIntentActivities(launcherIntent, PackageManager.MATCH_ALL)
              .mapNotNull { info ->
                val packageName = info.activityInfo?.packageName ?: return@mapNotNull null
                if (isMasqControlPlanePackage(packageName)) return@mapNotNull null
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
    if (!BuildConfig.MASQ_SYSTEM_TUNNEL_ENABLED) {
      promise.reject(
          "E_VPN_PREVIEW_DISABLED",
          "System routing is not available in this preview.",
      )
      return
    }
    val desiredMode = SystemRoutingMode.fromWireName(mode)
    if (desiredMode != SystemRoutingMode.WHOLE_DEVICE &&
        desiredMode != SystemRoutingMode.SELECTED_APPS) {
      promise.reject("E_VPN_MODE", "Choose a valid MASQ traffic scope.")
      return
    }
    if (!MasqPacketTunnelJni.isAvailable) {
      promise.reject("E_VPN_UNAVAILABLE", "The native MASQ packet tunnel is missing from this build.")
      return
    }
    if (!MasqCoreJni.isAvailable) {
      promise.reject("E_CORE_UNAVAILABLE", "The native MASQ core is missing from this build.")
      return
    }
    val apps =
        runCatching {
          val values = JSONArray(appIdsJson)
          val rawApps =
              (0 until values.length()).map { index ->
                values.get(index) as? String
                    ?: throw IllegalArgumentException("Package IDs must be strings.")
              }
          requireNotNull(canonicalizeSystemRoutingPackageIds(rawApps))
        }.getOrElse {
          promise.reject("E_VPN_APPS", "The selected Android app list is invalid.")
          return
        }
    if (apps.any(::isMasqControlPlanePackage)) {
      promise.reject(
          "E_VPN_APPS",
          "MASQ cannot route either MASQ app process until native socket protection is available.",
      )
      return
    }
    if (desiredMode == SystemRoutingMode.WHOLE_DEVICE && apps.isNotEmpty()) {
      promise.reject("E_VPN_APPS", "Whole-device routing cannot contain selected apps.")
      return
    }
    if (desiredMode == SystemRoutingMode.SELECTED_APPS && apps.isEmpty()) {
      promise.reject("E_VPN_APPS", "Choose at least one app to protect.")
      return
    }
    val missingPackage = apps.firstOrNull { !isInstalledPackage(it) }
    if (missingPackage != null) {
      promise.reject("E_VPN_APPS", "A selected Android app is no longer installed.")
      return
    }

    val storedPolicy = systemRoutingPolicyStore.loadForServiceStart()
    val status = JSONObject(MasqVpnService.publishDesiredPolicy(storedPolicy))
    val expectedRevision: Long?
    val reuseRevision: Long?
    when (storedPolicy) {
      SystemRoutingPolicyLoadResult.Missing -> {
        if (status.optBoolean("tunPresent", false)) {
          promise.reject(
              "E_VPN_ACTIVE",
              "Finish stopping the current system tunnel before starting a new scope.",
          )
          return
        }
        expectedRevision = null
        reuseRevision = null
      }
      is SystemRoutingPolicyLoadResult.ExplicitOff -> {
        if (status.optBoolean("tunPresent", false)) {
          promise.reject(
              "E_VPN_ACTIVE",
              "Finish stopping the current system tunnel before starting a new scope.",
          )
          return
        }
        expectedRevision = storedPolicy.policy.revision
        reuseRevision = null
      }
      is SystemRoutingPolicyLoadResult.BlockRequired -> {
        promise.reject(
            "E_VPN_POLICY",
            "The saved Android routing policy is unsafe and must be reset.",
        )
        return
      }
      is SystemRoutingPolicyLoadResult.Ready -> {
        val existing = storedPolicy.policy
        if (existing.desiredMode == desiredMode &&
            existing.selectedApps == apps &&
            !existing.failClosedDesired) {
          expectedRevision = existing.revision
          reuseRevision = existing.revision
        } else {
          if (status.optBoolean("tunPresent", false)) {
            promise.reject(
                "E_VPN_ACTIVE",
                "Turn off the current system tunnel before changing its scope.",
            )
            return
          }
          expectedRevision = existing.revision
          reuseRevision = null
        }
      }
    }

    MasqCoreLifecycle.executor.execute {
      try {
        restoreCoreIfNeeded()
        val coreStatus = JSONObject(MasqCoreJni.nativeGetStatus())
        val proxyPort = coreStatus.optInt("proxyPort", 0)
        if (
            coreStatus.optString("phase") != "connected" ||
                proxyPort !in 1..65535
        ) {
          promise.reject(
              "E_NOT_CONNECTED",
              "Connect a valid MASQ route before enabling system traffic.",
          )
          return@execute
        }
        val coreGeneration = MasqCoreLifecycle.startGeneration.get()
        callbackExecutor.execute {
          requestSystemTunnelPermissionOrStart(
              desiredMode,
              apps,
              expectedRevision,
              reuseRevision,
              coreGeneration,
              promise,
          )
        }
      } catch (error: SecureWalletStore.UnreadableException) {
        promise.reject(
            "E_WALLET_STORAGE_UNREADABLE",
            "The encrypted consumer wallet could not be read safely. Unlock the device and retry.",
            error,
        )
      } catch (error: Exception) {
        promise.reject("E_CORE_STATUS", "The MASQ core status is unavailable.", error)
      }
    }
  }

  private fun requestSystemTunnelPermissionOrStart(
      mode: SystemRoutingMode,
      apps: List<String>,
      expectedRevision: Long?,
      reuseRevision: Long?,
      coreGeneration: Long,
      promise: Promise,
  ) {
    synchronized(MasqCoreLifecycle.lock) {
      if (coreGeneration != MasqCoreLifecycle.startGeneration.get()) {
        promise.reject(
            "E_VPN_STALE_CORE",
            "The MASQ core changed before system routing could start.",
        )
        return
      }
    }
    val permissionIntent = VpnService.prepare(reactApplicationContext)
    val request =
        PendingTunnelRequest(
            mode = mode,
            apps = apps,
            expectedRevision = expectedRevision,
            reuseRevision = reuseRevision,
            coreGeneration = coreGeneration,
            promise = promise,
        )
    if (permissionIntent == null) {
      persistAndStartSystemTunnel(request)
      return
    }
    val activity = reactApplicationContext.getCurrentActivity()
    if (activity == null) {
      promise.reject("E_VPN_ACTIVITY", "Open MASQ before approving Android VPN access.")
      return
    }
    val requestRegistered =
        synchronized(lifecycleLock) {
          if (moduleInvalidated || pendingTunnelRequest != null) {
            false
          } else {
            pendingTunnelRequest = request
            true
          }
        }
    if (!requestRegistered) {
      promise.reject("E_VPN_ACTIVITY", "Open MASQ before approving Android VPN access.")
      return
    }
    try {
      activity.startActivityForResult(permissionIntent, VPN_PERMISSION_REQUEST)
    } catch (error: Exception) {
      val shouldReject =
          synchronized(lifecycleLock) {
            if (pendingTunnelRequest === request) {
              pendingTunnelRequest = null
              true
            } else {
              false
            }
          }
      if (shouldReject) {
        promise.reject(
            "E_VPN_PERMISSION",
            "Android could not open the VPN permission dialog.",
            error,
        )
      }
    }
  }

  private fun stopSystemTunnel(promise: Promise) {
    val offPolicy =
        when (val load = systemRoutingPolicyStore.loadForServiceStart()) {
          is SystemRoutingPolicyLoadResult.ExplicitOff -> load.policy
          is SystemRoutingPolicyLoadResult.Ready ->
              when (val write =
                  systemRoutingPolicyStore.persistOff(load.policy.revision)) {
                is SystemRoutingPolicyWriteResult.Stored -> write.policy
                else -> {
                  rejectPolicyWrite(promise, write, "stop")
                  return
                }
              }
          SystemRoutingPolicyLoadResult.Missing ->
              when (val write = systemRoutingPolicyStore.persistOff(null)) {
                is SystemRoutingPolicyWriteResult.Stored -> write.policy
                else -> {
                  rejectPolicyWrite(promise, write, "stop")
                  return
                }
              }
          is SystemRoutingPolicyLoadResult.BlockRequired -> {
            promise.reject(
                "E_VPN_POLICY",
                "The saved Android routing policy is unsafe and cannot be stopped authoritatively.",
            )
            return
          }
        }
    val offLoad = SystemRoutingPolicyLoadResult.ExplicitOff(offPolicy)
    val requestId = TUNNEL_REQUEST_COUNTER.getAndIncrement()
    val operation =
        PendingTunnelStop(
            requestId = requestId,
            policyRevision = offPolicy.revision,
            promise = promise,
        )
    val timeoutMessage =
        "Android did not confirm that the MASQ system tunnel stopped."
    val timeout =
        Runnable {
          completeTunnelStop(operation) {
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

      MasqVpnService.publishDesiredPolicy(
          offLoad,
          SystemRoutingTransition.STOPPING,
      )
      try {
        MasqVpnService.registerStopAcknowledgement(requestId) { acknowledgement ->
          completeTunnelStop(operation) {
            val error = acknowledgement.error
            val status = acknowledgement.status
            if (error != null || status == null) {
              val message = error ?: "The MASQ system tunnel stop response was invalid."
              promise.reject("E_VPN_STOP", message)
            } else {
              val value = runCatching { JSONObject(status) }.getOrNull()
              if (value == null ||
                  value.optBoolean("active", true) ||
                  value.optBoolean("tunPresent", true) ||
                  value.optLong("desiredRevision", -1) != operation.policyRevision ||
                  value.optString("routingPhase") != "off") {
                promise.reject(
                    "E_VPN_STOP",
                    "Android returned an unconfirmed MASQ tunnel stop state.",
                )
              } else {
                promise.resolve(status)
              }
            }
          }
        }
      } catch (error: RuntimeException) {
        completeTunnelStop(operation) {
          val message =
              "Android could not register the MASQ tunnel stop acknowledgement."
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
          promise.reject("E_VPN_STOP", message, error)
        }
        return
      }

      val intent =
          Intent(reactApplicationContext, MasqVpnService::class.java)
              .setAction(MasqVpnService.ACTION_STOP)
              .putExtra(MasqVpnService.EXTRA_COMMAND_REQUEST_ID, requestId)
              .putExtra(MasqVpnService.EXTRA_POLICY_REVISION, offPolicy.revision)
      try {
        val dispatched = reactApplicationContext.startService(intent)
        if (dispatched == null) {
          throw IllegalStateException("Android did not dispatch the MASQ tunnel stop request.")
        }
      } catch (error: Exception) {
        completeTunnelStop(operation) {
          val message = "Android could not dispatch the MASQ tunnel stop request."
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

  private fun completeTunnelReset(
      operation: PendingTunnelReset,
      settlement: () -> Unit,
  ): Boolean =
      synchronized(lifecycleLock) {
        if (!operation.completed.compareAndSet(false, true)) {
          return@synchronized false
        }
        pendingTunnelResets.remove(operation.requestId)
        operation.timeoutFuture.getAndSet(null)?.cancel(false)
        MasqVpnService.cancelResetAcknowledgement(operation.requestId)
        settlement()
        true
      }

  private fun completeTunnelStart(
      operation: PendingTunnelStart,
      settlement: () -> Boolean,
  ): Boolean =
      synchronized(lifecycleLock) {
        if (!operation.completed.compareAndSet(false, true)) {
          return@synchronized false
        }
        pendingTunnelStarts.remove(operation.requestId)
        operation.timeoutFuture.getAndSet(null)?.cancel(false)
        MasqVpnService.cancelStartAcknowledgement(operation.requestId)
        settlement()
      }

  override fun invalidate() {
    invalidatePendingStarts()
    val abandoned =
        synchronized(lifecycleLock) {
          val starts = mutableListOf<PendingTunnelStart>()
          val stops = mutableListOf<PendingTunnelStop>()
          val resets = mutableListOf<PendingTunnelReset>()
          val permissionRequest = pendingTunnelRequest
          pendingTunnelRequest = null
          if (!moduleInvalidated) {
            moduleInvalidated = true
            pendingTunnelStarts.values.toList().forEach { operation ->
              if (operation.completed.compareAndSet(false, true)) {
                operation.timeoutFuture.getAndSet(null)?.cancel(false)
                MasqVpnService.cancelStartAcknowledgement(operation.requestId)
                starts.add(operation)
              }
            }
            pendingTunnelStarts.clear()
            pendingTunnelStops.values.toList().forEach { operation ->
              if (operation.completed.compareAndSet(false, true)) {
                operation.timeoutFuture.getAndSet(null)?.cancel(false)
                MasqVpnService.cancelStopAcknowledgement(operation.requestId)
                stops.add(operation)
              }
            }
            pendingTunnelStops.clear()
            pendingTunnelResets.values.toList().forEach { operation ->
              if (operation.completed.compareAndSet(false, true)) {
                operation.timeoutFuture.getAndSet(null)?.cancel(false)
                MasqVpnService.cancelResetAcknowledgement(operation.requestId)
                resets.add(operation)
              }
            }
            pendingTunnelResets.clear()
          }
          AbandonedTunnelOperations(starts, stops, resets, permissionRequest)
        }
    stopAcknowledgementExecutor.shutdownNow()
    discoveryExecutor.shutdownNow()
    reactApplicationContext.removeActivityEventListener(tunnelActivityListener)
    abandoned.starts.forEach { operation ->
      runCatching {
        operation.promise.reject(
            "E_MODULE_INVALIDATED",
            "The MASQ native module shut down before system routing became active.",
        )
      }
    }
    abandoned.stops.forEach { operation ->
      runCatching {
        operation.promise.reject(
            "E_MODULE_INVALIDATED",
            "The MASQ native module shut down before the system tunnel stop was confirmed.",
        )
      }
    }
    abandoned.resets.forEach { operation ->
      runCatching {
        operation.promise.reject(
            "E_MODULE_INVALIDATED",
            "The MASQ native module shut down before the system-routing reset was confirmed.",
        )
      }
    }
    abandoned.permissionRequest?.promise?.reject(
        "E_MODULE_INVALIDATED",
        "The MASQ native module shut down before VPN permission was completed.",
    )
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
    if (mode == "masq" || mode == "direct") {
      try {
        selectTemporaryBrowserProfile(mode)
      } catch (error: RuntimeException) {
        promise.reject(
            "E_BROWSER_PROFILE",
            "Android could not prepare the temporary browser profile.",
            error,
        )
        return
      }
    }

    val request =
        BrowserRoutingRequest(
            this,
            mode,
            MasqCoreLifecycle.startGeneration.get(),
            promise,
        )
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
    next?.owner?.applyBrowserRoutingMode(next)
  }

  private fun applyBrowserRoutingMode(request: BrowserRoutingRequest) {
    if (moduleInvalidated) {
      finishBrowserRoutingWithError(
          request,
          "E_MODULE_INVALIDATED",
          "The MASQ native module is shutting down.",
      )
      return
    }
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

    MasqCoreLifecycle.executor.execute {
      try {
        requireCurrentBrowserCore(request)
        val status = JSONObject(MasqCoreJni.nativeGetStatus())
        requireCurrentBrowserCore(request)
        if (status.optString("phase") != "connected") {
          callbackExecutor.execute {
            finishBrowserRoutingWithError(
                request,
                "E_NOT_CONNECTED",
                "Build a MASQ route first.",
            )
          }
          return@execute
        }
        val proxyPort = status.optInt("proxyPort", 0)
        if (proxyPort !in 1..65535) {
          callbackExecutor.execute {
            finishBrowserRoutingWithError(
                request,
                "E_PROXY_PORT",
                "The MASQ core returned an invalid proxy port.",
            )
          }
          return@execute
        }

        callbackExecutor.execute {
          try {
            requireCurrentBrowserCore(request)
            val config =
                ProxyConfig.Builder()
                    .addProxyRule("http://127.0.0.1:$proxyPort")
                    // Intentionally no addDirect(): failure must never bypass MASQ.
                    .build()
            ProxyController.getInstance().setProxyOverride(config, callbackExecutor) {
              confirmMasqBrowserRouting(request)
            }
          } catch (_: StaleBrowserCoreException) {
            rejectStaleBrowserRouting(request)
          } catch (error: RuntimeException) {
            failBrowserRoutingClosed(
                request,
                "E_PROXY_APPLY",
                "The local MASQ proxy could not be configured.",
                error,
            )
          }
        }
      } catch (_: StaleBrowserCoreException) {
        callbackExecutor.execute { rejectStaleBrowserRouting(request) }
      } catch (error: RuntimeException) {
        callbackExecutor.execute {
          failBrowserRoutingClosed(
              request,
              "E_PROXY_APPLY",
              "The local MASQ proxy could not be configured.",
              error,
          )
        }
      }
    }
  }

  private fun confirmMasqBrowserRouting(request: BrowserRoutingRequest) {
    MasqCoreLifecycle.executor.execute {
      try {
        requireCurrentBrowserCore(request)
        val result = MasqCoreJni.nativeSetProxyEnabled(true)
        requireCurrentBrowserCore(request)
        if (!statusSucceeded(result)) {
          throw IllegalStateException("The MASQ core did not confirm the browser proxy.")
        }
        callbackExecutor.execute {
          try {
            requireCurrentBrowserCore(request)
            finishBrowserRouting(request, "masq")
          } catch (_: StaleBrowserCoreException) {
            rejectStaleBrowserRouting(request)
          }
        }
      } catch (_: StaleBrowserCoreException) {
        callbackExecutor.execute { rejectStaleBrowserRouting(request) }
      } catch (error: RuntimeException) {
        callbackExecutor.execute {
          failBrowserRoutingClosed(
              request,
              "E_PROXY_STATE",
              "The MASQ core could not confirm the proxy.",
              error,
          )
        }
      }
    }
  }

  private fun applyDirectBrowserRouting(request: BrowserRoutingRequest) {
    try {
      requireCurrentBrowserCore(request)
    } catch (_: StaleBrowserCoreException) {
      rejectStaleBrowserRouting(request)
      return
    }
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
        MasqCoreLifecycle.executor.execute {
          try {
            requireCurrentBrowserCore(request)
            if (MasqCoreJni.isAvailable) {
              val result = MasqCoreJni.nativeSetProxyEnabled(false)
              if (!statusSucceeded(result)) {
                throw IllegalStateException(
                    "The MASQ core did not confirm direct browser routing.",
                )
              }
            }
            requireCurrentBrowserCore(request)
            callbackExecutor.execute {
              try {
                requireCurrentBrowserCore(request)
                finishBrowserRouting(request, "direct")
              } catch (_: StaleBrowserCoreException) {
                rejectStaleBrowserRouting(request)
              }
            }
          } catch (_: StaleBrowserCoreException) {
            callbackExecutor.execute { rejectStaleBrowserRouting(request) }
          } catch (error: RuntimeException) {
            callbackExecutor.execute {
              failBrowserRoutingClosed(
                  request,
                  "E_PROXY_STATE",
                  "The MASQ core could not confirm direct browser routing.",
                  error,
              )
            }
          }
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

  private fun validateBrowserSite(
      mode: String,
      hostname: String,
      promise: Promise,
  ): BrowserSite? {
    val normalized = hostname.lowercase(Locale.ROOT)
    val valid =
        (mode == "masq" || mode == "direct") &&
            hostname == normalized &&
            hostname.length in 1..253 &&
            !hostname.endsWith(".") &&
            hostname != "localhost" &&
            !hostname.endsWith(".local") &&
            hostname.split(".").size >= 2 &&
            hostname.split(".").all { label ->
              label.length in 1..63 &&
                  BROWSER_SITE_LABEL_PATTERN.matches(label)
            }
    if (!valid) {
      promise.reject(
          "E_BROWSER_SITE",
          "Choose an exact public HTTPS hostname for MASQ or Direct browsing.",
      )
      return null
    }
    return BrowserSite(mode, hostname)
  }

  private fun browserProfilesSupported(): Boolean =
      WebViewFeature.isFeatureSupported(WebViewFeature.MULTI_PROFILE)

  private fun selectBrowserSite(mode: String, hostname: String): String {
    val prefix = browserSitePreferencePrefix(mode, hostname)
    val persistentSupported = browserProfilesSupported()
    val remembered =
        persistentSupported &&
            preferences.getBoolean("${prefix}remember-sign-in", false)
    if (remembered) {
      selectActiveBrowserProfile(
          mode,
          persistentBrowserProfileName(mode, hostname),
          persistent = true,
      )
    } else {
      selectTemporaryBrowserProfile(mode)
    }
    return browserSiteSettingsJson(mode, hostname)
  }

  private fun selectTemporaryBrowserProfile(mode: String) {
    selectActiveBrowserProfile(
        mode,
        temporaryBrowserProfileName(mode),
        persistent = false,
    )
  }

  private fun selectActiveBrowserProfile(
      mode: String,
      profileName: String,
      persistent: Boolean,
  ) {
    val saved =
        preferences
            .edit()
            .putString(ACTIVE_BROWSER_PROFILE_KEY, profileName)
            .putString(ACTIVE_BROWSER_MODE_KEY, mode)
            .putBoolean(ACTIVE_BROWSER_PROFILE_PERSISTENT_KEY, persistent)
            .commit()
    if (!saved) {
      throw IllegalStateException("The active Android WebView profile could not be saved.")
    }
  }

  private fun browserSiteSettingsJson(mode: String, hostname: String): String {
    val prefix = browserSitePreferencePrefix(mode, hostname)
    val persistentSupported = browserProfilesSupported()
    return JSONObject()
        .put("hostname", hostname)
        .put("mode", mode)
        .put("persistentSessionsSupported", persistentSupported)
        .put("protectionDisabled", preferences.getBoolean("${prefix}protection-disabled", false))
        .put(
            "rememberSignIn",
            persistentSupported &&
                preferences.getBoolean("${prefix}remember-sign-in", false),
        )
        .toString()
  }

  private fun browserSitePreferencePrefix(mode: String, hostname: String): String =
      "$BROWSER_SITE_PREFERENCE_PREFIX$mode.${sha256(hostname)}."

  private fun clearRememberedBrowserStorage(clearProtectionExceptions: Boolean) {
    val activeMode = preferences.getString(ACTIVE_BROWSER_MODE_KEY, "masq") ?: "masq"
    selectTemporaryBrowserProfile(if (activeMode == "direct") "direct" else "masq")
    val editor = preferences.edit()
    preferences.all.keys
        .filter { key ->
          key.startsWith(BROWSER_SITE_PREFERENCE_PREFIX) &&
              (clearProtectionExceptions || key.endsWith("remember-sign-in"))
        }
        .forEach { key -> editor.remove(key) }
    if (!editor.commit()) {
      throw IllegalStateException("Remembered browser settings could not be deleted.")
    }
    if (browserProfilesSupported()) {
      val temporaryProfiles =
          setOf(temporaryBrowserProfileName("masq"), temporaryBrowserProfileName("direct"))
      ProfileStore.getInstance().allProfileNames
          .filter { profileName ->
            profileName.startsWith(BROWSER_PROFILE_PREFIX) &&
                (clearProtectionExceptions || profileName !in temporaryProfiles)
          }
          .forEach { profileName -> ProfileStore.getInstance().deleteProfile(profileName) }
    } else if (clearProtectionExceptions) {
      WebStorage.getInstance().deleteAllData()
      CookieManager.getInstance().removeAllCookies(null)
      CookieManager.getInstance().flush()
    }
  }

  private fun persistentBrowserProfileName(mode: String, hostname: String): String =
      "$BROWSER_PROFILE_PREFIX${mode}_${sha256("site:$hostname")}"

  private fun temporaryBrowserProfileName(mode: String): String =
      "$BROWSER_PROFILE_PREFIX${mode}_${sha256("temporary")}"

  private fun sha256(value: String): String =
      MessageDigest.getInstance("SHA-256")
          .digest(value.toByteArray(StandardCharsets.UTF_8))
          .joinToString("") { byte -> "%02x".format(byte.toInt() and 0xff) }

  private fun activeBrowserProfile(): Profile? {
    if (!browserProfilesSupported()) return null
    val profileName = preferences.getString(ACTIVE_BROWSER_PROFILE_KEY, null) ?: return null
    return ProfileStore.getInstance().getProfile(profileName)
  }

  private fun clearBrowserWebsiteData(
      onComplete: () -> Unit,
      onError: (Throwable) -> Unit,
  ) {
    try {
      if (
          preferences.getBoolean(ACTIVE_BROWSER_PROFILE_PERSISTENT_KEY, false) &&
              browserProfilesSupported()
      ) {
        // Explicitly remembered profiles survive routing teardown. Their
        // profile is isolated per exact host and per MASQ/Direct mode.
        onComplete()
        return
      }
      val profile = activeBrowserProfile()
      val webStorage = profile?.webStorage ?: WebStorage.getInstance()
      val cookieManager = profile?.cookieManager ?: CookieManager.getInstance()
      webStorage.deleteAllData()
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
    MasqCoreLifecycle.executor.execute {
      try {
        MasqCoreJni.nativeSetProxyEnabled(enabled)
      } catch (_: RuntimeException) {
        // WebView routing remains authoritative and blocked on cleanup paths.
      }
    }
  }

  private fun requireCurrentBrowserCore(request: BrowserRoutingRequest) {
    if (request.coreGeneration != MasqCoreLifecycle.startGeneration.get()) {
      throw StaleBrowserCoreException()
    }
  }

  private fun rejectStaleBrowserRouting(request: BrowserRoutingRequest) {
    failBrowserRoutingClosed(
        request,
        "E_BROWSER_STALE_CORE",
        "The MASQ core changed before browser routing could be enabled.",
        StaleBrowserCoreException(),
    )
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
    next?.owner?.applyBrowserRoutingMode(next)
  }

  private data class BrowserRoutingRequest(
      val owner: MasqCoreModule,
      val mode: String,
      val coreGeneration: Long,
      val promise: Promise,
  )

  private fun persistAndStartSystemTunnel(request: PendingTunnelRequest) {
    MasqCoreLifecycle.executor.execute {
      synchronized(MasqCoreLifecycle.lock) {
        if (request.coreGeneration != MasqCoreLifecycle.startGeneration.get()) {
          request.promise.reject(
              "E_VPN_STALE_CORE",
              "The MASQ core changed before system routing could start.",
          )
          return@synchronized
        }
        if (moduleInvalidated) {
          request.promise.reject(
              "E_MODULE_INVALIDATED",
              "The MASQ native module is shutting down.",
          )
          return@synchronized
        }
        val coreStatus =
            try {
              restoreCoreIfNeeded()
              JSONObject(MasqCoreJni.nativeGetStatus())
            } catch (error: SecureWalletStore.UnreadableException) {
              request.promise.reject(
                  "E_WALLET_STORAGE_UNREADABLE",
                  "The encrypted consumer wallet could not be read safely. Unlock the device and retry.",
                  error,
              )
              return@synchronized
            } catch (error: Exception) {
              request.promise.reject(
                  "E_CORE_STATUS",
                  "The MASQ core status is unavailable.",
                  error,
              )
              return@synchronized
            }
        val proxyPort = coreStatus.optInt("proxyPort", 0)
        if (coreStatus.optString("phase") != "connected" || proxyPort !in 1..65535) {
          request.promise.reject(
              "E_NOT_CONNECTED",
              "Connect a valid MASQ route before enabling system traffic.",
          )
          return@synchronized
        }

        val policy =
            if (request.reuseRevision != null) {
              val load = systemRoutingPolicyStore.loadForServiceStart()
              val stored = (load as? SystemRoutingPolicyLoadResult.Ready)?.policy
              if (stored == null ||
                  stored.revision != request.reuseRevision ||
                  stored.desiredMode != request.mode ||
                  stored.selectedApps != request.apps ||
                  stored.failClosedDesired) {
                request.promise.reject(
                    "E_VPN_POLICY_CONFLICT",
                    "The saved MASQ routing policy changed while permission was pending.",
                )
                return@synchronized
              }
              stored
            } else {
              when (
                  val write =
                      systemRoutingPolicyStore.persistBeforeStart(
                          expectedRevision = request.expectedRevision,
                          desiredMode = request.mode,
                          packageIds = request.apps,
                          explicitConsentTimestampMs = System.currentTimeMillis(),
                          failClosedDesired = false,
                      )) {
                is SystemRoutingPolicyWriteResult.Stored -> write.policy
                else -> {
                  rejectPolicyWrite(request.promise, write, "start")
                  return@synchronized
                }
              }
            }
        startSystemTunnel(
            policy,
            proxyPort,
            request.coreGeneration,
            request.promise,
        )
      }
    }
  }

  private fun startSystemTunnel(
      policy: DesiredSystemRoutingPolicy,
      proxyPort: Int,
      coreGeneration: Long,
      promise: Promise,
  ) {
    val requestId = TUNNEL_REQUEST_COUNTER.getAndIncrement()
    val operation =
        PendingTunnelStart(
            requestId = requestId,
            policyRevision = policy.revision,
            coreGeneration = coreGeneration,
            promise = promise,
        )
    val readyLoad = SystemRoutingPolicyLoadResult.Ready(policy)
    val timeoutMessage =
        "Android did not confirm that MASQ system routing became active."
    val timeout =
        Runnable {
          completeTunnelStart(operation) {
            promise.reject("E_VPN_START_TIMEOUT", timeoutMessage)
            false
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
      pendingTunnelStarts[requestId] = operation
      MasqVpnService.publishDesiredPolicy(
          readyLoad,
          SystemRoutingTransition.STARTING_BLOCKING,
      )
      try {
        MasqVpnService.registerStartAcknowledgement(requestId) { acknowledgement ->
          completeTunnelStart(operation) {
            val error = acknowledgement.error
            val serialized = acknowledgement.status
            if (error != null || serialized == null) {
              promise.reject(
                  "E_VPN_START",
                  error ?: "The MASQ system tunnel start response was invalid.",
              )
              return@completeTunnelStart false
            }
            val status =
                runCatching { JSONObject(serialized) }
                    .getOrNull()
                    ?.let {
                      TunnelStartStatusSnapshot(
                          active = it.optBoolean("active", false),
                          routingPhase = it.optString("routingPhase"),
                          desiredRevision = it.optLong("desiredRevision", -1),
                          appliedRevision = it.optLong("appliedRevision", -1),
                          tunPresent = it.optBoolean("tunPresent", false),
                          translatorReady =
                              it.optBoolean("translatorReady", false),
                          coreRouteReady =
                              it.optBoolean("coreRouteReady", false),
                      )
                    }
            val currentCoreGeneration =
                MasqCoreLifecycle.startGeneration.get()
            val semanticallyAccepted =
                tunnelStartAcknowledgementIsSemanticallyAccepted(
                    status = status,
                    error = null,
                    expectedPolicyRevision = operation.policyRevision,
                    expectedCoreGeneration = operation.coreGeneration,
                    currentCoreGeneration = currentCoreGeneration,
                )
            if (operation.coreGeneration != currentCoreGeneration) {
              promise.reject(
                  "E_VPN_STALE_CORE",
                  "The MASQ core changed before system routing became active.",
              )
              false
            } else if (semanticallyAccepted) {
              promise.resolve(serialized)
              true
            } else {
              promise.reject(
                  "E_VPN_START",
                  "Android did not confirm the exact active MASQ routing revision.",
              )
              false
            }
          }
        }
      } catch (error: RuntimeException) {
        completeTunnelStart(operation) {
          promise.reject(
              "E_VPN_START",
              "Android could not register the MASQ tunnel start acknowledgement.",
              error,
          )
          false
        }
        return
      }

      try {
        operation.timeoutFuture.set(
            stopAcknowledgementExecutor.schedule(
                timeout,
                START_TUNNEL_TIMEOUT_MS,
                TimeUnit.MILLISECONDS,
            ))
      } catch (error: RuntimeException) {
        completeTunnelStart(operation) {
          promise.reject(
              "E_VPN_START",
              "Android could not monitor MASQ tunnel activation.",
              error,
          )
          false
        }
        return
      }

      val intent =
          Intent(reactApplicationContext, MasqVpnService::class.java)
              .setAction(MasqVpnService.ACTION_START)
              .putExtra(MasqVpnService.EXTRA_COMMAND_REQUEST_ID, requestId)
              .putExtra(MasqVpnService.EXTRA_POLICY_REVISION, policy.revision)
              .putExtra(MasqVpnService.EXTRA_PROXY_PORT, proxyPort)
              .putExtra(MasqVpnService.EXTRA_CORE_GENERATION, coreGeneration)
      try {
        ContextCompat.startForegroundService(reactApplicationContext, intent)
      } catch (error: Exception) {
        completeTunnelStart(operation) {
          promise.reject(
              "E_VPN_START_DISPATCH",
              "Android could not dispatch MASQ system routing.",
              error,
          )
          false
        }
      }
    }
  }

  @Suppress("DEPRECATION")
  private fun isInstalledPackage(packageId: String): Boolean =
      runCatching {
        reactApplicationContext.packageManager.getApplicationInfo(packageId, 0)
      }.isSuccess

  private fun rejectPolicyWrite(
      promise: Promise,
      write: SystemRoutingPolicyWriteResult,
      operation: String,
  ) {
    val reason =
        when (write) {
          is SystemRoutingPolicyWriteResult.Conflict ->
              "The saved MASQ routing revision changed before $operation."
          is SystemRoutingPolicyWriteResult.Rejected ->
              "The MASQ routing policy was rejected: ${write.reason.wireCode}."
          is SystemRoutingPolicyWriteResult.BlockRequired ->
              "The saved MASQ routing policy is unsafe: ${write.reason.wireCode}."
          is SystemRoutingPolicyWriteResult.IndeterminateCommit ->
              "Android could not verify the persisted MASQ routing policy."
          is SystemRoutingPolicyWriteResult.Stored ->
              "Android returned an unexpected MASQ policy state."
        }
    promise.reject("E_VPN_POLICY", reason)
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
      try {
        val current = JSONObject(MasqCoreJni.nativeGetStatus())
        val savedConfig = preferences.getString(SAVED_CONFIG_KEY, null)
        if (current.isNull("chain") && savedConfig != null) {
          val result = MasqCoreJni.nativeConfigure(prepareNativeConfig(savedConfig))
          if (!statusSucceeded(result)) {
            restoreAttempted = true
            return
          }
        }
        val savedWallet = walletStore.load()
        if (current.isNull("walletAddress") && savedWallet != null) {
          MasqCoreJni.nativeImportWallet(savedWallet)
        }
        restoreAttempted = true
      } catch (error: Exception) {
        restoreAttempted = false
        throw error
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

  private fun statusSucceeded(statusJson: String): Boolean {
    val phase = JSONObject(statusJson).opt("phase")
    return phase is String &&
        phase in
            setOf(
                "unconfigured",
                "ready",
                "connecting",
                "connected",
                "paused",
                "stopping",
                "blocked",
            )
  }

  private fun nullableStatusString(status: JSONObject, field: String): String? {
    if (!status.has(field)) {
      throw IllegalArgumentException("The MASQ core status is missing $field.")
    }
    if (status.isNull(field)) return null
    val value = status.opt(field)
    if (value !is String || value.isBlank()) {
      throw IllegalArgumentException("The MASQ core status contains an invalid $field.")
    }
    return value
  }

  private fun walletSecretsMatch(before: String?, after: String?): Boolean {
    if (before == null || after == null) return before == after
    return MessageDigest.isEqual(
        before.toByteArray(StandardCharsets.UTF_8),
        after.toByteArray(StandardCharsets.UTF_8),
    )
  }

  private fun isExactNetworkProfileResetStatus(status: JSONObject): Boolean {
    val minHops = status.opt("minHops")
    return isExactStoppedRouteStatus(status) &&
        status.has("chain") &&
        status.isNull("chain") &&
        minHops is Number &&
        minHops.toDouble() == 1.0 &&
        status.has("exitCountry") &&
        status.isNull("exitCountry") &&
        status.opt("exitCountryFallback") == true
  }

  private fun isExactFullResetStatus(status: JSONObject): Boolean =
      isExactNetworkProfileResetStatus(status) &&
          status.has("walletAddress") &&
          status.isNull("walletAddress")

  private fun isExactWalletRemovalStatus(status: JSONObject): Boolean =
      isExactStoppedRouteStatus(status) &&
          status.has("walletAddress") &&
          status.isNull("walletAddress")

  private fun isExactStoppedRouteStatus(status: JSONObject): Boolean {
    val connectedNeighbors = status.opt("connectedNeighbors")
    val routeStage = status.opt("routeStage")
    val routeHops = status.opt("routeHops")
    val availableExitCountries = status.opt("availableExitCountries")
    return status.opt("phase") == "unconfigured" &&
        connectedNeighbors is Number &&
        connectedNeighbors.toDouble() == 0.0 &&
        routeStage is Number &&
        routeStage.toDouble() == 0.0 &&
        routeHops is Number &&
        routeHops.toDouble() == 0.0 &&
        availableExitCountries is JSONArray &&
        availableExitCountries.length() == 0 &&
        status.opt("proxyEnabled") == false &&
        status.has("proxyPort") &&
        status.isNull("proxyPort") &&
        status.has("lastError") &&
        status.isNull("lastError")
  }

  private fun rejectNetworkProfileReset(promise: Promise, error: Throwable? = null) {
    if (error == null) {
      promise.reject("E_NETWORK_PROFILE_RESET", NETWORK_PROFILE_RESET_MESSAGE)
    } else {
      promise.reject("E_NETWORK_PROFILE_RESET", NETWORK_PROFILE_RESET_MESSAGE, error)
    }
  }

  private fun rejectWalletPreservation(promise: Promise, error: Throwable? = null) {
    if (error == null) {
      promise.reject("E_WALLET_PRESERVATION", WALLET_PRESERVATION_MESSAGE)
    } else {
      promise.reject("E_WALLET_PRESERVATION", WALLET_PRESERVATION_MESSAGE, error)
    }
  }

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
    private const val ACTIVE_BROWSER_PROFILE_KEY = "browser-profile.active"
    private const val ACTIVE_BROWSER_MODE_KEY = "browser-profile.mode"
    private const val ACTIVE_BROWSER_PROFILE_PERSISTENT_KEY =
        "browser-profile.persistent"
    private const val BROWSER_PROFILE_PREFIX = "masq_"
    private const val BROWSER_SITE_PREFERENCE_PREFIX = "browser-site."
    private const val BLOCKED_BROWSER_PROXY = "http://127.0.0.1:1"
    private const val START_CANCELLED_MESSAGE =
        "The MASQ connection attempt was cancelled."
    private const val SAVED_CONFIG_INVALID_MESSAGE =
        "The saved MASQ network profile is invalid."
    private const val NETWORK_PROFILE_RESET_MESSAGE =
        "The MASQ network profile could not be reset safely."
    private const val WALLET_PRESERVATION_MESSAGE =
        "The saved consumer wallet could not be preserved during network-profile recovery."
    private val browserRoutingLock = Any()
    private val browserRoutingQueue = ArrayDeque<BrowserRoutingRequest>()
    private var browserRoutingInFlight = false
    // Strictly exceeds translator readiness (3s) plus the serialized native
    // CONNECT preflight (up to 15s) and normal executor/FGS dispatch overhead.
    // Covers a 10s terminal translator recovery, 3s replacement readiness,
    // 15s real CONNECT preflight, and service/executor scheduling margin.
    private const val START_TUNNEL_TIMEOUT_MS = 40_000L
    private const val STOP_TUNNEL_TIMEOUT_MS = 15_000L
    private const val RESET_TUNNEL_TIMEOUT_MS = 15_000L
    private val TUNNEL_REQUEST_COUNTER = AtomicLong(1L)
    private val BROWSER_PROTECTION_FIELDS =
        setOf(
            "blockAdsAndTrackers",
            "blockCrossSiteCookies",
            "hideCookieBanners",
            "rejectOptionalCookies",
            "youtubeBestEffort",
        )
    private val BROWSER_SITE_LABEL_PATTERN =
        Regex("^[a-z0-9](?:[a-z0-9-]*[a-z0-9])?$")
    private const val VPN_PERMISSION_REQUEST = 4108
  }

  private data class BrowserSite(
      val mode: String,
      val hostname: String,
  )

  private data class PendingTunnelRequest(
      val mode: SystemRoutingMode,
      val apps: List<String>,
      val expectedRevision: Long?,
      val reuseRevision: Long?,
      val coreGeneration: Long,
      val promise: Promise,
  )

  private data class PendingTunnelStart(
      val requestId: Long,
      val policyRevision: Long,
      val coreGeneration: Long,
      val promise: Promise,
      val completed: AtomicBoolean = AtomicBoolean(false),
      val timeoutFuture: AtomicReference<ScheduledFuture<*>?> = AtomicReference(),
  )

  private data class PendingTunnelStop(
      val requestId: Long,
      val policyRevision: Long,
      val promise: Promise,
      val completed: AtomicBoolean = AtomicBoolean(false),
      val timeoutFuture: AtomicReference<ScheduledFuture<*>?> = AtomicReference(),
  )

  private data class PendingTunnelReset(
      val requestId: Long,
      val promise: Promise,
      val completed: AtomicBoolean = AtomicBoolean(false),
      val timeoutFuture: AtomicReference<ScheduledFuture<*>?> = AtomicReference(),
  )

  private data class AbandonedTunnelOperations(
      val starts: List<PendingTunnelStart>,
      val stops: List<PendingTunnelStop>,
      val resets: List<PendingTunnelReset>,
      val permissionRequest: PendingTunnelRequest?,
  )

  private class StaleStartException : IllegalStateException()
  private class StaleBrowserCoreException : IllegalStateException()
}
