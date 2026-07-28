import React from 'react';
import { Text } from 'react-native';
import ReactTestRenderer from 'react-test-renderer';

import { HomeScreen } from '../src/screens/HomeScreen';
import {
  EMPTY_STATUS,
  type CoreStatus,
  type NetworkStatus,
} from '../src/core/types';
import { EMPTY_WALLET_BALANCE } from '../src/core/walletBalance';
import {
  UNSUPPORTED_SYSTEM_TUNNEL,
  type SystemTunnelStatus,
} from '../src/core/systemTunnel';

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
  } = {},
) {
  return (
    <HomeScreen
      busy={options.busy ?? true}
      profileReady={options.profileReady ?? true}
      initializationState={options.initializationState ?? 'ready'}
      profileRecoveryAvailable={options.profileRecoveryAvailable ?? false}
      connectionProgress={{ step: 2, total: 5, label: 'Finding nodes' }}
      entryNodeRefresh={{ attempt: 1, maxAttempts: 3 }}
      issue={null}
      walletBalance={EMPTY_WALLET_BALANCE}
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

  it('hides unvalidated Android system-routing controls while the tunnel is off', () => {
    let renderer!: ReactTestRenderer.ReactTestRenderer;
    ReactTestRenderer.act(() => {
      renderer = ReactTestRenderer.create(
        homeScreen(
          jest.fn(),
          { phase: 'ready' },
          {
            systemTunnel: {
              ...UNSUPPORTED_SYSTEM_TUNNEL,
              supported: true,
            },
          },
        ),
      );
    });

    expect(JSON.stringify(renderer.toJSON())).not.toContain('TRAFFIC SCOPE');
    ReactTestRenderer.act(() => renderer.unmount());
  });

  it('keeps a shutdown path visible for a tunnel left by an earlier preview', () => {
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
            text =>
              text.props.children === 'Turn off experimental system routing',
          ),
    );
    ReactTestRenderer.act(() => control.props.onPress());
    expect(onOpenTrafficRouting).toHaveBeenCalledTimes(1);
    ReactTestRenderer.act(() => renderer.unmount());
  });

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
    expect(JSON.stringify(renderer.toJSON())).not.toContain(
      'CONSUMER FUNDS',
    );
    expect(onOpenSetup).not.toHaveBeenCalled();
    ReactTestRenderer.act(() => renderer.unmount());
  });

  it('retries failed profile initialization without opening settings', () => {
    const onRetryInitialization = jest.fn();
    const onOpenSetup = jest.fn();
    let renderer!: ReactTestRenderer.ReactTestRenderer;
    ReactTestRenderer.act(() => {
      renderer = ReactTestRenderer.create(
        homeScreen(jest.fn(), { phase: 'unconfigured' }, {
          busy: false,
          initializationState: 'error',
          onOpenSetup,
          onRetryInitialization,
          profileReady: false,
        }),
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
        homeScreen(jest.fn(), { phase: 'unconfigured' }, {
          busy: false,
          initializationState: 'error',
          onRecoverNetworkProfile,
          profileReady: false,
          profileRecoveryAvailable: true,
        }),
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
});
