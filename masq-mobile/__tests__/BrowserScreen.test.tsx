import React from 'react';
import { TextInput } from 'react-native';
import ReactTestRenderer from 'react-test-renderer';

import type {
  BrowserProtectionConfiguration,
  BrowserProtectionPreferences,
} from '../src/core/browserProtection';

const mockReload = jest.fn();
const mockGoBack = jest.fn();
const mockPrepareBrowserProtection = jest.fn();
const mockSetBrowserProtection = jest.fn();
const PUBLIC_PROTECTION: BrowserProtectionConfiguration = {
  blockAdsAndTrackers: true,
  blockCrossSiteCookies: true,
  hideCookieBanners: true,
  rejectOptionalCookies: false,
  youtubeBestEffort: false,
  nativeRequestBlocking: true,
  youtubeBestEffortAvailable: false,
};

jest.mock('react-native-webview', () => {
  const ReactModule = require('react');
  const ReactNative = require('react-native');
  const MockWebView = ReactModule.forwardRef((props: object, ref: unknown) => {
    ReactModule.useImperativeHandle(ref, () => ({
      goBack: mockGoBack,
      reload: mockReload,
    }));
    return ReactModule.createElement(ReactNative.View, {
      ...props,
      testID: 'private-webview',
    });
  });
  return { __esModule: true, default: MockWebView, WebView: MockWebView };
});

import { masqCore } from '../src/core/masqCore';
import { BrowserScreen } from '../src/screens/BrowserScreen';

