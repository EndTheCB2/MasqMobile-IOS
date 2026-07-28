declare const __dirname: string;

const { readFileSync } = require('fs');
const { spawnSync } = require('child_process');
const path = require('path');

describe('iOS native MASQ core linkage', () => {
  const bridgeSource = readFileSync(
    path.resolve(__dirname, '../ios/MasqMobile/RCTMasqCore.mm'),
    'utf8',
  );
  const bridgeHeaderSource = readFileSync(
    path.resolve(__dirname, '../ios/MasqMobile/RCTMasqCore.h'),
    'utf8',
  );
  const nativeSpecSource = readFileSync(
    path.resolve(__dirname, '../specs/NativeMasqCore.ts'),
    'utf8',
  );
  const browserSource = readFileSync(
    path.resolve(
      __dirname,
      '../node_modules/react-native-webview/apple/RNCWebViewImpl.m',
    ),
    'utf8',
  );
  const browserPatch = readFileSync(
    path.resolve(__dirname, '../patches/react-native-webview+14.0.1.patch'),
    'utf8',
  );
  const browserScreen = readFileSync(
    path.resolve(__dirname, '../src/screens/BrowserScreen.tsx'),
    'utf8',
  );
  const streamConnectorSource = readFileSync(
    path.resolve(
      __dirname,
      '../../masq-node-mobile/node/src/sub_lib/stream_connector.rs',
    ),
    'utf8',
  );
  const mobileRuntimeSource = readFileSync(
    path.resolve(
      __dirname,
      '../../masq-node-mobile/node/src/mobile_runtime.rs',
    ),
    'utf8',
  );
  const neighborhoodSource = readFileSync(
    path.resolve(
      __dirname,
      '../../masq-node-mobile/node/src/neighborhood/mod.rs',
    ),
    'utf8',
  );
  const web3HttpTransportSource = readFileSync(
    path.resolve(
      __dirname,
      '../../masq-node-mobile/vendor/web3-0.11.0/src/transports/http.rs',
    ),
    'utf8',
  );
  const archiveScriptPath = path.resolve(
    __dirname,
    '../scripts/archive-ios-app-store.sh',
  );
  const archiveScript = readFileSync(archiveScriptPath, 'utf8');
  const webViewPatchGuardPath = path.resolve(
    __dirname,
    '../scripts/verify-ios-webview-patch.sh',
  );
  const webViewPatchGuard = readFileSync(webViewPatchGuardPath, 'utf8');
  const directInstallScript = readFileSync(
    path.resolve(__dirname, '../scripts/install-ios-direct-private.sh'),
    'utf8',
  );
  const projectSource = readFileSync(
    path.resolve(__dirname, '../ios/MasqMobile.xcodeproj/project.pbxproj'),
    'utf8',
  );

  it('uses direct static-library references in Release builds', () => {
    expect(bridgeSource).not.toContain('dlsym(');
    expect(bridgeSource).toContain('&masq_mobile_get_status');
    expect(bridgeSource).toContain('&masq_mobile_configure');
    expect(bridgeSource).toContain('&masq_mobile_import_wallet');
    expect(bridgeSource).toContain('&masq_mobile_update_min_hops');
    expect(bridgeSource).toContain('&masq_mobile_start');
    expect(bridgeSource).toContain('&masq_mobile_shutdown');
    expect(bridgeSource).toContain('&masq_mobile_string_free');
  });

  it('proves iOS wallet preservation before completing a network-profile-only reset', () => {
    const resetBody = bridgeSource.slice(
      bridgeSource.indexOf('- (void)resetNetworkProfile:'),
      bridgeSource.indexOf('- (void)removeWallet:'),
    );
    const terminalStatusBody = bridgeSource.slice(
      bridgeSource.indexOf('BOOL isResetNetworkProfileTerminalStatus('),
      bridgeSource.indexOf('NSString *_Nullable nativeConfig('),
    );
    const profileRemoval = resetBody.indexOf(
      '[defaults removeObjectForKey:MasqConfigDefaultsKey]',
    );
    const nativeReset = resetBody.indexOf(
      'symbol<NoArgumentFunction>("masq_mobile_reset_network_profile")',
    );
    const walletRestore = resetBody.indexOf(
      'symbol<StringArgumentFunction>("masq_mobile_import_wallet")',
    );

    expect(resetBody).toContain('if (!coreAvailable())');
    expect(resetBody).toContain(
      'loadWalletSecretForPreservation(&savedWalletBefore)',
    );
    expect(resetBody).toContain('beforeWalletAddress && !savedWalletBefore');
    expect(resetBody).toContain(
      'loadWalletSecretForPreservation(&savedWalletAfter)',
    );
    expect(resetBody).toContain(
      '[savedWalletBefore isEqualToString:savedWalletAfter]',
    );
    expect(resetBody).toContain(
      '[beforeWalletAddress isEqualToString:finalWalletAddress]',
    );
    expect(resetBody).toContain(
      '[importedWalletAddress isEqualToString:finalWalletAddress]',
    );
    expect(resetBody).toContain(
      'isResetNetworkProfileTerminalStatus(walletStatus)',
    );
    expect(resetBody).toContain('E_NETWORK_PROFILE_RESET');
    expect(resetBody).toContain('E_WALLET_PRESERVATION');
    expect(resetBody).not.toContain('deleteWalletSecret');
    expect(resetBody).not.toContain('resetBrowserProtectionPreferences');

    expect(profileRemoval).toBeGreaterThan(
      resetBody.indexOf('loadWalletSecretForPreservation(&savedWalletBefore)'),
    );
    expect(nativeReset).toBeGreaterThan(
      resetBody.indexOf('loadWalletSecretForPreservation(&savedWalletBefore)'),
    );
    expect(walletRestore).toBeGreaterThan(nativeReset);
    expect(resetBody.lastIndexOf('"masq_mobile_get_status"')).toBeGreaterThan(
      walletRestore,
    );
    expect(profileRemoval).toBeGreaterThan(
      resetBody.lastIndexOf('"masq_mobile_get_status"'),
    );

    expect(terminalStatusBody).toContain(
      '[status[@"phase"] isEqualToString:@"unconfigured"]',
    );
    expect(terminalStatusBody).toContain('status[@"chain"] == [NSNull null]');
    expect(terminalStatusBody).toContain(
      '[status[@"connectedNeighbors"] isEqual:@0]',
    );
    expect(terminalStatusBody).toContain(
      '[status[@"routeStage"] isEqual:@0]',
    );
    expect(terminalStatusBody).toContain(
      '[status[@"routeHops"] isEqual:@0]',
    );
    expect(terminalStatusBody).toContain('[status[@"minHops"] isEqual:@1]');
    expect(terminalStatusBody).toContain(
      'status[@"exitCountry"] == [NSNull null]',
    );
    expect(terminalStatusBody).toContain(
      '[status[@"exitCountryFallback"] isEqual:@YES]',
    );
    expect(terminalStatusBody).toContain(
      '[(NSArray *)status[@"availableExitCountries"] count] == 0',
    );
    expect(terminalStatusBody).toContain(
      '[status[@"proxyEnabled"] isEqual:@NO]',
    );
    expect(terminalStatusBody).toContain(
      'status[@"proxyPort"] == [NSNull null]',
    );
    expect(terminalStatusBody).toContain(
      'status[@"lastError"] == [NSNull null]',
    );
  });

  it('uses a stable code for unreadable saved iOS profiles', () => {
    const getSavedConfigurationBody = bridgeSource.slice(
      bridgeSource.indexOf('- (void)getSavedConfiguration:'),
      bridgeSource.indexOf('- (void)configure:'),
    );

    expect(getSavedConfigurationBody).toContain('E_SAVED_CONFIG_INVALID');
  });

  it('uses a process-global fence so an old iOS module cannot undo a reset', () => {
    const startBody = bridgeSource.slice(
      bridgeSource.indexOf('- (void)start:'),
      bridgeSource.indexOf('- (void)reset:'),
    );
    const resetBody = bridgeSource.slice(
      bridgeSource.indexOf('- (void)resetNetworkProfile:'),
      bridgeSource.indexOf('- (void)removeWallet:'),
    );
    const staleGuard = startBody.indexOf(
      'generation != gCoreStartGeneration',
    );
    const nativeConfigure = startBody.indexOf(
      'symbol<StringArgumentFunction>("masq_mobile_configure")',
    );
    const profileWrite = startBody.indexOf(
      'setObject:refreshedConfig',
    );
    const nativeStart = startBody.indexOf(
      'symbol<NoArgumentFunction>("masq_mobile_start")',
    );

    expect(bridgeSource).toContain(
      'static NSUInteger gCoreStartGeneration = 0;',
    );
    expect(bridgeSource).toContain('NSObject *coreLifecycleLock()');
    expect(startBody).toContain('generation = gCoreStartGeneration');
    expect(startBody).toContain('@synchronized(coreLifecycleLock())');
    expect(startBody).toContain('E_CORE_START_CANCELLED');
    expect(staleGuard).toBeGreaterThan(-1);
    expect(nativeConfigure).toBeGreaterThan(staleGuard);
    expect(nativeStart).toBeGreaterThan(nativeConfigure);
    expect(profileWrite).toBeGreaterThan(nativeStart);
    expect(resetBody.indexOf('gCoreStartGeneration += 1')).toBeLessThan(
      resetBody.indexOf(
        '[defaults removeObjectForKey:MasqConfigDefaultsKey]',
      ),
    );
  });

  it('serializes iOS profile mutations and validates destructive terminal states before deletion', () => {
    const getSavedConfigurationBody = bridgeSource.slice(
      bridgeSource.indexOf('- (void)getSavedConfiguration:'),
      bridgeSource.indexOf('- (void)configure:'),
    );
    const configureBody = bridgeSource.slice(
      bridgeSource.indexOf('- (void)configure:'),
      bridgeSource.indexOf('- (void)importWallet:'),
    );
    const importWalletBody = bridgeSource.slice(
      bridgeSource.indexOf('- (void)importWallet:'),
      bridgeSource.indexOf('- (void)updateMinHops:'),
    );
    const updateMinHopsBody = bridgeSource.slice(
      bridgeSource.indexOf('- (void)updateMinHops:'),
      bridgeSource.indexOf('- (void)start:'),
    );
    const fullResetBody = bridgeSource.slice(
      bridgeSource.indexOf('- (void)reset:'),
      bridgeSource.indexOf('- (void)resetNetworkProfile:'),
    );
    const removeWalletBody = bridgeSource.slice(
      bridgeSource.indexOf('- (void)removeWallet:'),
      bridgeSource.indexOf('- (void)preflightBrowserProxy:'),
    );

    expect(getSavedConfigurationBody).toContain(
      '@synchronized(coreLifecycleLock())',
    );
    for (const mutation of [
      configureBody,
      importWalletBody,
      updateMinHopsBody,
    ]) {
      expect(mutation).toContain('@synchronized(coreLifecycleLock())');
      expect(mutation).toContain('gCoreStartGeneration += 1');
    }
    expect(fullResetBody.indexOf('if (!coreAvailable())')).toBeLessThan(
      fullResetBody.indexOf('deleteWalletSecret()'),
    );
    expect(fullResetBody.indexOf('"masq_mobile_reset"')).toBeLessThan(
      fullResetBody.indexOf('deleteWalletSecret()'),
    );
    expect(
      fullResetBody.indexOf('isFullResetTerminalStatus(resetStatus)'),
    ).toBeLessThan(fullResetBody.indexOf('deleteWalletSecret()'));
    expect(fullResetBody.lastIndexOf('"masq_mobile_get_status"')).toBeLessThan(
      fullResetBody.indexOf('resolve(finalResult)'),
    );
    expect(removeWalletBody.indexOf('if (!coreAvailable())')).toBeLessThan(
      removeWalletBody.indexOf('deleteWalletSecret()'),
    );
    expect(removeWalletBody.indexOf('"masq_mobile_remove_wallet"')).toBeLessThan(
      removeWalletBody.indexOf('deleteWalletSecret()'),
    );
    expect(
      removeWalletBody.indexOf(
        'isWalletRemovalTerminalStatus(removalStatus)',
      ),
    ).toBeLessThan(removeWalletBody.indexOf('deleteWalletSecret()'));
    expect(
      removeWalletBody.lastIndexOf('"masq_mobile_get_status"'),
    ).toBeLessThan(removeWalletBody.indexOf('resolve(finalResult)'));
  });

  it('fences stale iOS browser proxy callbacks behind the core lifecycle', () => {
    const browserRoutingBody = bridgeSource.slice(
      bridgeSource.indexOf('- (void)setBrowserRoutingMode:'),
      bridgeSource.indexOf(
        '- (std::shared_ptr<facebook::react::TurboModule>)',
      ),
    );

    expect(browserRoutingBody).toContain(
      'coreGeneration = gCoreStartGeneration',
    );
    expect(browserRoutingBody).toContain(
      'coreGeneration != gCoreStartGeneration',
    );
    expect(browserRoutingBody).toContain(
      '[mode isEqualToString:@"direct"]',
    );
    expect(browserRoutingBody).toContain(
      'self.invalidated ||\n            coreGeneration != gCoreStartGeneration',
    );
    expect(browserRoutingBody).toContain('E_BROWSER_STALE_CORE');
    expect(browserRoutingBody).toContain(
      '@synchronized(coreLifecycleLock())',
    );
  });

  it('restores iOS saved network configuration and wallet independently', () => {
    const restoreBody = bridgeSource.slice(
      bridgeSource.indexOf('- (BOOL)restoreCoreIfNeeded'),
      bridgeSource.indexOf('- (void)stop:'),
    );

    expect(restoreBody).toContain('if (savedConfig) {');
    expect(restoreBody).toContain('if (savedWallet) {');
    expect(restoreBody).toContain('"masq_mobile_configure"');
    expect(restoreBody).toContain('"masq_mobile_import_wallet"');
    expect(restoreBody).not.toContain(
      'if (!savedConfig || !savedWallet)',
    );
  });

  it('invalidates every queued iOS core mutation when its bridge is torn down', () => {
    const invalidationBody = bridgeSource.slice(
      bridgeSource.indexOf('- (void)invalidate {'),
      bridgeSource.indexOf('- (void)getStatus:'),
    );
    const startBody = bridgeSource.slice(
      bridgeSource.indexOf('- (void)start:'),
      bridgeSource.indexOf('- (void)reset:'),
    );
    const preflightBody = bridgeSource.slice(
      bridgeSource.indexOf('- (void)preflightBrowserProxy:'),
      bridgeSource.indexOf('- (void)getSystemTunnelStatus:'),
    );
    const stopBody = bridgeSource.slice(
      bridgeSource.indexOf('- (void)stop:'),
      bridgeSource.indexOf('- (void)shutdown:'),
    );
    const shutdownBody = bridgeSource.slice(
      bridgeSource.indexOf('- (void)shutdown:'),
      bridgeSource.indexOf('- (void)setBrowserRoutingMode:'),
    );

    expect(bridgeHeaderSource).toContain('#import <React/RCTInvalidating.h>');
    expect(bridgeHeaderSource).toContain(
      '<NativeMasqCoreSpec, RCTInvalidating>',
    );
    expect(invalidationBody).toContain('self.invalidated = YES');
    expect(invalidationBody).toContain('gCoreStartGeneration += 1');
    expect(invalidationBody).toContain('gBrowserProtectionGeneration += 1');
    expect(startBody).toContain(
      'self.invalidated || generation != gCoreStartGeneration',
    );
    for (const operation of [preflightBody, stopBody, shutdownBody]) {
      expect(operation).toContain('generation = gCoreStartGeneration');
      expect(operation).toContain(
        'self.invalidated || generation != gCoreStartGeneration',
      );
      expect(operation.indexOf('generation = gCoreStartGeneration'))
        .toBeLessThan(operation.indexOf('dispatch_async('));
    }
    expect(preflightBody.indexOf('generation != gCoreStartGeneration'))
      .toBeLessThan(preflightBody.indexOf('"masq_mobile_preflight_proxy"'));
  });

  it('rejects malformed iOS core operation status instead of treating it as success', () => {
    const statusSucceededBody = bridgeSource.slice(
      bridgeSource.indexOf('BOOL statusSucceeded('),
      bridgeSource.indexOf('NSDictionary *_Nullable decodedStatus('),
    );

    expect(statusSucceededBody).toContain(
      '[decoded isKindOfClass:[NSDictionary class]]',
    );
    expect(statusSucceededBody).toContain(
      '[phase isKindOfClass:[NSString class]]',
    );
    expect(statusSucceededBody).toContain(
      '[successfulPhases containsObject:(NSString *)phase]',
    );
    expect(statusSucceededBody).not.toContain(
      '![(NSString *)phase isEqualToString:@"error"]',
    );
  });

  it('rejects unsafe node-finder URL components before archiving', () => {
    const validate = (value: string) =>
      spawnSync(
        'bash',
        [
          '-c',
          'source "$1"; validate_node_finder_url "$2"',
          'validate-node-finder',
          archiveScriptPath,
          value,
        ],
        { encoding: 'utf8' },
      ).status;

    expect(validate('https://nodes.example.org:443/masq')).toBe(0);
    expect(validate('HTTPS://nodes.example.org')).toBe(0);
    expect(validate('http://nodes.example.org')).not.toBe(0);
    expect(validate('https://user:secret@nodes.example.org')).not.toBe(0);
    expect(validate('https://nodes.example.org?environment=prod')).not.toBe(0);
    expect(validate('https://nodes.example.org#production')).not.toBe(0);
    expect(validate('https://invalid_host.example.org')).not.toBe(0);
    expect(validate('https:///missing-host')).not.toBe(0);
  });

  it('binds MASQ and direct WebViews to isolated fail-closed session stores', () => {
    const requiredDataStoreCalls = [
      'masq_private_browser_data_store()',
      'masq_direct_browser_data_store()',
      'masq_persistent_browser_data_store()',
      'masq_direct_persistent_browser_data_store()',
    ];
    requiredDataStoreCalls.forEach(requiredCall => {
      expect(browserSource).toContain(requiredCall);
      expect(browserPatch).toContain(requiredCall);
    });
    [
      'masq_private_browser_data_store(void)',
      'masq_direct_browser_data_store(void)',
      'masq_persistent_browser_data_store(void)',
      'masq_direct_persistent_browser_data_store(void)',
    ].forEach(requiredDeclaration => {
      expect(bridgeSource).toContain(requiredDeclaration);
    });
    expect(bridgeSource).toContain('MasqBlockedBrowserProxyPort');
    expect(bridgeSource).toContain(
      'nw_proxy_config_set_failover_allowed(proxy, false)',
    );
    expect(bridgeSource).not.toContain(
      '[WKWebsiteDataStore defaultDataStore].proxyConfigurations',
    );

    expect(browserSource).toContain('if (_incognito)');
    expect(browserSource).toContain('} else {');
    expect(browserScreen).toContain('incognito');
    expect(browserScreen).toContain(
      'cacheEnabled={Boolean(siteSettings?.rememberSignIn)}',
    );
  });

  it('switches exact ephemeral browser modes without ever opening the MASQ store directly', () => {
    expect(nativeSpecSource).toContain('setBrowserRoutingMode(mode: string)');
    const routingBody = bridgeSource.slice(
      bridgeSource.indexOf('- (void)setBrowserRoutingMode:'),
      bridgeSource.indexOf('- (std::shared_ptr<facebook::react::TurboModule>)'),
    );
    const blockedStart = routingBody.indexOf(
      'if ([mode isEqualToString:@"blocked"])',
    );
    const directStart = routingBody.indexOf(
      'if ([mode isEqualToString:@"direct"])',
    );
    const masqStart = routingBody.indexOf(
      '// The only remaining validated mode is MASQ.',
    );
    const blockedBody = routingBody.slice(blockedStart, directStart);
    const directBody = routingBody.slice(directStart, masqStart);
    const masqBody = routingBody.slice(masqStart);

    expect(routingBody).toContain('![mode isEqualToString:@"blocked"]');
    expect(routingBody).toContain('![mode isEqualToString:@"masq"]');
    expect(routingBody).toContain('![mode isEqualToString:@"direct"]');
    expect(routingBody).toContain('E_BROWSER_ROUTING_MODE');
    expect(routingBody).not.toContain('NSUserDefaults');
    expect(bridgeSource).not.toContain('- (void)setBrowserProxy:');

    expect(blockedBody).toContain(
      'configurePrivateBrowserProxy(masqBrowserDataStore()',
    );
    expect(blockedBody).toContain(
      'configurePrivateBrowserProxy(directBrowserDataStore()',
    );
    expect(blockedBody).toContain('MasqBlockedBrowserProxyPort');
    expect(blockedBody).toContain(
      '"masq_mobile_set_proxy_enabled"',
    );
    expect(blockedBody).toContain('false');
    expect(blockedBody).toContain('resolve(@"blocked")');

    expect(directBody).toContain(
      'configurePrivateBrowserProxy(masqBrowserDataStore()',
    );
    expect(directBody).toContain(
      '[directBrowserDataStore() setProxyConfigurations:@[]]',
    );
    expect(directBody).not.toContain(
      '[masqBrowserDataStore() setProxyConfigurations:@[]]',
    );
    expect(directBody).toContain(
      '"masq_mobile_set_proxy_enabled"',
    );
    expect(directBody).toContain('false');
    expect(directBody).toContain('resolve(@"direct")');

    expect(masqBody).toContain(
      'configurePrivateBrowserProxy(directBrowserDataStore()',
    );
    expect(masqBody).toContain(
      'configurePrivateBrowserProxy(masqBrowserDataStore()',
    );
    expect(masqBody).toContain('@"connected"');
    expect(masqBody).toContain('![status isKindOfClass:[NSDictionary class]]');
    expect(masqBody).toContain('port.integerValue < 1');
    expect(masqBody).toContain('port.integerValue > 65535');
    expect(masqBody).toContain('"masq_mobile_set_proxy_enabled"');
    expect(masqBody).toMatch(
      /"masq_mobile_set_proxy_enabled"\),\s+true\)/,
    );
    expect(masqBody).toContain('resolve(@"masq")');

    expect(bridgeSource.match(/setProxyConfigurations:@\[\]/g)).toHaveLength(2);
    expect(bridgeSource).not.toContain(
      '[masqBrowserDataStore() setProxyConfigurations:@[]]',
    );
    expect(bridgeSource).not.toContain(
      '[masqPersistentBrowserDataStore() setProxyConfigurations:@[]]',
    );
  });

  it('installs native browser protection before the private WebView loads', () => {
    expect(nativeSpecSource).toContain('prepareBrowserProtection()');
    expect(nativeSpecSource).toContain(
      'setBrowserProtection(configJson: string)',
    );
    expect(bridgeSource).toContain('compileContentRuleListForIdentifier');
    expect(bridgeSource).toContain('@"type" : @"block-cookies"');
    expect(bridgeSource).toContain('@"type" : @"css-display-none"');
    expect(bridgeSource).toContain(
      'void masq_configure_private_browser_content_controller(',
    );

    const requiredBinding =
      'masq_configure_private_browser_content_controller(';
    expect(browserSource).toContain(requiredBinding);
    expect(browserPatch).toContain(requiredBinding);
    expect(browserSource).not.toContain('if (_incognito || !_cacheEnabled)');
    expect(browserPatch).not.toContain('if (_incognito || !_cacheEnabled)');
    expect(browserScreen).toContain(
      'await masqCore.prepareBrowserProtection()',
    );
    expect(bridgeSource).toContain('gBrowserProtectionGeneration');
    expect(bridgeSource).toContain('staleBrowserProtectionError()');
    const preparationBody = bridgeSource.slice(
      bridgeSource.indexOf('- (void)prepareBrowserProtection:'),
      bridgeSource.indexOf('- (void)setBrowserProtection:'),
    );
    expect(preparationBody).toContain('clearTemporaryBrowserDataStores');
    const protectedDataCleanupBody = bridgeSource.slice(
      bridgeSource.indexOf('void clearTemporaryBrowserDataStores('),
      bridgeSource.indexOf('void clearAllBrowserDataStores('),
    );
    expect(protectedDataCleanupBody).toContain('masqBrowserDataStore()');
    expect(protectedDataCleanupBody).toContain('directBrowserDataStore()');
    const genericDataCleanupBody = bridgeSource.slice(
      bridgeSource.indexOf('void clearBrowserDataStores('),
      bridgeSource.indexOf('void clearTemporaryBrowserDataStores('),
    );
    expect(genericDataCleanupBody).toContain(
      'removeDataOfTypes:WKWebsiteDataStore.allWebsiteDataTypes',
    );
    expect(preparationBody).toContain(
      'isCurrentBrowserProtectionOperation(generation)',
    );
  });

  it('persists exactly five strict browser-protection preferences', () => {
    const defaultsBody = bridgeSource.slice(
      bridgeSource.indexOf(
        'NSDictionary *browserProtectionPreferencesFromDefaults()',
      ),
      bridgeSource.indexOf(
        'NSDictionary *_Nullable decodeBrowserProtectionPreferences(',
      ),
    );
    const decoderBody = bridgeSource.slice(
      bridgeSource.indexOf(
        'NSDictionary *_Nullable decodeBrowserProtectionPreferences(',
      ),
      bridgeSource.indexOf('void saveBrowserProtectionPreferences('),
    );
    const expectedKeysBody = decoderBody.slice(
      decoderBody.indexOf('NSSet<NSString *> *expectedKeys'),
      decoderBody.indexOf('if (jsonError'),
    );

    expect(expectedKeysBody.match(/@"[^"]+"/g)).toEqual([
      '@"blockAdsAndTrackers"',
      '@"blockCrossSiteCookies"',
      '@"hideCookieBanners"',
      '@"rejectOptionalCookies"',
      '@"youtubeBestEffort"',
    ]);
    expect(decoderBody).toContain('dictionary.count != expectedKeys.count');
    expect(decoderBody).toContain(
      '[[NSSet setWithArray:dictionary.allKeys] isEqualToSet:expectedKeys]',
    );
    expect(bridgeSource).toContain(
      'NSString *const MasqBrowserRejectOptionalCookiesKey =',
    );
    expect(bridgeSource).toContain(
      '[defaults objectForKey:MasqBrowserRejectOptionalCookiesKey]',
    );
    expect(defaultsBody).toContain(
      '? @([defaults boolForKey:MasqBrowserRejectOptionalCookiesKey])',
    );
    expect(defaultsBody).toMatch(/@"rejectOptionalCookies"\s*:[\s\S]*?: @NO,/);
    expect(bridgeSource).toContain(
      '[preferences[@"rejectOptionalCookies"] boolValue]',
    );
    expect(bridgeSource).toContain(
      '[defaults removeObjectForKey:MasqBrowserRejectOptionalCookiesKey]',
    );
    expect(bridgeSource).toContain(
      'NSMutableDictionary *response = [preferences mutableCopy]',
    );
  });

  it('fails every iOS build when the fail-closed WebView patch is absent', () => {
    expect(spawnSync('bash', [webViewPatchGuardPath]).status).toBe(0);
    expect(projectSource).toContain('Verify Fail-Closed WebView Patch');
    expect(projectSource).toContain(
      '$(SRCROOT)/../scripts/verify-ios-webview-patch.sh',
    );
    expect(archiveScript).toContain('verify-ios-webview-patch.sh');
    expect(webViewPatchGuard).toContain('masq_private_browser_data_store()');
    expect(webViewPatchGuard).toContain('masq_direct_browser_data_store()');
    expect(webViewPatchGuard).toContain('masq_persistent_browser_data_store()');
    expect(webViewPatchGuard).toContain(
      'masq_direct_persistent_browser_data_store()',
    );
    expect(webViewPatchGuard).toContain(
      'masq_configure_private_browser_content_controller(',
    );
  });

  it('keeps targeted YouTube filtering out of public iOS configurations', () => {
    expect(
      projectSource.match(/MASQ_PRIVATE_YOUTUBE_AD_BLOCKER=0/g),
    ).toHaveLength(2);
    expect(bridgeSource).toContain('#if MASQ_PRIVATE_YOUTUBE_AD_BLOCKER == 1');
    expect(bridgeSource).toContain('__masqPrivateYouTubeFilter');
    expect(bridgeSource).not.toContain('googlevideo\\\\.com');
    expect(archiveScript).toContain(
      'rg -a -l \'__masqPrivateYouTubeFilter\' "$APP_PATH"',
    );
    expect(archiveScript).toContain(
      'The private YouTube filter was found in the public App Store archive.',
    );
  });

  it('offers a separately signed no-NFT direct-install build with the private filter', () => {
    expect(directInstallScript).toContain('MASQ_PRIVATE_YOUTUBE_AD_BLOCKER=1');
    expect(directInstallScript).toContain(
      "rg -a -q '__masqPrivateYouTubeFilter'",
    );
    expect(directInstallScript).toContain(
      "rg -a -q 'E_ACCESS_PASS|checkAccessPass|NFT access'",
    );
    expect(directInstallScript).toContain('MASQ_BUNDLE_IDENTIFIER');
    expect(directInstallScript).not.toMatch(
      /DEVELOPMENT_TEAM\s*=\s*[A-Z0-9]{10}/,
    );
  });

  it('does not claim iOS system routing without a signed extension', () => {
    expect(bridgeSource).toContain('@"supported" : @NO');
    expect(bridgeSource).toContain('E_NETWORK_EXTENSION');
    expect(bridgeSource).toContain(
      'Whole-device routing requires a separately entitled iOS Packet Tunnel extension.',
    );
    expect(bridgeSource).not.toContain('#import <NetworkExtension/');
  });

  it('routes both asynchronous and blocking iOS sockets through CFStream', () => {
    expect(streamConnectorSource).toContain(
      'connect_with_apple_stream(socket_addr, logger)',
    );
    expect(streamConnectorSource).toContain(
      '#[cfg(target_os = "ios")]\nfn connect_one_socket',
    );
    expect(web3HttpTransportSource).toContain('struct AppleConnector');
    expect(web3HttpTransportSource).toContain('masq_apple_tcp_connect');
    expect(web3HttpTransportSource).toContain('to_socket_addrs()?');
  });

  it('does not include the removed public-exit verification bridge', () => {
    expect(nativeSpecSource).not.toContain('verifyExit');
    expect(bridgeSource).not.toContain('verifyExit');
    expect(bridgeSource).not.toContain('cdn-cgi/trace');
  });

  it('rotates fresh entry nodes during a live retry and clears stale connection state', () => {
    expect(bridgeSource).toContain('Network results intentionally stay first');
    expect(bridgeSource).not.toContain(
      'if (![status[@"phase"] isEqualToString:@"ready"])',
    );
    expect(mobileRuntimeSource).toContain('PENDING_ENTRY_NODES');
    expect(mobileRuntimeSource).toContain('take_pending_entry_nodes');
    expect(neighborhoodSource).toContain(
      'replace_initial_neighbors_for_mobile_retry',
    );
    expect(neighborhoodSource).toContain(
      'initial_peer_shutdown_demotes_stale_mobile_connection_status',
    );
  });
});
