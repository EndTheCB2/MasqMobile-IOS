declare const __dirname: string;

const { readFileSync } = require('fs');
const { spawnSync } = require('child_process');
const path = require('path');

describe('iOS native MASQ core linkage', () => {
  const bridgeSource = readFileSync(
    path.resolve(__dirname, '../ios/MasqMobile/RCTMasqCore.mm'),
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
        {encoding: 'utf8'},
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

  it('binds MASQ and direct WebViews to separate fail-closed ephemeral stores', () => {
    expect(bridgeSource).toContain(
      'WKWebsiteDataStore *masq_private_browser_data_store(void)',
    );
    expect(bridgeSource).toContain(
      'WKWebsiteDataStore *masq_direct_browser_data_store(void)',
    );
    expect(bridgeSource).toContain('MasqBlockedBrowserProxyPort');
    expect(bridgeSource).toContain(
      'nw_proxy_config_set_failover_allowed(proxy, false)',
    );
    expect(bridgeSource).not.toContain(
      '[WKWebsiteDataStore defaultDataStore].proxyConfigurations',
    );

    const requiredMasqBinding =
      'wkWebViewConfig.websiteDataStore = masq_private_browser_data_store()';
    const requiredDirectBinding =
      'wkWebViewConfig.websiteDataStore = masq_direct_browser_data_store()';
    expect(browserSource).toContain(requiredMasqBinding);
    expect(browserSource).toContain(requiredDirectBinding);
    expect(browserPatch).toContain(requiredMasqBinding);
    expect(browserPatch).toContain(requiredDirectBinding);
    expect(browserSource).toContain('if (_incognito)');
    expect(browserSource).toContain('} else if (!_cacheEnabled)');
    expect(browserScreen).toContain('incognito');
    expect(browserScreen).toContain('cacheEnabled={false}');
  });

  it('switches exact ephemeral browser modes without ever opening the MASQ store directly', () => {
    expect(nativeSpecSource).toContain(
      'setBrowserRoutingMode(mode: string)',
    );
    const routingBody = bridgeSource.slice(
      bridgeSource.indexOf('- (void)setBrowserRoutingMode:'),
      bridgeSource.indexOf(
        '- (std::shared_ptr<facebook::react::TurboModule>)',
      ),
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
      'symbol<BooleanArgumentFunction>("masq_mobile_set_proxy_enabled")',
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
      'symbol<BooleanArgumentFunction>("masq_mobile_set_proxy_enabled")',
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
    expect(masqBody).toContain(
      '![status isKindOfClass:[NSDictionary class]]',
    );
    expect(masqBody).toContain('port.integerValue < 1');
    expect(masqBody).toContain('port.integerValue > 65535');
    expect(masqBody).toContain(
      'symbol<BooleanArgumentFunction>("masq_mobile_set_proxy_enabled"), true',
    );
    expect(masqBody).toContain('resolve(@"masq")');

    expect(
      bridgeSource.match(/setProxyConfigurations:@\[\]/g),
    ).toHaveLength(1);
    expect(bridgeSource).not.toContain(
      '[masqBrowserDataStore() setProxyConfigurations:@[]]',
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
    expect(browserSource).toContain('if (_incognito || !_cacheEnabled)');
    expect(browserPatch).toContain('if (_incognito || !_cacheEnabled)');
    expect(browserScreen).toContain(
      'await masqCore.prepareBrowserProtection()',
    );
    expect(bridgeSource).toContain('gBrowserProtectionGeneration');
    expect(bridgeSource).toContain('staleBrowserProtectionError()');
    const preparationBody = bridgeSource.slice(
      bridgeSource.indexOf('- (void)prepareBrowserProtection:'),
      bridgeSource.indexOf('- (void)setBrowserProtection:'),
    );
    expect(preparationBody).toContain(
      'clearProtectedBrowserDataStores',
    );
    const protectedDataCleanupBody = bridgeSource.slice(
      bridgeSource.indexOf('void clearProtectedBrowserDataStores('),
      bridgeSource.indexOf('BOOL isJsonBoolean('),
    );
    expect(protectedDataCleanupBody).toContain('masqBrowserDataStore()');
    expect(protectedDataCleanupBody).toContain('directBrowserDataStore()');
    expect(protectedDataCleanupBody).toContain(
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
    expect(decoderBody).toContain(
      'dictionary.count != expectedKeys.count',
    );
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
    expect(defaultsBody).toMatch(
      /@"rejectOptionalCookies"\s*:[\s\S]*?: @NO,/,
    );
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

  it('hides the HLN privacy-gate host only on HLN pages without clicking consent', () => {
    const bannerRulesBody = bridgeSource.slice(
      bridgeSource.indexOf(
        'NSArray<NSDictionary *> *cookieBannerRules()',
      ),
      bridgeSource.indexOf(
        '#if MASQ_PRIVATE_YOUTUBE_AD_BLOCKER == 1',
        bridgeSource.indexOf(
          'NSArray<NSDictionary *> *cookieBannerRules()',
        ),
      ),
    );
    const genericSelector = bannerRulesBody.slice(
      bannerRulesBody.indexOf('#onetrust-banner-sdk'),
      bannerRulesBody.indexOf(
        '}),',
        bannerRulesBody.indexOf('#onetrust-banner-sdk'),
      ),
    );

    expect(genericSelector).not.toContain('#pg-shadow-host-dom');
    expect(bannerRulesBody).toContain(
      '@"if-domain" : @[ @"hln.be", @"*.hln.be" ]',
    );
    expect(bannerRulesBody).toContain(
      '@"selector" : @"#pg-shadow-host-dom"',
    );
    expect(bannerRulesBody).not.toContain('myprivacy.dpgmedia.be');
    expect(bannerRulesBody.toLowerCase()).not.toContain('click');
    expect(bannerRulesBody.toLowerCase()).not.toContain('accept');
  });

  it('fails every iOS build when the fail-closed WebView patch is absent', () => {
    expect(spawnSync('bash', [webViewPatchGuardPath]).status).toBe(0);
    expect(projectSource).toContain('Verify Fail-Closed WebView Patch');
    expect(projectSource).toContain(
      '$(SRCROOT)/../scripts/verify-ios-webview-patch.sh',
    );
    expect(archiveScript).toContain('verify-ios-webview-patch.sh');
    expect(webViewPatchGuard).toContain(
      'wkWebViewConfig.websiteDataStore = masq_private_browser_data_store();',
    );
    expect(webViewPatchGuard).toContain(
      'wkWebViewConfig.websiteDataStore = masq_direct_browser_data_store();',
    );
    expect(webViewPatchGuard).toContain(
      'masq_configure_private_browser_content_controller(',
    );
    expect(webViewPatchGuard).toContain(
      'if (_incognito || !_cacheEnabled)',
    );
  });

  it('keeps targeted YouTube filtering out of public iOS configurations', () => {
    expect(
      projectSource.match(/MASQ_PRIVATE_YOUTUBE_AD_BLOCKER=0/g),
    ).toHaveLength(2);
    expect(bridgeSource).toContain(
      '#if MASQ_PRIVATE_YOUTUBE_AD_BLOCKER == 1',
    );
    expect(bridgeSource).toContain('__masqPrivateYouTubeFilter');
    expect(bridgeSource).not.toContain('googlevideo\\\\.com');
    expect(archiveScript).toContain(
      "rg -a -l '__masqPrivateYouTubeFilter' \"$APP_PATH\"",
    );
    expect(archiveScript).toContain(
      'The private YouTube filter was found in the public App Store archive.',
    );
  });

  it('offers a separately signed no-NFT direct-install build with the private filter', () => {
    expect(directInstallScript).toContain(
      'MASQ_PRIVATE_YOUTUBE_AD_BLOCKER=1',
    );
    expect(directInstallScript).toContain(
      "rg -a -q '__masqPrivateYouTubeFilter'",
    );
    expect(directInstallScript).toContain(
      "rg -a -q 'E_ACCESS_PASS|checkAccessPass|NFT access'",
    );
    expect(directInstallScript).toContain(
      'MASQ_BUNDLE_IDENTIFIER',
    );
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