describe('BrowserScreen recovery lifecycle', () => {
  beforeEach(() => {
    mockReload.mockReset();
    mockGoBack.mockReset();
    mockPrepareBrowserProtection
      .mockReset()
      .mockResolvedValue({ ...PUBLIC_PROTECTION });
    mockSetBrowserProtection.mockReset().mockImplementation(
      async (
        preferences: BrowserProtectionPreferences,
      ): Promise<BrowserProtectionConfiguration> => ({
        ...PUBLIC_PROTECTION,
        ...preferences,
      }),
    );
    jest
      .spyOn(masqCore, 'prepareBrowserProtection')
      .mockImplementation(mockPrepareBrowserProtection);
    jest
      .spyOn(masqCore, 'setBrowserProtection')
      .mockImplementation(mockSetBrowserProtection);
  });

  afterEach(() => {
    jest.restoreAllMocks();
    jest.useRealTimers();
  });

  it('cancels a scheduled retry when the user navigates elsewhere', async () => {
    const renderer = await renderBrowser();
    jest.useFakeTimers();
    const address = renderer.root.findByType(TextInput);
    ReactTestRenderer.act(() => {
      address.props.onChangeText('first.example');
    });
    await submitAddress(address);
    const webView = renderer.root.findByProps({ testID: 'private-webview' });
    ReactTestRenderer.act(() => {
      webView.props.onError({
        nativeEvent: {
          code: -1005,
          description: 'connection lost',
          domain: 'NSURLErrorDomain',
        },
      });
      address.props.onChangeText('second.example');
    });
    await submitAddress(address);
    ReactTestRenderer.act(() => {
      jest.advanceTimersByTime(5000);
    });

    expect(mockReload).not.toHaveBeenCalled();
    ReactTestRenderer.act(() => renderer.unmount());
  });

  it('automatically reloads a transient Android WebView failure', async () => {
    const renderer = await renderBrowser();
    jest.useFakeTimers();
    const address = renderer.root.findByType(TextInput);
    ReactTestRenderer.act(() => {
      address.props.onChangeText('example.com');
    });
    await submitAddress(address);
    const privateWebView = renderer.root.findByProps({
      testID: 'private-webview',
    });

    ReactTestRenderer.act(() => {
      privateWebView.props.onError({
        nativeEvent: {
          code: -8,
          description: 'timeout',
          domain: 'android.webkit.WebViewClient',
        },
      });
      jest.advanceTimersByTime(600);
    });

    expect(mockReload).toHaveBeenCalledTimes(1);
    ReactTestRenderer.act(() => renderer.unmount());
  });

  it('opens free-text searches with Timpi in the selected browser mode', async () => {
    const renderer = await renderBrowser();
    const address = renderer.root.findByType(TextInput);

    expect(address.props.accessibilityLabel).toBe(
      'Private search or web address',
    );
    expect(address.props.placeholder).toBe(
      'Search with Timpi or enter a website',
    );
    expect(address.props.returnKeyType).toBe('search');

    ReactTestRenderer.act(() => {
      address.props.onChangeText('private mobile browser');
    });
    await submitAddress(address);

    expect(
      renderer.root.findByProps({ testID: 'private-webview' }).props.source,
    ).toEqual({
      uri: 'https://timpi.com/search?q=private%20mobile%20browser',
    });
    ReactTestRenderer.act(() => renderer.unmount());
  });

  it('opens ENS names through eth.limo while keeping the logical address visible', async () => {
    const renderer = await renderBrowser();
    const address = renderer.root.findByType(TextInput);

    ReactTestRenderer.act(() => {
      address.props.onChangeText('project.eth/docs?q=1#intro');
    });
    await submitAddress(address);

    expect(
      renderer.root.findByProps({ testID: 'private-webview' }).props.source,
    ).toEqual({
      uri: 'https://project.eth.limo/docs?q=1#intro',
    });
    expect(renderer.root.findByType(TextInput).props.value).toBe(
      'https://project.eth/docs?q=1#intro',
    );
    ReactTestRenderer.act(() => renderer.unmount());
  });

  it('applies fail-closed private-session WebView settings', async () => {
    const renderer = await renderBrowser();
    const address = renderer.root.findByType(TextInput);
    ReactTestRenderer.act(() => {
      address.props.onChangeText('example.com');
    });
    await submitAddress(address);
    const privateWebView = renderer.root.findByProps({
      testID: 'private-webview',
    });

    expect(privateWebView.props).toMatchObject({
      allowFileAccess: false,
      allowFileAccessFromFileURLs: false,
      allowUniversalAccessFromFileURLs: false,
      allowsLinkPreview: false,
      cacheEnabled: false,
      fraudulentWebsiteWarningEnabled: true,
      geolocationEnabled: false,
      incognito: true,
      javaScriptCanOpenWindowsAutomatically: false,
      mediaCapturePermissionGrantType: 'deny',
      mediaPlaybackRequiresUserAction: true,
      mixedContentMode: 'never',
      originWhitelist: ['*'],
      setSupportMultipleWindows: false,
      sharedCookiesEnabled: false,
      thirdPartyCookiesEnabled: false,
      useSharedProcessPool: false,
      webviewDebuggingEnabled: false,
    });
    ReactTestRenderer.act(() => renderer.unmount());
  });

  it('keeps the deredactie HTTP redirect inside MASQ by upgrading it to HTTPS', async () => {
    const renderer = await renderBrowser();
    const address = renderer.root.findByType(TextInput);
    ReactTestRenderer.act(() => {
      address.props.onChangeText('www.deredactie.be');
    });
    await submitAddress(address);
    let privateWebView = renderer.root.findByProps({
      testID: 'private-webview',
    });

    let allowed = true;
    await ReactTestRenderer.act(async () => {
      allowed = privateWebView.props.onShouldStartLoadWithRequest({
        isTopFrame: true,
        navigationType: 'other',
        url: 'http://deredactie.be/',
      });
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(allowed).toBe(false);
    privateWebView = renderer.root.findByProps({ testID: 'private-webview' });
    expect(privateWebView.props.source).toEqual({
      uri: 'https://deredactie.be/',
    });
    ReactTestRenderer.act(() => renderer.unmount());
  });

  it('blocks external schemes without changing the private page', async () => {
    const renderer = await renderBrowser();
    const address = renderer.root.findByType(TextInput);
    ReactTestRenderer.act(() => {
      address.props.onChangeText('example.com');
    });
    await submitAddress(address);
    let privateWebView = renderer.root.findByProps({
      testID: 'private-webview',
    });

    let allowed = true;
    ReactTestRenderer.act(() => {
      allowed = privateWebView.props.onShouldStartLoadWithRequest({
        isTopFrame: true,
        navigationType: 'click',
        url: 'mailto:news@example.com',
      });
    });

    expect(allowed).toBe(false);
    privateWebView = renderer.root.findByProps({ testID: 'private-webview' });
    expect(privateWebView.props.source).toEqual({
      uri: 'https://example.com/',
    });
    ReactTestRenderer.act(() => renderer.unmount());
  });

  it('blocks navigation until native protection preparation succeeds', async () => {
    let resolvePreparation!: (value: BrowserProtectionConfiguration) => void;
    mockPrepareBrowserProtection.mockReturnValue(
      new Promise<BrowserProtectionConfiguration>(resolve => {
        resolvePreparation = resolve;
      }),
    );
    let renderer!: ReactTestRenderer.ReactTestRenderer;
    ReactTestRenderer.act(() => {
      renderer = ReactTestRenderer.create(
        <BrowserScreen mode="masq" onClose={jest.fn()} />,
      );
    });
    const address = renderer.root.findByType(TextInput);
    expect(address.props.editable).toBe(false);
    expect(findProtectionDisclosure(renderer).props).toMatchObject({
      accessibilityState: { busy: true, expanded: false },
      accessibilityValue: { text: 'Preparing before navigation' },
    });

    ReactTestRenderer.act(() => {
      address.props.onChangeText('example.com');
      address.props.onSubmitEditing();
    });
    expect(
      renderer.root.findAllByProps({ testID: 'private-webview' }),
    ).toHaveLength(0);
    expect(JSON.stringify(renderer.toJSON())).toContain(
      'Wait for browser protection to finish preparing.',
    );

    await ReactTestRenderer.act(async () => {
      resolvePreparation({ ...PUBLIC_PROTECTION });
      await Promise.resolve();
      await Promise.resolve();
    });
    expect(renderer.root.findByType(TextInput).props.editable).toBe(true);
    expect(findProtectionDisclosure(renderer).props).toMatchObject({
      accessibilityState: { busy: false, expanded: false },
      accessibilityValue: { text: 'Network and page filtering' },
    });
    ReactTestRenderer.act(() => renderer.unmount());
  });

  it('does not mount a destination before its isolated profile is selected', async () => {
    let resolveSecondSite!: (
      value: Awaited<ReturnType<typeof masqCore.getBrowserSiteSettings>>,
    ) => void;
    const getSiteSettings = jest
      .spyOn(masqCore, 'getBrowserSiteSettings')
      .mockImplementation(async (mode, hostname) => {
        if (hostname === 'second.example') {
          return new Promise(resolve => {
            resolveSecondSite = resolve;
          });
        }
        return {
          hostname,
          mode,
          persistentSessionsSupported: true,
          protectionDisabled: false,
          rememberSignIn: hostname === 'first.example',
        };
      });
    const renderer = await renderBrowser();
    const address = renderer.root.findByType(TextInput);

    ReactTestRenderer.act(() => {
      address.props.onChangeText('first.example');
    });
    await ReactTestRenderer.act(async () => {
      address.props.onSubmitEditing();
      await Promise.resolve();
    });
    expect(
      renderer.root.findByProps({ testID: 'private-webview' }).props.source,
    ).toEqual({ uri: 'https://first.example/' });

    ReactTestRenderer.act(() => {
      address.props.onChangeText('second.example');
    });
    await ReactTestRenderer.act(async () => {
      address.props.onSubmitEditing();
      await Promise.resolve();
    });
    expect(
      renderer.root.findAllByProps({ testID: 'private-webview' }),
    ).toHaveLength(0);

    await ReactTestRenderer.act(async () => {
      resolveSecondSite({
        hostname: 'second.example',
        mode: 'masq',
        persistentSessionsSupported: true,
        protectionDisabled: false,
        rememberSignIn: false,
      });
      await Promise.resolve();
    });
    expect(getSiteSettings).toHaveBeenLastCalledWith(
      'masq',
      'second.example',
    );
    expect(
      renderer.root.findByProps({ testID: 'private-webview' }).props.source,
    ).toEqual({ uri: 'https://second.example/' });
    ReactTestRenderer.act(() => renderer.unmount());
  });

  it('switches profiles for cross-site links and server redirects', async () => {
    const getSiteSettings = jest
      .spyOn(masqCore, 'getBrowserSiteSettings')
      .mockImplementation(async (mode, hostname) => ({
        hostname,
        mode,
        persistentSessionsSupported: true,
        protectionDisabled: false,
        rememberSignIn: false,
      }));
    const renderer = await renderBrowser();
    const address = renderer.root.findByType(TextInput);
    ReactTestRenderer.act(() => {
      address.props.onChangeText('first.example');
    });
    await ReactTestRenderer.act(async () => {
      address.props.onSubmitEditing();
      await Promise.resolve();
    });

    let webView = renderer.root.findByProps({ testID: 'private-webview' });
    let allowed = true;
    await ReactTestRenderer.act(async () => {
      allowed = webView.props.onShouldStartLoadWithRequest({
        hasGesture: true,
        isRedirect: false,
        isTopFrame: true,
        navigationType: 'click',
        url: 'https://second.example/',
      });
      await Promise.resolve();
      await Promise.resolve();
    });
    expect(allowed).toBe(false);
    expect(getSiteSettings).toHaveBeenLastCalledWith(
      'masq',
      'second.example',
    );
    webView = renderer.root.findByProps({ testID: 'private-webview' });
    expect(webView.props.source).toEqual({
      uri: 'https://second.example/',
    });

    await ReactTestRenderer.act(async () => {
      allowed = webView.props.onShouldStartLoadWithRequest({
        hasGesture: false,
        isRedirect: true,
        isTopFrame: true,
        navigationType: 'other',
        url: 'https://login.example/',
      });
      await Promise.resolve();
      await Promise.resolve();
    });
    expect(allowed).toBe(false);
    expect(getSiteSettings).toHaveBeenLastCalledWith(
      'masq',
      'login.example',
    );
    expect(
      renderer.root.findByProps({ testID: 'private-webview' }).props.source,
    ).toEqual({ uri: 'https://login.example/' });
    ReactTestRenderer.act(() => renderer.unmount());
  });

  it('keeps navigation disabled after preparation fails and retries safely', async () => {
    mockPrepareBrowserProtection
      .mockRejectedValueOnce(new Error('Native content rules are unavailable.'))
      .mockResolvedValueOnce({ ...PUBLIC_PROTECTION });
    const renderer = await renderBrowser();
    expect(renderer.root.findByType(TextInput).props.editable).toBe(false);
    expect(JSON.stringify(renderer.toJSON())).toContain(
      'Native content rules are unavailable.',
    );
    const retry = renderer.root.findAll(
      node =>
        node.props.accessibilityLabel === 'Retry browser protection' &&
        typeof node.props.onPress === 'function',
    )[0];

    await ReactTestRenderer.act(async () => {
      retry.props.onPress();
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(mockPrepareBrowserProtection).toHaveBeenCalledTimes(2);
    expect(renderer.root.findByType(TextInput).props.editable).toBe(true);
    ReactTestRenderer.act(() => renderer.unmount());
  });

  it('keeps protection settings collapsed by default behind an accessible disclosure', async () => {
    const renderer = await renderBrowser();
    let disclosure = findProtectionDisclosure(renderer);

    expect(disclosure.props.accessibilityState).toMatchObject({
      expanded: false,
    });
    expect(
      renderer.root.findAll(node => node.props.accessibilityRole === 'switch'),
    ).toHaveLength(0);
    expect(JSON.stringify(renderer.toJSON())).toContain('Browser protection');
    expect(JSON.stringify(renderer.toJSON())).toContain(
      'Network and page filtering',
    );
    expect(JSON.stringify(renderer.toJSON())).not.toContain(
      'Optional cookie rejection is off by default',
    );

    ReactTestRenderer.act(() => {
      disclosure.props.onPress();
    });
    disclosure = findProtectionDisclosure(renderer);
    expect(disclosure.props.accessibilityState).toMatchObject({
      expanded: true,
    });
    for (const label of [
      'Ads & trackers',
      'Cross-site cookies',
      'Hide resolved banners',
      'Reject optional cookies',
    ]) {
      expect(findProtectionToggle(renderer, label)).toBeDefined();
    }
    expect(JSON.stringify(renderer.toJSON())).toContain(
      'Consent rejection only uses verified Reject controls',
    );

    ReactTestRenderer.act(() => {
      disclosure.props.onPress();
    });
    expect(
      findProtectionDisclosure(renderer).props.accessibilityState,
    ).toMatchObject({
      expanded: false,
    });
    expect(
      renderer.root.findAll(node => node.props.accessibilityRole === 'switch'),
    ).toHaveLength(0);
    ReactTestRenderer.act(() => renderer.unmount());
  });

  it('shows native defaults and injects only generic cosmetic filtering', async () => {
    const renderer = await renderBrowser();
    expandProtectionSettings(renderer);
    for (const label of [
      'Ads & trackers',
      'Cross-site cookies',
      'Hide resolved banners',
    ]) {
      expect(
        findProtectionToggle(renderer, label).props.accessibilityState,
      ).toMatchObject({
        checked: true,
        disabled: false,
      });
    }
    expect(
      findProtectionToggle(renderer, 'Reject optional cookies').props
        .accessibilityState,
    ).toMatchObject({
      checked: false,
      disabled: false,
    });
    expect(JSON.stringify(renderer.toJSON())).toContain(
      'Consent rejection only uses verified Reject controls',
    );
    expect(JSON.stringify(renderer.toJSON())).toContain('never selects Accept');
    expect(
      renderer.root.findAllByProps({
        accessibilityLabel: 'YouTube best effort',
      }),
    ).toHaveLength(0);
    expect(JSON.stringify(renderer.toJSON())).toContain(
      'YouTube-specific ad filtering is unavailable',
    );

    const address = renderer.root.findByType(TextInput);
    ReactTestRenderer.act(() => {
      address.props.onChangeText('example.com');
    });
    await submitAddress(address);
    const webView = renderer.root.findByProps({ testID: 'private-webview' });
    expect(webView.props.injectedJavaScript).toContain('[data-ad-slot]');
    expect(webView.props.injectedJavaScript).toContain('#onetrust-banner-sdk');
    expect(webView.props.injectedJavaScript.toLowerCase()).not.toContain(
      'youtube',
    );
    expect(webView.props.injectedJavaScript.toLowerCase()).not.toContain(
      'ytp-',
    );
    expect(webView.props.injectedJavaScript.toLowerCase()).not.toContain(
      'googlevideo',
    );
    ReactTestRenderer.act(() => renderer.unmount());
  });

  it('persists a toggle through native code and remounts the active page', async () => {
    const renderer = await renderBrowser();
    expandProtectionSettings(renderer);
    const address = renderer.root.findByType(TextInput);
    ReactTestRenderer.act(() => {
      address.props.onChangeText('example.com');
    });
    await submitAddress(address);
    const initialWebView = renderer.root.findByProps({
      testID: 'private-webview',
    });
    const adsToggle = findProtectionToggle(renderer, 'Ads & trackers');

    await ReactTestRenderer.act(async () => {
      adsToggle.props.onPress();
      await Promise.resolve();
    });

    expect(mockSetBrowserProtection).toHaveBeenCalledWith({
      blockAdsAndTrackers: false,
      blockCrossSiteCookies: true,
      hideCookieBanners: true,
      rejectOptionalCookies: false,
      youtubeBestEffort: false,
    });
    const remountedWebView = renderer.root.findByProps({
      testID: 'private-webview',
    });
    expect(remountedWebView).not.toBe(initialWebView);
    expect(remountedWebView.props.source).toEqual({
      uri: 'https://example.com/',
    });
    expect(remountedWebView.props.injectedJavaScript).not.toContain(
      '[data-ad-slot]',
    );
    ReactTestRenderer.act(() => renderer.unmount());
  });

  it('keeps optional-cookie rejection opt-in and persists explicit consent', async () => {
    const renderer = await renderBrowser();
    expandProtectionSettings(renderer);
    const rejectToggle = findProtectionToggle(
      renderer,
      'Reject optional cookies',
    );

    expect(rejectToggle.props.accessibilityState.checked).toBe(false);
    await ReactTestRenderer.act(async () => {
      rejectToggle.props.onPress();
      await Promise.resolve();
    });

    expect(mockSetBrowserProtection).toHaveBeenCalledWith({
      blockAdsAndTrackers: true,
      blockCrossSiteCookies: true,
      hideCookieBanners: true,
      rejectOptionalCookies: true,
      youtubeBestEffort: false,
    });
    expect(
      findProtectionToggle(renderer, 'Reject optional cookies').props
        .accessibilityState.checked,
    ).toBe(true);
    ReactTestRenderer.act(() => renderer.unmount());
  });

  it('opts a single MASQ site into its isolated remembered profile', async () => {
    jest.spyOn(masqCore, 'getBrowserSiteSettings').mockResolvedValue({
      hostname: 'example.com',
      mode: 'masq',
      persistentSessionsSupported: true,
      protectionDisabled: false,
      rememberSignIn: false,
    });
    const setSiteSettings = jest
      .spyOn(masqCore, 'setBrowserSiteSettings')
      .mockResolvedValue({
        hostname: 'example.com',
        mode: 'masq',
        persistentSessionsSupported: true,
        protectionDisabled: false,
        rememberSignIn: true,
      });
    const renderer = await renderBrowser();
    const address = renderer.root.findByType(TextInput);
    ReactTestRenderer.act(() => {
      address.props.onChangeText('example.com');
    });
    await ReactTestRenderer.act(async () => {
      address.props.onSubmitEditing();
      await Promise.resolve();
      await Promise.resolve();
    });
    expandProtectionSettings(renderer);

    await ReactTestRenderer.act(async () => {
      findProtectionToggle(
        renderer,
        'Remember sign-in for this site',
      ).props.onPress();
      await Promise.resolve();
    });

    expect(setSiteSettings).toHaveBeenCalledWith(
      'masq',
      'example.com',
      true,
      false,
    );
    expect(
      renderer.root.findByProps({ testID: 'private-webview' }).props,
    ).toMatchObject({
      cacheEnabled: true,
    });
    ReactTestRenderer.act(() => renderer.unmount());
  });

  it('keeps the Android cookie prop aligned with the native preference', async () => {
    const renderer = await renderBrowser();
    expandProtectionSettings(renderer);
    const address = renderer.root.findByType(TextInput);
    ReactTestRenderer.act(() => {
      address.props.onChangeText('example.com');
    });
    await submitAddress(address);
    expect(
      renderer.root.findByProps({ testID: 'private-webview' }).props
        .thirdPartyCookiesEnabled,
    ).toBe(false);

    await ReactTestRenderer.act(async () => {
      findProtectionToggle(renderer, 'Cross-site cookies').props.onPress();
      await Promise.resolve();
    });

    expect(mockSetBrowserProtection).toHaveBeenCalledWith({
      blockAdsAndTrackers: true,
      blockCrossSiteCookies: false,
      hideCookieBanners: true,
      rejectOptionalCookies: false,
      youtubeBestEffort: false,
    });
    expect(
      renderer.root.findByProps({ testID: 'private-webview' }).props
        .thirdPartyCookiesEnabled,
    ).toBe(true);
    ReactTestRenderer.act(() => renderer.unmount());
  });

  it('exposes YouTube best effort only when the native private build allows it', async () => {
    mockPrepareBrowserProtection.mockResolvedValue({
      ...PUBLIC_PROTECTION,
      youtubeBestEffortAvailable: true,
    });
    mockSetBrowserProtection.mockImplementation(
      async (
        preferences: BrowserProtectionPreferences,
      ): Promise<BrowserProtectionConfiguration> => ({
        ...PUBLIC_PROTECTION,
        ...preferences,
        youtubeBestEffortAvailable: true,
      }),
    );
    const renderer = await renderBrowser();
    expandProtectionSettings(renderer);
    const youtubeToggle = findProtectionToggle(renderer, 'YouTube best effort');
    expect(JSON.stringify(renderer.toJSON())).toContain(
      'Experimental: may interrupt playback',
    );
    expect(JSON.stringify(renderer.toJSON())).toContain(
      'conflict with YouTube',
    );
    expect(JSON.stringify(renderer.toJSON())).not.toContain(
      'unavailable in this public build',
    );

    await ReactTestRenderer.act(async () => {
      youtubeToggle.props.onPress();
      await Promise.resolve();
    });

    expect(mockSetBrowserProtection).toHaveBeenCalledWith({
      blockAdsAndTrackers: true,
      blockCrossSiteCookies: true,
      hideCookieBanners: true,
      rejectOptionalCookies: false,
      youtubeBestEffort: true,
    });
    ReactTestRenderer.act(() => renderer.unmount());
  });

  it('keeps the current settings and offers retry when native apply fails', async () => {
    mockSetBrowserProtection.mockRejectedValue(
      new Error('Content rules could not be compiled.'),
    );
    const renderer = await renderBrowser();
    expandProtectionSettings(renderer);
    const adsToggle = findProtectionToggle(renderer, 'Ads & trackers');

    await ReactTestRenderer.act(async () => {
      adsToggle.props.onPress();
      await Promise.resolve();
    });

    expect(
      findProtectionToggle(renderer, 'Ads & trackers').props.accessibilityState
        .checked,
    ).toBe(true);
    expect(JSON.stringify(renderer.toJSON())).toContain(
      'Content rules could not be compiled.',
    );
    expect(JSON.stringify(renderer.toJSON())).toContain('Retry protection');
    ReactTestRenderer.act(() => renderer.unmount());
  });

  it('labels direct browsing honestly while keeping browser safeguards active', async () => {
    const renderer = await renderBrowser('direct');
    const initialContent = JSON.stringify(renderer.toJSON());
    expect(initialContent).toContain('DIRECT · MASQ OFF');
    expect(initialContent).not.toContain(
      'public IP of your current connection or VPN',
    );
    expect(initialContent).not.toContain('not routed through MASQ');
    expect(initialContent).toContain('Direct browser ready');
    expect(initialContent).not.toContain('MASQ PRIVATE');
    expect(initialContent).not.toContain('Private session ready');
    expect(initialContent).not.toContain('browse through MASQ');
    let directDisclosure = findProtectionDisclosure(renderer, 'direct');
    expect(directDisclosure.props).toMatchObject({
      accessibilityState: { busy: false, expanded: false },
      accessibilityValue: { text: 'Network and page filtering' },
    });
    ReactTestRenderer.act(() => directDisclosure.props.onPress());
    directDisclosure = findProtectionDisclosure(renderer, 'direct');
    expect(directDisclosure.props.accessibilityState.expanded).toBe(true);
    expect(findProtectionToggle(renderer, 'Ads & trackers')).toBeDefined();
    ReactTestRenderer.act(() => directDisclosure.props.onPress());
    expect(
      findProtectionDisclosure(renderer, 'direct').props.accessibilityState
        .expanded,
    ).toBe(false);

    const address = renderer.root.findByType(TextInput);
    expect(address.props.accessibilityLabel).toBe(
      'Direct search or web address',
    );
    ReactTestRenderer.act(() => {
      address.props.onChangeText('example.com');
    });
    await submitAddress(address);

    const directWebView = renderer.root.findByProps({
      testID: 'private-webview',
    });
    expect(directWebView.props).toMatchObject({
      allowFileAccess: false,
      allowFileAccessFromFileURLs: false,
      allowUniversalAccessFromFileURLs: false,
      cacheEnabled: false,
      incognito: false,
      mixedContentMode: 'never',
      sharedCookiesEnabled: false,
      thirdPartyCookiesEnabled: false,
      useSharedProcessPool: false,
    });
    expect(directWebView.props.injectedJavaScript).toContain('[data-ad-slot]');
    expect(directWebView.props.injectedJavaScript).toContain(
      '#onetrust-banner-sdk',
    );
    ReactTestRenderer.act(() => directWebView.props.onLoadStart());
    expect(JSON.stringify(renderer.toJSON())).toContain('Loading directly');
    ReactTestRenderer.act(() => renderer.unmount());
  });

  it('uses direct-connection recovery copy outside MASQ mode', async () => {
    const renderer = await renderBrowser('direct');
    const address = renderer.root.findByType(TextInput);
    ReactTestRenderer.act(() => {
      address.props.onChangeText('example.com');
    });
    await submitAddress(address);
    const directWebView = renderer.root.findByProps({
      testID: 'private-webview',
    });

    ReactTestRenderer.act(() => {
      directWebView.props.onError({
        nativeEvent: {
          code: -1005,
          description: 'connection lost',
          domain: 'NSURLErrorDomain',
        },
      });
    });

    const content = JSON.stringify(renderer.toJSON());
    expect(content).toContain('normal internet connection was interrupted');
    expect(content).not.toContain('private route changed');
    ReactTestRenderer.act(() => renderer.unmount());
  });

  it('uses routing-neutral HTTPS policy copy in direct mode', async () => {
    const renderer = await renderBrowser('direct');
    const address = renderer.root.findByType(TextInput);
    ReactTestRenderer.act(() => {
      address.props.onChangeText('http://example.com');
    });
    await submitAddress(address);

    const content = JSON.stringify(renderer.toJSON());
    expect(content).toContain('This browser only allows HTTPS addresses');
    expect(content).not.toContain('MASQ Mobile only allows HTTPS');
    ReactTestRenderer.act(() => renderer.unmount());
  });
});

async function renderBrowser(mode: 'masq' | 'direct' = 'masq') {
  let renderer!: ReactTestRenderer.ReactTestRenderer;
  await ReactTestRenderer.act(async () => {
    renderer = ReactTestRenderer.create(
      <BrowserScreen mode={mode} onClose={jest.fn()} />,
    );
    await Promise.resolve();
    await Promise.resolve();
  });
  expect(mockPrepareBrowserProtection).toHaveBeenCalledTimes(1);
  return renderer;
}

async function submitAddress(
  address: ReactTestRenderer.ReactTestInstance,
) {
  await ReactTestRenderer.act(async () => {
    address.props.onSubmitEditing();
    await Promise.resolve();
    await Promise.resolve();
  });
}

function findProtectionToggle(
  renderer: ReactTestRenderer.ReactTestRenderer,
  label: string,
) {
  const matches = renderer.root.findAll(
    node =>
      node.props.accessibilityLabel === label &&
      node.props.accessibilityRole === 'switch' &&
      typeof node.props.onPress === 'function',
  );
  if (matches.length === 0) {
    throw new Error(`Protection toggle "${label}" was not rendered.`);
  }
  return matches[0];
}

function findProtectionDisclosure(
  renderer: ReactTestRenderer.ReactTestRenderer,
  mode: 'masq' | 'direct' = 'masq',
) {
  const accessibilityLabel =
    mode === 'masq'
      ? 'Browser protection settings'
      : 'Browser safeguards settings';
  const matches = renderer.root.findAll(
    node =>
      node.props.accessibilityLabel === accessibilityLabel &&
      node.props.accessibilityRole === 'button' &&
      typeof node.props.onPress === 'function',
  );
  if (matches.length === 0) {
    throw new Error(
      `Protection disclosure "${accessibilityLabel}" was missing.`,
    );
  }
  return matches[0];
}

function expandProtectionSettings(
  renderer: ReactTestRenderer.ReactTestRenderer,
) {
  const disclosure = findProtectionDisclosure(renderer);
  if (!disclosure.props.accessibilityState?.expanded) {
    ReactTestRenderer.act(() => {
      disclosure.props.onPress();
    });
  }
}
