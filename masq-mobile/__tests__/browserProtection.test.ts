import {
  browserProtectionPreferences,
  buildBrowserCosmeticProtectionScript,
  decodeBrowserProtectionConfiguration,
  DEFAULT_BROWSER_PROTECTION_PREFERENCES,
  encodeBrowserProtectionPreferences,
  type BrowserProtectionConfiguration,
} from '../src/core/browserProtection';

const CONFIGURATION: BrowserProtectionConfiguration = {
  blockAdsAndTrackers: true,
  blockCrossSiteCookies: true,
  hideCookieBanners: true,
  rejectOptionalCookies: false,
  youtubeBestEffort: false,
  nativeRequestBlocking: true,
  youtubeBestEffortAvailable: false,
};

describe('browser protection boundary', () => {
  it('uses privacy-preserving defaults without enabling private YouTube handling', () => {
    expect(DEFAULT_BROWSER_PROTECTION_PREFERENCES).toEqual({
      blockAdsAndTrackers: true,
      blockCrossSiteCookies: true,
      hideCookieBanners: true,
      rejectOptionalCookies: false,
      youtubeBestEffort: false,
    });
  });

  it('strictly decodes the exact seven-field native response', () => {
    expect(
      decodeBrowserProtectionConfiguration(JSON.stringify(CONFIGURATION)),
    ).toEqual(CONFIGURATION);
  });

  it.each([
    ['malformed JSON', '{'],
    [
      'a missing field',
      JSON.stringify({
        blockAdsAndTrackers: true,
        blockCrossSiteCookies: true,
        hideCookieBanners: true,
        rejectOptionalCookies: false,
        youtubeBestEffort: false,
        nativeRequestBlocking: true,
      }),
    ],
    ['an extra field', JSON.stringify({ ...CONFIGURATION, unexpected: false })],
    [
      'a non-boolean field',
      JSON.stringify({ ...CONFIGURATION, blockAdsAndTrackers: 1 }),
    ],
    ['an array', JSON.stringify(Object.values(CONFIGURATION))],
  ])('rejects %s', (_label, serialized) => {
    expect(() => decodeBrowserProtectionConfiguration(serialized)).toThrow(
      /browser protection|native browser protection/i,
    );
  });

  it('rejects an unavailable YouTube mode reported as enabled', () => {
    expect(() =>
      decodeBrowserProtectionConfiguration(
        JSON.stringify({
          ...CONFIGURATION,
          youtubeBestEffort: true,
          youtubeBestEffortAvailable: false,
        }),
      ),
    ).toThrow('enabled but unavailable');
  });

  it('encodes only the five user preference booleans in stable order', () => {
    expect(
      encodeBrowserProtectionPreferences(
        browserProtectionPreferences({
          ...CONFIGURATION,
          youtubeBestEffortAvailable: true,
          youtubeBestEffort: true,
        }),
      ),
    ).toBe(
      '{"blockAdsAndTrackers":true,"blockCrossSiteCookies":true,' +
        '"hideCookieBanners":true,"rejectOptionalCookies":false,' +
        '"youtubeBestEffort":true}',
    );
  });

  it.each([
    [
      'an extra capability field',
      {
        blockAdsAndTrackers: true,
        blockCrossSiteCookies: true,
        hideCookieBanners: true,
        rejectOptionalCookies: false,
        youtubeBestEffort: false,
        nativeRequestBlocking: true,
      },
    ],
    [
      'a missing preference',
      {
        blockAdsAndTrackers: true,
        blockCrossSiteCookies: true,
        hideCookieBanners: true,
        rejectOptionalCookies: false,
      },
    ],
    [
      'a non-boolean preference',
      {
        blockAdsAndTrackers: 'yes',
        blockCrossSiteCookies: true,
        hideCookieBanners: true,
        rejectOptionalCookies: false,
        youtubeBestEffort: false,
      },
    ],
  ])('refuses to encode %s', (_label, preferences) => {
    expect(() =>
      encodeBrowserProtectionPreferences(preferences as never),
    ).toThrow(/browser protection preferences/i);
  });

  it('builds generic ad and cookie-banner cosmetic filtering', () => {
    const script = buildBrowserCosmeticProtectionScript(CONFIGURATION);
    expect(script).toContain('[data-ad-slot]');
    expect(script).toContain('doubleclick.net');
    expect(script).toContain('#onetrust-banner-sdk');
    expect(script).toContain("document.createElement('style')");
    expect(script).toContain('MutationObserver');
    expect(script).not.toContain('fetch(');
    expect(script).not.toContain('XMLHttpRequest');
  });

  it('contains no YouTube-specific page or media manipulation', () => {
    const script = buildBrowserCosmeticProtectionScript({
      ...CONFIGURATION,
      youtubeBestEffort: true,
      youtubeBestEffortAvailable: true,
    }).toLowerCase();
    expect(script).not.toContain('youtube');
    expect(script).not.toContain('googlevideo');
    expect(script).not.toContain('ytp-');
    expect(script).not.toContain('playbackrate');
    expect(script).not.toContain('currenttime');
  });

  it('scopes optional-cookie rejection and never targets Accept', () => {
    const script = buildBrowserCosmeticProtectionScript({
      ...CONFIGURATION,
      rejectOptionalCookies: true,
    });
    expect(script).toContain("hostname === 'myprivacy.dpgmedia.be'");
    expect(script).toContain("hostname === 'myprivacy.dpgmedia.nl'");
    expect(script).toContain("pathname === '/consent'");
    expect(script).toContain("querySelector('#pg-configure-btn')");
    expect(script).toContain("querySelector('#pg-reject-btn')");
    expect(script).toContain('maxConsentAttempts = 24');
    expect(script).not.toContain('#pg-accept-btn');
    expect(script.indexOf('if (isDpgPrivacyHost)')).toBeLessThan(
      script.indexOf("document.createElement('style')"),
    );
  });

  it.each([
    ['myprivacy.dpgmedia.be', '/consent', true],
    ['myprivacy.dpgmedia.nl', '/consent', true],
    ['www.myprivacy.dpgmedia.be', '/consent', false],
    ['myprivacy.dpgmedia.be.evil.example', '/consent', false],
    ['myprivacy.dpgmedia.be', '/consent/', false],
    ['myprivacy.dpgmedia.be', '/other', false],
  ])(
    'only rejects optional cookies on the exact supported scope (%s%s)',
    (hostname, pathname, shouldReject) => {
      const clicks = runConsentScript(
        buildBrowserCosmeticProtectionScript({
          ...CONFIGURATION,
          blockAdsAndTrackers: false,
          hideCookieBanners: false,
          rejectOptionalCookies: true,
        }),
        hostname,
        pathname,
      );
      expect(clicks).toEqual(shouldReject ? ['configure', 'reject'] : []);
    },
  );

  it('does not act on supported consent pages while rejection is disabled', () => {
    expect(
      runConsentScript(
        buildBrowserCosmeticProtectionScript(CONFIGURATION),
        'myprivacy.dpgmedia.be',
        '/consent',
      ),
    ).toEqual([]);
  });

  it('uses only the DPG shadow host for HLN cookie-banner hiding', () => {
    const script = buildBrowserCosmeticProtectionScript(CONFIGURATION);
    expect(script).toContain(
      "hostname === 'hln.be' || hostname.endsWith('.hln.be')",
    );
    expect(script).toContain("? ['#pg-shadow-host-dom']");
    expect(script).toContain("document.body.style.height === '100vh'");
    expect(script).toContain("document.body.style.overflow === 'hidden'");
    expect(script).toContain('hlnConsentHostEncountered');
  });

  it('still injects consent handling when only rejection is active', () => {
    expect(
      buildBrowserCosmeticProtectionScript({
        ...CONFIGURATION,
        blockAdsAndTrackers: false,
        hideCookieBanners: false,
        rejectOptionalCookies: true,
      }),
    ).toContain('#pg-reject-btn');
  });

  it('does not inject page handling when every page preference is off', () => {
    expect(
      buildBrowserCosmeticProtectionScript({
        ...CONFIGURATION,
        blockAdsAndTrackers: false,
        hideCookieBanners: false,
        rejectOptionalCookies: false,
      }),
    ).toBe('true;');
  });
});

