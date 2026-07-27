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

    expect(releaseBuilder).toContain('MASQ_ANDROID_EXPECTED_CERT_SHA256');
    expect(releaseBuilder).toContain('ALLOW_DEVELOPMENT_NODE_FINDER=YES');
    expect(releaseBuilder).toContain(
      'MASQ_ANDROID_KEY_ALIAS="${MASQ_ANDROID_KEY_ALIAS:-masq-mobile-preview2}"',
    );
    expect(releaseBuilder).toContain('SIGNED_TEMP_APK');
    expect(releaseBuilder).toContain('--v4-signing-enabled false');
    expect(releaseBuilder).toContain('ci\\.invalid\\.example');
    expect(apkVerifier).toContain('com.endthecb2.masqmobile');
    expect(apkVerifier).toContain('CN=Android Debug');
    expect(apkVerifier).toContain('--verbose --print-certs --Werr');
    expect(apkVerifier).toContain('/Users/[A-Za-z0-9._-]+');
    expect(apkVerifier).toContain(
      'verify-android-native-elf.js" --apk "$APK_PATH"',
    );
    expect(mobileCi).toContain(
      'Verify Android native linkage in the assembled APK',
    );
    expect(mobileCi).toContain('npm run verify:android:native:apk');
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

  it('packages a fail-closed Android system packet tunnel', () => {
    const buildScript = read('scripts/build-rust-android.sh');

    expect(manifest).toContain('android.permission.BIND_VPN_SERVICE');
    expect(manifest).toContain('android.net.VpnService');
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
    expect(invalidation).toContain('ioExecutor.shutdownNow()');
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

  it('refreshes and reachability-tests entry nodes before starting', () => {
    expect(discovery).toContain('NODE_FINDER_ATTEMPTS = 6');
    expect(discovery).toContain('Socket().use');
    expect(discovery).toContain('saveCached(chain, reachable)');
    expect(moduleSource).toContain(
      'entryNodeDiscovery.discover(chain, preferredNodes)',
    );
    expect(moduleSource).toContain('E_ENTRY_NODE_DISCOVERY');
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
