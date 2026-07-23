export interface BrowserProtectionPreferences {
  blockAdsAndTrackers: boolean;
  blockCrossSiteCookies: boolean;
  hideCookieBanners: boolean;
  rejectOptionalCookies: boolean;
  youtubeBestEffort: boolean;
}

export interface BrowserProtectionConfiguration
  extends BrowserProtectionPreferences {
  nativeRequestBlocking: boolean;
  youtubeBestEffortAvailable: boolean;
}

export const DEFAULT_BROWSER_PROTECTION_PREFERENCES: BrowserProtectionPreferences =
  {
    blockAdsAndTrackers: true,
    blockCrossSiteCookies: true,
    hideCookieBanners: true,
    rejectOptionalCookies: false,
    youtubeBestEffort: false,
  };

export const FALLBACK_BROWSER_PROTECTION_CONFIGURATION: BrowserProtectionConfiguration =
  {
    ...DEFAULT_BROWSER_PROTECTION_PREFERENCES,
    nativeRequestBlocking: false,
    youtubeBestEffortAvailable: false,
  };

const PREFERENCE_KEYS = [
  'blockAdsAndTrackers',
  'blockCrossSiteCookies',
  'hideCookieBanners',
  'rejectOptionalCookies',
  'youtubeBestEffort',
] as const;

const CONFIGURATION_KEYS = [
  ...PREFERENCE_KEYS,
  'nativeRequestBlocking',
  'youtubeBestEffortAvailable',
] as const;

export function decodeBrowserProtectionConfiguration(
  serialized: string,
): BrowserProtectionConfiguration {
  let parsed: unknown;
  try {
    parsed = JSON.parse(serialized);
  } catch {
    throw new Error(
      'The native browser protection configuration is not valid JSON.',
    );
  }

  assertExactBooleanRecord(parsed, CONFIGURATION_KEYS, 'configuration');
  const configuration = parsed as unknown as BrowserProtectionConfiguration;
  if (
    configuration.youtubeBestEffort &&
    !configuration.youtubeBestEffortAvailable
  ) {
    throw new Error(
      'YouTube best-effort blocking is enabled but unavailable in this build.',
    );
  }
  return { ...configuration };
}

export function encodeBrowserProtectionPreferences(
  preferences: BrowserProtectionPreferences,
): string {
  assertExactBooleanRecord(preferences, PREFERENCE_KEYS, 'preferences');
  return JSON.stringify({
    blockAdsAndTrackers: preferences.blockAdsAndTrackers,
    blockCrossSiteCookies: preferences.blockCrossSiteCookies,
    hideCookieBanners: preferences.hideCookieBanners,
    rejectOptionalCookies: preferences.rejectOptionalCookies,
    youtubeBestEffort: preferences.youtubeBestEffort,
  });
}

export function browserProtectionPreferences(
  configuration: BrowserProtectionConfiguration,
): BrowserProtectionPreferences {
  return {
    blockAdsAndTrackers: configuration.blockAdsAndTrackers,
    blockCrossSiteCookies: configuration.blockCrossSiteCookies,
    hideCookieBanners: configuration.hideCookieBanners,
    rejectOptionalCookies: configuration.rejectOptionalCookies,
    youtubeBestEffort: configuration.youtubeBestEffort,
  };
}

