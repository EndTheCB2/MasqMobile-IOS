import bundledBrowserProtectionRules from './browserProtectionRules.v3.json';

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

export type BrowserProtectionPreset = 'balanced' | 'strict';

export interface BrowserProtectionRules {
  version: number;
  adSelectors: string[];
  rejectSelectors: string[];
  resolvedBannerSelectors: string[];
}

// Kept in code as the last-known-good fail-safe. A malformed future bundled
// rule file can therefore never disable the reviewed local protection set.
const LAST_KNOWN_GOOD_RULES: BrowserProtectionRules = {
  version: 3,
  adSelectors: [
    '[data-ad-slot]',
    '[data-ad-client]',
    'iframe[src*="doubleclick.net"]',
    'iframe[src*="googlesyndication.com"]',
    'iframe[src*="googleadservices.com"]',
  ],
  rejectSelectors: [
    '#onetrust-reject-all-handler',
    '#CybotCookiebotDialogBodyButtonDecline',
    '#didomi-notice-disagree-button',
    '[data-testid="uc-deny-all-button"]',
  ],
  resolvedBannerSelectors: [
    '#onetrust-banner-sdk',
    '#onetrust-consent-sdk',
    '#CybotCookiebotDialog',
    '.didomi-popup-container',
    '[data-testid="uc-banner-content"]',
    '[data-nosnippet="cookie-banner"]',
  ],
};

export const ACTIVE_BROWSER_PROTECTION_RULES = loadBrowserProtectionRules(
  bundledBrowserProtectionRules,
);
export const BROWSER_PROTECTION_RULES_VERSION =
  ACTIVE_BROWSER_PROTECTION_RULES.version;

