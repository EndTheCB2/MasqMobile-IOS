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
  const strings = read('android/app/src/main/res/values/strings.xml');
  const mainActivity = read(
    'android/app/src/main/java/com/masqmobile/MainActivity.kt',
  );
  const vpnService = read(
    'android/app/src/main/java/com/masqmobile/MasqVpnService.kt',
  );
  const pendingConsentStore = read(
    'android/app/src/main/java/com/masqmobile/SystemRoutingPendingConsentStore.kt',
  );
  const sessionService = read(
    'android/app/src/main/java/com/masqmobile/MasqSessionService.kt',
  );
  const packageReplacementReceiver = read(
    'android/app/src/main/java/com/masqmobile/MasqPackageReplacementReceiver.kt',
  );
  const backgroundRecovery = read(
    'android/app/src/main/java/com/masqmobile/MasqBackgroundSessionRecovery.kt',
  );
  const packetJni = read(
    'android/app/src/main/java/com/masqmobile/MasqPacketTunnelJni.kt',
  );
  const packetTranslator = read(
    'android/app/src/main/java/com/masqmobile/SystemRoutingTranslator.kt',
  );
  const packageLifecycle = read(
    'android/app/src/main/java/com/masqmobile/SystemRoutingPackageLifecycle.kt',
  );
  const terminalCoordinator = read(
    'android/app/src/main/java/com/masqmobile/SystemRoutingTerminalCoordinator.kt',
  );
  const startAuthority = read(
    'android/app/src/main/java/com/masqmobile/SystemRoutingStartAuthority.kt',
  );
  const notificationPermission = read(
    'android/app/src/main/java/com/masqmobile/SystemRoutingNotificationPermission.kt',
  );
  const routingPolicyStore = read(
    'android/app/src/main/java/com/masqmobile/SystemRoutingPolicyStore.kt',
  );
  const packetTunnelRust = read('native/masq-packet-tunnel/src/lib.rs');
  const packetTunnelLifecycle = read(
    'native/masq-packet-tunnel/src/lifecycle.rs',
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
  const webViewSource = read(
    'node_modules/react-native-webview/android/src/main/java/com/reactnativecommunity/webview/RNCWebView.java',
  );
  const webViewManagerSource = read(
    'node_modules/react-native-webview/android/src/main/java/com/reactnativecommunity/webview/RNCWebViewManagerImpl.kt',
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
    expect(coreJni).toContain('nativeConfirmDebtSettlement');
    expect(rustAndroid).toContain('nativeConfirmDebtSettlement');
    expect(moduleSource).not.toContain('BiometricPrompt');
    expect(buildScript).toContain('--remap-path-prefix=$HOME=/build/source');
    expect(buildScript).toContain(
      'MASQ_ANDROID_CARGO_TARGET_DIR must use a privacy-neutral temporary path',
    );
    expect(buildScript).toContain(
      '"$HOME/"* | "$ROOT_DIR/"* | /Users/* | /home/*',
    );
    expect(buildScript).toContain('llvm-ar');
    expect(buildScript).toContain('llvm-ranlib');
    expect(buildScript).toContain('-Clink-arg=-Wl,-z,defs');
    expect(buildScript.match(/--link-builtins/g)).toHaveLength(2);
    expect(buildScript).toContain(
      'verify-android-native-elf.js" --jni-dir "$OUTPUT_DIR"',
    );
    expect(buildScript).toContain("-name 'libsysinfo-*.so'");
    expect(buildScript).toContain("-name 'libsysinfo.so'");
    expect(buildScript).toContain("-name 'libtun2proxy-*.so'");
    expect(buildScript).toContain("-name 'libtun2proxy.so'");
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
    expect(apkVerifier).toContain('if ! command -v rg');
    expect(apkVerifier).toContain('rg --no-config');
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
    expect(vpnService).toContain('SystemRoutingDiagnostic.TRANSLATOR_RETURNED');
    expect(vpnService).toContain(
      '"Captured traffic is blocked because the community-route translator returned."',
    );
    expect(moduleSource).toContain('MasqCoreJni.nativeSetProxyEnabled(false)');
    expect(moduleSource).toContain('MasqCoreJni.nativeSetProxyEnabled(true)');
    expect(moduleSource).toContain('browserRoutingQueue');
    expect(masqRouting).toContain(
      'onReady = { applyMasqBrowserRoutingAfterBlock(request) }',
    );
    expect(masqRouting.indexOf('installBlockedBrowserState(')).toBeLessThan(
      masqRouting.indexOf('MasqCoreJni.nativeGetStatus()'),
    );
    expect(masqRouting).toContain('status.optInt("connectedNeighbors", 0) < 1');
    expect(masqRouting).toContain('status.optInt("routeStage", 0) < 2');
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

  it('blocks every top-level non-GET WebView request before Android can follow an opaque redirect', () => {
    expect(webViewClientSource).toContain(
      'shouldInterceptRequest(WebView view, WebResourceRequest request)',
    );
    expect(webViewClientSource).toContain('request.isForMainFrame()');
    expect(webViewClientSource).toContain(
      'request.isForMainFrame() && !"GET".equalsIgnoreCase(request.getMethod())',
    );
    expect(webViewClientSource).toContain(
      'Collections.singletonMap("Cache-Control", "no-store")',
    );
    expect(webViewClientSource).toContain(
      'new ByteArrayInputStream(blockedBody)',
    );
  });

  it('blocks bounded exact-host ad subresources before Android downloads them', () => {
    expect(webViewClientSource).toContain('!request.isForMainFrame()');
    expect(webViewClientSource).toContain(
      'shouldBlockResource(request.getUrl())',
    );
    expect(webViewClientSource).toContain('204');
    expect(webViewClientSource).toContain('"No Content"');
    expect(webViewSource).toContain('MAX_BLOCKED_RESOURCE_HOSTS = 64');
    expect(webViewSource).toContain('host.equals(blockedHost)');
    expect(webViewSource).toContain('host.endsWith("." + blockedHost)');
    expect(webViewSource).not.toContain('host.contains(blockedHost)');
    expect(webViewManagerSource).toContain('setAndroidBlockedResourceHosts');
    expect(webViewManagerSource).toContain('.take(64)');
    expect(webViewPatch).toContain('androidBlockedResourceHosts');
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
    expect(blockedRouting).toContain('clearBrowserWebsiteData(');
    expect(blockedRouting).toContain('onComplete = {');
    expect(blockedRouting).toContain('isBrowserRoutingRequestLive(request)');
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

  it('detaches Android WebView documents before destroying their proxy sessions', () => {
    const cleanup = webViewSource.slice(
      webViewSource.indexOf('protected void cleanupCallbacksAndDestroy()'),
      webViewSource.indexOf('\n    @Override\n    public void destroy()'),
    );

    expect(cleanup).toContain('stopLoading();');
    expect(cleanup).toContain('setWebViewClient(null);');
    expect(cleanup).toContain('loadUrl("about:blank");');
    expect(cleanup).toContain('clearHistory();');
    expect(cleanup).toContain('removeAllViews();');
    expect(cleanup.indexOf('stopLoading();')).toBeLessThan(
      cleanup.indexOf('loadUrl("about:blank");'),
    );
    expect(cleanup.indexOf('loadUrl("about:blank");')).toBeLessThan(
      cleanup.indexOf('destroy();'),
    );
    expect(webViewPatch).toContain("leave Chromium's proxy CONNECT sockets");
    expect(webViewPatch).toContain('+        loadUrl("about:blank");');
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
    const directReleaseBuilder = read(
      'scripts/build-android-direct-release.sh',
    );
    const systemTunnel = read('src/core/systemTunnel.ts');

    expect(manifest).toContain('android.permission.BIND_VPN_SERVICE');
    expect(manifest).toContain('android.net.VpnService');
    expect(manifest).toContain('android.net.VpnService.SUPPORTS_ALWAYS_ON');
    expect(manifest).toContain('android:value="false"');
    expect(manifest).toContain(
      '<package android:name="com.endthecb2.masqmobile" />',
    );
    expect(manifest).toContain(
      '<package android:name="com.endthecb2.masqmobile.dogfood" />',
    );
    expect(gradle).toContain(
      'System.getenv("MASQ_ENABLE_UNSAFE_SYSTEM_ROUTING_DOGFOOD") == "YES"',
    );
    expect(gradle).toContain(
      'def applicationLabelResource = unsafeSystemRoutingDogfoodEnabled ?',
    );
    expect(gradle).toContain('applicationId "com.endthecb2.masqmobile"');
    expect(gradle).toContain('if (unsafeSystemRoutingDogfoodEnabled) {');
    expect(gradle).toContain('applicationIdSuffix ".dogfood"');
    expect(gradle).toContain('versionNameSuffix "-dogfood"');
    expect(gradle).toContain('versionCode 6017');
    expect(gradle).toContain(
      'manifestPlaceholders = [masqAppLabel: applicationLabelResource]',
    );
    expect(manifest.match(/android:label="\$\{masqAppLabel\}"/g)).toHaveLength(
      2,
    );
    expect(strings).toContain(
      '<string name="app_name_dogfood">Masq Mobile community version</string>',
    );
    expect(strings).toContain(
      '<string name="app_name">Masq Mobile community version</string>',
    );
    expect(gradle).toContain(
      '"MASQ_SYSTEM_TUNNEL_ENABLED",\n            unsafeSystemRoutingDogfoodEnabled.toString()',
    );
    expect(gradle).not.toContain(
      'buildConfigField "boolean", "MASQ_SYSTEM_TUNNEL_ENABLED", "false"',
    );
    expect(systemTunnel).not.toContain('SYSTEM_TUNNEL_PUBLICLY_ENABLED');
    expect(directReleaseBuilder).toContain(
      'if [ "${MASQ_ENABLE_UNSAFE_SYSTEM_ROUTING_DOGFOOD:-NO}" = "YES" ]',
    );
    expect(directReleaseBuilder).toContain(
      'direct public releases cannot enable unsafe system-routing dogfood',
    );
    expect(directReleaseBuilder).toContain(
      'unset MASQ_ENABLE_UNSAFE_SYSTEM_ROUTING_DOGFOOD',
    );
    expect(
      directReleaseBuilder.indexOf(
        'if [ "${MASQ_ENABLE_UNSAFE_SYSTEM_ROUTING_DOGFOOD:-NO}" = "YES" ]',
      ),
    ).toBeLessThan(
      directReleaseBuilder.indexOf(
        ': "${MASQ_NODE_FINDER_URL:?Set MASQ_NODE_FINDER_URL',
      ),
    );
    expect(vpnService).toContain(
      'PUBLIC_MASQ_PACKAGE_ID = "com.endthecb2.masqmobile"',
    );
    expect(vpnService).toContain(
      'DOGFOOD_MASQ_PACKAGE_ID = "com.endthecb2.masqmobile.dogfood"',
    );
    expect(moduleSource).toContain(
      'if (isMasqControlPlanePackage(packageName))',
    );
    expect(moduleSource).toContain(
      'if (apps.any(::isMasqControlPlanePackage))',
    );
    expect(vpnService).toContain('builder.addDisallowedApplication(packageId)');
    expect(vpnService).toContain('BuildConfig.MASQ_SYSTEM_TUNNEL_ENABLED &&');
    expect(moduleSource).toContain('"E_VPN_PREVIEW_DISABLED"');
    expect(moduleSource).toContain(
      'if (!BuildConfig.MASQ_SYSTEM_TUNNEL_ENABLED)',
    );
    expect(vpnService).toContain('MasqTunPrefix("0.0.0.0", 0)');
    expect(vpnService).toContain('MasqTunPrefix("::", 0)');
    expect(vpnService).toContain(
      'MasqTunNetworkConfiguration.routes.forEach { prefix ->',
    );
    expect(vpnService).toContain(
      'builder.addRoute(prefix.address, prefix.prefixLength)',
    );
    expect(vpnService).toContain(
      'MASQ_CONTROL_PLANE_PACKAGE_IDS.forEach { packageId ->',
    );
    expect(vpnService).toContain(
      'policy.selectedApps.forEach(builder::addAllowedApplication)',
    );
    expect(vpnService).toContain('traffic remains blocked');
    expect(packetJni).toContain('System.loadLibrary("masq_packet_tunnel")');
    expect(buildScript).toContain('native/masq-packet-tunnel/Cargo.toml');
    expect(buildScript).toContain('--package masq-packet-tunnel');
    expect(moduleSource).toContain(
      'VpnService.prepare(reactApplicationContext)',
    );
  });

  it('exposes a generation-safe native packet-tunnel lifecycle', () => {
    expect(packetJni).toContain('external fun nativeStateJson(): String');
    expect(packetJni).toContain('START_UNEXPECTED_CLEAN_RETURN = -2');
    expect(packetTunnelRust).toContain(
      'Java_com_masqmobile_MasqPacketTunnelJni_nativeStateJson',
    );
    expect(packetTunnelRust).toContain('START_STALE_COMPLETION');
    expect(packetTunnelRust).not.toContain('TUNNEL_CANCELLATION');
    expect(packetTunnelLifecycle).toContain('struct ActiveSession');
    expect(packetTunnelLifecycle).toContain('generation: u64');
    expect(packetTunnelLifecycle).toContain('TunnelState::Starting');
    expect(packetTunnelLifecycle).toContain('TunnelState::Running');
    expect(packetTunnelLifecycle).toContain(
      'lifecycle.state = TunnelState::Stopping',
    );
    expect(packetTunnelLifecycle).toContain('lifecycle.active = None');
    expect(
      packetTunnelLifecycle.indexOf('pub(crate) fn request_stop'),
    ).toBeLessThan(packetTunnelLifecycle.indexOf('pub(crate) fn complete'));
    expect(packetTunnelLifecycle).toContain(
      'stale_completion_cannot_remove_or_cancel_the_new_generation',
    );
    expect(packetTranslator).toContain('expectedNativeGeneration');
    expect(packetTranslator).toContain('TranslatorStartResult.NativeBusy');
    expect(packetTranslator).toContain(
      'snapshot.generation == run.expectedNativeGeneration',
    );
  });

  it('pauses and atomically rebuilds package-scoped Android tunnels', () => {
    expect(vpnService).toContain('Intent.ACTION_PACKAGE_ADDED');
    expect(vpnService).toContain('Intent.ACTION_PACKAGE_REMOVED');
    expect(vpnService).toContain('Intent.ACTION_PACKAGE_REPLACED');
    expect(vpnService).toContain('Intent.ACTION_PACKAGE_CHANGED');
    expect(vpnService).toContain('ContextCompat.RECEIVER_EXPORTED');
    expect(vpnService).toContain('unregisterReceiver(packageChangeReceiver)');
    expect(packageLifecycle).toContain(
      'systemRoutingPackageChangeAffectsPolicy(',
    );
    expect(packageLifecycle).toContain('class SystemRoutingPackageChangeDrain');

    const invalidation = vpnService.slice(
      vpnService.indexOf('private fun observePackageScopeChange('),
      vpnService.indexOf('private fun drainPackageScopeChanges()'),
    );
    expect(invalidation.indexOf('localTunCaptureValid = false')).toBeLessThan(
      invalidation.indexOf('translator.requestStopWithoutRelease()'),
    );

    const rebuild = vpnService.slice(
      vpnService.indexOf('private fun rebuildInvalidatedTun('),
      vpnService.indexOf('private fun retireInvalidatedTun('),
    );
    expect(rebuild.indexOf('builder.establish()')).toBeLessThan(
      rebuild.indexOf('oldDescriptor.close()'),
    );
  });

  it('persists only a short-lived exact VPN-consent continuation', () => {
    expect(pendingConsentStore).toContain(
      'const val MAX_AGE_MS = 10 * 60 * 1000L',
    );
    expect(pendingConsentStore).toContain(
      'const val PREFERENCES_NAME = "masq-system-routing-pending-consent"',
    );
    expect(pendingConsentStore).toContain('KEY_SELECTED_APPS');
    expect(pendingConsentStore).not.toContain('KEY_WALLET');
    expect(pendingConsentStore).not.toContain('KEY_SEED');
    expect(pendingConsentStore).not.toContain('KEY_RPC');
    expect(moduleSource).toContain(
      'continueApprovedSystemTunnelConsent(persisted)',
    );
    expect(moduleSource).toContain(
      'pendingSystemRoutingConsentStore.load(System.currentTimeMillis())',
    );
    expect(moduleSource).toContain('.put("engineGeneration", 0)');
  });

  it('persists exact revisions and waits for native return before closing the VPN', () => {
    const stopDispatch = moduleSource.slice(
      moduleSource.indexOf('private fun stopSystemTunnel(promise: Promise)'),
      moduleSource.indexOf(
        'override fun setBrowserRoutingMode(mode: String, promise: Promise)',
      ),
    );
    const serviceStop = vpnService.slice(
      vpnService.indexOf('private fun handleStop('),
      vpnService.indexOf('private fun ensureBlockingTun('),
    );
    const safeClose = vpnService.slice(
      vpnService.indexOf('private fun stopAndCloseAllTunnelsSafely('),
      vpnService.indexOf('private fun publish('),
    );
    const startDispatch = moduleSource.slice(
      moduleSource.indexOf('private fun persistAndStartSystemTunnel('),
      moduleSource.indexOf('private fun ifCoreAvailable('),
    );

    expect(startDispatch).toContain(
      'systemRoutingPolicyStore.persistBeforeStart(',
    );
    expect(startDispatch).toContain('failClosedDesired = false');
    expect(startDispatch).toContain(
      'MasqVpnService.registerStartAcknowledgement(requestId)',
    );
    expect(startDispatch).not.toContain('promise.resolve(status)');
    expect(startDispatch).toContain(
      '.putExtra(MasqVpnService.EXTRA_POLICY_REVISION, policy.revision)',
    );
    expect(startDispatch).not.toContain('MasqVpnService.EXTRA_MODE');
    expect(startDispatch).not.toContain('MasqVpnService.EXTRA_APPS');
    expect(stopDispatch).toContain('systemRoutingPolicyStore.persistOff(');
    expect(stopDispatch).toContain(
      'MasqVpnService.registerStopAcknowledgement(requestId)',
    );
    expect(stopDispatch).toContain(
      '.putExtra(MasqVpnService.EXTRA_POLICY_REVISION, offPolicy.revision)',
    );
    expect(stopDispatch).toContain('stopAcknowledgementExecutor.schedule(');
    expect(stopDispatch).toContain('E_VPN_STOP_TIMEOUT');
    expect(stopDispatch).toContain('E_VPN_STOP_DISPATCH');
    expect(stopDispatch).toContain('if (dispatched == null)');
    expect(vpnService).toContain(
      'intent.getLongExtra(EXTRA_POLICY_REVISION, NO_REVISION)',
    );
    expect(serviceStop).toContain('stopAndCloseAllTunnelsSafely()');
    expect(safeClose).toContain(
      'terminalCoordinator.closeOrJoin(TRANSLATOR_STOP_TIMEOUT_MS)',
    );
    expect(safeClose).toContain('stopTranslatorSafely(');
    expect(safeClose).toContain('terminalCoordinator.retain(');
    expect(safeClose).not.toContain('descriptor?.close()');
    expect(safeClose.indexOf('terminalCoordinator.retain(')).toBeLessThan(
      safeClose.indexOf('adoptedTerminalLeaseEpoch = retainResult?.epoch'),
    );
    expect(
      safeClose.indexOf('adoptedTerminalLeaseEpoch = retainResult?.epoch'),
    ).toBeLessThan(safeClose.indexOf('tunnelDescriptor = null'));
    expect(safeClose.indexOf('terminalCoordinator.retain(')).toBeLessThan(
      safeClose.indexOf(
        'terminalCoordinator.closeOrJoin(TRANSLATOR_STOP_TIMEOUT_MS)',
      ),
    );
    expect(safeClose.indexOf('tunnelDescriptor = null')).toBeLessThan(
      safeClose.indexOf(
        'terminalCoordinator.closeOrJoin(TRANSLATOR_STOP_TIMEOUT_MS)',
      ),
    );
    expect(serviceStop.indexOf('stopAndCloseAllTunnelsSafely()')).toBeLessThan(
      serviceStop.indexOf('settleStop(it, status, null)'),
    );
    expect(packetTranslator).toContain(
      'run.future.get(remainingNanos, TimeUnit.NANOSECONDS)',
    );
    expect(packetTranslator).toContain(
      'TranslatorStopResult.TimedOutKeepBlocking',
    );
    expect(routingPolicyStore).toContain('synchronized(processLock)');
    expect(moduleSource).toContain('values.get(index) as? String');
    expect(moduleSource).not.toContain(
      '.filterNot { it == reactApplicationContext.packageName }',
    );
    expect(
      vpnService.indexOf('validateInstalledPackages(policy)'),
    ).toBeLessThan(vpnService.indexOf('val builder ='));
  });

  it('runs full reset through an acknowledged service-owned safe-close path', () => {
    const resetService = vpnService.slice(
      vpnService.indexOf('private fun handleExplicitReset('),
      vpnService.indexOf('private fun ensureBlockingTun('),
    );
    const resetModule = moduleSource.slice(
      moduleSource.indexOf('override fun reset(promise: Promise)'),
      moduleSource.indexOf(
        'override fun resetNetworkProfile(promise: Promise)',
      ),
    );
    const safeClose = vpnService.slice(
      vpnService.indexOf('private fun stopAndCloseAllTunnelsSafely('),
      vpnService.indexOf('private fun publish('),
    );

    expect(vpnService).toContain(
      'const val ACTION_RESET = "com.masqmobile.RESET_SYSTEM_TUNNEL"',
    );
    expect(resetModule).toContain('resetSystemTunnelForFullReset(promise)');
    expect(resetModule).toContain(
      'MasqVpnService.registerResetAcknowledgement(requestId)',
    );
    expect(resetModule).toContain('.setAction(MasqVpnService.ACTION_RESET)');
    expect(resetModule).toContain('finishFullReset(promise)');
    expect(resetService).toContain('policyStore.clearAfterExplicitReset()');
    expect(resetService).not.toContain('persistOff(');
    expect(resetService).toContain('terminalCoordinator.beginExplicitReset()');
    expect(resetService.indexOf('stopAndCloseAllTunnelsSafely()')).toBeLessThan(
      resetService.indexOf('policyStore.clearAfterExplicitReset()'),
    );
    expect(safeClose.indexOf('terminalCoordinator.retain(')).toBeLessThan(
      safeClose.indexOf(
        'terminalCoordinator.closeOrJoin(TRANSLATOR_STOP_TIMEOUT_MS)',
      ),
    );
    expect(
      resetService.indexOf('policyStore.clearAfterExplicitReset()'),
    ).toBeLessThan(resetService.indexOf('settleReset(it, statusJson(), null)'));
    expect(resetService).toContain('terminalCoordinator.snapshot() != null');
    expect(resetService).toContain('!processNativeReleaseConfirmed()');
    expect(resetService).toContain(
      'is SystemRoutingPolicyClearResult.IndeterminateClear',
    );
    expect(resetService).toContain('SystemRoutingTransition.BLOCKED');
  });

  it('publishes terminal truth synchronously and no-TUN only after ordered cleanup', () => {
    const destruction = vpnService.slice(
      vpnService.indexOf('override fun onDestroy()'),
      vpnService.indexOf('private fun currentAlwaysOn()'),
    );
    const publish = vpnService.slice(
      vpnService.indexOf('private fun publish('),
      vpnService.indexOf('override fun onRevoke()'),
    );

    const firstTerminalPublish = destruction.indexOf(
      'publishServiceDestroyed(',
    );
    const asyncCleanup = destruction.indexOf('controlExecutor.execute');
    const safeClose = destruction.indexOf(
      'stopAndCloseTunnelSafely()',
      asyncCleanup,
    );
    const finalPublish = destruction.indexOf(
      'publishServiceDestroyed(',
      safeClose,
    );
    expect(destruction.indexOf('destroyed = true')).toBeLessThan(
      firstTerminalPublish,
    );
    expect(firstTerminalPublish).toBeLessThan(asyncCleanup);
    expect(asyncCleanup).toBeLessThan(safeClose);
    expect(safeClose).toBeLessThan(finalPublish);
    expect(destruction).toContain('ServiceDestructionSnapshot(');
    expect(destruction).toContain('terminalCoordinator.retain(');
    expect(destruction).toContain(
      'retainedAppliedPolicy = terminalSnapshot.retainedAppliedPolicy',
    );
    expect(destruction).toContain('tunPresent = terminalSnapshot.tunPresent');
    expect(destruction).toContain(
      'captureValid = terminalSnapshot.captureValid',
    );
    expect(destruction.indexOf('terminalCoordinator.retain(')).toBeLessThan(
      destruction.indexOf('tunnelDescriptor = null'),
    );
    expect(destruction.indexOf('tunnelDescriptor = null')).toBeLessThan(
      destruction.indexOf('terminalCoordinator.snapshot()'),
    );
    expect(destruction).toContain('stopAndCloseTunnelSafely()');
    expect(destruction).toContain(
      'retainedAppliedPolicy = retainedAfterClose.first',
    );
    expect(destruction).toContain('tunPresent = retainedAfterClose.second');
    expect(destruction).toContain('settleOwnedRequests(');
    expect(destruction).not.toContain('acknowledgeAllStarts(');
    expect(destruction).not.toContain('acknowledgeAllStops(');
    expect(destruction).not.toContain('acknowledgeAllResets(');
    expect(destruction).toContain('ownerEpoch = serviceEpoch');
    expect(publish).toContain('if (destroyed) return');
    expect(vpnService).toContain('currentProxyPort = null');
    expect(vpnService).toContain('systemRoutingStatusAfterServiceDestroyed(');
  });

  it('requires semantic start acceptance under the exact core generation lock', () => {
    const startDispatch = moduleSource.slice(
      moduleSource.indexOf('private fun startSystemTunnel('),
      moduleSource.indexOf(
        '@Suppress("DEPRECATION")\n  private fun isInstalledPackage',
      ),
    );
    const activation = vpnService.slice(
      vpnService.indexOf('TranslatorReadiness.Ready ->'),
      vpnService.indexOf('private fun startTranslatorForCapturedDescriptor('),
    );

    expect(startDispatch).toContain(
      'tunnelStartAcknowledgementIsSemanticallyAccepted(',
    );
    expect(moduleSource).toContain(
      'private const val START_TUNNEL_TIMEOUT_MS = 45_000L',
    );
    expect(startDispatch).toContain(
      'currentCoreGeneration = currentCoreGeneration',
    );
    expect(startDispatch).toContain('return@completeTunnelStart false');
    expect(startDispatch).toContain('promise.resolve(serialized)');
    expect(startDispatch).toContain('\n              true');
    expect(startAuthority).toContain(
      'expectedCoreGeneration == currentCoreGeneration',
    );
    expect(startAuthority).toContain(
      'status.appliedRevision == expectedPolicyRevision',
    );
    expect(activation).toContain('synchronized(MasqCoreLifecycle.lock)');
    expect(activation).toContain('acknowledgementAccepted &&');
    const acknowledgement = activation.indexOf(
      'settleStart(it, candidateStatus, null)',
    );
    expect(acknowledgement).toBeGreaterThan(-1);
    expect(
      activation.indexOf('coreGeneration ==', acknowledgement),
    ).toBeGreaterThan(acknowledgement);
    expect(activation.indexOf('authorityStillExact')).toBeLessThan(
      activation.indexOf('SystemRoutingTransition.IDLE'),
    );
  });

  it('coordinates retained TUN ownership across service recreation and rejects stale callbacks', () => {
    const resetService = vpnService.slice(
      vpnService.indexOf('private fun handleExplicitReset('),
      vpnService.indexOf('private fun ensureBlockingTun('),
    );
    const translatorReturn = vpnService.slice(
      vpnService.indexOf('private fun handleTranslatorReturn('),
      vpnService.indexOf('private fun handleStop('),
    );

    expect(terminalCoordinator).toContain(
      'val translator: SystemRoutingTranslator',
    );
    expect(terminalCoordinator).toContain('val resource: Resource');
    expect(terminalCoordinator).toContain(
      'var cleanup: CompletableFuture<TerminalLeaseCloseResult>?',
    );
    expect(terminalCoordinator).toContain('var captureValid: Boolean');
    expect(terminalCoordinator).toContain('fun invalidateCapture(');
    expect(terminalCoordinator).toContain('adoptedEpoch: Long? = null');
    expect(terminalCoordinator).toContain('candidate.translator.stopAndAwait(');
    expect(terminalCoordinator).toContain(
      'candidate.translator.confirmsProcessReleased(candidate.ownership)',
    );
    expect(
      terminalCoordinator.indexOf(
        'candidate.translator.confirmsProcessReleased(candidate.ownership)',
      ),
    ).toBeLessThan(
      terminalCoordinator.indexOf('closeResource(candidate.resource)'),
    );
    expect(resetService).toContain('stopAndCloseAllTunnelsSafely()');
    expect(resetService).toContain('policyStore.clearAfterExplicitReset()');
    expect(translatorReturn).toContain('runAttemptEpoch');
    expect(packetTranslator).toContain(
      'finalSnapshot.generation == run.expectedNativeGeneration',
    );
    expect(packetTranslator).toContain('val runAttemptEpoch: Long');
    expect(packetTranslator).toContain('proof?.ownership == expectedOwnership');
    expect(vpnService).toContain('claimStatusEpoch(serviceEpoch)');
    expect(vpnService).toContain('if (ownerEpoch < currentStatusOwnerEpoch)');
    expect(vpnService).toContain('ownedResetRequests');
  });

  it('treats revoked capture as direct-risk cleanup ownership and republishes terminal handoff', () => {
    const revoke = vpnService.slice(
      vpnService.indexOf('override fun onRevoke()'),
      vpnService.indexOf('override fun onDestroy()'),
    );
    const destruction = vpnService.slice(
      vpnService.indexOf('override fun onDestroy()'),
      vpnService.indexOf('private fun currentAlwaysOn()'),
    );
    const sticky = vpnService.slice(
      vpnService.indexOf('private fun handleStickyRestart()'),
      vpnService.indexOf('private fun restoreBlockingTun('),
    );
    const start = vpnService.slice(
      vpnService.indexOf('private fun handleStart('),
      vpnService.indexOf('private fun handleStartWithPermit('),
    );

    expect(revoke.indexOf('revoked = true')).toBeLessThan(
      revoke.indexOf('terminalCoordinator.invalidateCapture('),
    );
    expect(
      revoke.indexOf('terminalCoordinator.invalidateCapture('),
    ).toBeLessThan(revoke.indexOf('controlExecutor.execute'));
    expect(revoke).toContain('tunPresentOverride = false');
    expect(destruction).toContain(
      'captureValid = terminalSnapshot.captureValid',
    );
    expect(revoke).toContain(
      'Pair(tunnelDescriptor, adoptedTerminalLeaseEpoch)',
    );
    expect(revoke).toContain('adoptedEpoch = revokedOwnership.second');
    expect(destruction.indexOf('terminalCoordinator.retain(')).toBeLessThan(
      destruction.indexOf('adoptedTerminalLeaseEpoch = retainResult?.epoch'),
    );
    expect(
      destruction.indexOf('adoptedTerminalLeaseEpoch = retainResult?.epoch'),
    ).toBeLessThan(destruction.indexOf('tunnelDescriptor = null'));
    expect(destruction).toContain('SystemRoutingDiagnostic.PERMISSION_REVOKED');
    expect(sticky).toContain(
      'terminalCoordinator.closeOrJoin(TRANSLATOR_STOP_TIMEOUT_MS)',
    );
    expect(sticky).toContain('handleStickyRestart()');
    expect(start).toContain(
      'terminalCoordinator.closeOrJoin(TRANSLATOR_STOP_TIMEOUT_MS)',
    );
    expect(start).toContain(
      'handleStart(\n              revision,\n              proxyPort,\n              coreGeneration,\n              engineGeneration,\n              expectedNetworkEpoch,\n              requestId,\n              recoveryAction,',
    );
    expect(vpnService).toContain(
      'terminalCoordinator.snapshot()?.captureValid == true',
    );
  });

  it('retries a retained terminal lease when sticky ExplicitOff cleanup fails', () => {
    const stopWithPermit = vpnService.slice(
      vpnService.indexOf('private fun handleStopWithPermit('),
      vpnService.indexOf('private fun handleExplicitReset('),
    );
    const stop = vpnService.slice(
      vpnService.indexOf('private fun handleStop('),
      vpnService.indexOf('private fun handleStopWithPermit('),
    );
    const retry = vpnService.slice(
      vpnService.indexOf('private fun scheduleTerminalCleanupRetryIfBlocked()'),
      vpnService.indexOf('private fun terminalCloseResult('),
    );

    expect(
      stopWithPermit.match(/scheduleTerminalCleanupRetryIfBlocked\(\)/g),
    ).toHaveLength(2);
    expect(stop).toContain('scheduleTerminalCleanupRetryIfBlocked()');
    expect(retry).toContain('terminalCoordinator.blocksNewStart()');
    expect(retry).toContain('scheduleStickyHandoffRetry()');
    expect(vpnService).toContain(
      'is SystemRoutingPolicyLoadResult.ExplicitOff ->\n          handleStop(load.policy.revision, requestId = null)',
    );
  });

  it('requires deterministic native initialization pending before translator readiness', () => {
    const run = packetTunnelRust.slice(
      packetTunnelRust.indexOf('fn run('),
      packetTunnelRust.indexOf('fn stop()'),
    );

    expect(run).toContain('worker.as_mut().poll(context)');
    expect(run).toContain('Poll::Pending => None');
    expect(run).toContain('if let Some(result) = immediate');
    expect(run.indexOf('worker.as_mut().poll(context)')).toBeLessThan(
      run.indexOf('TUNNEL_LIFECYCLE.mark_running(generation)'),
    );
    expect(vpnService).toContain('translator.awaitReadiness');
  });

  it('guards notification channels on API 24 and labels system routing as limited dogfood', () => {
    const creation = vpnService.slice(
      vpnService.indexOf('override fun onCreate()'),
      vpnService.indexOf('override fun onStartCommand('),
    );

    expect(creation).toContain(
      'if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O)',
    );
    expect(creation.indexOf('Build.VERSION_CODES.O')).toBeLessThan(
      creation.indexOf('NotificationChannel('),
    );
    expect(vpnService).toContain(
      'only captured IPv4 TCP/443 and virtual DNS are translated through MASQ',
    );
    expect(vpnService).toContain(
      'All other captured IP traffic, including other TCP ports, non-DNS UDP, IPv6, ICMP',
    );
    expect(vpnService).toContain(
      'Installed MASQ packages are excluded when the route is created.',
    );
    expect(vpnService).toContain('Package IDs and');
    expect(vpnService).toContain('consent timestamps stay on-device');
    expect(vpnService).toContain('Shared-UID apps and attached restricted');
    expect(vpnService).toContain('work profiles are separate.');
    expect(vpnService).toContain(
      'TLS handshake and encrypted HEAD request to example.com',
    );
    expect(vpnService).toContain(
      'translator.hasOwnedRun() &&\n            translatorEngineGeneration != engineGeneration',
    );
    expect(rustCore).toContain(
      'write_all(b"CONNECT example.com:443 HTTP/1.1\\r\\nHost: example.com:443\\r\\n\\r\\n")',
    );
    expect(rustCore).toContain(
      'tls.write_all(b"HEAD / HTTP/1.1\\r\\nHost: example.com\\r\\nConnection: close\\r\\n\\r\\n")',
    );
    expect(vpnService).toContain('temporary loopback proxy');
    expect(packetTunnelRust).toContain('args.ipv6_enabled = false');
    expect(vpnService).toContain(
      'Direct traffic can resume after service or process death.',
    );
    expect(vpnService).toContain('Android Always-on VPN and');
    expect(vpnService).toContain(
      '\\"Block connections without VPN\\" are unsupported.',
    );
    expect(vpnService).not.toContain('Device traffic is protected by MASQ.');
  });

  it('enforces Android 13 notification permission natively for recovered activation', () => {
    const stickyRestore = vpnService.slice(
      vpnService.indexOf('private fun restoreBlockingTun('),
      vpnService.indexOf('private fun handleStart('),
    );
    const start = vpnService.slice(
      vpnService.indexOf('private fun handleStartWithPermit('),
      vpnService.indexOf('private fun startTranslatorForCapturedDescriptor('),
    );
    const establish = vpnService.slice(
      vpnService.indexOf('private fun ensureBlockingTun('),
      vpnService.indexOf('private fun validateInstalledPackages('),
    );
    const stop = vpnService.slice(
      vpnService.indexOf('private fun handleStopWithPermit('),
      vpnService.indexOf('private fun handleExplicitReset('),
    );

    expect(manifest).toContain('android.permission.POST_NOTIFICATIONS');
    expect(notificationPermission).toContain(
      'sdkInt >= ANDROID_POST_NOTIFICATIONS_API_LEVEL',
    );
    expect(notificationPermission).toContain(
      'desiredMode != SystemRoutingMode.OFF',
    );
    expect(stickyRestore).toContain(
      'refuseActivationWithoutNotification(load, requestId = null)',
    );
    expect(start).toContain(
      'refuseActivationWithoutNotification(load, requestId)',
    );
    expect(
      establish.indexOf('notificationPermissionDiagnostic(policy)'),
    ).toBeLessThan(establish.indexOf('builder.establish()'));
    expect(
      establish.lastIndexOf('notificationPermissionDiagnostic(policy)'),
    ).toBeGreaterThan(establish.indexOf('builder.establish()'));
    expect(establish.indexOf('tunnelDescriptor = descriptor')).toBeLessThan(
      establish.lastIndexOf('notificationPermissionDiagnostic(policy)'),
    );
    expect(establish).not.toContain('descriptor.close()');
    expect(start.match(/refuseActivationWithoutNotification\(/g)).toHaveLength(
      2,
    );
    expect(
      stickyRestore.match(/refuseActivationWithoutNotification\(/g),
    ).toHaveLength(2);
    expect(stop).not.toContain('notificationPermissionDiagnostic(');
    expect(notificationPermission).toContain(
      'SystemRoutingDiagnostic.NOTIFICATION_PERMISSION_REQUIRED',
    );
  });

  it('keeps only the user-requested consumer session alive during screen lock', () => {
    const recoveryRequest = sessionService.slice(
      sessionService.indexOf('private fun requestRecovery('),
      sessionService.indexOf('private fun cancelRecovery()'),
    );
    const recoveryCancellation = sessionService.slice(
      sessionService.indexOf('private fun cancelRecovery()'),
      sessionService.indexOf('private fun isRecoveryCurrent('),
    );
    const wakeAcquisition = sessionService.slice(
      sessionService.indexOf('private fun acquireTimedWakeLock('),
      sessionService.indexOf('private fun releaseWakeLock()'),
    );

    expect(manifest).toContain('android.permission.WAKE_LOCK');
    expect(manifest).toContain('android:name=".MasqSessionService"');
    expect(manifest).toContain('android:foregroundServiceType="specialUse"');
    expect(manifest).toContain('android:stopWithTask="false"');
    expect(manifest).toContain(
      'User-initiated MASQ consumer peer session kept active while the screen is locked',
    );
    expect(sessionService).toContain('class MasqSessionService : Service()');
    expect(sessionService).not.toContain(
      'class MasqSessionService : VpnService()',
    );
    expect(sessionService).toContain('PowerManager.PARTIAL_WAKE_LOCK');
    expect(sessionService).toContain('setReferenceCounted(false)');
    expect(sessionService).toContain('lock.acquire(WAKE_LOCK_TIMEOUT_MILLIS)');
    expect(wakeAcquisition).toContain(
      'val scheduleRenewal = forceRenewal || !lock.isHeld',
    );
    expect(wakeAcquisition.indexOf('if (scheduleRenewal)')).toBeLessThan(
      wakeAcquisition.indexOf(
        'mainHandler.removeCallbacks(renewWakeLockRunnable)',
      ),
    );
    expect(sessionService).toContain('runCatching { lock.release() }');
    expect(sessionService).toContain('ServiceCompat.startForeground(');
    expect(sessionService).toContain(
      'ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE',
    );
    expect(sessionService).toContain('.setOngoing(true)');
    expect(sessionService).toContain('return START_STICKY');
    expect(sessionService).toContain('return START_NOT_STICKY');
    expect(sessionService).toContain('MasqCoreLifecycle.executor.execute');
    expect(recoveryRequest).toContain('recoveryExecutor.submit');
    expect(recoveryCancellation).toContain('recoveryFuture?.cancel(true)');
    expect(sessionService).toContain('hasTerminalEntryRecoverySignal()');
    expect(sessionService).toContain('terminalEntryRetryDelayMillis()');
    expect(sessionService).toContain('recovery.recordKnownGoodRoute(snapshot)');
    expect(sessionService).toContain('recovery.recordSavedRouteProofFailure()');
    expect(backgroundRecovery).toContain('fun recordKnownGoodRoute(');
    expect(backgroundRecovery).toContain('fun recordSavedRouteProofFailure()');
    expect(backgroundRecovery).toContain(
      'entryNodeDiscovery.recordKnownGoodRoute(chain, descriptors, snapshot)',
    );
    expect(backgroundRecovery).toContain(
      'entryNodeDiscovery.recordRouteProofFailure(chain, descriptors)',
    );
    const periodicProofEscalation = sessionService.slice(
      sessionService.indexOf(
        'MasqPeriodicRouteProofFailureAction.FAIL_CLOSED_RESTART\n      ) {',
      ),
      sessionService.indexOf('// The first two transient failures'),
    );
    expect(periodicProofEscalation).toContain(
      'recovery.recordSavedRouteProofFailure()',
    );
    expect(
      periodicProofEscalation.indexOf(
        'recovery.recordSavedRouteProofFailure()',
      ),
    ).toBeLessThan(
      periodicProofEscalation.indexOf(
        'MasqVpnService.publishCoreRouteUnavailable(this)',
      ),
    );
    expect(sessionService).toContain('MasqSessionIntentStore(context)');
    expect(sessionService).toContain('activeInstance.get()?.adoptGeneration');
    expect(sessionService).toContain(
      'screenOff && networkAvailable && cpuRequired',
    );
    expect(sessionService).toContain('phase == "connected" &&');
    expect(sessionService).toContain('connectedNeighbors > 0 &&');
    expect(sessionService).toContain('routeStage >= 2 &&');
    expect(sessionService).toContain('isEntryConnectedAwaitingRoute()');
    expect(sessionService).toContain('proxyPort in 1..65535');
    expect(sessionService).toContain(
      'NetworkCapabilities.NET_CAPABILITY_VALIDATED',
    );
    expect(sessionService).toContain('CONNECTING_PROGRESS_TIMEOUT_MILLIS');
    expect(sessionService).toContain('recoveryEpoch.incrementAndGet()');
    expect(sessionService).toContain('stopSelfResult(startId)');
    expect(sessionService).toContain(
      'requestRecovery(nextRecoveryDelayMillis())',
    );
    expect(sessionService).toContain('MasqCoreJni.nativeRefreshRouteProof()');
    expect(sessionService).toContain('shouldApplyMasqSessionSnapshot(');
    expect(sessionService).toContain(
      'recoveryBackoff = recoveryBackoff.afterStarted(now)',
    );
    expect(sessionService).toContain(
      'isNonMutatingRouteProofRefreshFailure(',
    );
    expect(recoveryRequest.indexOf('cpuRequired = true')).toBeLessThan(
      recoveryRequest.indexOf(
        'if (recoveryRunningToken != NO_RECOVERY_TOKEN) return',
      ),
    );
    expect(backgroundRecovery).toContain('SecureWalletStore(context)');
    expect(backgroundRecovery).toContain('entryNodeDiscovery.discover(');
    expect(backgroundRecovery).toContain('MasqCoreJni.nativeStart()');
    expect(backgroundRecovery).toContain('MasqCoreJni.nativePreflightProxy()');
    expect(backgroundRecovery).toContain('MasqRecoveryRouteVerificationGate()');
    expect(backgroundRecovery).toContain('isEntryConnectedAwaitingRoute()');
    expect(backgroundRecovery).toContain(
      'entryNodeDiscovery.recordConnectionFailure(chain, preferredNodes)',
    );
    expect(
      backgroundRecovery.indexOf(
        'entryNodeDiscovery.recordConnectionFailure(chain, preferredNodes)',
      ),
    ).toBeLessThan(backgroundRecovery.indexOf('entryNodeDiscovery.discover('));
    expect(backgroundRecovery).toContain(
      'MasqCoreLifecycle.startGeneration.get() == recoveryGeneration',
    );
    expect(
      backgroundRecovery.indexOf('entryNodeDiscovery.discover('),
    ).toBeLessThan(backgroundRecovery.indexOf('walletStore.load()'));
    expect(sessionService).not.toContain('VpnService.prepare');
    expect(sessionService).not.toContain('VpnService.Builder');
    expect(sessionService).not.toContain(
      'class MasqSessionService : VpnService()',
    );
    expect(sessionService).toContain('MasqCoreJni.nativeShutdown()');
  });

  it('restores only durable consumer intent after this APK is replaced', () => {
    expect(manifest).toContain(
      'android:name=".MasqPackageReplacementReceiver"',
    );
    expect(manifest).toContain(
      'android:name="android.intent.action.MY_PACKAGE_REPLACED"',
    );
    expect(packageReplacementReceiver).toContain(
      'intent?.action != Intent.ACTION_MY_PACKAGE_REPLACED',
    );
    expect(packageReplacementReceiver).toContain(
      'MasqSessionService.ensureRunningIfDesired(context.applicationContext)',
    );
    expect(packageReplacementReceiver).not.toContain('setDesired(');
  });

  it('cleans up Android native executors and pending tunnel acknowledgements', () => {
    const invalidation = moduleSource.slice(
      moduleSource.indexOf('override fun invalidate()'),
      moduleSource.indexOf(
        'override fun setBrowserRoutingMode(mode: String, promise: Promise)',
      ),
    );

    expect(moduleSource).toContain(
      'pendingTunnelStarts = mutableMapOf<Long, PendingTunnelStart>()',
    );
    expect(moduleSource).toContain(
      'pendingTunnelStops = mutableMapOf<Long, PendingTunnelStop>()',
    );
    expect(moduleSource).toContain(
      'pendingTunnelResets = mutableMapOf<Long, PendingTunnelReset>()',
    );
    expect(moduleSource).toContain(
      'operation.completed.compareAndSet(false, true)',
    );
    expect(invalidation).toContain(
      'MasqVpnService.cancelStartAcknowledgement(operation.requestId)',
    );
    expect(invalidation).toContain(
      'MasqVpnService.cancelStopAcknowledgement(operation.requestId)',
    );
    expect(invalidation).toContain(
      'MasqVpnService.cancelResetAcknowledgement(operation.requestId)',
    );
    expect(invalidation).toContain(
      'operation.timeoutFuture.getAndSet(null)?.cancel(false)',
    );
    expect(invalidation).toContain('stopAcknowledgementExecutor.shutdownNow()');
    expect(invalidation).not.toContain('MasqCoreLifecycle.executor.shutdown');
    expect(invalidation).toContain('super.invalidate()');
  });

  it('restores a persisted scope as a blocker and reports revocation honestly', () => {
    const sticky = vpnService.slice(
      vpnService.indexOf('private fun handleStickyRestart()'),
      vpnService.indexOf('private fun handleStart('),
    );

    expect(vpnService).toContain('return START_STICKY');
    expect(sticky).toContain('policyStore.loadForServiceStart()');
    expect(sticky).toContain('ensureBlockingTun(load.policy)');
    expect(sticky).not.toContain('translator.start(');
    expect(vpnService).toContain('override fun onRevoke()');
    expect(vpnService).toContain('SystemRoutingTransition.REVOKED');
    expect(vpnService).toContain('tunPresentOverride = false');
    expect(vpnService).toContain('currentLockdown()');
    expect(vpnService).toContain(
      'SystemRoutingDiagnostic.LOCKDOWN_UNSUPPORTED',
    );
  });

  it('rejects stale tunnel commands without mutating the current runtime status', () => {
    const staleStart = vpnService.slice(
      vpnService.indexOf('if (load !is SystemRoutingPolicyLoadResult.Ready ||'),
      vpnService.indexOf('val policy = load.policy'),
    );
    const staleStop = vpnService.slice(
      vpnService.indexOf(
        'if (load !is SystemRoutingPolicyLoadResult.ExplicitOff ||',
      ),
      vpnService.indexOf(
        'publish(\n        load,\n        SystemRoutingTransition.STOPPING',
      ),
    );

    expect(staleStart).toContain('settleStart(');
    expect(staleStart).not.toContain('publish(');
    expect(staleStop).toContain('settleStop(');
    expect(staleStop).not.toContain('publish(');
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

  it('keeps scheduled route-proof refresh failures observational and privacy-safe', () => {
    const refreshJni = rustAndroid.slice(
      rustAndroid.indexOf(
        'Java_com_masqmobile_MasqCoreJni_nativeRefreshRouteProof',
      ),
      rustAndroid.indexOf(
        'Java_com_masqmobile_MasqCoreJni_nativeSetProxyEnabled',
      ),
    );

    expect(coreJni).toContain('external fun nativeRefreshRouteProof(): String');
    expect(refreshJni).toContain('refresh_route_proof_status()');
    expect(refreshJni).not.toContain('status_after(');
    expect(rustCore).toContain('begin_route_proof_refresh');
    expect(rustCore).toContain('complete_route_proof_refresh');
    expect(rustCore).toContain('route_proof_refresh_ticket_is_current');
    expect(rustCore).not.toContain('restore_healthy_route_state');
    expect(rustCore).toContain('E_PRIVATE_ROUTE_REFRESH_FAILED');
    expect(rustCore).toContain('E_PRIVATE_ROUTE_REFRESH_NOT_READY');
    expect(rustCore).toContain('routeProofRefresh');
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
    expect(
      preservationRead.match(/PreservationRead\.Unreadable/g)?.length,
    ).toBeGreaterThanOrEqual(4);
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

  it('refreshes, probes, ranks, caches, and quarantines entry nodes before starting', () => {
    expect(gradle).toContain(
      'implementation("com.squareup.okhttp3:okhttp:4.9.2")',
    );
    expect(discovery).toContain('NODE_FINDER_ATTEMPTS = 12');
    expect(discovery).toContain('NODE_FINDER_MAX_CONCURRENT_REQUESTS = 12');
    expect(discovery).toContain('NODE_FINDER_BUDGET_MS = 6_000L');
    expect(discovery).toContain('ENTRY_PROBE_CONNECT_TIMEOUT_MS = 900');
    expect(discovery).toContain('ENTRY_PROBE_BUDGET_MS = 2_500L');
    expect(discovery).toContain('MAX_PROBED_ENTRY_IDENTITIES = 8');
    expect(discovery).toContain('ENTRY_PROBE_MAX_CONCURRENCY = 8');
    expect(discovery).toContain('CacheControl.FORCE_NETWORK');
    expect(discovery).toContain('MAX_RESPONSE_BYTES = 1024');
    expect(discovery).toContain('ConnectionSpec.MODERN_TLS');
    expect(discovery).toContain('followRedirects(false)');
    expect(discovery).toContain('freshDescriptors = freshDescriptors');
    expect(discovery).toContain('preferredDescriptors = preferredNodes');
    expect(discovery).toContain('candidate.publicKey in selectedKeys');
    expect(discovery).toContain('candidate.host in selectedHosts');
    expect(discovery).toContain('saveCached(chain, result.cacheDescriptors)');
    expect(discovery).toContain('planEntryNodeProbes(');
    expect(discovery).toContain(
      'planEntryNodeProbes(candidates, maxIdentities, generation)',
    );
    expect(discovery).toContain(
      'maxIdentities: Int = MAX_PROBED_ENTRY_IDENTITIES',
    );
    expect(discovery).toContain(
      'Math.floorMod(generation, candidate.ports.size)',
    );
    expect(discovery).toContain('plan.primaryTargets');
    expect(discovery).toContain('entryNodeProbeFallbackRequired(');
    expect(discovery).toContain('plan.fallbackTargets');
    expect(discovery).toContain('"NF_PROBE_PRIMARY"');
    expect(discovery).toContain('"NF_PROBE_FALLBACK"');
    expect(discovery).toContain('planEntryNodeProbePhases(');
    expect(discovery).toContain('MAX_QUARANTINED_STANDBY_PROBE_IDENTITIES');
    expect(discovery).toContain('"NF_PROBE_STANDBY"');
    expect(discovery).toContain('ENTRY_PROBE_LATENCY_BAND_MS = 100');
    expect(discovery).toContain('"best_band"');
    expect(discovery).toContain('"worst_band"');
    expect(discovery).not.toContain('"best_ms"');
    expect(discovery).not.toContain('"worst_ms"');
    expect(discovery).toContain('EntryNodePortProbe');
    expect(discovery).toContain('recordConnectionFailure(');
    expect(discovery).toContain('recordRouteProofFailure(');
    expect(discovery).toContain('recordKnownGoodRoute(');
    expect(discovery).toContain('KNOWN_GOOD_TTL_MS = 24 * 60 * 60_000L');
    expect(discovery).toContain('MIN_REQUIRED_ENTRY_NODES = 2');
    expect(discovery).toContain('MAX_RUNTIME_ENTRY_NODES = 3');
    expect(discovery).toContain(
      'MAX_KNOWN_GOOD_CANDIDATES = MAX_RUNTIME_ENTRY_NODES',
    );
    expect(discovery).toContain('private val finderSessionNonce = UUID.randomUUID().toString()');
    expect(discovery).toContain('.addQueryParameter(');
    expect(discovery).toContain('"refresh",');
    expect(discovery).toContain(
      '"$finderSessionNonce-${generation.toUInt()}-$attempt"',
    );
    expect(discovery).toContain('decodeKnownGoodEntryNodes(');
    expect(discovery).toContain('prioritizeKnownGoodEntryNodes(');
    expect(discovery).toContain('"NF_KNOWN_GOOD_OK"');
    expect(discovery).toContain('deprioritizeAttemptedEntryNodes(');
    expect(discovery).toContain(
      'ROUTE_FAILURE_DEPRIORITIZATION_MS = 2 * 60_000L',
    );
    expect(discovery).toContain('excludeQuarantinedEntryNodes(');
    expect(discovery).toContain('mergePortVariants(');
    expect(discovery).toContain('"NF_SELECTION_OK"');
    expect(discovery).toContain(
      'safeDiagnostic(code: String, vararg metrics: Pair<String, Int>)',
    );
    expect(discovery).toContain('NF_CODE_PATTERN = Regex("NF_[A-Z_]+")');
    expect(discovery).not.toContain('Log.i(LOG_TAG, descriptor');
    expect(discovery).not.toContain('Log.i(LOG_TAG, publicKey');
    expect(discovery).not.toContain('Log.i(LOG_TAG, host');
    expect(discovery).toContain('Socket().use');
    expect(discovery).toContain('InetSocketAddress(host, port)');
    expect(moduleSource).toContain(
      'entryNodeDiscovery.discover(chain, preferredNodes)',
    );
    expect(moduleSource).toContain(
      'entryNodeDiscovery.recordConnectionFailure(chain, preferredNodes)',
    );
    expect(moduleSource).toContain(
      'val lastError = safeLastErrorValue(status.opt("lastError"))',
    );
    expect(moduleSource).toContain('ENTRY_NODE_QUARANTINE_CODES');
    expect(moduleSource).toContain('routeStage != 0');
    expect(backgroundRecovery).toContain(
      'lastError = initialSnapshot.lastError',
    );
    expect(backgroundRecovery).toContain(
      'entryNodeDiscovery.recordRouteProofFailure(chain, preferredNodes)',
    );
    expect(backgroundRecovery).toContain(
      'var routeProofFailed =\n        shouldDeprioritizeAttemptedEntryNodes(',
    );
    expect(moduleSource).toContain(
      'recordEntrySelectionFeedbackFromSavedConfig(currentStatus)',
    );
    expect(moduleSource).toContain(
      'recordKnownGoodEntrySelectionFromSavedConfig(JSONObject(status))',
    );
    expect(moduleSource).toContain(
      'entryNodeDiscovery.recordKnownGoodRoute(chain, preferredNodes, snapshot)',
    );
    expect(moduleSource).toContain(
      'JSONArray(discoveryResult.runtimeDescriptors)',
    );
    expect(moduleSource).toContain(
      'JSONArray(discoveryResult.persistentDescriptors)',
    );
    expect(moduleSource).toContain(
      'MasqCoreJni.nativeConfigure(prepareNativeConfig(runtimeConfig))',
    );
    expect(moduleSource).toContain(
      'if (!shouldDiscoverEntryNodesBeforeStart(currentStatus.optString("phase")))',
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
      moduleSource.indexOf(
        'private fun invalidatePendingStarts(preservePendingConsent: Boolean = false)',
      ),
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
    const backgroundStart = sessionService.slice(
      sessionService.indexOf('fun start(context: Context): Long'),
      sessionService.indexOf('fun stop(context: Context): Boolean'),
    );

    expect(moduleSource).toContain('internal object MasqCoreLifecycle');
    expect(moduleSource).toContain('val startGeneration = AtomicLong(0L)');
    expect(moduleSource).toContain(
      'val executor = Executors.newSingleThreadExecutor()',
    );
    expect(backgroundStart).toContain(
      'MasqCoreLifecycle.startGeneration.incrementAndGet()',
    );
    expect(
      backgroundStart.indexOf(
        'activeInstance.get()?.recoveryEpoch?.incrementAndGet()',
      ),
    ).toBeLessThan(
      backgroundStart.indexOf(
        'MasqCoreLifecycle.startGeneration.incrementAndGet()',
      ),
    );
    expect(backgroundStart).toContain('intentStore.clearDesiredFailClosed()');
    expect(start.indexOf('MasqSessionService.start(')).toBeLessThan(
      start.indexOf('MasqCoreLifecycle.executor.execute'),
    );
    expect(start).toContain('catch (_: StaleStartException)');
    expect(start).not.toContain('promise.resolve(MasqCoreJni.nativeStart())');
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
      start.indexOf('requireCurrentStart(generation)', configureIndex),
    ).toBeLessThan(configuredStartIndex);
    expect(
      resolution.indexOf(
        'MasqCoreLifecycle.startGeneration.get() != generation',
      ),
    ).toBeLessThan(resolution.indexOf('preferences.edit()'));
    expect(configuredStartIndex).toBeLessThan(
      start.indexOf(
        'resolveStart(generation, promise, started, refreshedConfig)',
      ),
    );
    expect(stop.indexOf('invalidatePendingStarts()')).toBeLessThan(
      stop.indexOf('MasqCoreLifecycle.executor.execute'),
    );
    expect(stop.indexOf('MasqSessionService.stop(')).toBeLessThan(
      stop.indexOf('MasqCoreLifecycle.executor.execute'),
    );
    expect(stop.indexOf('MasqCoreLifecycle.executor.execute')).toBeLessThan(
      stop.indexOf('MasqCoreJni.nativeStop()'),
    );
    expect(shutdown.indexOf('invalidatePendingStarts()')).toBeLessThan(
      shutdown.indexOf('MasqCoreLifecycle.executor.execute'),
    );
    expect(shutdown.indexOf('MasqSessionService.stop(')).toBeLessThan(
      shutdown.indexOf('MasqCoreLifecycle.executor.execute'),
    );
    expect(shutdown.indexOf('MasqCoreLifecycle.executor.execute')).toBeLessThan(
      shutdown.indexOf('MasqCoreJni.nativeShutdown()'),
    );
    expect(
      shutdown.indexOf('recordEntrySelectionFeedbackFromSavedConfig('),
    ).toBeLessThan(shutdown.indexOf('MasqCoreJni.nativeShutdown()'));
    expect(rejection).toContain(
      'promise.reject("E_CORE_START_CANCELLED", START_CANCELLED_MESSAGE)',
    );
    expect(rejection).toContain('} else if (error == null) {');
    expect(rejection).not.toContain('MasqSessionService.stop');
  });

  it('uses validated network identity and retains recovery authority while system routing is active', () => {
    const networkStatus = moduleSource.slice(
      moduleSource.indexOf('override fun getNetworkStatus(promise: Promise)'),
      moduleSource.indexOf('override fun getNodeFinderUrl(promise: Promise)'),
    );
    const stop = moduleSource.slice(
      moduleSource.indexOf('override fun stop(promise: Promise)'),
      moduleSource.indexOf('override fun shutdown(promise: Promise)'),
    );
    const shutdown = moduleSource.slice(
      moduleSource.indexOf('override fun shutdown(promise: Promise)'),
      moduleSource.indexOf(
        'private fun recordEntrySelectionFeedbackFromSavedConfig(',
      ),
    );
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

    expect(networkStatus).toContain(
      'NetworkCapabilities.NET_CAPABILITY_VALIDATED',
    );
    expect(networkStatus).toContain('resolveMasqValidatedUnderlayNetwork(');
    expect(networkStatus).toContain('underlay?.network?.networkHandle');
    expect(networkStatus).toContain('MasqNetworkStatusLifecycle.tracker');
    expect(networkStatus).not.toContain('System.currentTimeMillis()');
    expect(stop.indexOf('authorizeSessionSupervisorStop(')).toBeLessThan(
      stop.indexOf('MasqSessionService.stop('),
    );
    expect(shutdown.indexOf('authorizeSessionSupervisorStop(')).toBeLessThan(
      shutdown.indexOf('MasqSessionService.stop('),
    );
    expect(reset.indexOf('resetConfirmed')).toBeLessThan(
      reset.indexOf('MasqSessionService.stop('),
    );
    expect(reset.indexOf('MasqSessionService.stop(')).toBeLessThan(
      reset.indexOf('finishFullReset(promise)'),
    );
    expect(networkReset).toContain(
      'stopSessionSupervisorAfterConfirmedSystemRoutingOff(promise)',
    );
    expect(
      networkReset.indexOf(
        'stopSessionSupervisorAfterConfirmedSystemRoutingOff(promise)',
      ),
    ).toBeLessThan(
      networkReset.indexOf('MasqCoreJni.nativeResetNetworkProfile()'),
    );
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
    ).toBeLessThan(removeWallet.indexOf('MasqCoreJni.nativeRemoveWallet()'));
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
    expect(
      discoveryCompletion.indexOf('entryNodeDiscovery.discover('),
    ).toBeLessThan(
      discoveryCompletion.indexOf('MasqCoreLifecycle.executor.execute'),
    );
    expect(
      discoveryCompletion.indexOf('MasqCoreLifecycle.executor.execute'),
    ).toBeLessThan(discoveryCompletion.indexOf('MasqCoreJni.nativeConfigure('));
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
    const preflight = moduleSource.slice(
      moduleSource.indexOf('override fun preflightBrowserProxy('),
      moduleSource.indexOf('override fun getDebtSummary('),
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
    expect(getStatus).toContain(
      'recordKnownGoodEntrySelectionFromSavedConfig(JSONObject(status))',
    );
    expect(preflight).toContain('MasqCoreJni.nativePreflightProxy()');
    expect(preflight).toContain(
      'MasqCoreJni.nativePreflightProxy().also { status ->',
    );
    expect(preflight).toContain('runCatching {');
    expect(preflight).toContain(
      'recordKnownGoodEntrySelectionFromSavedConfig(JSONObject(status))',
    );
    expect(
      setSystemTunnel.indexOf('MasqCoreLifecycle.executor.execute'),
    ).toBeLessThan(setSystemTunnel.indexOf('restoreCoreIfNeeded()'));
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
    expect(
      removeWallet.indexOf('MasqCoreJni.nativeRemoveWallet()'),
    ).toBeLessThan(removeWallet.indexOf('walletStore.delete()'));
    expect(removeWallet.indexOf('isExactWalletRemovalStatus(')).toBeLessThan(
      removeWallet.indexOf('walletStore.delete()'),
    );
    expect(
      removeWallet.lastIndexOf('MasqCoreJni.nativeGetStatus()'),
    ).toBeLessThan(removeWallet.indexOf('promise.resolve(finalStatusJson)'));
  });

  it('rejects missing or malformed Android core phases as unsuccessful', () => {
    const statusSucceeded = moduleSource.slice(
      moduleSource.indexOf('private fun statusSucceeded('),
      moduleSource.indexOf('private fun nullableStatusString('),
    );

    expect(statusSucceeded).toContain(
      'val phase = JSONObject(statusJson).opt("phase")',
    );
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
    const proxyMutationGuards = moduleSource.slice(
      moduleSource.indexOf('private fun setBrowserProxyOverride('),
      moduleSource.indexOf('private data class BrowserRoutingRequest('),
    );
    const abortRouting = moduleSource.slice(
      moduleSource.indexOf('private fun abortBrowserRoutingRequestClosed('),
      moduleSource.indexOf(
        'private fun armFailClosedBrowserTransitionTimeout(',
      ),
    );
    const failClosedRouting = moduleSource.slice(
      moduleSource.indexOf(
        'private fun armFailClosedBrowserTransitionTimeout(',
      ),
      moduleSource.indexOf('private fun invalidateBrowserRoutingRequests('),
    );
    const failClosedCompletion = moduleSource.slice(
      moduleSource.indexOf('private fun completeFailClosedBrowserTransition('),
      moduleSource.indexOf('private fun timeoutFailClosedBrowserTransition('),
    );
    const failClosedTimeout = moduleSource.slice(
      moduleSource.indexOf('private fun timeoutFailClosedBrowserTransition('),
      moduleSource.indexOf('private fun invalidateBrowserRoutingRequests('),
    );
    const companion = moduleSource.slice(
      moduleSource.indexOf('companion object {'),
      moduleSource.indexOf('private data class BrowserSite('),
    );

    expect(browserRouting).toContain('MasqCoreLifecycle.startGeneration.get()');
    expect(browserRouting).toContain('requireCurrentBrowserCore(request)');
    expect(moduleSource).toContain('E_BROWSER_STALE_CORE');
    expect(browserRouting).toContain('MasqCoreLifecycle.executor.execute');
    expect(
      browserRouting.indexOf('requireCurrentBrowserCore(request)'),
    ).toBeLessThan(
      browserRouting.indexOf('MasqCoreJni.nativeSetProxyEnabled(true)'),
    );
    expect(
      masqConfirmation.match(/requireCurrentBrowserCore\(request\)/g),
    ).toHaveLength(3);
    expect(
      directRouting.indexOf('requireCurrentBrowserCore(request)'),
    ).toBeLessThan(directRouting.indexOf('clearBrowserProxyOverride(request)'));
    expect(
      directRouting.match(/requireCurrentBrowserCore\(request\)/g)?.length,
    ).toBeGreaterThanOrEqual(4);
    expect(companion).toContain(
      'private val browserRoutingQueue = ArrayDeque<BrowserRoutingRequest>()',
    );
    expect(moduleSource).toContain(
      'private const val BROWSER_ROUTING_TIMEOUT_MS = 12_000L',
    );
    expect(moduleSource).toContain(
      'private const val BROWSER_FAIL_CLOSED_TIMEOUT_MS = 12_000L',
    );
    expect(browserRouting).toContain('timeoutBrowserRoutingRequest(request)');
    expect(abortRouting).toContain('scheduleFailClosedBrowserTransition');
    expect(abortRouting).not.toContain('startNextBrowserRoutingRequest');
    expect(failClosedRouting).toContain(
      'browserProxyCallbackFence.hasActiveMutation()',
    );
    expect(failClosedRouting).toContain(
      'ProxyController.getInstance().setProxyOverride',
    );
    expect(
      failClosedCompletion.indexOf(
        'browserProxyCallbackFence.complete(ticket)',
      ),
    ).toBeLessThan(
      failClosedCompletion.indexOf('startNextBrowserRoutingRequest'),
    );
    expect(failClosedTimeout).toContain(
      'browserProxyCallbackFence.markActiveTimedOut()',
    );
    expect(failClosedTimeout).toContain('browserProxyFenceTimedOut = true');
    expect(failClosedTimeout).toContain('browserRoutingQueue.removeFirst()');
    expect(moduleSource).toContain(
      'request.completed.compareAndSet(false, true)',
    );
    expect(browserRouting).toContain('E_BROWSER_ROUTING_SUPERSEDED');
    expect(browserRouting).toContain('E_BROWSER_ROUTING_TIMEOUT');
    expect(moduleSource).toContain('invalidateBrowserRoutingRequests()');
    expect(browserRouting).toContain('enqueueBrowserRoutingRequest(');
    expect(browserRouting).toContain('prioritizeBlocked = true');
    expect(browserRouting).not.toContain('.addDirect()');
    expect(proxyMutationGuards).toContain('synchronized(browserRoutingLock)');
    expect(proxyMutationGuards).toContain('browserRoutingActive !== request');
    expect(proxyMutationGuards).toContain(
      'ProxyController.getInstance().setProxyOverride',
    );
    expect(proxyMutationGuards).toContain(
      'ProxyController.getInstance().clearProxyOverride',
    );
    expect(proxyMutationGuards).toContain('beginBrowserProxyMutation(request)');
    expect(proxyMutationGuards).toContain(
      'BrowserProxyCallbackCompletion.STALE',
    );
  });

  it('keeps timed-out Android proxy callbacks fenced until their matching callback', () => {
    const callbackFence = moduleSource.slice(
      moduleSource.indexOf('internal class BrowserProxyCallbackFence'),
      moduleSource.indexOf('internal fun safeCoreStatusDiagnostic('),
    );
    const timeoutMutation = callbackFence.slice(
      callbackFence.indexOf('fun markTimedOut('),
      callbackFence.indexOf('fun complete('),
    );

    expect(callbackFence).toContain('if (activeTicket != null) return null');
    expect(callbackFence).toContain(
      'if (activeTicket != ticket) return BrowserProxyCallbackCompletion.STALE',
    );
    expect(timeoutMutation).toContain('activeTimedOut = true');
    expect(timeoutMutation).not.toContain('activeTicket = null');
    expect(callbackFence).toContain(
      'BrowserProxyCallbackCompletion.CURRENT_AFTER_TIMEOUT',
    );
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

  it('persists one strict browser search provider and clears it only on full reset', () => {
    const providerBridge = moduleSource.slice(
      moduleSource.indexOf(
        'override fun getBrowserSearchProvider(promise: Promise)',
      ),
      moduleSource.indexOf(
        'override fun getBrowserSiteSettings(mode: String, hostname: String, promise: Promise)',
      ),
    );
    const browserStorageCleanup = moduleSource.slice(
      moduleSource.indexOf(
        'private fun clearRememberedBrowserStorage(clearProtectionExceptions: Boolean)',
      ),
      moduleSource.indexOf('private fun persistentBrowserProfileName('),
    );
    const fullReset = moduleSource.slice(
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
    const rememberedDataCleanup = moduleSource.slice(
      moduleSource.indexOf('override fun clearRememberedBrowserData('),
      moduleSource.indexOf('override fun getSavedConfiguration('),
    );

    expect(nativeSpec).toContain('getBrowserSearchProvider(): Promise<string>');
    expect(nativeSpec).toContain(
      "setBrowserSearchProvider(provider: 'timpi' | 'duckduckgo'): Promise<string>",
    );
    expect(coreFacade).toContain('type BrowserSearchProvider');
    expect(coreFacade).toContain(
      "serialized !== 'timpi' && serialized !== 'duckduckgo'",
    );
    expect(providerBridge).toContain('provider !in BROWSER_SEARCH_PROVIDERS');
    expect(providerBridge).toContain(
      'preferences.edit().putString(BROWSER_SEARCH_PROVIDER_KEY, provider).commit()',
    );
    expect(providerBridge).toContain('readBrowserSearchProvider() != provider');
    expect(moduleSource).toContain(
      'private const val DEFAULT_BROWSER_SEARCH_PROVIDER = "timpi"',
    );
    expect(moduleSource).toContain(
      'private val BROWSER_SEARCH_PROVIDERS = setOf("timpi", "duckduckgo")',
    );
    expect(fullReset).toContain(
      'clearRememberedBrowserStorage(clearProtectionExceptions = true)',
    );
    expect(browserStorageCleanup).toContain('if (clearProtectionExceptions)');
    expect(browserStorageCleanup).toContain(
      'editor.remove(BROWSER_SEARCH_PROVIDER_KEY)',
    );
    expect(rememberedDataCleanup).toContain(
      'clearRememberedBrowserStorage(clearProtectionExceptions = false)',
    );
    expect(rememberedDataCleanup).not.toContain('BROWSER_SEARCH_PROVIDER_KEY');
    expect(networkReset).not.toContain('BROWSER_SEARCH_PROVIDER_KEY');
  });
});
