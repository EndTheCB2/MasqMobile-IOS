import React from 'react';
import { Text } from 'react-native';
import ReactTestRenderer from 'react-test-renderer';

import { HomeScreen } from '../src/screens/HomeScreen';
import {
  EMPTY_STATUS,
  type CoreStatus,
  type DebtSettlementQuote,
  type DebtSummary,
  type NetworkStatus,
} from '../src/core/types';
import { EMPTY_WALLET_BALANCE } from '../src/core/walletBalance';
import {
  UNSUPPORTED_SYSTEM_TUNNEL,
  type SystemTunnelStatus,
} from '../src/core/systemTunnel';
import type { EntryNodeRefreshProgress } from '../src/core/entryNodeRefresh';

const network: NetworkStatus = {
  available: true,
  constrained: false,
  expensive: false,
  generation: 1,
  interface: 'wifi',
};

function homeScreen(
  onDisconnect: () => void,
  statusOverrides: Partial<CoreStatus> = {},
  options: {
    busy?: boolean;
    profileReady?: boolean;
    initializationState?: 'loading' | 'ready' | 'error';
    profileRecoveryAvailable?: boolean;
    network?: NetworkStatus;
    onOpenDirectBrowser?: () => void;
    onOpenSetup?: () => void;
    onRetryInitialization?: () => void;
    onRecoverNetworkProfile?: () => void;
    onOpenTrafficRouting?: () => void;
    systemTunnel?: SystemTunnelStatus;
    entryNodeRefresh?: EntryNodeRefreshProgress | null;
    connectionProgress?: { step: number; total: number; label: string };
    debtSummary?: DebtSummary;
    debtSettlementQuote?: DebtSettlementQuote | null;
    onReviewDebtSettlement?: () => void;
    onConfirmDebtSettlement?: () => void;
    onDismissDebtSettlement?: () => void;
  } = {},
) {
  return (
    <HomeScreen
      busy={options.busy ?? true}
      profileReady={options.profileReady ?? true}
      initializationState={options.initializationState ?? 'ready'}
      profileRecoveryAvailable={options.profileRecoveryAvailable ?? false}
      connectionProgress={
        options.connectionProgress ?? {
          step: 2,
          total: 5,
          label: 'Finding nodes',
        }
      }
      entryNodeRefresh={
        options.entryNodeRefresh === undefined
          ? {
              attempt: 1,
              maxAttempts: 3,
              stage: 'discovery',
            }
          : options.entryNodeRefresh
      }
      issue={null}
      walletBalance={EMPTY_WALLET_BALANCE}
      debtSummary={options.debtSummary}
      debtSettlementQuote={options.debtSettlementQuote}
      systemTunnel={options.systemTunnel ?? UNSUPPORTED_SYSTEM_TUNNEL}
      network={options.network ?? network}
      onConnect={jest.fn()}
      onRetryInitialization={options.onRetryInitialization ?? jest.fn()}
      onRecoverNetworkProfile={options.onRecoverNetworkProfile ?? jest.fn()}
      onDisconnect={onDisconnect}
      onOpenBrowser={jest.fn()}
      onOpenDirectBrowser={options.onOpenDirectBrowser ?? jest.fn()}
      onOpenSetup={options.onOpenSetup ?? jest.fn()}
      onOpenTrafficRouting={options.onOpenTrafficRouting ?? jest.fn()}
      onOpenPrivacy={jest.fn()}
      onOpenSystemSettings={jest.fn()}
      onRemoveWallet={jest.fn()}
      onReset={jest.fn()}
      onResetNetwork={jest.fn()}
      onRetry={jest.fn()}
      onShareDiagnostics={jest.fn()}
      onUpdateMinHops={jest.fn()}
      onRefreshWalletBalance={jest.fn()}
      onReviewDebtSettlement={
        options.onReviewDebtSettlement ?? jest.fn()
      }
      onConfirmDebtSettlement={
        options.onConfirmDebtSettlement ?? jest.fn()
      }
      onDismissDebtSettlement={
        options.onDismissDebtSettlement ?? jest.fn()
      }
      status={{
        ...EMPTY_STATUS,
        chain: 'base-mainnet',
        engineAvailable: true,
        phase: 'connecting',
        walletAddress: '0x1234567890abcdef',
        ...statusOverrides,
      }}
    />
  );
}

