import {
  useCallback,
  useRef,
  useEffect,
  useMemo,
  useState,
  type ComponentType,
  type RefAttributes,
} from 'react';
import {
  ActivityIndicator,
  AppState,
  BackHandler,
  Platform,
  Pressable,
  StyleSheet,
  Text,
  TextInput,
  View,
  type StyleProp,
  type ViewStyle,
} from 'react-native';
import WebView, { type WebViewNavigation } from 'react-native-webview';

import { decideBrowserNavigation } from '../core/browserNavigation';
import {
  browserProtectionPreferences,
  browserProtectionPreset,
  buildBrowserCosmeticProtectionScript,
  type BrowserProtectionConfiguration,
  type BrowserProtectionPreferences,
} from '../core/browserProtection';
import { resolveBrowserInputTarget } from '../core/browserInput';
import { decideBrowserRecovery } from '../core/browserRecovery';
import {
  browserSiteHostname,
  type BrowserSiteSettings,
} from '../core/browserSiteSettings';
import {
  displayBrowserUrl,
  resolveBrowserUrlTarget,
  type BrowserTarget,
} from '../core/browserTarget';
import { masqCore } from '../core/masqCore';
import { colors, radii } from '../ui/theme';

export type BrowserMode = 'masq' | 'direct';
export type BrowserCloseReason = 'user' | 'background';

interface Props {
  mode: BrowserMode;
  onClose: (reason?: BrowserCloseReason) => Promise<void> | void;
}

interface ShouldStartLoadRequest extends WebViewNavigation {
  hasGesture?: boolean;
  isRedirect?: boolean;
  isTopFrame: boolean;
}

interface BrowserWebViewProps {
  allowFileAccess: boolean;
  allowFileAccessFromFileURLs: boolean;
  allowUniversalAccessFromFileURLs: boolean;
  allowsBackForwardNavigationGestures: boolean;
  allowsLinkPreview: boolean;
  cacheEnabled: boolean;
  fraudulentWebsiteWarningEnabled: boolean;
  geolocationEnabled: boolean;
  incognito: boolean;
  injectedJavaScript: string;
  injectedJavaScriptBeforeContentLoaded: string;
  injectedJavaScriptBeforeContentLoadedForMainFrameOnly: boolean;
  injectedJavaScriptForMainFrameOnly: boolean;
  javaScriptCanOpenWindowsAutomatically: boolean;
  javaScriptEnabled: boolean;
  mediaCapturePermissionGrantType: 'deny';
  mediaPlaybackRequiresUserAction: boolean;
  mixedContentMode: 'never';
  onContentProcessDidTerminate: () => void;
  onError: (event: {
    nativeEvent: { code: number; description: string; domain: string };
  }) => void;
  onHttpError: (event: {
    nativeEvent: { statusCode: number; description: string; url: string };
  }) => void;
  onLoad: () => void;
  onLoadEnd: () => void;
  onLoadStart: () => void;
  onNavigationStateChange: (event: WebViewNavigation) => void;
  onShouldStartLoadWithRequest: (request: ShouldStartLoadRequest) => boolean;
  originWhitelist: string[];
  setSupportMultipleWindows: boolean;
  sharedCookiesEnabled: boolean;
  source: { uri: string };
  style: StyleProp<ViewStyle>;
  thirdPartyCookiesEnabled: boolean;
  useSharedProcessPool: boolean;
  webviewDebuggingEnabled: boolean;
}

const BrowserWebView = WebView as unknown as ComponentType<
  BrowserWebViewProps & RefAttributes<WebView>
>;
const MAX_TRANSIENT_RETRIES = 2;
const MAX_HTTPS_REDIRECT_UPGRADES = 4;
const FORM_SUBMISSION_NAVIGATIONS = new Set(['formsubmit', 'formresubmit']);
type BrowserCloseState = 'open' | 'closing' | 'failed';