export const DEFAULT_BROWSER_PROTECTION_PREFERENCES: BrowserProtectionPreferences =
  {
    blockAdsAndTrackers: true,
    blockCrossSiteCookies: true,
    hideCookieBanners: false,
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

export function browserProtectionPreset(
  preset: BrowserProtectionPreset,
  youtubeBestEffort = false,
): BrowserProtectionPreferences {
  return preset === 'strict'
    ? {
        blockAdsAndTrackers: true,
        blockCrossSiteCookies: true,
        hideCookieBanners: true,
        rejectOptionalCookies: true,
        youtubeBestEffort,
      }
    : {
        blockAdsAndTrackers: true,
        blockCrossSiteCookies: true,
        hideCookieBanners: false,
        rejectOptionalCookies: false,
        youtubeBestEffort,
      };
}

export function buildBrowserCosmeticProtectionScript(
  configuration: BrowserProtectionConfiguration,
): string {
  const adSelectors = configuration.blockAdsAndTrackers
    ? ACTIVE_BROWSER_PROTECTION_RULES.adSelectors
    : [];
  if (adSelectors.length === 0 && !configuration.rejectOptionalCookies) {
    return 'true;';
  }

  const adSelectorJson = JSON.stringify(adSelectors);
  const hideCookieBannersJson = JSON.stringify(configuration.hideCookieBanners);
  const rejectOptionalCookiesJson = JSON.stringify(
    configuration.rejectOptionalCookies,
  );
  const rejectSelectorJson = JSON.stringify(
    ACTIVE_BROWSER_PROTECTION_RULES.rejectSelectors,
  );
  const resolvedBannerSelectorJson = JSON.stringify(
    ACTIVE_BROWSER_PROTECTION_RULES.resolvedBannerSelectors,
  );
  return `
    (() => {
      const marker = '__masqGenericCosmeticProtectionV${BROWSER_PROTECTION_RULES_VERSION}';
      if (window[marker]) {
        return;
      }
      window[marker] = true;

      const hostname = String(window.location.hostname || '').toLowerCase();
      const pathname = String(window.location.pathname || '');
      const adSelectors = ${adSelectorJson};
      const hideElement = element => {
        element.style.setProperty('display', 'none', 'important');
        element.setAttribute('aria-hidden', 'true');
      };
      const hideMatches = (root, selectors) => {
        if (!root || typeof root.querySelectorAll !== 'function') {
          return;
        }
        for (const selector of selectors) {
          for (const element of root.querySelectorAll(selector)) {
            hideElement(element);
          }
        }
      };
      const restoreScroll = () => {
        if (!document.body || !document.body.style) {
          return;
        }
        document.body.style.removeProperty('height');
        document.body.style.removeProperty('overflow');
        document.documentElement.style.removeProperty('overflow');
      };
      const allRoots = () => {
        const roots = [document];
        const knownShadowHosts = [
          '#usercentrics-root',
          '[data-testid="uc-app-container"]',
          '#didomi-host',
          '#onetrust-consent-sdk',
        ];
        if (typeof document.querySelector === 'function') {
          for (const selector of knownShadowHosts) {
            const host = document.querySelector(selector);
            if (host && host.shadowRoot) {
              roots.push(host.shadowRoot);
            }
          }
        }
        return roots;
      };
      const clickExactReject = (root, selector) => {
        const element = root && root.querySelector
          ? root.querySelector(selector)
          : null;
        if (!element || typeof element.click !== 'function') {
          return false;
        }
        element.click();
        return true;
      };
      const rejectSelectors = ${rejectSelectorJson};
      const residualConsentSelectors = ${resolvedBannerSelectorJson};
      let consentRejected = false;
      let dpgConfigured = false;
      let consentAttempts = 0;
      const rejectOptional = () => {
        if (!${rejectOptionalCookiesJson} || consentRejected) {
          return;
        }
        consentAttempts += 1;
        const isDpgConsentPage =
          (hostname === 'myprivacy.dpgmedia.be' ||
            hostname === 'myprivacy.dpgmedia.nl') &&
          pathname === '/consent';
        if (isDpgConsentPage) {
          const host = document.querySelector('#pg-shadow-host-dom');
          const root = host && host.shadowRoot;
          if (root && !dpgConfigured) {
            dpgConfigured = clickExactReject(root, '#pg-configure-btn');
          }
          if (root && dpgConfigured) {
            consentRejected = clickExactReject(root, '#pg-reject-btn');
          }
        } else {
          for (const root of allRoots()) {
            for (const selector of rejectSelectors) {
              if (clickExactReject(root, selector)) {
                consentRejected = true;
                break;
              }
            }
            if (consentRejected) {
              break;
            }
          }
        }
        if (consentRejected) {
          if (${hideCookieBannersJson}) {
            window.setTimeout(() => {
              hideMatches(document, residualConsentSelectors);
              const dpgHost = document.querySelector('#pg-shadow-host-dom');
              if (dpgHost) {
                hideElement(dpgHost);
              }
              restoreScroll();
            }, 120);
          }
          return;
        }
        if (consentAttempts < 32) {
          window.setTimeout(rejectOptional, 250);
        }
      };
      const start = () => {
        const root = document.documentElement;
        if (!root) {
          return;
        }
        if (adSelectors.length > 0) {
          const style = document.createElement('style');
          style.setAttribute(
            'data-masq-protection',
            'generic-v${BROWSER_PROTECTION_RULES_VERSION}',
          );
          style.textContent = adSelectors
            .map(selector => selector + '{display:none!important;}')
            .join('\\n');
          (document.head || root).appendChild(style);
          hideMatches(document, adSelectors);
        }
        new MutationObserver(records => {
          for (const record of records) {
            for (const node of record.addedNodes) {
              if (node && node.nodeType === 1) {
                hideMatches(node, adSelectors);
                if (typeof node.matches === 'function') {
                  for (const selector of adSelectors) {
                    if (node.matches(selector)) {
                      hideElement(node);
                      break;
                    }
                  }
                }
              }
            }
          }
          rejectOptional();
        }).observe(root, { childList: true, subtree: true });
        rejectOptional();
      };
      if (document.documentElement) {
        start();
      } else if (typeof document.addEventListener === 'function') {
        document.addEventListener('DOMContentLoaded', start, { once: true });
      } else {
        rejectOptional();
      }
    })();
    true;
  `;
}

export function loadBrowserProtectionRules(
  candidate: unknown,
): BrowserProtectionRules {
  try {
    if (
      !candidate ||
      typeof candidate !== 'object' ||
      Array.isArray(candidate)
    ) {
      throw new Error('Browser protection rules must be an object.');
    }
    const record = candidate as Record<string, unknown>;
    const expectedKeys = [
      'adSelectors',
      'rejectSelectors',
      'resolvedBannerSelectors',
      'version',
    ];
    if (
      Object.keys(record).sort().join('|') !== expectedKeys.sort().join('|') ||
      typeof record.version !== 'number' ||
      !Number.isInteger(record.version) ||
      record.version < LAST_KNOWN_GOOD_RULES.version
    ) {
      throw new Error('Browser protection rule metadata is invalid.');
    }
    const adSelectors = validSelectorList(record.adSelectors, false);
    const rejectSelectors = validSelectorList(record.rejectSelectors, true);
    const resolvedBannerSelectors = validSelectorList(
      record.resolvedBannerSelectors,
      false,
    );
    return {
      version: record.version,
      adSelectors,
      rejectSelectors,
      resolvedBannerSelectors,
    };
  } catch {
    return {
      version: LAST_KNOWN_GOOD_RULES.version,
      adSelectors: [...LAST_KNOWN_GOOD_RULES.adSelectors],
      rejectSelectors: [...LAST_KNOWN_GOOD_RULES.rejectSelectors],
      resolvedBannerSelectors: [
        ...LAST_KNOWN_GOOD_RULES.resolvedBannerSelectors,
      ],
    };
  }
}

function validSelectorList(value: unknown, rejectOnly: boolean): string[] {
  if (
    !Array.isArray(value) ||
    value.length < 1 ||
    value.length > 1000 ||
    value.some(
      selector =>
        typeof selector !== 'string' ||
        selector.length < 1 ||
        selector.length > 500,
    )
  ) {
    throw new Error('Browser protection selector list is invalid.');
  }
  const selectors = [...new Set(value as string[])];
  if (
    rejectOnly &&
    selectors.some(selector => {
      const normalized = selector.toLowerCase();
      return (
        normalized.includes('accept') ||
        !/(reject|deny|decline|disagree)/.test(normalized)
      );
    })
  ) {
    throw new Error(
      'Consent automation may only target explicit Reject controls.',
    );
  }
  return selectors;
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
