declare const __dirname: string;

export {};

const { readFileSync } = require('fs');
const path = require('path');

const read = (relativePath: string) =>
  readFileSync(path.resolve(__dirname, '..', relativePath), 'utf8');

describe('Android native MASQ core integration', () => {
  const gradle = read('android/app/build.gradle');
  const moduleSource = read(
    'android/app/src/main/java/com/masqmobile/MasqCoreModule.kt',
  );
  const nativeSpec = read('specs/NativeMasqCore.ts');
  const coreFacade = read('src/core/masqCore.ts');
  const walletStore = read(
    'android/app/src/main/java/com/masqmobile/SecureWalletStore.kt',
  );
  const discovery = read(
    'android/app/src/main/java/com/masqmobile/EntryNodeDiscovery.kt',
  );
  const launchVerifier = read(
    '../masq-node-mobile/node/src/daemon/launch_verifier.rs',
  );
  const manifest = read('android/app/src/main/AndroidManifest.xml');
  const mainActivity = read(
    'android/app/src/main/java/com/masqmobile/MainActivity.kt',
  );
  const vpnService = read(
    'android/app/src/main/java/com/masqmobile/MasqVpnService.kt',
  );
  const packetJni = read(
    'android/app/src/main/java/com/masqmobile/MasqPacketTunnelJni.kt',
  );
  const coreJni = read(
    'android/app/src/main/java/com/masqmobile/MasqCoreJni.kt',
  );
  const rustAndroid = read('native/masq-mobile-core/src/android.rs');
  const rustCore = read('native/masq-mobile-core/src/core.rs');
  const webViewPatch = read('patches/react-native-webview+14.0.1.patch');
  const webViewClientSource = read(
    'node_modules/react-native-webview/android/src/main/java/com/reactnativecommunity/webview/RNCWebViewClient.java',
  );
  const mobileCi = read('../.github/workflows/mobile-ci.yml');

  it('builds and packages the real Rust node engine for supported Android ABIs', () => {
    const buildScript = read('scripts/build-rust-android.sh');

    expect(gradle).toContain('it.name == "preBuild"');
    expect(gradle).toContain('dependsOn(buildMasqRustAndroid)');
    expect(gradle).toContain('dependsOn(verifyMasqWebViewProfilePatch)');
    expect(gradle).toContain('applicationId "com.endthecb2.masqmobile"');
    expect(gradle).toContain('abiFilters "arm64-v8a", "x86_64"');
    expect(gradle).toContain('-ffile-prefix-map=');
    expect(gradle).not.toContain('signingConfig signingConfigs.debug');
    expect(buildScript).toContain('--features node-engine');
    expect(buildScript).toContain('--remap-path-prefix=$HOME=/build/source');
    expect(buildScript).toContain('llvm-ar');
    expect(buildScript).toContain('llvm-ranlib');
    expect(buildScript).toContain('-Clink-arg=-Wl,-z,defs');
    expect(buildScript.match(/--link-builtins/g)).toHaveLength(2);
    expect(buildScript).toContain(
      'verify-android-native-elf.js" --jni-dir "$OUTPUT_DIR"',
    );
    expect(buildScript).toContain("-name 'libsysinfo-*.so'");
    expect(buildScript).toContain("-name 'libtun2proxy-*.so'");
    expect(buildScript).not.toContain("! -name 'libmasq_mobile_core.so'");
    expect(buildScript).toContain('cd "$(dirname "$MANIFEST")"');
    expect(buildScript).toContain(
      'RUSTC="$(rustup which --toolchain "$RUST_TOOLCHAIN" rustc)"',
    );
    expect(buildScript.indexOf('--manifest-path')).toBeLessThan(
      buildScript.indexOf('\n  build'),
    );
    expect(launchVerifier).toContain(
      '#[cfg(any(target_os = "linux", target_os = "android"))]',
    );
  });

  it('uses dev2 only for Debug and requires an explicit Release node-finder', () => {
    expect(gradle).toContain(
      'def debugNodeFinderUrl = configuredNodeFinderUrl ?: "https://dev2.api.masq.ai"',
    );
    expect(gradle).toContain(
      'def releaseNodeFinderUrl = configuredNodeFinderUrl ?: ""',
    );
    expect(gradle).toContain('tasks.register("validateReleaseNodeFinderUrl")');
    expect(gradle).toContain('dependsOn(validateReleaseNodeFinderUrl)');
    expect(gradle).toContain('parsed.rawUserInfo == null');
    expect(gradle).toContain('parsed.rawQuery == null');
    expect(gradle).toContain('parsed.rawFragment == null');
  });

  it('builds direct-distribution APKs with pinned signing and privacy gates', () => {
    const releaseBuilder = read('scripts/build-android-direct-release.sh');
    const apkVerifier = read('scripts/verify-android-apk-privacy.sh');
    const sourcePrivacy = read('scripts/verify-source-privacy.sh');

    expect(releaseBuilder).toContain('MASQ_ANDROID_EXPECTED_CERT_SHA256');
    expect(releaseBuilder).toContain(
      'APPROVED_CERT_SHA256="346611622A6BCC187C0D31F54B2EF74903F830086FB17770F65016929DFE9F41"',
    );
    expect(releaseBuilder).toContain('ALLOW_DEVELOPMENT_NODE_FINDER=YES');
    expect(releaseBuilder).toContain(
      'MASQ_ANDROID_KEY_ALIAS="${MASQ_ANDROID_KEY_ALIAS:-masq-mobile-preview2}"',
    );
    expect(releaseBuilder).toContain('SIGNED_TEMP_APK');
    expect(releaseBuilder).toContain('MASQ_ANDROID_EXPECTED_VERSION_CODE');
    expect(releaseBuilder).toContain('--v4-signing-enabled false');
    expect(releaseBuilder).toContain(
      'unset MASQ_ANDROID_KEY_PASSWORD MASQ_ANDROID_KEYSTORE_PASSWORD',
    );
    expect(releaseBuilder).not.toContain(
      'export MASQ_ANDROID_KEY_PASSWORD MASQ_ANDROID_KEYSTORE_PASSWORD',
    );
    expect(
      releaseBuilder.indexOf(
        'unset MASQ_ANDROID_KEY_PASSWORD MASQ_ANDROID_KEYSTORE_PASSWORD',
      ),
    ).toBeLessThan(releaseBuilder.indexOf('./gradlew clean assembleRelease'));
    expect(releaseBuilder).toContain(
      'MASQ_ANDROID_KEYSTORE_PASSWORD="$KEYSTORE_PASSWORD_VALUE"',
    );
    expect(releaseBuilder).toContain('ci\\.invalid\\.example');
    expect(apkVerifier).toContain('com.endthecb2.masqmobile');
    expect(apkVerifier).toContain('MASQ_ANDROID_EXPECTED_VERSION_CODE');
    expect(apkVerifier).toContain('CN=Android Debug');
    expect(apkVerifier).toContain('--verbose --print-certs --Werr');
    expect(apkVerifier).toContain('/Users/[A-Za-z0-9._-]+');
    expect(apkVerifier).toContain(
      'verify-android-native-elf.js" --apk "$APK_PATH"',
    );
    expect(sourcePrivacy).toContain("--glob '!.git'");
    expect(mobileCi).toContain(
      'Verify Android native linkage in the assembled APK',
    );
    expect(mobileCi).toContain('npm run verify:android:native:apk');
  });

  it('protects wallet entry and app previews from Android screen capture', () => {
    expect(mainActivity).toContain('WindowManager.LayoutParams.FLAG_SECURE');
    expect(mainActivity).toContain('window.addFlags');
  });

  it('supports explicit blocked, MASQ, and direct WebView routing modes', () => {
    const masqRouting = moduleSource.slice(
      moduleSource.indexOf(
        'private fun applyMasqBrowserRouting(request: BrowserRoutingRequest)',
      ),
      moduleSource.indexOf(
        'private fun applyDirectBrowserRouting(request: BrowserRoutingRequest)',
      ),
    );
    const directRouting = moduleSource.slice(
      moduleSource.indexOf(
        'private fun applyDirectBrowserRouting(request: BrowserRoutingRequest)',
      ),
      moduleSource.indexOf('private fun failBrowserRoutingClosed('),
    );

    expect(moduleSource).toContain('WebViewFeature.PROXY_OVERRIDE');
    expect(moduleSource).not.toContain(
      'com.facebook.fbreact.specs.NativeMasqCoreSpec',
    );
    expect(moduleSource).toContain(
      'override fun setBrowserRoutingMode(mode: String, promise: Promise)',
    );
    expect(moduleSource).toContain(
      'mode != "blocked" && mode != "masq" && mode != "direct"',
    );
    expect(moduleSource).toContain('.addProxyRule(BLOCKED_BROWSER_PROXY)');
    expect(moduleSource).toContain(
      '.addProxyRule("http://127.0.0.1:$proxyPort")',
    );
    expect(moduleSource).not.toContain('.addDirect()');
    expect(moduleSource).toContain(
      'ProxyController.getInstance().clearProxyOverride(callbackExecutor)',
    );
    expect(moduleSource).toContain('tunnelStatus.optBoolean("active", false)');
    expect(directRouting).toContain(
      'if (tunnelPhase != "off" || tunnelActive)',
    );
    expect(vpnService).toContain(
      '"The MASQ packet translator stopped. Traffic remains blocked.",',
    );
    expect(vpnService).toContain('"blocked",\n              true,');
    expect(moduleSource).toContain('MasqCoreJni.nativeSetProxyEnabled(false)');
    expect(moduleSource).toContain('MasqCoreJni.nativeSetProxyEnabled(true)');
    expect(moduleSource).toContain('browserRoutingQueue');
    expect(masqRouting).toContain(
      'onReady = { applyMasqBrowserRoutingAfterBlock(request) }',
    );
    expect(masqRouting.indexOf('installBlockedBrowserState(')).toBeLessThan(
      masqRouting.indexOf('MasqCoreJni.nativeGetStatus()'),
    );
    expect(moduleSource).not.toContain('override fun setBrowserProxy(');
    expect(nativeSpec).toContain(
      'setBrowserRoutingMode(mode: string): Promise<string>',
    );
    expect(coreFacade).toContain("serialized !== 'blocked'");
    expect(coreFacade).toContain("serialized !== 'masq'");
    expect(coreFacade).toContain("serialized !== 'direct'");
    expect(coreFacade).toContain(
      'The native core returned an invalid browser routing mode.',
    );
    expect(moduleSource).not.toContain('verifyExit');
    expect(moduleSource).not.toContain('cdn-cgi/trace');
  });

  it('blocks top-level non-GET WebView requests before Android can submit them', () => {
    expect(webViewClientSource).toContain(
      'shouldInterceptRequest(WebView view, WebResourceRequest request)',
    );
    expect(webViewClientSource).toContain('request.isForMainFrame()');
    expect(webViewClientSource).toContain(
      '!"GET".equalsIgnoreCase(request.getMethod())',
    );
    expect(webViewClientSource).toContain(
      'Collections.singletonMap("Cache-Control", "no-store")',
    );
    expect(webViewClientSource).toContain(
      'new ByteArrayInputStream(blockedBody)',
    );
  });

  it('clears Android WebView cookies and website storage in blocked mode', () => {
    const blockedRouting = moduleSource.slice(
      moduleSource.indexOf('private fun installBlockedBrowserState('),
      moduleSource.indexOf(
        'private fun applyMasqBrowserRouting(request: BrowserRoutingRequest)',
      ),
    );
    const websiteDataCleanup = moduleSource.slice(
      moduleSource.indexOf('private fun clearBrowserWebsiteData('),
      moduleSource.indexOf('private fun syncCoreBrowserProxy('),
    );
    const failClosedRecovery = moduleSource.slice(
      moduleSource.indexOf('private fun failBrowserRoutingClosed('),
      moduleSource.indexOf('private fun clearBrowserWebsiteData('),
    );

    expect(moduleSource).toContain('import android.webkit.CookieManager');
    expect(moduleSource).toContain('import android.webkit.WebStorage');
    expect(blockedRouting).toContain(
      'clearBrowserWebsiteData(onReady, onError)',
    );
    expect(websiteDataCleanup).toContain(
      'profile?.webStorage ?: WebStorage.getInstance()',
    );
    expect(websiteDataCleanup).toContain('webStorage.deleteAllData()');
    expect(websiteDataCleanup).toContain('cookieManager.removeAllCookies');
    expect(websiteDataCleanup).toContain('cookieManager.flush()');
    expect(websiteDataCleanup).toContain('onError(error)');
    expect(websiteDataCleanup).toContain('return@removeAllCookies');
    expect(failClosedRecovery).toContain('installBlockedBrowserState(');
  });

  it('isolates remembered sign-ins by exact host and MASQ/direct profile when supported', () => {
    expect(moduleSource).toContain('WebViewFeature.MULTI_PROFILE');
    expect(moduleSource).toContain('ProfileStore.getInstance()');
    expect(moduleSource).toContain('persistentSessionsSupported');
    expect(moduleSource).toContain(
      'override fun getBrowserSiteSettings(mode: String, hostname: String, promise: Promise)',
    );
    expect(moduleSource).toContain('override fun setBrowserSiteSettings(');
    expect(moduleSource).toContain('override fun clearBrowserSiteData(');
    expect(moduleSource).toContain('override fun clearRememberedBrowserData(');
    expect(moduleSource).toContain('sha256("site:$hostname")');
    expect(moduleSource).toContain('temporaryBrowserProfileName("masq")');
    expect(moduleSource).toContain('temporaryBrowserProfileName("direct")');
    expect(moduleSource).toContain(
      'clearRememberedBrowserStorage(clearProtectionExceptions = true)',
    );
    expect(webViewPatch).toContain(
      'WebViewCompat.setProfile(webView, profileName)',
    );
    expect(webViewPatch).toContain(
      'WebViewFeature.isFeatureSupported(WebViewFeature.MULTI_PROFILE)',
    );
    expect(webViewPatch).toContain('settings.savePassword = false');
    expect(webViewPatch).toContain('settings.saveFormData = false');
    expect(moduleSource).not.toContain('.getCookie(');
    expect(nativeSpec).toContain(
      'getBrowserSiteSettings(mode: string, hostname: string): Promise<string>',
    );
    expect(coreFacade).toContain('persistentSessionsSupported: false');
  });

  it('packages the Android system packet tunnel behind a public safety gate', () => {
    const buildScript = read('scripts/build-rust-android.sh');
    const systemTunnel = read('src/core/systemTunnel.ts');

    expect(manifest).toContain('android.permission.BIND_VPN_SERVICE');
    expect(manifest).toContain('android.net.VpnService');
    expect(manifest).toContain('android.net.VpnService.SUPPORTS_ALWAYS_ON');
    expect(manifest).toContain('android:value="false"');
    expect(gradle).toContain(
      'buildConfigField "boolean", "MASQ_SYSTEM_TUNNEL_ENABLED", "false"',
    );
    expect(systemTunnel).toContain(
      'export const SYSTEM_TUNNEL_PUBLICLY_ENABLED = false',
    );
    expect(vpnService).toContain('BuildConfig.MASQ_SYSTEM_TUNNEL_ENABLED &&');
    expect(moduleSource).toContain('"E_VPN_PREVIEW_DISABLED"');
    expect(moduleSource).toContain(
      'if (!BuildConfig.MASQ_SYSTEM_TUNNEL_ENABLED)',
    );
    expect(vpnService).toContain('.addRoute("0.0.0.0", 0)');
    expect(vpnService).toContain('.addRoute("::", 0)');
    expect(vpnService).toContain(
      'builder.addDisallowedApplication(packageName)',
    );
    expect(vpnService).toContain(
      'selectedApps.forEach(builder::addAllowedApplication)',
    );
    expect(vpnService).toContain('Traffic remains blocked');
    expect(packetJni).toContain('System.loadLibrary("masq_packet_tunnel")');
    expect(buildScript).toContain('native/masq-packet-tunnel/Cargo.toml');
    expect(buildScript).toContain('--package masq-packet-tunnel');
    expect(moduleSource).toContain(
      'VpnService.prepare(reactApplicationContext)',
    );
  });

  it('waits for confirmed Android VPN descriptor shutdown before resolving off', () => {
    const stopDispatch = moduleSource.slice(
      moduleSource.indexOf('private fun stopSystemTunnel(promise: Promise)'),
      moduleSource.indexOf(
        'override fun setBrowserRoutingMode(mode: String, promise: Promise)',
      ),
    );
    const serviceStop = vpnService.slice(
      vpnService.indexOf('private fun stopTunnel('),
      vpnService.indexOf('private fun notification('),
    );

    expect(moduleSource).not.toContain(
      'promise.resolve(MasqVpnService.markOff())',
    );
    expect(stopDispatch).toContain(
      'MasqVpnService.registerStopAcknowledgement(requestId)',
    );
    expect(stopDispatch).toContain(
      '.putExtra(MasqVpnService.EXTRA_STOP_REQUEST_ID, requestId)',
    );
    expect(stopDispatch).toContain('stopAcknowledgementExecutor.schedule(');
    expect(stopDispatch).toContain('E_VPN_STOP_TIMEOUT');
    expect(stopDispatch).toContain('E_VPN_STOP_DISPATCH');
    expect(stopDispatch).toContain('if (dispatched == null)');
    expect(vpnService).toContain(
      'intent.getLongExtra(EXTRA_STOP_REQUEST_ID, NO_STOP_REQUEST)',
    );
    expect(vpnService).toContain(
      'stopTunnel(requestId.takeIf { it != NO_STOP_REQUEST })',
    );
    expect(serviceStop).toContain('descriptor?.close()');
    expect(serviceStop.indexOf('descriptor?.close()')).toBeLessThan(
      serviceStop.indexOf('updateStatus("off"'),
    );
    expect(serviceStop.indexOf('updateStatus("off"')).toBeLessThan(
      serviceStop.indexOf('acknowledgeStop(it, status, null)'),
    );
    expect(vpnService).toContain('currentPhase = "blocked"');
    expect(vpnService).toContain('currentActive = true');
    expect(vpnService).toContain('stopAcknowledgements.remove(requestId)');
  });

  it('cleans up Android native executors and pending stop acknowledgements', () => {
    const invalidation = moduleSource.slice(
      moduleSource.indexOf('override fun invalidate()'),
      moduleSource.indexOf(
        'override fun setBrowserRoutingMode(mode: String, promise: Promise)',
      ),
    );

    expect(moduleSource).toContain(
      'pendingTunnelStops = mutableMapOf<Long, PendingTunnelStop>()',
    );
    expect(moduleSource).toContain(
      'operation.completed.compareAndSet(false, true)',
    );
    expect(invalidation).toContain(
      'MasqVpnService.cancelStopAcknowledgement(operation.requestId)',
    );
    expect(invalidation).toContain(
      'operation.timeoutFuture.getAndSet(null)?.cancel(false)',
    );
    expect(invalidation).toContain('stopAcknowledgementExecutor.shutdownNow()');
    expect(invalidation).not.toContain('MasqCoreLifecycle.executor.shutdown');
    expect(invalidation).toContain('super.invalidate()');
  });

  it('exposes an acknowledged full core teardown for explicit direct browsing', () => {
    expect(nativeSpec).toContain('shutdown(): Promise<string>');
    expect(moduleSource).toContain('override fun shutdown(promise: Promise)');
    expect(moduleSource).toContain('MasqCoreJni.nativeShutdown()');
    expect(coreJni).toContain('external fun nativeShutdown(): String');
    expect(rustAndroid).toContain(
      'Java_com_masqmobile_MasqCoreJni_nativeShutdown',
    );
    expect(rustCore).toContain('pub fn shutdown(&mut self)');
  });

  it('persists the wallet only as Android Keystore encrypted ciphertext', () => {
    expect(walletStore).toContain('AndroidKeyStore');
    expect(walletStore).toContain('AES/GCM/NoPadding');
    expect(walletStore).toContain('setRandomizedEncryptionRequired(true)');
    expect(walletStore).toContain('Base64.encodeToString(encrypted');
    expect(walletStore).toContain('.commit()');
    expect(moduleSource).toContain('walletStore.save(privateKey)');
    expect(moduleSource).toContain('val savedWallet = walletStore.load()');
    expect(moduleSource).toContain('walletStore.delete()');
  });

  it('keeps encrypted wallet storage untouched during network-profile recovery', () => {
    const networkReset = moduleSource.slice(
      moduleSource.indexOf(
        'override fun resetNetworkProfile(promise: Promise)',
      ),
      moduleSource.indexOf('override fun removeWallet(promise: Promise)'),
    );
    const rustNetworkReset = rustCore.slice(
      rustCore.indexOf('pub fn reset_network_profile(&mut self)'),
      rustCore.indexOf('pub fn remove_wallet(&mut self)'),
    );

    expect(networkReset).toContain('.remove(SAVED_CONFIG_KEY)');
    expect(networkReset).toContain('MasqCoreJni.nativeResetNetworkProfile()');
    expect(
      networkReset.match(/walletStore\.readForPreservation\(\)/g),
    ).toHaveLength(2);
    expect(networkReset).not.toContain('walletStore.load()');
    expect(networkReset).toContain(
      'MasqCoreJni.nativeImportWallet(savedWalletAfterReset)',
    );
    expect(networkReset).toContain(
      'walletSecretsMatch(savedWalletBeforeReset, savedWalletAfterReset)',
    );
    expect(networkReset).toContain(
      'isExactNetworkProfileResetStatus(finalStatus)',
    );
    expect(networkReset).toContain(
      'isExactNetworkProfileResetStatus(importStatus)',
    );
    expect(networkReset).toContain(
      'finalWalletAddress != importedWalletAddress',
    );
    expect(networkReset).toContain('rejectWalletPreservation(promise');
    expect(networkReset).toContain('rejectNetworkProfileReset(promise');
    expect(moduleSource).toContain('"E_WALLET_PRESERVATION"');
    expect(moduleSource).toContain('"E_NETWORK_PROFILE_RESET"');
    expect(moduleSource).toContain('routeStage.toDouble() == 0.0');
    expect(moduleSource).toContain('routeHops.toDouble() == 0.0');
    expect(moduleSource).toContain('minHops.toDouble() == 1.0');
    expect(moduleSource).toContain('availableExitCountries.length() == 0');
    expect(networkReset).not.toContain('walletStore.delete()');
    expect(rustNetworkReset).toContain('self.config = None');
    expect(rustNetworkReset).not.toContain('self.wallet = None');
  });

  it('reads wallet preservation state without deleting unreadable ciphertext', () => {
    const preservationRead = walletStore.slice(
      walletStore.indexOf('fun readForPreservation()'),
      walletStore.indexOf('fun delete()'),
    );
    const ordinaryLoad = walletStore.slice(
      walletStore.indexOf('fun load()'),
      walletStore.indexOf('fun readForPreservation()'),
    );

    expect(walletStore).toContain('sealed class PreservationRead');
    expect(walletStore).toContain(
      'override fun toString(): String = "Readable([REDACTED])"',
    );
    expect(walletStore).not.toContain('data class Readable');
    expect(preservationRead).toContain(
      'encryptedValue == null && initializationVector == null',
    );
    expect(preservationRead).toContain(
      'encryptedValue == null || initializationVector == null',
    );
    expect(preservationRead).toContain('?: return PreservationRead.Unreadable');
    expect(preservationRead).toContain('catch (_: Exception)');
    expect(preservationRead.match(/PreservationRead\.Unreadable/g)?.length).toBeGreaterThanOrEqual(
      4,
    );
    expect(preservationRead).not.toContain('deleteEncryptedValue()');
    expect(preservationRead).not.toContain('walletStore.delete()');
    expect(ordinaryLoad).toContain('PreservationRead.Unreadable');
    expect(ordinaryLoad).toContain('throw UnreadableException()');
    expect(ordinaryLoad).not.toContain('deleteEncryptedValue()');
  });

  it('surfaces unreadable encrypted wallet storage without inviting overwrite', () => {
    const getStatus = moduleSource.slice(
      moduleSource.indexOf('override fun getStatus(promise: Promise)'),
      moduleSource.indexOf('override fun getNetworkStatus(promise: Promise)'),
    );
    const restore = moduleSource.slice(
      moduleSource.indexOf('private fun restoreCoreIfNeeded()'),
      moduleSource.indexOf('private fun prepareNativeConfig('),
    );

    expect(getStatus).toContain('E_WALLET_STORAGE_UNREADABLE');
    expect(getStatus).toContain('without resetting or re-importing the wallet');
    expect(restore).toContain('val savedWallet = walletStore.load()');
    expect(restore).toContain('restoreAttempted = false');
    expect(restore).toContain('throw error');
    expect(restore).not.toContain('runCatching');
  });

  it('rejects unreadable saved Android profiles with one stable error contract', () => {
    const getSavedConfiguration = moduleSource.slice(
      moduleSource.indexOf(
        'override fun getSavedConfiguration(promise: Promise)',
      ),
      moduleSource.indexOf('override fun configure('),
    );

    expect(getSavedConfiguration).toContain('migrateConfig(saved)');
    expect(getSavedConfiguration).toContain('.commit()');
    expect(getSavedConfiguration).toContain(
      'MasqCoreLifecycle.executor.execute',
    );
    expect(getSavedConfiguration.match(/E_SAVED_CONFIG_INVALID/g)).toHaveLength(
      2,
    );
    expect(moduleSource).toContain(
      '"The saved MASQ network profile is invalid."',
    );
  });

  it('refreshes and reachability-tests entry nodes before starting', () => {
    expect(discovery).toContain('NODE_FINDER_ATTEMPTS = 6');
    expect(discovery).toContain('Socket().use');
    expect(discovery).toContain('saveCached(chain, reachable)');
    expect(moduleSource).toContain(
      'entryNodeDiscovery.discover(chain, preferredNodes)',
    );
    expect(moduleSource).toContain('E_ENTRY_NODE_DISCOVERY');
  });

  it('serializes start cancellation and final native stop state', () => {
    const start = moduleSource.slice(
      moduleSource.indexOf('override fun start(promise: Promise)'),
      moduleSource.indexOf('override fun stop(promise: Promise)'),
    );
    const stop = moduleSource.slice(
      moduleSource.indexOf('override fun stop(promise: Promise)'),
      moduleSource.indexOf('override fun shutdown(promise: Promise)'),
    );
    const shutdown = moduleSource.slice(
      moduleSource.indexOf('override fun shutdown(promise: Promise)'),
      moduleSource.indexOf('private fun invalidatePendingStarts()'),
    );
    const resolution = moduleSource.slice(
      moduleSource.indexOf('private fun resolveStart('),
      moduleSource.indexOf('private fun rejectStart('),
    );
    const rejection = moduleSource.slice(
      moduleSource.indexOf('private fun rejectStart('),
      moduleSource.indexOf('override fun reset(promise: Promise)'),
    );
    const configureIndex = start.indexOf('MasqCoreJni.nativeConfigure');
    const configuredStartIndex = start.indexOf(
      'MasqCoreJni.nativeStart()',
      configureIndex,
    );

    expect(moduleSource).toContain('private object MasqCoreLifecycle');
    expect(moduleSource).toContain('val startGeneration = AtomicLong(0L)');
    expect(moduleSource).toContain(
      'val executor = Executors.newSingleThreadExecutor()',
    );
    expect(start).toContain(
      'MasqCoreLifecycle.startGeneration.incrementAndGet()',
    );
    expect(start).toContain('catch (_: StaleStartException)');
    expect(start).not.toContain(
      'promise.resolve(MasqCoreJni.nativeStart())',
    );
    expect(start.indexOf('requireCurrentStart(generation)')).toBeLessThan(
      start.indexOf('entryNodeDiscovery.discover'),
    );
    expect(
      start.indexOf(
        'requireCurrentStart(generation)',
        start.indexOf('entryNodeDiscovery.discover'),
      ),
    ).toBeLessThan(start.indexOf('MasqCoreJni.nativeConfigure'));
    expect(
      start.indexOf(
        'requireCurrentStart(generation)',
        configureIndex,
      ),
    ).toBeLessThan(configuredStartIndex);
    expect(
      resolution.indexOf(
        'MasqCoreLifecycle.startGeneration.get() != generation',
      ),
    ).toBeLessThan(
      resolution.indexOf('preferences.edit()'),
    );
    expect(configuredStartIndex).toBeLessThan(
      start.indexOf('resolveStart(generation, promise, started, refreshedConfig)'),
    );
    expect(stop.indexOf('invalidatePendingStarts()')).toBeLessThan(
      stop.indexOf('MasqCoreLifecycle.executor.execute'),
    );
    expect(stop.indexOf('MasqCoreLifecycle.executor.execute')).toBeLessThan(
      stop.indexOf('MasqCoreJni.nativeStop()'),
    );
    expect(shutdown.indexOf('invalidatePendingStarts()')).toBeLessThan(
      shutdown.indexOf('MasqCoreLifecycle.executor.execute'),
    );
    expect(
      shutdown.indexOf('MasqCoreLifecycle.executor.execute'),
    ).toBeLessThan(
      shutdown.indexOf('MasqCoreJni.nativeShutdown()'),
    );
    expect(rejection).toContain(
      'promise.reject("E_CORE_START_CANCELLED", START_CANCELLED_MESSAGE)',
    );
    expect(rejection).toContain('} else if (error == null) {');
  });

  it('serializes destructive Android actions behind one process-global start fence', () => {
    const reset = moduleSource.slice(
      moduleSource.indexOf('override fun reset(promise: Promise)'),
      moduleSource.indexOf(
        'override fun resetNetworkProfile(promise: Promise)',
      ),
    );
    const networkReset = moduleSource.slice(
      moduleSource.indexOf(
        'override fun resetNetworkProfile(promise: Promise)',
      ),
      moduleSource.indexOf('override fun removeWallet(promise: Promise)'),
    );
    const removeWallet = moduleSource.slice(
      moduleSource.indexOf('override fun removeWallet(promise: Promise)'),
      moduleSource.indexOf('override fun preflightBrowserProxy('),
    );

    for (const operation of [reset, networkReset, removeWallet]) {
      expect(operation.indexOf('invalidatePendingStarts()')).toBeLessThan(
        operation.indexOf('MasqCoreLifecycle.executor.execute'),
      );
    }
    expect(reset.indexOf('MasqCoreLifecycle.executor.execute')).toBeLessThan(
      reset.indexOf('MasqCoreJni.nativeReset()'),
    );
    expect(
      networkReset.indexOf('MasqCoreLifecycle.executor.execute'),
    ).toBeLessThan(
      networkReset.indexOf('MasqCoreJni.nativeResetNetworkProfile()'),
    );
    expect(
      removeWallet.indexOf('MasqCoreLifecycle.executor.execute'),
    ).toBeLessThan(
      removeWallet.indexOf('MasqCoreJni.nativeRemoveWallet()'),
    );
    expect(moduleSource).not.toContain('private val ioExecutor');
  });

  it('keeps slow entry-node discovery off the process-global lifecycle executor', () => {
    const startSnapshot = moduleSource.slice(
      moduleSource.indexOf('override fun start(promise: Promise)'),
      moduleSource.indexOf('private fun completeStartAfterDiscovery('),
    );
    const discoveryCompletion = moduleSource.slice(
      moduleSource.indexOf('private fun completeStartAfterDiscovery('),
      moduleSource.indexOf('override fun stop(promise: Promise)'),
    );

    expect(moduleSource).toContain(
      'private val discoveryExecutor = Executors.newSingleThreadExecutor()',
    );
    expect(startSnapshot).toContain('MasqCoreLifecycle.executor.execute');
    expect(startSnapshot).toContain('discoveryExecutor.execute');
    expect(startSnapshot).not.toContain('entryNodeDiscovery.discover(');
    expect(discoveryCompletion.indexOf('entryNodeDiscovery.discover(')).toBeLessThan(
      discoveryCompletion.indexOf('MasqCoreLifecycle.executor.execute'),
    );
    expect(
      discoveryCompletion.indexOf('MasqCoreLifecycle.executor.execute'),
    ).toBeLessThan(
      discoveryCompletion.indexOf('MasqCoreJni.nativeConfigure('),
    );
    expect(discoveryCompletion).toContain(
      'synchronized(MasqCoreLifecycle.lock)',
    );
    expect(moduleSource).toContain('discoveryExecutor.shutdownNow()');
  });

  it('serializes restore and direct core mutations across Android module instances', () => {
    const getStatus = moduleSource.slice(
      moduleSource.indexOf('override fun getStatus(promise: Promise)'),
      moduleSource.indexOf('override fun getNetworkStatus(promise: Promise)'),
    );
    const configure = moduleSource.slice(
      moduleSource.indexOf('override fun configure('),
      moduleSource.indexOf('override fun importWallet('),
    );
    const importWallet = moduleSource.slice(
      moduleSource.indexOf('override fun importWallet('),
      moduleSource.indexOf('override fun updateMinHops('),
    );
    const updateMinHops = moduleSource.slice(
      moduleSource.indexOf('override fun updateMinHops('),
      moduleSource.indexOf('override fun start('),
    );
    const setSystemTunnel = moduleSource.slice(
      moduleSource.indexOf('override fun setSystemTunnel('),
      moduleSource.indexOf('private fun stopSystemTunnel('),
    );

    for (const operation of [
      getStatus,
      configure,
      importWallet,
      updateMinHops,
      setSystemTunnel,
    ]) {
      expect(operation).toContain('MasqCoreLifecycle.executor.execute');
    }
    expect(getStatus).toContain('E_CORE_RESTORE');
    expect(setSystemTunnel.indexOf('MasqCoreLifecycle.executor.execute')).toBeLessThan(
      setSystemTunnel.indexOf('restoreCoreIfNeeded()'),
    );
    expect(moduleSource).toContain('E_VPN_STALE_CORE');
    for (const operation of [configure, importWallet, updateMinHops]) {
      expect(operation.indexOf('invalidatePendingStarts()')).toBeLessThan(
        operation.indexOf('MasqCoreLifecycle.executor.execute'),
      );
    }
  });

  it('validates destructive Android terminal state before deleting durable data', () => {
    const reset = moduleSource.slice(
      moduleSource.indexOf('override fun reset(promise: Promise)'),
      moduleSource.indexOf(
        'override fun resetNetworkProfile(promise: Promise)',
      ),
    );
    const removeWallet = moduleSource.slice(
      moduleSource.indexOf('override fun removeWallet(promise: Promise)'),
      moduleSource.indexOf('override fun preflightBrowserProxy('),
    );
    const networkReset = moduleSource.slice(
      moduleSource.indexOf(
        'override fun resetNetworkProfile(promise: Promise)',
      ),
      moduleSource.indexOf('override fun removeWallet(promise: Promise)'),
    );

    expect(reset.indexOf('if (!MasqCoreJni.isAvailable)')).toBeLessThan(
      reset.indexOf('clearRememberedBrowserStorage('),
    );
    expect(reset.indexOf('MasqCoreJni.nativeReset()')).toBeLessThan(
      reset.indexOf('walletStore.delete()'),
    );
    expect(reset.indexOf('isExactFullResetStatus(')).toBeLessThan(
      reset.indexOf('walletStore.delete()'),
    );
    expect(reset.lastIndexOf('MasqCoreJni.nativeGetStatus()')).toBeLessThan(
      reset.indexOf('promise.resolve(finalStatusJson)'),
    );
    expect(reset.indexOf('MasqCoreJni.nativeReset()')).toBeLessThan(
      reset.indexOf('clearRememberedBrowserStorage('),
    );
    expect(
      networkReset.lastIndexOf('MasqCoreJni.nativeGetStatus()'),
    ).toBeLessThan(
      networkReset.indexOf('preferences.edit().remove(SAVED_CONFIG_KEY)'),
    );
    expect(removeWallet.indexOf('if (!MasqCoreJni.isAvailable)')).toBeLessThan(
      removeWallet.indexOf('walletStore.delete()'),
    );
    expect(removeWallet.indexOf('MasqCoreJni.nativeRemoveWallet()')).toBeLessThan(
      removeWallet.indexOf('walletStore.delete()'),
    );
    expect(removeWallet.indexOf('isExactWalletRemovalStatus(')).toBeLessThan(
      removeWallet.indexOf('walletStore.delete()'),
    );
    expect(removeWallet.lastIndexOf('MasqCoreJni.nativeGetStatus()')).toBeLessThan(
      removeWallet.indexOf('promise.resolve(finalStatusJson)'),
    );
  });

  it('rejects missing or malformed Android core phases as unsuccessful', () => {
    const statusSucceeded = moduleSource.slice(
      moduleSource.indexOf('private fun statusSucceeded('),
      moduleSource.indexOf('private fun nullableStatusString('),
    );

    expect(statusSucceeded).toContain('val phase = JSONObject(statusJson).opt("phase")');
    expect(statusSucceeded).toContain('phase is String');
    expect(statusSucceeded).toContain('phase in');
    expect(statusSucceeded).not.toContain('optString("phase") != "error"');
  });

  it('fences stale Android browser proxy callbacks behind the core lifecycle', () => {
    const browserRouting = moduleSource.slice(
      moduleSource.indexOf('override fun setBrowserRoutingMode('),
      moduleSource.indexOf('private fun validateBrowserSite('),
    );
    const masqConfirmation = moduleSource.slice(
      moduleSource.indexOf('private fun confirmMasqBrowserRouting('),
      moduleSource.indexOf('private fun applyDirectBrowserRouting('),
    );
    const directRouting = moduleSource.slice(
      moduleSource.indexOf('private fun applyDirectBrowserRouting('),
      moduleSource.indexOf('private fun failBrowserRoutingClosed('),
    );
    const companion = moduleSource.slice(
      moduleSource.indexOf('companion object {'),
      moduleSource.indexOf('private data class BrowserSite('),
    );

    expect(browserRouting).toContain(
      'MasqCoreLifecycle.startGeneration.get()',
    );
    expect(browserRouting).toContain('requireCurrentBrowserCore(request)');
    expect(moduleSource).toContain('E_BROWSER_STALE_CORE');
    expect(browserRouting).toContain('MasqCoreLifecycle.executor.execute');
    expect(
      browserRouting.indexOf('requireCurrentBrowserCore(request)'),
    ).toBeLessThan(
      browserRouting.indexOf('MasqCoreJni.nativeSetProxyEnabled(true)'),
    );
    expect(masqConfirmation.match(/requireCurrentBrowserCore\(request\)/g))
      .toHaveLength(3);
    expect(directRouting.indexOf('requireCurrentBrowserCore(request)'))
      .toBeLessThan(
        directRouting.indexOf(
          'ProxyController.getInstance().clearProxyOverride',
        ),
      );
    expect(directRouting.match(/requireCurrentBrowserCore\(request\)/g)?.length)
      .toBeGreaterThanOrEqual(4);
    expect(companion).toContain(
      'private val browserRoutingQueue = ArrayDeque<BrowserRoutingRequest>()',
    );
    expect(moduleSource).toContain('next?.owner?.applyBrowserRoutingMode(next)');
  });

  it('persists only strict Android browser protection preferences', () => {
    expect(moduleSource).toContain(
      'override fun prepareBrowserProtection(promise: Promise)',
    );
    expect(moduleSource).toContain(
      'override fun setBrowserProtection(configJson: String, promise: Promise)',
    );
    expect(moduleSource).toContain('fields != BROWSER_PROTECTION_FIELDS');
    expect(moduleSource).toContain('config.opt(field) !is Boolean');
    expect(moduleSource).toContain(
      'preferences.getBoolean(BLOCK_ADS_AND_TRACKERS_KEY, true)',
    );
    expect(moduleSource).toContain(
      'preferences.getBoolean(BLOCK_CROSS_SITE_COOKIES_KEY, true)',
    );
    expect(moduleSource).toContain(
      'preferences.getBoolean(HIDE_COOKIE_BANNERS_KEY, false)',
    );
    expect(moduleSource).toContain(
      'preferences.getBoolean(REJECT_OPTIONAL_COOKIES_KEY, false)',
    );
    expect(moduleSource).toContain(
      'preferences.getBoolean(YOUTUBE_BEST_EFFORT_KEY, false)',
    );
    expect(moduleSource).toContain(
      '.putBoolean(BLOCK_ADS_AND_TRACKERS_KEY, blockAdsAndTrackers)',
    );
    expect(moduleSource).toContain(
      '.putBoolean(BLOCK_CROSS_SITE_COOKIES_KEY, blockCrossSiteCookies)',
    );
    expect(moduleSource).toContain(
      '.putBoolean(HIDE_COOKIE_BANNERS_KEY, hideCookieBanners)',
    );
    expect(moduleSource).toContain(
      '.putBoolean(REJECT_OPTIONAL_COOKIES_KEY, rejectOptionalCookies)',
    );
    expect(moduleSource).toContain(
      '.putBoolean(YOUTUBE_BEST_EFFORT_KEY, youtubeBestEffort)',
    );
    expect(moduleSource).toContain(
      '.put("rejectOptionalCookies", rejectOptionalCookies)',
    );
    expect(moduleSource).toContain('.put("nativeRequestBlocking", false)');
    expect(moduleSource).toContain('.put("youtubeBestEffortAvailable", false)');
  });
});