export function BrowserScreen({ mode, onClose }: Props) {
  const isMasq = mode === 'masq';
  const webView = useRef<WebView>(null);
  const closeInFlight = useRef(false);
  const closeReason = useRef<BrowserCloseReason>('user');
  const retryCount = useRef(0);
  const httpsRedirectUpgrades = useRef(0);
  const retryTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const protectionOperation = useRef(0);
  const siteSettingsOperation = useRef(0);
  const [input, setInput] = useState('');
  const [url, setUrl] = useState<string | null>(null);
  const [isEnsTarget, setIsEnsTarget] = useState(false);
  const [canGoBack, setCanGoBack] = useState(false);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [protection, setProtection] =
    useState<BrowserProtectionConfiguration | null>(null);
  const [protectionBusy, setProtectionBusy] = useState(true);
  const [protectionError, setProtectionError] = useState<string | null>(null);
  const [protectionExpanded, setProtectionExpanded] = useState(false);
  const [siteSettings, setSiteSettings] = useState<BrowserSiteSettings | null>(
    null,
  );
  const [siteSettingsBusy, setSiteSettingsBusy] = useState(false);
  const [webViewGeneration, setWebViewGeneration] = useState(0);
  const [closeState, setCloseState] = useState<BrowserCloseState>('open');
  const [closeError, setCloseError] = useState<string | null>(null);
  const protectionStatusText = siteSettings?.protectionDisabled
    ? 'Off for this site'
    : protectionBusy && !protection
    ? 'Preparing before navigation'
    : protection?.nativeRequestBlocking
    ? 'Network and page filtering'
    : protection
    ? isMasq
      ? 'Private page filtering'
      : 'Page filtering'
    : 'Navigation paused';

  const cancelScheduledRetry = useCallback(() => {
    if (retryTimer.current) {
      clearTimeout(retryTimer.current);
      retryTimer.current = null;
    }
  }, []);

  const prepareProtection = useCallback(async () => {
    const operation = ++protectionOperation.current;
    setProtectionBusy(true);
    setProtectionError(null);
    try {
      const prepared = await masqCore.prepareBrowserProtection();
      if (operation === protectionOperation.current) {
        setProtection(prepared);
      }
    } catch (caught) {
      if (operation === protectionOperation.current) {
        setProtection(null);
        setProtectionError(
          caught instanceof Error
            ? caught.message
            : isMasq
            ? 'Browser protection could not be prepared.'
            : 'Browser safeguards could not be prepared.',
        );
      }
    } finally {
      if (operation === protectionOperation.current) {
        setProtectionBusy(false);
      }
    }
  }, [isMasq]);

  useEffect(() => {
    prepareProtection().catch(() => undefined);
    return () => {
      protectionOperation.current += 1;
      siteSettingsOperation.current += 1;
      cancelScheduledRetry();
    };
  }, [cancelScheduledRetry, prepareProtection]);

  const closeBrowser = useCallback(
    async (reason?: BrowserCloseReason) => {
      if (closeInFlight.current) {
        return;
      }
      const requestedReason = reason ?? closeReason.current;
      closeReason.current = requestedReason;
      closeInFlight.current = true;
      protectionOperation.current += 1;
      siteSettingsOperation.current += 1;
      cancelScheduledRetry();
      // Rendering the close state unmounts the WebView before native blocking is
      // awaited. A rejected acknowledgement never restores the page.
      setCloseState('closing');
      setCloseError(null);
      try {
        await onClose(requestedReason);
      } catch (caught) {
        setCloseState('failed');
        setCloseError(
          caught instanceof Error
            ? caught.message
            : 'Browser traffic could not be confirmed blocked.',
        );
      } finally {
        closeInFlight.current = false;
      }
    },
    [cancelScheduledRetry, onClose],
  );

  useEffect(() => {
    const subscription = AppState.addEventListener('change', state => {
      if (state !== 'active' && closeState === 'open') {
        closeBrowser('background').catch(() => undefined);
      }
    });
    return () => subscription.remove();
  }, [closeBrowser, closeState]);

  useEffect(() => {
    if (Platform.OS !== 'android') {
      return;
    }
    const subscription = BackHandler.addEventListener(
      'hardwareBackPress',
      () => {
        if (closeState !== 'open') {
          return true;
        }
        if (canGoBack) {
          webView.current?.goBack();
          return true;
        }
        closeBrowser().catch(() => undefined);
        return true;
      },
    );
    return () => subscription.remove();
  }, [canGoBack, closeBrowser, closeState]);

  const cosmeticProtectionScript = useMemo(
    () =>
      protection && !siteSettings?.protectionDisabled
        ? buildBrowserCosmeticProtectionScript(protection)
        : 'true;',
    [protection, siteSettings?.protectionDisabled],
  );

  const openTarget = useCallback(
    async (target: BrowserTarget) => {
      const operation = ++siteSettingsOperation.current;
      setSiteSettingsBusy(true);
      setError(null);
      cancelScheduledRetry();
      retryCount.current = 0;
      httpsRedirectUpgrades.current = 0;
      try {
        const hostname = browserSiteHostname(target.transportUrl);
        // Unmount the previous WebView before native code changes the active
        // profile. The destination is mounted only after the exact profile and
        // protection exception have been selected.
        setUrl(null);
        setSiteSettings(null);
        setCanGoBack(false);
        setLoading(false);
        setIsEnsTarget(target.isEns);
        setInput(target.displayUrl);
        const settings = await masqCore.getBrowserSiteSettings(mode, hostname);
        if (operation !== siteSettingsOperation.current) {
          return;
        }
        cancelScheduledRetry();
        retryCount.current = 0;
        httpsRedirectUpgrades.current = 0;
        setSiteSettings(settings);
        setUrl(target.transportUrl);
        setWebViewGeneration(current => current + 1);
      } catch (caught) {
        if (operation === siteSettingsOperation.current) {
          setError(
            caught instanceof Error
              ? caught.message
              : 'Website privacy settings could not be prepared.',
          );
        }
      } finally {
        if (operation === siteSettingsOperation.current) {
          setSiteSettingsBusy(false);
        }
      }
    },
    [cancelScheduledRetry, mode],
  );

  const navigate = async () => {
    if (!protection || protectionBusy) {
      setError(
        isMasq
          ? 'Wait for browser protection to finish preparing.'
          : 'Wait for browser safeguards to finish preparing.',
      );
      return;
    }
    try {
      await openTarget(resolveBrowserInputTarget(input));
    } catch (caught) {
      setError(browserAddressError(caught));
    }
  };

  const navigationChanged = (event: WebViewNavigation) => {
    setCanGoBack(event.canGoBack);
    setInput(displayBrowserUrl(event.url));
  };

  const retryPage = () => {
    if (!protection || protectionBusy) {
      return;
    }
    cancelScheduledRetry();
    retryCount.current = 0;
    setError(null);
    setLoading(true);
    webView.current?.reload();
  };

  const handleLoadError = (
    code: number,
    description: string,
    domain: string,
  ) => {
    cancelScheduledRetry();
    setLoading(false);
    const recovery = decideBrowserRecovery(
      { code, description, domain },
      retryCount.current,
      MAX_TRANSIENT_RETRIES,
      mode,
    );
    retryCount.current = recovery.nextAttempt;
    setError(
      isEnsTarget
        ? `The ENS gateway could not load this .eth website. ${recovery.message}`
        : recovery.message,
    );
    if (recovery.retry && recovery.delayMs !== null) {
      retryTimer.current = setTimeout(() => {
        retryTimer.current = null;
        setError(null);
        setLoading(true);
        webView.current?.reload();
      }, recovery.delayMs);
    }
  };

  const shouldStartNavigation = (request: ShouldStartLoadRequest) => {
    if (!protection || protectionBusy) {
      setError('Navigation was blocked until browser safeguards are ready.');
      return false;
    }
    const decision = decideBrowserNavigation(request);
    if (decision.action === 'allow') {
      if (
        request.isTopFrame &&
        siteSettings &&
        browserSiteHostname(decision.url) !== siteSettings.hostname
      ) {
        if (FORM_SUBMISSION_NAVIGATIONS.has(request.navigationType ?? '')) {
          setError(
            'The browser blocked this form submission because changing website session profiles would discard the submitted data. Open the destination first.',
          );
          return false;
        }
        // Every cross-site top-frame transition, including a server redirect,
        // is re-opened only after the destination's exact-host profile and
        // protection exception have been selected. This prevents a remembered
        // session or a site exception from silently following the redirect.
        openTarget(resolveBrowserUrlTarget(decision.url)).catch(
          () => undefined,
        );
        return false;
      }
      return true;
    }
    if (decision.action === 'block') {
      if (decision.message) {
        setError(decision.message);
      }
      return false;
    }

    if (
      httpsRedirectUpgrades.current >= MAX_HTTPS_REDIRECT_UPGRADES ||
      decision.url === url
    ) {
      setError(
        'The website repeatedly tried to leave HTTPS. Navigation was blocked.',
      );
      return false;
    }

    httpsRedirectUpgrades.current += 1;
    cancelScheduledRetry();
    retryCount.current = 0;
    const redirectedTarget = resolveBrowserUrlTarget(decision.url);
    openTarget({
      ...redirectedTarget,
      displayUrl:
        decision.action === 'redirect'
          ? decision.displayUrl
          : redirectedTarget.displayUrl,
    }).catch(() => undefined);
    return false;
  };

  const applyProtection = async (next: BrowserProtectionPreferences) => {
    if (!protection || protectionBusy) {
      return;
    }
    if (next.youtubeBestEffort && !protection.youtubeBestEffortAvailable) {
      return;
    }
    const operation = ++protectionOperation.current;
    setProtectionBusy(true);
    setProtectionError(null);
    try {
      const applied = await masqCore.setBrowserProtection(next);
      if (operation !== protectionOperation.current) {
        return;
      }
      setProtection(applied);
      cancelScheduledRetry();
      retryCount.current = 0;
      httpsRedirectUpgrades.current = 0;
      setCanGoBack(false);
      setError(null);
      setLoading(Boolean(url));
      setWebViewGeneration(current => current + 1);
    } catch (caught) {
      if (operation === protectionOperation.current) {
        setProtectionError(
          caught instanceof Error
            ? caught.message
            : 'Browser protection settings could not be applied.',
        );
      }
    } finally {
      if (operation === protectionOperation.current) {
        setProtectionBusy(false);
      }
    }
  };

  const toggleProtection = async (key: keyof BrowserProtectionPreferences) => {
    if (!protection || protectionBusy) {
      return;
    }
    const preferences = browserProtectionPreferences(protection);
    await applyProtection({
      ...preferences,
      [key]: !preferences[key],
    });
  };

  const updateSiteSettings = async (
    rememberSignIn: boolean,
    protectionDisabled: boolean,
  ) => {
    if (!siteSettings || siteSettingsBusy) {
      return;
    }
    const operation = ++siteSettingsOperation.current;
    setSiteSettingsBusy(true);
    setProtectionError(null);
    try {
      const updated = await masqCore.setBrowserSiteSettings(
        mode,
        siteSettings.hostname,
        rememberSignIn,
        protectionDisabled,
      );
      if (operation !== siteSettingsOperation.current) {
        return;
      }
      setSiteSettings(updated);
      if (updated.protectionDisabled !== siteSettings.protectionDisabled) {
        await prepareProtection();
      }
      setWebViewGeneration(current => current + 1);
    } catch (caught) {
      if (operation === siteSettingsOperation.current) {
        setProtectionError(
          caught instanceof Error
            ? caught.message
            : 'Website privacy settings could not be saved.',
        );
      }
    } finally {
      if (operation === siteSettingsOperation.current) {
        setSiteSettingsBusy(false);
      }
    }
  };

  const forgetCurrentSite = async () => {
    if (!siteSettings || siteSettingsBusy) {
      return;
    }
    const operation = ++siteSettingsOperation.current;
    setSiteSettingsBusy(true);
    setProtectionError(null);
    try {
      const updated = await masqCore.clearBrowserSiteData(
        mode,
        siteSettings.hostname,
      );
      if (operation === siteSettingsOperation.current) {
        setSiteSettings(updated);
        await prepareProtection();
        setWebViewGeneration(current => current + 1);
      }
    } catch (caught) {
      if (operation === siteSettingsOperation.current) {
        setProtectionError(
          caught instanceof Error
            ? caught.message
            : 'Website data could not be removed.',
        );
      }
    } finally {
      if (operation === siteSettingsOperation.current) {
        setSiteSettingsBusy(false);
      }
    }
  };

  const clearRememberedSignIns = async () => {
    if (siteSettingsBusy) {
      return;
    }
    const operation = ++siteSettingsOperation.current;
    setSiteSettingsBusy(true);
    setProtectionError(null);
    try {
      await masqCore.clearRememberedBrowserData();
      if (operation === siteSettingsOperation.current) {
        setSiteSettings(current =>
          current ? { ...current, rememberSignIn: false } : current,
        );
        setWebViewGeneration(current => current + 1);
      }
    } catch (caught) {
      if (operation === siteSettingsOperation.current) {
        setProtectionError(
          caught instanceof Error
            ? caught.message
            : 'Remembered website sessions could not be removed.',
        );
      }
    } finally {
      if (operation === siteSettingsOperation.current) {
        setSiteSettingsBusy(false);
      }
    }
  };

  if (closeState !== 'open') {
    const closeFailed = closeState === 'failed';
    return (
      <View
        accessibilityViewIsModal
        style={[styles.screen, styles.closeState]}
        testID="browser-close-state"
      >
        {closeFailed ? null : (
          <ActivityIndicator color={colors.violet} size="large" />
        )}
        <Text style={styles.closeStateTitle}>
          {closeFailed
            ? 'Browser close needs confirmation'
            : 'Closing browser safely'}
        </Text>
        <Text style={styles.closeStateText}>
          {closeFailed
            ? 'The page stays closed. Retry the close confirmation; browsing will not resume.'
            : 'The page is closed while MASQ confirms that browser traffic is blocked.'}
        </Text>
        {closeError ? (
          <Text accessibilityRole="alert" style={styles.closeStateError}>
            {closeError}
          </Text>
        ) : null}
        {closeFailed ? (
          <Pressable
            accessibilityLabel="Retry closing browser"
            accessibilityRole="button"
            onPress={() => closeBrowser().catch(() => undefined)}
            style={styles.closeStateButton}
          >
            <Text style={styles.closeStateButtonText}>Retry close</Text>
          </Pressable>
        ) : null}
      </View>
    );
  }

  return (
    <View style={styles.screen}>
      <View style={styles.topBar}>
        <Pressable
          accessibilityLabel={
            isMasq ? 'Close private browser' : 'Close direct browser'
          }
          accessibilityRole="button"
          onPress={() => closeBrowser().catch(() => undefined)}
          style={styles.iconButton}
        >
          <Text style={styles.close}>×</Text>
        </Pressable>
        <View style={[styles.protected, !isMasq && styles.directBadge]}>
          <View
            style={[styles.protectedDot, !isMasq && styles.directBadgeDot]}
          />
          <Text
            style={[styles.protectedText, !isMasq && styles.directBadgeText]}
          >
            {isMasq ? 'MASQ PRIVATE' : 'DIRECT · MASQ OFF'}
          </Text>
        </View>
        <Pressable
          accessibilityLabel={
            isMasq ? 'Reload page privately' : 'Reload direct page'
          }
          accessibilityRole="button"
          disabled={!url || !protection || protectionBusy}
          onPress={retryPage}
          style={[
            styles.iconButton,
            (!url || !protection || protectionBusy) && styles.disabled,
          ]}
        >
          <Text style={styles.reload}>↻</Text>
        </Pressable>
      </View>
      <View style={styles.addressRow}>
        <Pressable
          accessibilityLabel={
            isMasq ? 'Go back in private browser' : 'Go back in direct browser'
          }
          accessibilityRole="button"
          disabled={!canGoBack || protectionBusy}
          onPress={() => webView.current?.goBack()}
          style={[
            styles.backButton,
            (!canGoBack || protectionBusy) && styles.disabled,
          ]}
        >
          <Text style={styles.back}>‹</Text>
        </Pressable>
        <TextInput
          accessibilityLabel={
            isMasq
              ? 'Private search or web address'
              : 'Direct search or web address'
          }
          autoCapitalize="none"
          autoCorrect={false}
          editable={Boolean(protection) && !protectionBusy && !siteSettingsBusy}
          keyboardType="default"
          onChangeText={setInput}
          onSubmitEditing={() => navigate().catch(() => undefined)}
          placeholder="Search with Timpi or enter a website"
          placeholderTextColor="#61788B"
          returnKeyType="search"
          selectTextOnFocus
          style={styles.address}
          value={input}
        />
      </View>
      <View style={styles.protectionPanel}>
        <Pressable
          accessibilityHint="Expands or collapses the protection settings"
          accessibilityLabel={
            isMasq
              ? 'Browser protection settings'
              : 'Browser safeguards settings'
          }
          accessibilityRole="button"
          accessibilityState={{
            busy: protectionBusy,
            expanded: protectionExpanded,
          }}
          accessibilityValue={{ text: protectionStatusText }}
          hitSlop={4}
          onPress={() => setProtectionExpanded(expanded => !expanded)}
          style={styles.protectionHeading}
        >
          <View style={styles.protectionHeadingCopy}>
            <Text style={styles.protectionTitle}>
              {isMasq ? 'Browser protection' : 'Browser safeguards'}
            </Text>
            <Text style={styles.protectionStatus}>{protectionStatusText}</Text>
          </View>
          <View style={styles.protectionHeadingAction}>
            {protectionBusy ? (
              <ActivityIndicator color={colors.violet} size="small" />
            ) : null}
            <Text accessibilityElementsHidden style={styles.protectionChevron}>
              {protectionExpanded ? '▴' : '▾'}
            </Text>
          </View>
        </Pressable>
        {protectionExpanded ? (
          <>
            {protection ? (
              <View style={styles.protectionOptions}>
                <PresetButton
                  disabled={protectionBusy}
                  label="Balanced"
                  onPress={() =>
                    applyProtection(
                      browserProtectionPreset(
                        'balanced',
                        protection.youtubeBestEffort,
                      ),
                    ).catch(() => undefined)
                  }
                />
                <PresetButton
                  disabled={protectionBusy}
                  label="Strict"
                  onPress={() =>
                    applyProtection(
                      browserProtectionPreset(
                        'strict',
                        protection.youtubeBestEffort,
                      ),
                    ).catch(() => undefined)
                  }
                />
                <ProtectionToggle
                  disabled={protectionBusy}
                  enabled={protection.blockAdsAndTrackers}
                  label="Ads & trackers"
                  onPress={() =>
                    toggleProtection('blockAdsAndTrackers').catch(
                      () => undefined,
                    )
                  }
                />
                <ProtectionToggle
                  disabled={protectionBusy}
                  enabled={protection.blockCrossSiteCookies}
                  label="Cross-site cookies"
                  onPress={() =>
                    toggleProtection('blockCrossSiteCookies').catch(
                      () => undefined,
                    )
                  }
                />
                <ProtectionToggle
                  disabled={protectionBusy}
                  enabled={protection.hideCookieBanners}
                  label="Hide resolved banners"
                  onPress={() =>
                    toggleProtection('hideCookieBanners').catch(() => undefined)
                  }
                />
                <ProtectionToggle
                  disabled={protectionBusy}
                  enabled={protection.rejectOptionalCookies}
                  label="Reject optional cookies"
                  onPress={() =>
                    toggleProtection('rejectOptionalCookies').catch(
                      () => undefined,
                    )
                  }
                />
                {protection.youtubeBestEffortAvailable ? (
                  <ProtectionToggle
                    disabled={protectionBusy}
                    enabled={protection.youtubeBestEffort}
                    label="YouTube best effort"
                    onPress={() =>
                      toggleProtection('youtubeBestEffort').catch(
                        () => undefined,
                      )
                    }
                  />
                ) : null}
              </View>
            ) : null}
            {siteSettings ? (
              <View style={styles.siteSettings}>
                <Text style={styles.siteSettingsTitle}>
                  This site · {siteSettings.hostname}
                </Text>
                <View style={styles.protectionOptions}>
                  <ProtectionToggle
                    disabled={siteSettingsBusy || protectionBusy}
                    enabled={siteSettings.protectionDisabled}
                    label="Protection off for this site"
                    onPress={() =>
                      updateSiteSettings(
                        siteSettings.rememberSignIn,
                        !siteSettings.protectionDisabled,
                      ).catch(() => undefined)
                    }
                  />
                  <ProtectionToggle
                    disabled={
                      siteSettingsBusy ||
                      !siteSettings.persistentSessionsSupported
                    }
                    enabled={siteSettings.rememberSignIn}
                    label="Remember sign-in for this site"
                    onPress={() =>
                      updateSiteSettings(
                        !siteSettings.rememberSignIn,
                        siteSettings.protectionDisabled,
                      ).catch(() => undefined)
                    }
                  />
                </View>
                {!siteSettings.persistentSessionsSupported ? (
                  <Text style={styles.protectionHint}>
                    This device WebView cannot isolate remembered website
                    sessions. Sign-in stays temporary.
                  </Text>
                ) : (
                  <Text style={styles.protectionHint}>
                    Remembered sign-in stores WebView cookies and website data
                    in the selected MASQ or Direct profile. Cross-site links and
                    redirects switch profiles. Android blocks top-frame non-GET
                    forms that could bypass that switch. MASQ never reads
                    passwords or session tokens. Some providers, including
                    Google, may refuse embedded sign-in.
                  </Text>
                )}
                <View style={styles.siteActions}>
                  <Pressable
                    accessibilityLabel="Forget data for this site"
                    accessibilityRole="button"
                    disabled={siteSettingsBusy}
                    onPress={() => forgetCurrentSite().catch(() => undefined)}
                  >
                    <Text style={styles.siteAction}>Forget this site</Text>
                  </Pressable>
                  <Pressable
                    accessibilityLabel="Clear all remembered sign-ins"
                    accessibilityRole="button"
                    disabled={siteSettingsBusy}
                    onPress={() =>
                      clearRememberedSignIns().catch(() => undefined)
                    }
                  >
                    <Text style={styles.siteAction}>
                      Clear all remembered sign-ins
                    </Text>
                  </Pressable>
                </View>
              </View>
            ) : null}
            {protection && !protection.youtubeBestEffortAvailable ? (
              <Text style={styles.protectionHint}>
                YouTube-specific ad filtering is unavailable in this public
                build.
              </Text>
            ) : null}
            {protection?.youtubeBestEffortAvailable ? (
              <Text style={styles.protectionWarning}>
                Experimental: may interrupt playback and may conflict with
                YouTube&apos;s terms.
              </Text>
            ) : null}
            {protection ? (
              <Text style={styles.protectionHint}>
                {isMasq
                  ? 'Consent rejection only uses verified Reject controls on supported consent managers. Unknown consent gates stay visible; MASQ never selects Accept.'
                  : 'Consent rejection only uses verified Reject controls on supported consent managers. Unknown consent gates stay visible; the browser never selects Accept.'}
              </Text>
            ) : null}
            {protection ? (
              <Text style={styles.protectionHint}>
                Filter changes reload this page. Temporary sessions remain
                isolated; explicitly remembered sessions are not erased.
              </Text>
            ) : null}
          </>
        ) : null}
        {protectionError ? (
          <View accessibilityRole="alert" style={styles.protectionErrorRow}>
            <Text style={styles.protectionError}>{protectionError}</Text>
            <Pressable
              accessibilityLabel={
                isMasq ? 'Retry browser protection' : 'Retry browser safeguards'
              }
              accessibilityRole="button"
              disabled={protectionBusy}
              onPress={() => prepareProtection().catch(() => undefined)}
            >
              <Text style={styles.protectionRetry}>
                {isMasq ? 'Retry protection' : 'Retry safeguards'}
              </Text>
            </Pressable>
          </View>
        ) : null}
      </View>
      {error ? (
        <View accessibilityRole="alert" style={styles.errorRow}>
          <Text style={styles.error}>{error}</Text>
          {url ? (
            <Pressable
              accessibilityRole="button"
              disabled={protectionBusy}
              onPress={retryPage}
            >
              <Text style={styles.retry}>Retry</Text>
            </Pressable>
          ) : null}
        </View>
      ) : null}
      <View style={styles.webContainer}>
        {url && protection && siteSettings && !siteSettingsBusy ? (
          <BrowserWebView
            key={`${mode}-webview-${webViewGeneration}`}
            ref={webView}
            allowFileAccess={false}
            allowFileAccessFromFileURLs={false}
            allowUniversalAccessFromFileURLs={false}
            allowsBackForwardNavigationGestures
            allowsLinkPreview={false}
            cacheEnabled={Boolean(siteSettings?.rememberSignIn)}
            fraudulentWebsiteWarningEnabled
            geolocationEnabled={false}
            incognito={Platform.OS === 'ios' && isMasq}
            injectedJavaScript={cosmeticProtectionScript}
            injectedJavaScriptBeforeContentLoaded={cosmeticProtectionScript}
            injectedJavaScriptBeforeContentLoadedForMainFrameOnly={
              Platform.OS !== 'ios'
            }
            injectedJavaScriptForMainFrameOnly={Platform.OS !== 'ios'}
            javaScriptCanOpenWindowsAutomatically={false}
            javaScriptEnabled
            mediaCapturePermissionGrantType="deny"
            mediaPlaybackRequiresUserAction
            mixedContentMode="never"
            onContentProcessDidTerminate={() => {
              handleLoadError(
                -1005,
                'The browser content process terminated.',
                'WebContent',
              );
            }}
            onError={({ nativeEvent }) =>
              handleLoadError(
                nativeEvent.code,
                nativeEvent.description,
                nativeEvent.domain,
              )
            }
            onHttpError={({ nativeEvent }) => {
              cancelScheduledRetry();
              retryCount.current = 0;
              setLoading(false);
              setError(
                isEnsTarget
                  ? `The ENS gateway returned HTTP ${nativeEvent.statusCode} for this .eth website.`
                  : isMasq
                  ? `The website returned HTTP ${nativeEvent.statusCode} through MASQ.`
                  : `The website returned HTTP ${nativeEvent.statusCode}.`,
              );
            }}
            onLoad={() => {
              retryCount.current = 0;
              httpsRedirectUpgrades.current = 0;
            }}
            onLoadEnd={() => setLoading(false)}
            onLoadStart={() => {
              setError(null);
              setLoading(true);
            }}
            onNavigationStateChange={navigationChanged}
            onShouldStartLoadWithRequest={shouldStartNavigation}
            originWhitelist={[
              // react-native-webview opens non-whitelisted URLs through Linking
              // before our callback. Route every scheme through our stricter
              // synchronous HTTPS policy so Safari is never a fallback.
              '*',
            ]}
            setSupportMultipleWindows={false}
            sharedCookiesEnabled={false}
            source={{ uri: url }}
            style={styles.webView}
            thirdPartyCookiesEnabled={
              Boolean(siteSettings?.protectionDisabled) ||
              !protection.blockCrossSiteCookies
            }
            useSharedProcessPool={false}
            webviewDebuggingEnabled={false}
          />
        ) : (
          <View style={styles.startPage}>
            <View style={[styles.shield, !isMasq && styles.directShield]}>
              <Text
                style={[styles.shieldText, !isMasq && styles.directShieldText]}
              >
                {isMasq ? 'M' : 'D'}
              </Text>
            </View>
            <Text style={styles.startTitle}>
              {protection
                ? isMasq
                  ? 'Private session ready'
                  : 'Direct browser ready'
                : protectionBusy
                ? isMasq
                  ? 'Preparing private session'
                  : 'Preparing direct browser'
                : isMasq
                ? 'Browser protection required'
                : 'Browser safeguards required'}
            </Text>
            <Text style={styles.startText}>
              Website sessions are temporary by default. You can opt in to
              remembering a sign-in for an individual site; MASQ and Direct keep
              separate browser profiles.
            </Text>
            <Text style={styles.startHint}>
              {protection
                ? isMasq
                  ? 'Search with Timpi or enter a public HTTPS address to browse through MASQ.'
                  : 'Search with Timpi or enter a public HTTPS address using your normal internet connection. Direct browsing is identified by the compact badge above.'
                : isMasq
                ? 'Resolve browser protection above before opening a website.'
                : 'Resolve browser safeguards above before opening a website.'}
            </Text>
            {protection ? (
              <Text style={styles.searchProvider}>
                Free-text searches open Timpi Search. ENS .eth websites use the
                HTTPS eth.limo gateway.
              </Text>
            ) : null}
          </View>
        )}
        {loading ? (
          <View pointerEvents="none" style={styles.loader}>
            <ActivityIndicator color={colors.violet} size="large" />
            <Text style={styles.loaderText}>
              {isMasq ? 'Loading through MASQ…' : 'Loading directly…'}
            </Text>
          </View>
        ) : null}
      </View>
    </View>
  );
}

