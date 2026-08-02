import { useEffect, useRef, useState } from 'react';
import {
  Alert,
  AppState,
  BackHandler,
  Linking,
  Platform,
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
import { isCoreReadyForSystemRouting } from './src/core/connectionReadiness';
import { buildRedactedDiagnostics } from './src/core/diagnostics';
import { classifyMasqIssue } from './src/core/issues';
import {
  PROFILE_NOT_READY_MESSAGE,
  useMasqController,
} from './src/hooks/useMasqController';
import {
  BrowserScreen,
  type BrowserCloseReason,
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
        // An active BrowserScreen immediately unmounts its WebView and owns the
        // acknowledged close/retry lifecycle. This listener handles only a
        // session that is still opening and has no WebView yet.
        if (route === 'browser' && browserMode) {
          return;
        }
        closeBrowserSession(masqCore).catch(() => undefined);
        setBrowserMode(null);
        if (browserOpening) {
          const interruptedMode = browserMode ?? openingBrowserMode.current;
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

  useEffect(() => {
    if (Platform.OS !== 'android') {
      return;
    }
    const subscription = BackHandler.addEventListener(
      'hardwareBackPress',
      () => {
        if (route === 'setup' || route === 'routing' || route === 'privacy') {
          setRoute('home');
          return true;
        }
        // BrowserScreen owns WebView history and fail-closed session shutdown.
        // Returning false lets its later listener handle the same event. Home
        // also returns false so Android keeps its normal app-exit behavior.
        return false;
      },
    );
    return () => subscription.remove();
  }, [route]);

  const openSetup = () => {
    setDirectBrowserError(null);
    if (!controller.profileReady) {
      setRouteError(PROFILE_NOT_READY_MESSAGE);
      return;
    }
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
    if (!controller.profileReady) {
      setRouteError(PROFILE_NOT_READY_MESSAGE);
      return;
    }
    setRouteError(null);
    controller.connect().catch(() => undefined);
  };

  const disconnect = () => {
    setDirectBrowserError(null);
    setRouteError(null);
    controller.disconnect().catch(() => undefined);
  };

  const openMasqBrowser = async () => {
    if (!controller.profileReady) {
      setRouteError(PROFILE_NOT_READY_MESSAGE);
      return;
    }
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
    if (!controller.profileReady) {
      setDirectBrowserError(null);
      setRouteError(PROFILE_NOT_READY_MESSAGE);
      return;
    }
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

  const confirmInitializationRecovery = () => {
    Alert.alert(
      'Reset invalid network profile?',
      'Use this only if Retry keeps failing. MASQ stops active routing and removes the saved chain, RPC, entry nodes and route preferences. Your consumer wallet remains on this device. Browsing stays blocked until MASQ Mobile confirms the reset.',
      [
        { text: 'Cancel', style: 'cancel' },
        {
          text: 'Reset network profile',
          style: 'destructive',
          onPress: () => {
            setDirectBrowserError(null);
            setRouteError(null);
            controller.recoverNetworkProfile().catch(() => undefined);
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

  const closeBrowser = async (reason: BrowserCloseReason = 'user') => {
    const closingMode = browserMode;
    browserOperation.current += 1;
    await closeBrowserSession(masqCore);
    setBrowserMode(null);
    await controller.refresh().catch(() => undefined);
    if (reason === 'background') {
      if (closingMode === 'masq') {
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
          profileReady={controller.profileReady}
          initializationState={controller.initializationState}
          profileRecoveryAvailable={controller.profileRecoveryAvailable}
          network={controller.network}
          entryNodeRefresh={controller.entryNodeRefresh}
          issue={activeIssue}
          walletBalance={controller.walletBalance}
          debtSummary={controller.debtSummary}
          debtSettlementQuote={controller.debtSettlementQuote}
          debtSettlementStatus={controller.debtSettlementStatus}
          debtSettlementBusy={controller.debtSettlementBusy}
          debtSettlementError={controller.debtSettlementError}
          systemTunnel={controller.systemTunnel}
          onConnect={connect}
          onRetryInitialization={() => {
            setDirectBrowserError(null);
            setRouteError(null);
            controller.retryInitialization().catch(() => undefined);
          }}
          onRecoverNetworkProfile={confirmInitializationRecovery}
          onDisconnect={disconnect}
          onOpenBrowser={openMasqBrowser}
          onOpenDirectBrowser={confirmDirectBrowser}
          onOpenSetup={openSetup}
          onOpenTrafficRouting={() => {
            if (!controller.profileReady) {
              setRouteError(PROFILE_NOT_READY_MESSAGE);
              return;
            }
            setRoute('routing');
          }}
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
          onRefreshDebtSummary={() =>
            controller.refreshDebtSummary().catch(() => undefined)
          }
          onReviewDebtSettlement={() =>
            controller.reviewDebtSettlement().catch(() => undefined)
          }
          onConfirmDebtSettlement={() =>
            controller.confirmDebtSettlement().catch(() => undefined)
          }
          onRetryDebtSettlement={() =>
            controller.retryDebtSettlement().catch(() => undefined)
          }
          onDismissDebtSettlement={controller.dismissDebtSettlement}
          onOpenSettlementTransaction={transactionHash => {
            const explorer =
              controller.status.chain === 'base-sepolia'
                ? 'https://sepolia.basescan.org/tx/'
                : 'https://basescan.org/tx/';
            Linking.openURL(`${explorer}${transactionHash}`).catch(() =>
              setRouteError('The BaseScan transaction link could not be opened.'),
            );
          }}
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
          connected={isCoreReadyForSystemRouting(controller.status)}
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
          systemRoutingSupported={controller.systemTunnel.supported}
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