function runConsentScript(
  script: string,
  hostname: string,
  pathname: string,
): string[] {
  const clicks: string[] = [];
  const queuedCallbacks: Array<() => void> = [];
  const shadowRoot = {
    querySelector: (selector: string) => {
      if (selector === '#pg-configure-btn') {
        return { click: () => clicks.push('configure') };
      }
      if (selector === '#pg-reject-btn') {
        return { click: () => clicks.push('reject') };
      }
      if (selector.includes('accept')) {
        return { click: () => clicks.push('accept') };
      }
      return null;
    },
  };
  const document = {
    readyState: 'complete',
    querySelector: (selector: string) =>
      selector === '#pg-shadow-host-dom' ? { shadowRoot } : null,
  };
  const window = {
    location: { hostname, pathname },
    setTimeout: (callback: () => void) => {
      queuedCallbacks.push(callback);
      return queuedCallbacks.length;
    },
  };
  const MutationObserver = class {
    observe() {}
  };

  // Run the self-contained page script only against inert test doubles. It
  // cannot access a real DOM, storage or the network.
  // eslint-disable-next-line no-new-func
  new Function('window', 'document', 'MutationObserver', script)(
    window,
    document,
    MutationObserver,
  );
  while (queuedCallbacks.length > 0) {
    queuedCallbacks.shift()?.();
  }
  return clicks;
}