describe('HomeScreen connection controls', () => {
  it('shows an entry-only stage as route building instead of connected', () => {
    let renderer!: ReactTestRenderer.ReactTestRenderer;
    ReactTestRenderer.act(() => {
      renderer = ReactTestRenderer.create(
        homeScreen(
          jest.fn(),
          {
            connectedNeighbors: 1,
            phase: 'connecting',
            proxyPort: 44_443,
            routeStage: 1,
          },
          {
            connectionProgress: {
              step: 4,
              total: 5,
              label: 'Preparing a private exit route',
            },
            entryNodeRefresh: null,
          },
        ),
      );
    });

    const rendered = JSON.stringify(renderer.toJSON());
    expect(rendered).toContain('Building private route');
    expect(rendered).toContain('Preparing a private exit route');
    expect(rendered).toContain('Cancel connection');
    expect(rendered).not.toContain('Open private browser');
    ReactTestRenderer.act(() => renderer.unmount());
  });

  it('shows private browsing only after stage two route proof', () => {
    let renderer!: ReactTestRenderer.ReactTestRenderer;
    ReactTestRenderer.act(() => {
      renderer = ReactTestRenderer.create(
        homeScreen(
          jest.fn(),
          {
            connectedNeighbors: 1,
            phase: 'connected',
            proxyPort: 44_443,
            routeHops: 3,
            routeStage: 2,
          },
          { busy: false },
        ),
      );
    });

    const rendered = JSON.stringify(renderer.toJSON());
    expect(rendered).toContain('MASQ route ready');
    expect(rendered).toContain('Open private browser');
    expect(rendered).not.toContain('Entry nodes connected');
    ReactTestRenderer.act(() => renderer.unmount());
  });

  it.each([
    [
      'discovery' as const,
      'Finding entry nodes · 2/6',
      'Finding reachable entry nodes (2/6)',
    ],
    [
      'handshake' as const,
      'Connecting to entry peer · 2/6',
      'Connecting to an entry peer (attempt 2/6)',
    ],
  ])(
    'shows the active %s attempt before connection completes',
    (stage, buttonLabel, progressLabel) => {
      let renderer!: ReactTestRenderer.ReactTestRenderer;
      ReactTestRenderer.act(() => {
        renderer = ReactTestRenderer.create(
          homeScreen(
            jest.fn(),
            {},
            {
              connectionProgress: {
                step: stage === 'discovery' ? 2 : 3,
                total: 5,
                label: progressLabel,
              },
              entryNodeRefresh: { attempt: 2, maxAttempts: 6, stage },
            },
          ),
        );
      });

      const screen = JSON.stringify(renderer.toJSON());
      expect(screen).toContain(buttonLabel);
      expect(screen).toContain(progressLabel);
      ReactTestRenderer.act(() => renderer.unmount());
    },
  );

  it('keeps an explicit cancel action available while connecting', () => {
    const onDisconnect = jest.fn();
    let renderer!: ReactTestRenderer.ReactTestRenderer;
    ReactTestRenderer.act(() => {
      renderer = ReactTestRenderer.create(homeScreen(onDisconnect));
    });

    const cancelLabel = renderer.root
      .findAllByType(Text)
      .find(node => node.props.children === 'Cancel connection');
    expect(cancelLabel).toBeDefined();
    const cancelButton = renderer.root.find(
      node =>
        node.props.accessibilityRole === 'button' &&
        node
          .findAllByType(Text)
          .some(text => text.props.children === 'Cancel connection'),
    );
    expect(cancelButton.props.disabled).toBeFalsy();

    ReactTestRenderer.act(() => cancelButton.props.onPress());
    expect(onDisconnect).toHaveBeenCalledTimes(1);
    ReactTestRenderer.act(() => renderer.unmount());
  });

  it('does not expose the removed public-exit verification component', () => {
    let renderer!: ReactTestRenderer.ReactTestRenderer;
    ReactTestRenderer.act(() => {
      renderer = ReactTestRenderer.create(
        homeScreen(jest.fn(), {
          connectedNeighbors: 1,
          phase: 'connected',
          proxyEnabled: true,
          routeHops: 1,
        }),
      );
    });

    const screen = JSON.stringify(renderer.toJSON());
    expect(screen).not.toContain('Verify exit');
    expect(screen).not.toContain('PUBLIC EXIT');
    ReactTestRenderer.act(() => renderer.unmount());
  });

  it('hides unavailable system-routing controls from the iOS release UI', () => {
    let renderer!: ReactTestRenderer.ReactTestRenderer;
    ReactTestRenderer.act(() => {
      renderer = ReactTestRenderer.create(
        homeScreen(jest.fn(), { phase: 'ready' }),
      );
    });

    const screen = JSON.stringify(renderer.toJSON());
    expect(screen).not.toContain('TRAFFIC SCOPE');
    expect(screen).not.toContain('Private browser only in this iOS build');
    expect(screen).toContain('PRIVACY & LEGAL');
    ReactTestRenderer.act(() => renderer.unmount());
  });

  it('shows dogfood controls only when native system routing is supported', () => {
    const onOpenTrafficRouting = jest.fn();
    let renderer!: ReactTestRenderer.ReactTestRenderer;
    ReactTestRenderer.act(() => {
      renderer = ReactTestRenderer.create(
        homeScreen(
          jest.fn(),
          { phase: 'ready' },
          {
            onOpenTrafficRouting,
            systemTunnel: {
              ...UNSUPPORTED_SYSTEM_TUNNEL,
              supported: true,
            },
          },
        ),
      );
    });

    const rendered = JSON.stringify(renderer.toJSON());
    expect(rendered).toContain('TRAFFIC SCOPE');
    expect(rendered).toContain('Private browser only');
    expect(rendered).toContain(
      'Configure experimental device or selected-app routing',
    );
    const control = renderer.root.find(
      node =>
        node.props.accessibilityRole === 'button' &&
        node
          .findAllByType(Text)
          .some(text => text.props.children === 'Private browser only'),
    );
    ReactTestRenderer.act(() => control.props.onPress());
    expect(onOpenTrafficRouting).toHaveBeenCalledTimes(1);
    ReactTestRenderer.act(() => renderer.unmount());
  });

  it('keeps native-supported routing visible but disabled until the profile is ready', () => {
    const onOpenTrafficRouting = jest.fn();
    let renderer!: ReactTestRenderer.ReactTestRenderer;
    ReactTestRenderer.act(() => {
      renderer = ReactTestRenderer.create(
        homeScreen(
          jest.fn(),
          { phase: 'ready' },
          {
            onOpenTrafficRouting,
            profileReady: false,
            systemTunnel: {
              ...UNSUPPORTED_SYSTEM_TUNNEL,
              supported: true,
            },
          },
        ),
      );
    });

    const control = renderer.root.find(
      node =>
        node.props.accessibilityRole === 'button' &&
        node
          .findAllByType(Text)
          .some(text => text.props.children === 'Private browser only'),
    );
    expect(control.props.accessibilityState.disabled).toBe(true);
    expect(control.props.disabled).toBe(true);
    expect(onOpenTrafficRouting).not.toHaveBeenCalled();
    ReactTestRenderer.act(() => renderer.unmount());
  });

  it('shows the native-reported active scope', () => {
    const onOpenTrafficRouting = jest.fn();
    let renderer!: ReactTestRenderer.ReactTestRenderer;
    ReactTestRenderer.act(() => {
      renderer = ReactTestRenderer.create(
        homeScreen(
          jest.fn(),
          { phase: 'connected' },
          {
            onOpenTrafficRouting,
            systemTunnel: {
              active: true,
              lastError: null,
              mode: 'wholeDevice',
              phase: 'active',
              selectedApps: [],
              supported: true,
              trafficDisposition: 'masq',
            },
          },
        ),
      );
    });

    const control = renderer.root.find(
      node =>
        node.props.accessibilityRole === 'button' &&
        node
          .findAllByType(Text)
          .some(
            text => text.props.children === 'Whole-device HTTPS route ready',
          ),
    );
    ReactTestRenderer.act(() => control.props.onPress());
    expect(onOpenTrafficRouting).toHaveBeenCalledTimes(1);
    ReactTestRenderer.act(() => renderer.unmount());
  });

  it('surfaces native direct-risk state instead of claiming an active route', () => {
    let renderer!: ReactTestRenderer.ReactTestRenderer;
    ReactTestRenderer.act(() => {
      renderer = ReactTestRenderer.create(
        homeScreen(
          jest.fn(),
          { phase: 'connected' },
          {
            systemTunnel: {
              active: false,
              appliedMode: 'off',
              appliedRevision: null,
              appliedSelectedApps: [],
              lastError: 'PROCESS_RESTARTED',
              mode: 'wholeDevice',
              phase: 'blocked',
              selectedApps: [],
              supported: true,
              trafficDisposition: 'directRisk',
            },
          },
        ),
      );
    });

    const rendered = JSON.stringify(renderer.toJSON());
    expect(rendered).toContain('Traffic may be direct');
    expect(rendered).toContain('Android cannot confirm capture');
    expect(rendered).not.toContain('dogfood route active');
    expect(rendered.toLowerCase()).not.toContain('protected');
    ReactTestRenderer.act(() => renderer.unmount());
  });

  it.each(['blocked', 'stopping'] as const)(
    'does not render legacy active %s state as a MASQ route',
    phase => {
      let renderer!: ReactTestRenderer.ReactTestRenderer;
      ReactTestRenderer.act(() => {
        renderer = ReactTestRenderer.create(
          homeScreen(
            jest.fn(),
            { phase: 'connected' },
            {
              systemTunnel: {
                active: true,
                lastError: 'LEGACY_STOP_STATE',
                mode: 'wholeDevice',
                phase,
                selectedApps: [],
                supported: true,
              },
            },
          ),
        );
      });

      const rendered = JSON.stringify(renderer.toJSON());
      expect(rendered).toContain('Captured system traffic is blocked');
      expect(rendered).not.toContain('dogfood route active');
      expect(rendered.toLowerCase()).not.toContain('protected');
      ReactTestRenderer.act(() => renderer.unmount());
    },
  );

  it('locks profile-dependent actions while the saved profile is loading', () => {
    const onOpenSetup = jest.fn();
    let renderer!: ReactTestRenderer.ReactTestRenderer;
    ReactTestRenderer.act(() => {
      renderer = ReactTestRenderer.create(
        homeScreen(
          jest.fn(),
          {
            chain: 'base-mainnet',
            phase: 'ready',
            walletAddress: '0x1234567890abcdef',
          },
          {
            busy: true,
            initializationState: 'loading',
            onOpenSetup,
            profileReady: false,
          },
        ),
      );
    });

    const settings = renderer.root.find(
      node =>
        node.props.accessibilityRole === 'button' &&
        node
          .findAllByType(Text)
          .some(text => text.props.children === 'Node & wallet settings'),
    );
    expect(settings.props.disabled).toBe(true);
    expect(settings.props.accessibilityState).toEqual({ disabled: true });
    expect(JSON.stringify(renderer.toJSON())).toContain(
      'Loading the complete saved profile',
    );
    expect(JSON.stringify(renderer.toJSON())).not.toContain('CONSUMER FUNDS');
    expect(onOpenSetup).not.toHaveBeenCalled();
    ReactTestRenderer.act(() => renderer.unmount());
  });

  it('retries failed profile initialization without opening settings', () => {
    const onRetryInitialization = jest.fn();
    const onOpenSetup = jest.fn();
    let renderer!: ReactTestRenderer.ReactTestRenderer;
    ReactTestRenderer.act(() => {
      renderer = ReactTestRenderer.create(
        homeScreen(
          jest.fn(),
          { phase: 'unconfigured' },
          {
            busy: false,
            initializationState: 'error',
            onOpenSetup,
            onRetryInitialization,
            profileReady: false,
          },
        ),
      );
    });

    const retry = renderer.root.find(
      node =>
        node.props.accessibilityRole === 'button' &&
        node
          .findAllByType(Text)
          .some(text => text.props.children === 'Retry profile loading'),
    );
    ReactTestRenderer.act(() => retry.props.onPress());
    expect(onRetryInitialization).toHaveBeenCalledTimes(1);
    expect(onOpenSetup).not.toHaveBeenCalled();
    ReactTestRenderer.act(() => renderer.unmount());
  });

  it('offers an explicit wallet-preserving recovery while all browsing stays blocked', () => {
    const onRecoverNetworkProfile = jest.fn();
    let renderer!: ReactTestRenderer.ReactTestRenderer;
    ReactTestRenderer.act(() => {
      renderer = ReactTestRenderer.create(
        homeScreen(
          jest.fn(),
          { phase: 'unconfigured' },
          {
            busy: false,
            initializationState: 'error',
            onRecoverNetworkProfile,
            profileReady: false,
            profileRecoveryAvailable: true,
          },
        ),
      );
    });

    const recovery = renderer.root.find(
      node =>
        node.props.accessibilityRole === 'button' &&
        node
          .findAllByType(Text)
          .some(
            text =>
              text.props.children === 'Reset network profile · keep wallet',
          ),
    );
    const directBrowser = renderer.root.find(
      node =>
        node.props.accessibilityRole === 'button' &&
        node
          .findAllByType(Text)
          .some(text => text.props.children === 'Browse without MASQ'),
    );
    expect(directBrowser.props.disabled).toBe(true);
    expect(JSON.stringify(renderer.toJSON())).toContain(
      'consumer wallet stays on this device',
    );
    ReactTestRenderer.act(() => recovery.props.onPress());
    expect(onRecoverNetworkProfile).toHaveBeenCalledTimes(1);
    ReactTestRenderer.act(() => renderer.unmount());
  });

  it.each([
    [
      'configured and connected',
      {
        connectedNeighbors: 1,
        phase: 'connected' as const,
        proxyEnabled: false,
        routeHops: 1,
      },
    ],
    [
      'not configured and disconnected',
      {
        chain: null,
        engineAvailable: false,
        phase: 'unconfigured' as const,
        walletAddress: null,
      },
    ],
  ])('offers explicit direct browsing when %s', (_label, statusOverrides) => {
    const onOpenDirectBrowser = jest.fn();
    let renderer!: ReactTestRenderer.ReactTestRenderer;
    ReactTestRenderer.act(() => {
      renderer = ReactTestRenderer.create(
        homeScreen(jest.fn(), statusOverrides, {
          busy: false,
          onOpenDirectBrowser,
        }),
      );
    });

    const directButton = renderer.root.find(
      node =>
        node.props.accessibilityRole === 'button' &&
        node
          .findAllByType(Text)
          .some(text => text.props.children === 'Browse without MASQ'),
    );
    expect(directButton.props.disabled).toBe(false);
    ReactTestRenderer.act(() => directButton.props.onPress());
    expect(onOpenDirectBrowser).toHaveBeenCalledTimes(1);
    expect(JSON.stringify(renderer.toJSON())).toContain(
      'MASQ Private never falls back to a direct connection',
    );
    ReactTestRenderer.act(() => renderer.unmount());
  });

  it('disables direct browsing only while busy or known to be offline', () => {
    let renderer!: ReactTestRenderer.ReactTestRenderer;
    ReactTestRenderer.act(() => {
      renderer = ReactTestRenderer.create(
        homeScreen(
          jest.fn(),
          { phase: 'ready' },
          {
            busy: false,
            network: { ...network, available: false },
          },
        ),
      );
    });

    const directButton = renderer.root.find(
      node =>
        node.props.accessibilityRole === 'button' &&
        node
          .findAllByType(Text)
          .some(text => text.props.children === 'Browse without MASQ'),
    );
    expect(directButton.props.disabled).toBe(true);
    ReactTestRenderer.act(() => renderer.unmount());
  });

  it('requires an explicit in-app confirmation without device authentication', () => {
    const onReviewDebtSettlement = jest.fn();
    const onConfirmDebtSettlement = jest.fn();
    let renderer!: ReactTestRenderer.ReactTestRenderer;
    ReactTestRenderer.act(() => {
      renderer = ReactTestRenderer.create(
        homeScreen(
          jest.fn(),
          { phase: 'ready' },
          {
            busy: false,
            debtSummary: {
              totalMasqWei: '230081000000000',
              creditorCount: 11,
              settlementInProgress: false,
            },
            onReviewDebtSettlement,
          },
        ),
      );
    });

    const reviewButton = renderer.root.find(
      node =>
        node.props.accessibilityRole === 'button' &&
        node
          .findAllByType(Text)
          .some(text => text.props.children === 'Review MASQ debts'),
    );
    ReactTestRenderer.act(() => reviewButton.props.onPress());
    expect(onReviewDebtSettlement).toHaveBeenCalledTimes(1);

    ReactTestRenderer.act(() => {
      renderer.update(
        homeScreen(
          jest.fn(),
          { phase: 'ready' },
          {
            busy: false,
            debtSummary: {
              totalMasqWei: '230081000000000',
              creditorCount: 11,
              settlementInProgress: false,
            },
            debtSettlementQuote: {
              quoteId: '0123456789abcdef0123456789abcdef',
              createdAtUnixSeconds: 1_700_000_000,
              expiresAtUnixSeconds: 1_700_000_300,
              totalMasqWei: '230081000000000',
              estimatedL2FeeWei: '4200000000000',
              masqBalanceWei: '1000000000000000000',
              baseEthBalanceWei: '100000000000000000',
              creditorCount: 11,
              hasMoreCreditors: false,
              feeEstimateIncludesL1DataFee: false,
              requiresDeviceAuthentication: false,
              requiresExplicitConfirmation: true,
            },
            onConfirmDebtSettlement,
          },
        ),
      );
    });

    expect(JSON.stringify(renderer.toJSON())).toContain(
      'No device code or biometric check is used',
    );
    const settleButton = renderer.root.find(
      node =>
        node.props.accessibilityRole === 'button' &&
        node
          .findAllByType(Text)
          .some(text => text.props.children === 'Settle now'),
    );
    ReactTestRenderer.act(() => settleButton.props.onPress());
    expect(onConfirmDebtSettlement).toHaveBeenCalledTimes(1);
    ReactTestRenderer.act(() => renderer.unmount());
  });
});
