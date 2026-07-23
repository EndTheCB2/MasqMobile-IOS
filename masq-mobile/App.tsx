import { useEffect, useRef, useState } from 'react';
import {
  Alert,
  AppState,
  Linking,
  Share,
  StatusBar,
  StyleSheet,
  Text,
  View,
} from 'react-native';
import { SafeAreaProvider, SafeAreaView } from 'react-native-safe-area-context';

import { masqCore } from './src/core/masqCore';
import {
  closeBrowserSession,
  prepareBrowserSession,
} from './src/core/browserSession';
import { buildRedactedDiagnostics } from './src/core/diagnostics';
import { classifyMasqIssue } from './src/core/issues';
import { useMasqController } from './src/hooks/useMasqController';
import {
  BrowserScreen,
  type BrowserMode,
} from './src/screens/BrowserScreen';
import { HomeScreen } from './src/screens/HomeScreen';
import { PrivacyScreen } from './src/screens/PrivacyScreen';
import { SetupScreen } from './src/screens/SetupScreen';
import { TrafficRoutingScreen } from './src/screens/TrafficRoutingScreen';
import { colors } from './src/ui/theme';

type Route = 'home' | 'setup' | 'browser' | 'routing' | 'privacy';
const DIRECT_SHUTDOWN_TIMEOUT_MS = 15_000;

const PRIVACY_POLICY_URL =
  'https://github.com/EndTheCB2/MasqMobile-IOS/blob/main/PRIVACY_POLICY.md';
const SOURCE_URL = 'https://github.com/EndTheCB2/MasqMobile-IOS';
const SUPPORT_URL = 'https://github.com/EndTheCB2/MasqMobile-IOS/issues';

function App() {
  return (
    <SafeAreaProvider>
      <StatusBar backgroundColor={colors.ink} barStyle="light-content" />
      <AppContent />
    </SafeAreaProvider>
  );
}