function ProtectionToggle({
  disabled,
  enabled,
  label,
  onPress,
}: {
  disabled: boolean;
  enabled: boolean;
  label: string;
  onPress: () => void;
}) {
  return (
    <Pressable
      accessibilityLabel={label}
      accessibilityRole="switch"
      accessibilityState={{ checked: enabled, disabled }}
      disabled={disabled}
      onPress={onPress}
      style={[
        styles.protectionOption,
        enabled && styles.protectionOptionEnabled,
        disabled && styles.disabled,
      ]}
    >
      <View
        style={[
          styles.protectionSwitch,
          enabled && styles.protectionSwitchEnabled,
        ]}
      >
        <View
          style={[
            styles.protectionSwitchThumb,
            enabled && styles.protectionSwitchThumbEnabled,
          ]}
        />
      </View>
      <Text style={styles.protectionOptionText}>{label}</Text>
    </Pressable>
  );
}

function PresetButton({
  disabled,
  label,
  onPress,
}: {
  disabled: boolean;
  label: string;
  onPress: () => void;
}) {
  return (
    <Pressable
      accessibilityLabel={`${label} browser protection`}
      accessibilityRole="button"
      disabled={disabled}
      onPress={onPress}
      style={[styles.presetButton, disabled && styles.disabled]}
    >
      <Text style={styles.presetButtonText}>{label}</Text>
    </Pressable>
  );
}