export function buildBrowserCosmeticProtectionScript(
  configuration: BrowserProtectionConfiguration,
): string {
  const adSelectors = configuration.blockAdsAndTrackers
    ? [
        '[data-ad-slot]',
        '[data-ad-client]',
        'iframe[src*="doubleclick.net"]',
        'iframe[src*="googlesyndication.com"]',
        'iframe[src*="googleadservices.com"]',
      ]
    : [];
  const genericCookieBannerSelectors = configuration.hideCookieBanners
    ? [
        '#onetrust-banner-sdk',
        '#onetrust-consent-sdk',
        '#CybotCookiebotDialog',
        '.didomi-popup-container',
        '[data-testid="uc-banner-content"]',
        '[data-nosnippet="cookie-banner"]',
      ]
    : [];

  if (
    adSelectors.length === 0 &&
    genericCookieBannerSelectors.length === 0 &&
    !configuration.rejectOptionalCookies
  ) {
    return 'true;';
  }

  const adSelectorJson = JSON.stringify(adSelectors);
  const genericCookieBannerSelectorJson = JSON.stringify(
    genericCookieBannerSelectors,
  );
  const hideCookieBannersJson = JSON.stringify(configuration.hideCookieBanners);
  const rejectOptionalCookiesJson = JSON.stringify(
    configuration.rejectOptionalCookies,
  );
  return `
    (() => {
      const marker = '__masqGenericCosmeticProtectionV2';
      if (window[marker]) {
        return;
      }
      window[marker] = true;

      const hostname = String(window.location.hostname || '').toLowerCase();
      const pathname = String(window.location.pathname || '');
      const isDpgPrivacyHost =
        hostname === 'myprivacy.dpgmedia.be' ||
        hostname === 'myprivacy.dpgmedia.nl';
      const isDpgConsentPage =
        isDpgPrivacyHost && pathname === '/consent';

      // Consent actions are deliberately isolated from generic cosmetic
      // filtering. MASQ only rejects optional cookies after an explicit opt-in.
      if (isDpgPrivacyHost) {
        if (!isDpgConsentPage || !${rejectOptionalCookiesJson}) {
          return;
        }

        const maxConsentAttempts = 24;
        let consentAttempts = 0;
        let configureClicked = false;
        const rejectOptionalCookies = () => {
          consentAttempts += 1;
          const shadowHost = document.querySelector('#pg-shadow-host-dom');
          const shadowRoot = shadowHost && shadowHost.shadowRoot;
          if (shadowRoot) {
            if (!configureClicked) {
              const configureButton =
                shadowRoot.querySelector('#pg-configure-btn');
              if (
                configureButton &&
                typeof configureButton.click === 'function'
              ) {
                configureButton.click();
                configureClicked = true;
              }
            }
            if (configureClicked) {
              const rejectButton = shadowRoot.querySelector('#pg-reject-btn');
              if (
                rejectButton &&
                typeof rejectButton.click === 'function'
              ) {
                rejectButton.click();
                return;
              }
            }
          }
          if (consentAttempts < maxConsentAttempts) {
            window.setTimeout(rejectOptionalCookies, 250);
          }
        };
        if (document.readyState === 'loading') {
          document.addEventListener(
            'DOMContentLoaded',
            rejectOptionalCookies,
            { once: true },
          );
        } else {
          rejectOptionalCookies();
        }
        return;
      }

      const isHln =
        hostname === 'hln.be' || hostname.endsWith('.hln.be');
      const adSelectors = ${adSelectorJson};
      const genericCookieBannerSelectors =
        ${genericCookieBannerSelectorJson};
      const selectors = [
        ...adSelectors,
        ...(${hideCookieBannersJson}
          ? isHln
            ? ['#pg-shadow-host-dom']
            : genericCookieBannerSelectors
          : []),
      ];
      if (selectors.length === 0) {
        return;
      }

      let hlnConsentHostEncountered = false;
      const restoreHlnScroll = () => {
        if (
          !isHln ||
          !hlnConsentHostEncountered ||
          !document.body ||
          !document.body.style
        ) {
          return;
        }
        if (document.body.style.height === '100vh') {
          document.body.style.removeProperty('height');
        }
        if (document.body.style.overflow === 'hidden') {
          document.body.style.removeProperty('overflow');
        }
      };
      const hideElement = (element, selector) => {
        element.style.setProperty('display', 'none', 'important');
        element.setAttribute('aria-hidden', 'true');
        if (isHln && selector === '#pg-shadow-host-dom') {
          hlnConsentHostEncountered = true;
          restoreHlnScroll();
        }
      };
      const hideMatches = root => {
        if (!root || typeof root.querySelectorAll !== 'function') {
          return;
        }
        for (const selector of selectors) {
          for (const element of root.querySelectorAll(selector)) {
            hideElement(element, selector);
          }
        }
      };
      const start = () => {
        const root = document.documentElement;
        if (!root) {
          return;
        }
        const style = document.createElement('style');
        style.setAttribute('data-masq-protection', 'generic-v2');
        style.textContent = selectors
          .map(selector => selector + '{display:none!important;}')
          .join('\\n');
        (document.head || root).appendChild(style);
        hideMatches(document);
        const observerOptions = { childList: true, subtree: true };
        if (isHln) {
          observerOptions.attributes = true;
          observerOptions.attributeFilter = ['style'];
        }
        new MutationObserver(records => {
          for (const record of records) {
            for (const node of record.addedNodes) {
              if (node && node.nodeType === 1) {
                hideMatches(node);
                if (typeof node.matches === 'function') {
                  for (const selector of selectors) {
                    if (node.matches(selector)) {
                      hideElement(node, selector);
                      break;
                    }
                  }
                }
              }
            }
          }
          restoreHlnScroll();
        }).observe(root, observerOptions);
      };
      if (document.documentElement) {
        start();
      } else {
        document.addEventListener('DOMContentLoaded', start, { once: true });
      }
    })();
    true;
  `;
}

function assertExactBooleanRecord(
  value: unknown,
  expectedKeys: readonly string[],
  label: string,
): asserts value is Record<string, boolean> {
  if (!value || typeof value !== 'object' || Array.isArray(value)) {
    throw new Error(`Browser protection ${label} must be an object.`);
  }

  const record = value as Record<string, unknown>;
  const actualKeys = Object.keys(record).sort();
  const sortedExpectedKeys = [...expectedKeys].sort();
  if (
    actualKeys.length !== sortedExpectedKeys.length ||
    actualKeys.some((key, index) => key !== sortedExpectedKeys[index])
  ) {
    throw new Error(
      `Browser protection ${label} contains missing or unsupported fields.`,
    );
  }
  for (const key of expectedKeys) {
    if (typeof record[key] !== 'boolean') {
      throw new Error(
        `Browser protection ${label} field "${key}" must be boolean.`,
      );
    }
  }
}