function AppContent() {
  const [route, setRoute] = useState<Route>('home');
  const [routeError, setRouteError] = useState<string | null>(null);
  const [directBrowserError, setDirectBrowserError] = useState<string | null>(
    null,
  );
  const [browserOpening, setBrowserOpening] = useState(false);
  const [browserMode, setBrowserMode] = useState<BrowserMode | null>(null);
  const [privacyShielded, setPrivacyShielded] = useState(false);
  const browserOperation = useRef(0);
  const browserOpenInFlight = useRef(false);
  const openingBrowserMode = useRef<BrowserMode | null>(null);
  const controller = useMasqController();
  const activeIssue = directBrowserError
    ? {
        action: 'none' as const,
        category: 'unknown' as const,
        code: null,
        message: directBrowserError,
      }
    : classifyMasqIssue(routeError, controller.network, controller.status) ||
      controller.issue ||
      classifyMasqIssue(
        controller.status.lastError,
        controller.network,
        controller.status,
      );

  useEffect(() => {
    const subscription = AppState.addEventListener('change', state => {
      setPrivacyShielded(state !== 'active');
      if (state !== 'active') {
        browserOperation.current += 1;
        closeBrowserSession(masqCore).catch(() => undefined);
        setBrowserMode(null);
        if (route === 'browser' || browserOpening) {
          const interruptedMode =
            browserMode ?? openingBrowserMode.current;
          setRoute('home');
          if (interruptedMode === 'masq') {
            setDirectBrowserError(null);
            setRouteError(
              'MASQ Private was closed while the app was in the background. Reopen it to run a new route check.',
            );
          } else {
            setRouteError(null);
            setDirectBrowserError(
              'The direct browser was closed while the app was in the background. Reopen it explicitly to start a new session.',
            );
          }
        }
      }
    });
    return () => subscription.remove();
  }, [browserMode, browserOpening, route]);

  const openSetup = () => {
    setDirectBrowserError(null);
    if (
      ['connecting', 'connected', 'paused', 'stopping'].includes(
        controller.status.phase,
      )
    ) {
      setRouteError(
        'Fully restart the app before changing active Node settings.',
      );
      return;
    }
    setRouteError(null);
    setRoute('setup');
  };

  const connect = () => {
    setDirectBrowserError(null);
    setRouteError(null);
    controller.connect().catch(() => undefined);
  };

  const disconnect = () => {
    setDirectBrowserError(null);
    setRouteError(null);
    controller.disconnect().catch(() => undefined);
  };

  const openMasqBrowser = async () => {
    if (browserOpenInFlight.current) {
      return;
    }
    const operation = ++browserOperation.current;
    browserOpenInFlight.current = true;
    openingBrowserMode.current = 'masq';
    setDirectBrowserError(null);
    setRouteError(null);
    setBrowserOpening(true);
    try {
      await prepareBrowserSession(masqCore, 'masq');
      if (operation !== browserOperation.current) {
        await closeBrowserSession(masqCore).catch(() => undefined);
        return;
      }
      await controller.refresh();
      if (operation !== browserOperation.current) {
        await closeBrowserSession(masqCore).catch(() => undefined);
        return;
      }
      setBrowserMode('masq');
      setRoute('browser');
    } catch (caught) {
      await closeBrowserSession(masqCore).catch(() => undefined);
      if (operation === browserOperation.current) {
        setBrowserMode(null);
        setRouteError(
          caught instanceof Error
            ? caught.message
            : 'Browser proxy unavailable.',
        );
      }
    } finally {
      browserOpenInFlight.current = false;
      openingBrowserMode.current = null;
      setBrowserOpening(false);
    }
  };

  const openDirectBrowser = async () => {
    if (browserOpenInFlight.current) {
      return;
    }
    const operation = ++browserOperation.current;
    browserOpenInFlight.current = true;
    openingBrowserMode.current = 'direct';
    setDirectBrowserError(null);
    setRouteError(null);
    setBrowserOpening(true);
    try {
      // Always run the controller disconnect, even when the visible status is `error` or
      // `unconfigured`: it cancels background reconnect intent, blocks WebViews, and obtains an
      // acknowledged system-tunnel stop where supported.
      await controller.disconnect();
      if (operation !== browserOperation.current) {
        await closeBrowserSession(masqCore).catch(() => undefined);
        return;
      }
      // Full teardown is idempotent. Calling it unconditionally prevents a stale/error status
      // from hiding an engine handle that could otherwise remain connected during Direct Browse.
      const shutdown = await withTimeout(
        masqCore.shutdown(),
        DIRECT_SHUTDOWN_TIMEOUT_MS,
        'MASQ shutdown did not finish. Direct browsing remains blocked.',
      );
      if (
        !['ready', 'unconfigured', 'blocked'].includes(shutdown.phase) ||
        shutdown.connectedNeighbors !== 0 ||
        shutdown.proxyEnabled ||
        shutdown.proxyPort !== null
      ) {
        throw new Error(
          'MASQ could not confirm that its peer connection and system routing stopped.',
        );
      }
      if (operation !== browserOperation.current) {
        await closeBrowserSession(masqCore).catch(() => undefined);
        return;
      }
      await prepareBrowserSession(masqCore, 'direct');
      if (operation !== browserOperation.current) {
        await closeBrowserSession(masqCore).catch(() => undefined);
        return;
      }
      setBrowserMode('direct');
      setRoute('browser');
    } catch (caught) {
      await closeBrowserSession(masqCore).catch(() => undefined);
      if (operation === browserOperation.current) {
        setBrowserMode(null);
        setRouteError(null);
        setDirectBrowserError(
          caught instanceof Error
            ? caught.message
            : 'Direct browsing is unavailable.',
        );
      }
    } finally {
      browserOpenInFlight.current = false;
      openingBrowserMode.current = null;
      setBrowserOpening(false);
    }
  };

  const confirmDirectBrowser = () => {
    Alert.alert(
      'Browse without MASQ?',
      'This stops any active MASQ connection and system routing, then uses your normal internet connection. Websites see the public IP used by your current connection or VPN. Your internet provider and DNS service can see normal connection metadata. MASQ hops and exit-country settings do not apply.',
      [
        { text: 'Cancel', style: 'cancel' },
        {
          text: 'Browse directly',
          onPress: () => {
            openDirectBrowser().catch(() => undefined);
          },
        },
      ],
    );
  };

  const shareDiagnostics = async () => {
    const diagnostics = buildRedactedDiagnostics(
      controller.status,
      controller.network,
      activeIssue?.message || null,
      activeIssue,
    );
    await Share.share({
      title: 'MASQ redacted diagnostics',
      message: JSON.stringify(diagnostics, null, 2),
    });
  };

  const closeBrowser = async () => {
    browserOperation.current += 1;
    await closeBrowserSession(masqCore).catch(() => undefined);
    setBrowserMode(null);
    await controller.refresh().catch(() => undefined);
    setRoute('home');
  };

  const openExternalLink = async (url: string) => {
    try {
      await Linking.openURL(url);
    } catch {
      Alert.alert(
        'Link unavailable',
        'The page could not be opened. Check your internet connection and try again.',
      );
    }
  };

  const confirmReset = () => {
    Alert.alert(
      'Reset MASQ?',
      'This removes the saved consumer wallet and network profile from this device. Your funds remain recoverable with your 12 words.',
      [
        { text: 'Cancel', style: 'cancel' },
        {
          text: 'Reset',
          style: 'destructive',
          onPress: () => {
            setRouteError(null);
            controller
              .reset()
              .then(() => setRoute('home'))
              .catch(() => undefined);
          },
        },
      ],
    );
  };

  const confirmNetworkReset = () => {
    Alert.alert(
      'Reset network profile?',
      'This removes the chain, RPC and entry-node settings. Your consumer wallet stays in secure device storage.',
      [
        { text: 'Cancel', style: 'cancel' },
        {
          text: 'Reset network',
          style: 'destructive',
          onPress: () =>
            controller.resetNetworkProfile().catch(() => undefined),
        },
      ],
    );
  };

  const confirmWalletRemoval = () => {
    Alert.alert(
      'Remove wallet from this device?',
      'The network profile stays saved. Make sure you still have the 12 recovery words before continuing.',
      [
        { text: 'Cancel', style: 'cancel' },
        {
          text: 'Remove wallet',
          style: 'destructive',
          onPress: () => controller.removeWallet().catch(() => undefined),
        },
      ],
    );
  };

  return (
    <SafeAreaView edges={['top', 'bottom']} style={styles.container}>
      {route === 'home' ? (
        <HomeScreen
          busy={controller.busy || browserOpening}
          connectionProgress={controller.connectionProgress}
          network={controller.network}
          entryNodeRefresh={controller.entryNodeRefresh}
          issue={activeIssue}
          walletBalance={controller.walletBalance}
          systemTunnel={controller.systemTunnel}
          onConnect={connect}
          onDisconnect={disconnect}
          onOpenBrowser={openMasqBrowser}
          onOpenDirectBrowser={confirmDirectBrowser}
          onOpenSetup={openSetup}
          onOpenTrafficRouting={() => setRoute('routing')}
          onOpenPrivacy={() => setRoute('privacy')}
          onReset={confirmReset}
          onResetNetwork={confirmNetworkReset}
          onRemoveWallet={confirmWalletRemoval}
          onRetry={connect}
          onOpenSystemSettings={() => Linking.openSettings()}
          onShareDiagnostics={() => shareDiagnostics().catch(() => undefined)}
          onUpdateMinHops={minHops =>
            controller.updateMinHops(minHops).catch(() => undefined)
          }
          onRefreshWalletBalance={() =>
            controller.refreshWalletBalance().catch(() => undefined)
          }
          status={controller.status}
        />
      ) : null}
      {route === 'setup' ? (
        <SetupScreen
          availableExitCountries={controller.status.availableExitCountries}
          busy={controller.busy}
          error={controller.issue?.message || null}
          exitCountryInventoryReady={controller.status.connectedNeighbors > 0}
          hasWallet={Boolean(controller.status.walletAddress)}
          initial={controller.draft}
          onBack={() => setRoute('home')}
          onSave={controller.saveSetup}
        />
      ) : null}
      {route === 'browser' && browserMode ? (
        <BrowserScreen mode={browserMode} onClose={closeBrowser} />
      ) : null}
      {route === 'routing' ? (
        <TrafficRoutingScreen
          busy={controller.systemTunnelBusy}
          connected={controller.status.phase === 'connected'}
          routableApps={controller.routableApps}
          status={controller.systemTunnel}
          onApply={controller.updateSystemTunnel}
          onBack={() => setRoute('home')}
        />
      ) : null}
      {route === 'privacy' ? (
        <PrivacyScreen
          onBack={() => setRoute('home')}
          onOpenPrivacyPolicy={() =>
            openExternalLink(PRIVACY_POLICY_URL).catch(() => undefined)
          }
          onOpenSource={() =>
            openExternalLink(SOURCE_URL).catch(() => undefined)
          }
          onOpenSupport={() =>
            openExternalLink(SUPPORT_URL).catch(() => undefined)
          }
        />
      ) : null}
      {privacyShielded ? (
        <View accessibilityViewIsModal style={styles.privacyShield}>
          <Text style={styles.privacyShieldBrand}>MASQ</Text>
          <Text style={styles.privacyShieldText}>Unlock to continue</Text>
        </View>
      ) : null}
    </SafeAreaView>
  );
}

async function withTimeout<T>(
  operation: Promise<T>,
  timeoutMs: number,
  message: string,
): Promise<T> {
  let timeout: ReturnType<typeof setTimeout> | undefined;
  try {
    return await Promise.race([
      operation,
      new Promise<never>((_resolve, reject) => {
        timeout = setTimeout(() => reject(new Error(message)), timeoutMs);
      }),
    ]);
  } finally {
    if (timeout) {
      clearTimeout(timeout);
    }
  }
}

const styles = StyleSheet.create({
  container: {
    backgroundColor: colors.ink,
    flex: 1,
  },
  privacyShield: {
    alignItems: 'center',
    backgroundColor: colors.ink,
    bottom: 0,
    justifyContent: 'center',
    left: 0,
    position: 'absolute',
    right: 0,
    top: 0,
    zIndex: 100,
  },
  privacyShieldBrand: {
    color: colors.white,
    fontSize: 32,
    fontWeight: '900',
    letterSpacing: 2,
  },
  privacyShieldText: {
    color: colors.muted,
    fontSize: 13,
    marginTop: 8,
  },
});

export default App;