function browserAddressError(caught: unknown): string {
  if (!(caught instanceof Error)) {
    return 'Invalid web address.';
  }
  if (caught.message === 'MASQ Mobile only allows HTTPS addresses.') {
    return 'This browser only allows HTTPS addresses.';
  }
  if (caught.message === 'Local addresses cannot be opened through MASQ.') {
    return 'Local addresses cannot be opened in this browser.';
  }
  return caught.message;
}

const styles = StyleSheet.create({
  screen: { backgroundColor: colors.ink, flex: 1 },
  closeState: {
    alignItems: 'center',
    justifyContent: 'center',
    paddingHorizontal: 28,
  },
  closeStateTitle: {
    color: colors.white,
    fontSize: 21,
    fontWeight: '800',
    marginTop: 18,
    textAlign: 'center',
  },
  closeStateText: {
    color: colors.muted,
    fontSize: 14,
    lineHeight: 21,
    marginTop: 10,
    maxWidth: 420,
    textAlign: 'center',
  },
  closeStateError: {
    color: '#FF9EAB',
    fontSize: 13,
    lineHeight: 19,
    marginTop: 12,
    maxWidth: 420,
    textAlign: 'center',
  },
  closeStateButton: {
    backgroundColor: colors.violet,
    borderRadius: radii.medium,
    marginTop: 20,
    minWidth: 150,
    paddingHorizontal: 18,
    paddingVertical: 12,
  },
  closeStateButtonText: {
    color: colors.white,
    fontSize: 14,
    fontWeight: '800',
    textAlign: 'center',
  },
  topBar: {
    alignItems: 'center',
    flexDirection: 'row',
    justifyContent: 'space-between',
    minHeight: 50,
    paddingHorizontal: 12,
  },
  iconButton: {
    alignItems: 'center',
    height: 40,
    justifyContent: 'center',
    width: 40,
  },
  close: { color: colors.white, fontSize: 30, fontWeight: '300' },
  reload: { color: colors.white, fontSize: 24 },
  protected: { alignItems: 'center', flexDirection: 'row', gap: 7 },
  directBadge: {
    backgroundColor: '#33250C',
    borderColor: '#8E6720',
    borderRadius: radii.pill,
    borderWidth: 1,
    paddingHorizontal: 10,
    paddingVertical: 6,
  },
  protectedDot: {
    backgroundColor: colors.mint,
    borderRadius: 5,
    height: 9,
    width: 9,
  },
  directBadgeDot: { backgroundColor: '#F3B94F' },
  protectedText: {
    color: colors.mint,
    fontSize: 11,
    fontWeight: '900',
    letterSpacing: 1.2,
  },
  directBadgeText: { color: '#FFD27A' },
  addressRow: {
    flexDirection: 'row',
    gap: 8,
    paddingBottom: 10,
    paddingHorizontal: 12,
  },
  protectionPanel: {
    backgroundColor: colors.panel,
    borderColor: colors.line,
    borderRadius: radii.medium,
    borderWidth: 1,
    marginBottom: 10,
    marginHorizontal: 12,
    padding: 10,
  },
  protectionHeading: {
    alignItems: 'center',
    flexDirection: 'row',
    justifyContent: 'space-between',
    minHeight: 44,
  },
  protectionHeadingCopy: {
    flex: 1,
  },
  protectionHeadingAction: {
    alignItems: 'center',
    flexDirection: 'row',
    gap: 10,
    paddingLeft: 10,
  },
  protectionChevron: {
    color: colors.white,
    fontSize: 16,
    fontWeight: '800',
    textAlign: 'center',
    width: 18,
  },
  protectionTitle: {
    color: colors.white,
    fontSize: 12,
    fontWeight: '800',
  },
  protectionStatus: {
    color: colors.muted,
    fontSize: 10,
    marginTop: 2,
  },
  protectionOptions: {
    flexDirection: 'row',
    flexWrap: 'wrap',
    gap: 7,
    marginTop: 9,
  },
  protectionOption: {
    alignItems: 'center',
    backgroundColor: colors.panelRaised,
    borderColor: colors.line,
    borderRadius: radii.small,
    borderWidth: 1,
    flexDirection: 'row',
    gap: 6,
    minHeight: 34,
    paddingHorizontal: 8,
  },
  protectionOptionEnabled: {
    borderColor: '#6B55A0',
  },
  presetButton: {
    alignItems: 'center',
    backgroundColor: '#261C3E',
    borderColor: colors.violet,
    borderRadius: radii.small,
    borderWidth: 1,
    justifyContent: 'center',
    minHeight: 34,
    paddingHorizontal: 12,
  },
  presetButtonText: {
    color: '#D8CBFF',
    fontSize: 10,
    fontWeight: '800',
  },
  protectionOptionText: {
    color: colors.white,
    fontSize: 10,
    fontWeight: '700',
  },
  protectionHint: {
    color: colors.muted,
    fontSize: 9,
    lineHeight: 13,
    marginTop: 7,
  },
  siteSettings: {
    borderTopColor: colors.line,
    borderTopWidth: 1,
    marginTop: 10,
    paddingTop: 9,
  },
  siteSettingsTitle: {
    color: colors.white,
    fontSize: 10,
    fontWeight: '800',
  },
  siteActions: {
    flexDirection: 'row',
    flexWrap: 'wrap',
    gap: 14,
    marginTop: 8,
  },
  siteAction: {
    color: colors.violet,
    fontSize: 10,
    fontWeight: '800',
  },
  protectionWarning: {
    color: '#FFD29B',
    fontSize: 9,
    lineHeight: 13,
    marginTop: 7,
  },
  protectionSwitch: {
    backgroundColor: '#4A5B69',
    borderRadius: 8,
    height: 14,
    padding: 2,
    width: 24,
  },
  protectionSwitchEnabled: {
    backgroundColor: colors.violet,
  },
  protectionSwitchThumb: {
    backgroundColor: colors.white,
    borderRadius: 5,
    height: 10,
    width: 10,
  },
  protectionSwitchThumbEnabled: {
    transform: [{ translateX: 10 }],
  },
  protectionErrorRow: {
    marginTop: 8,
  },
  protectionError: {
    color: '#FFB7C0',
    fontSize: 10,
    lineHeight: 14,
  },
  protectionRetry: {
    color: colors.violet,
    fontSize: 11,
    fontWeight: '800',
    marginTop: 5,
  },
  backButton: {
    alignItems: 'center',
    backgroundColor: colors.panel,
    borderRadius: 12,
    height: 44,
    justifyContent: 'center',
    width: 44,
  },
  disabled: { opacity: 0.35 },
  back: {
    color: colors.white,
    fontSize: 32,
    fontWeight: '300',
    lineHeight: 34,
  },
  address: {
    backgroundColor: colors.panel,
    borderColor: colors.line,
    borderRadius: radii.medium,
    borderWidth: 1,
    color: colors.white,
    flex: 1,
    fontSize: 14,
    height: 44,
    paddingHorizontal: 14,
  },
  errorRow: {
    alignItems: 'center',
    backgroundColor: '#351A22',
    flexDirection: 'row',
    gap: 10,
    paddingHorizontal: 14,
    paddingVertical: 8,
  },
  error: { color: '#FFB7C0', flex: 1, fontSize: 12 },
  retry: { color: colors.white, fontSize: 12, fontWeight: '900' },
  webContainer: { backgroundColor: '#07121D', flex: 1 },
  webView: { backgroundColor: colors.white, flex: 1 },
  startPage: {
    alignItems: 'center',
    flex: 1,
    justifyContent: 'center',
    paddingHorizontal: 36,
  },
  shield: {
    alignItems: 'center',
    backgroundColor: '#112E43',
    borderColor: colors.violet,
    borderRadius: 32,
    borderWidth: 1,
    height: 64,
    justifyContent: 'center',
    width: 64,
  },
  shieldText: { color: colors.mint, fontSize: 25, fontWeight: '900' },
  directShield: {
    backgroundColor: '#33250C',
    borderColor: '#8E6720',
  },
  directShieldText: { color: '#FFD27A' },
  startTitle: {
    color: colors.white,
    fontSize: 24,
    fontWeight: '800',
    marginTop: 20,
  },
  startText: {
    color: colors.muted,
    fontSize: 14,
    lineHeight: 21,
    marginTop: 10,
    textAlign: 'center',
  },
  startHint: {
    color: '#6E8AA0',
    fontSize: 12,
    marginTop: 18,
    textAlign: 'center',
  },
  searchProvider: {
    color: colors.violet,
    fontSize: 12,
    fontWeight: '700',
    marginTop: 10,
    textAlign: 'center',
  },
  loader: {
    alignItems: 'center',
    backgroundColor: colors.white,
    bottom: 0,
    justifyContent: 'center',
    left: 0,
    position: 'absolute',
    right: 0,
    top: 0,
  },
  loaderText: { color: '#5F5B69', fontSize: 13, marginTop: 12 },
});
