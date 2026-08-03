/**
 * @format
 */

import React from 'react';
import {
  Alert,
  AppState,
  BackHandler,
  Platform,
  Text,
  TextInput,
  type AppStateStatus,
} from 'react-native';
import ReactTestRenderer from 'react-test-renderer';

import {
  DEFAULT_SETUP,
  EMPTY_STATUS,
  type CoreStatus,
  type NetworkStatus,
} from '../src/core/types';
import { masqCore } from '../src/core/masqCore';
import { EMPTY_WALLET_BALANCE } from '../src/core/walletBalance';
import { UNSUPPORTED_SYSTEM_TUNNEL } from '../src/core/systemTunnel';

const mockPrepareBrowserSession = jest.fn();
const mockCloseBrowserSession = jest.fn();
const mockRepairPrivateBrowserRoute = jest.fn();
const TEST_PLATFORM_OS = Platform.OS;
const HARDWARE_BACK_EVENT = {
  timeStamp: 0,
  type: 'hardwareBackPress',
};
const mockController = {
  busy: false,
  profileReady: true,
  initializationState: 'ready' as 'loading' | 'ready' | 'error',
  profileRecoveryAvailable: false,
  connectionProgress: { step: 1, total: 5, label: 'Ready' },
  draft: DEFAULT_SETUP,
  entryNodeRefresh: null,
  issue: null,
  network: {
    available: true,
    constrained: false,
    expensive: false,
    generation: 1,
    interface: 'wifi',
  } as NetworkStatus,
  refresh: jest.fn().mockResolvedValue(EMPTY_STATUS),
  retryInitialization: jest.fn().mockResolvedValue(undefined),
  recoverNetworkProfile: jest.fn().mockResolvedValue(undefined),
  refreshWalletBalance: jest.fn().mockResolvedValue(undefined),
  removeWallet: jest.fn().mockResolvedValue(undefined),
  reset: jest.fn().mockResolvedValue(undefined),
  resetNetworkProfile: jest.fn().mockResolvedValue(undefined),
  routableApps: [],
  saveSetup: jest.fn().mockResolvedValue(undefined),
  status: { ...EMPTY_STATUS } as CoreStatus,
  systemTunnel: UNSUPPORTED_SYSTEM_TUNNEL,
  systemTunnelBusy: false,
  updateMinHops: jest.fn().mockResolvedValue(undefined),
  updateSystemTunnel: jest.fn().mockResolvedValue(undefined),
  walletBalance: EMPTY_WALLET_BALANCE,
  connect: jest.fn().mockResolvedValue(EMPTY_STATUS),
  disconnect: jest.fn().mockResolvedValue(EMPTY_STATUS),
};

jest.mock('../src/core/browserSession', () => ({
  closeBrowserSession: (...args: unknown[]) => mockCloseBrowserSession(...args),
  prepareBrowserSession: (...args: unknown[]) =>
    mockPrepareBrowserSession(...args),
  repairPrivateBrowserRoute: (...args: unknown[]) =>
    mockRepairPrivateBrowserRoute(...args),
}));

jest.mock('../src/hooks/useMasqController', () => ({
  useMasqController: () => mockController,
}));

jest.mock('../src/screens/BrowserScreen', () => {
  const ReactModule = require('react');
  const { AppState: NativeAppState, View } = require('react-native');
  return {
    BrowserScreen: ({
      mode,
      onClose,
      onRepairRoute,
    }: {
      mode: 'masq' | 'direct';
      onClose: (reason?: 'user' | 'background') => Promise<void> | void;
      onRepairRoute?: () => Promise<void>;
    }) => {
      ReactModule.useEffect(() => {
        const subscription = NativeAppState.addEventListener(
          'change',
          (state: AppStateStatus) => {
            if (state !== 'active') {
              Promise.resolve(onClose('background')).catch(() => undefined);
            }
          },
        );
        return () => subscription.remove();
      }, [onClose]);
      return ReactModule.createElement(View, {
        mode,
        onClose,
        onRepairRoute,
        testID: 'browser-screen',
      });
    },
  };
});

jest.mock('react-native-safe-area-context', () => {
  const ReactModule = require('react');
  const { View } = require('react-native');
  const Container = ({ children }: { children: React.ReactNode }) =>
    ReactModule.createElement(View, null, children);
  return {
    SafeAreaProvider: Container,
    SafeAreaView: Container,
  };
});

import App from '../App';

describe('App browser routing modes', () => {
  beforeEach(() => {
    mockPrepareBrowserSession
      .mockReset()
      .mockImplementation(async (_core, mode) => mode);
    mockCloseBrowserSession.mockReset().mockResolvedValue('blocked');
    mockRepairPrivateBrowserRoute.mockReset().mockResolvedValue(undefined);
    mockController.refresh.mockClear();
    mockController.retryInitialization.mockClear();
    mockController.recoverNetworkProfile.mockClear();
    mockController.connect.mockClear();
    mockController.disconnect.mockClear();
    mockController.systemTunnel = UNSUPPORTED_SYSTEM_TUNNEL;
    mockController.profileReady = true;
    mockController.initializationState = 'ready';
    mockController.profileRecoveryAvailable = false;
    mockController.busy = false;
    mockController.draft = DEFAULT_SETUP;
    mockController.status = { ...EMPTY_STATUS };
    mockController.network = {
      available: true,
      constrained: false,
      expensive: false,
      generation: 1,
      interface: 'wifi',
    };
  });

  afterEach(() => {
    (Platform as unknown as { OS: string }).OS = TEST_PLATFORM_OS;
    jest.restoreAllMocks();
    jest.useRealTimers();
  });

  it('requires an explicit warning confirmation before direct browsing', async () => {
    const alert = jest.spyOn(Alert, 'alert').mockImplementation(jest.fn());
    const renderer = await renderApp();
    const directButton = findButton(renderer, 'Browse without MASQ');

    ReactTestRenderer.act(() => directButton.props.onPress());

    expect(alert).toHaveBeenCalledTimes(1);
    expect(alert.mock.calls[0][0]).toBe('Browse without MASQ?');
    expect(alert.mock.calls[0][1]).toContain(
      'public IP used by your current connection or VPN',
    );
    expect(alert.mock.calls[0][1]).toContain(
      'stops any active MASQ connection and system routing',
    );
    expect(alert.mock.calls[0][1]).toContain('DNS service');
    expect(alert.mock.calls[0][1]).toContain(
      'MASQ hops and exit-country settings do not apply',
    );
    expect(mockPrepareBrowserSession).not.toHaveBeenCalled();

    const actions = alert.mock.calls[0][2] ?? [];
    const browseDirectly = actions.find(
      action => action.text === 'Browse directly',
    );
    await ReactTestRenderer.act(async () => {
      browseDirectly?.onPress?.();
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(mockPrepareBrowserSession).toHaveBeenCalledTimes(1);
    expect(mockPrepareBrowserSession.mock.calls[0][1]).toBe('direct');
    expect(mockController.disconnect).toHaveBeenCalledTimes(1);
    const browser = renderer.root.findByProps({ testID: 'browser-screen' });
    expect(browser.props.mode).toBe('direct');

    await ReactTestRenderer.act(async () => {
      await browser.props.onClose();
    });
    expect(mockCloseBrowserSession).toHaveBeenCalledTimes(1);
    expect(
      renderer.root.findAllByProps({ testID: 'browser-screen' }),
    ).toHaveLength(0);
    ReactTestRenderer.act(() => renderer.unmount());
  });

  it('requires a fresh confirmation for every direct session', async () => {
    const alert = jest.spyOn(Alert, 'alert').mockImplementation(jest.fn());
    const renderer = await renderApp();

    ReactTestRenderer.act(() =>
      findButton(renderer, 'Browse without MASQ').props.onPress(),
    );
    await ReactTestRenderer.act(async () => {
      alert.mock.calls[0][2]
        ?.find(action => action.text === 'Browse directly')
        ?.onPress?.();
      await Promise.resolve();
      await Promise.resolve();
    });
    const browser = renderer.root.findByProps({ testID: 'browser-screen' });
    await ReactTestRenderer.act(async () => {
      await browser.props.onClose();
    });

    ReactTestRenderer.act(() =>
      findButton(renderer, 'Browse without MASQ').props.onPress(),
    );
    expect(alert).toHaveBeenCalledTimes(2);
    expect(mockPrepareBrowserSession).toHaveBeenCalledTimes(1);

    await ReactTestRenderer.act(async () => {
      alert.mock.calls[1][2]
        ?.find(action => action.text === 'Browse directly')
        ?.onPress?.();
      await Promise.resolve();
      await Promise.resolve();
    });
    expect(mockPrepareBrowserSession).toHaveBeenCalledTimes(2);
    ReactTestRenderer.act(() => renderer.unmount());
  });

  it('disconnects an active MASQ mesh before direct browsing', async () => {
    mockController.status = {
      ...EMPTY_STATUS,
      chain: 'base-mainnet',
      connectedNeighbors: 1,
      engineAvailable: true,
      phase: 'connected',
      routeHops: 2,
      walletAddress: '0x1234567890abcdef',
    };
    const shutdown = jest.spyOn(masqCore, 'shutdown').mockResolvedValue({
      ...mockController.status,
      connectedNeighbors: 0,
      phase: 'ready',
      proxyEnabled: false,
      proxyPort: null,
      routeHops: 0,
    });
    const alert = jest.spyOn(Alert, 'alert').mockImplementation(jest.fn());
    const renderer = await renderApp();

    ReactTestRenderer.act(() =>
      findButton(renderer, 'Browse without MASQ').props.onPress(),
    );
    await ReactTestRenderer.act(async () => {
      alert.mock.calls[0][2]
        ?.find(action => action.text === 'Browse directly')
        ?.onPress?.();
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(mockController.disconnect).toHaveBeenCalledTimes(1);
    expect(mockController.disconnect.mock.invocationCallOrder[0]).toBeLessThan(
      shutdown.mock.invocationCallOrder[0],
    );
    expect(shutdown.mock.invocationCallOrder[0]).toBeLessThan(
      mockPrepareBrowserSession.mock.invocationCallOrder[0],
    );
    expect(shutdown).toHaveBeenCalledTimes(1);
    expect(mockPrepareBrowserSession.mock.calls[0][1]).toBe('direct');
    ReactTestRenderer.act(() => renderer.unmount());
  });

  it('can reconnect to MASQ after closing a direct session', async () => {
    const connected: CoreStatus = {
      ...EMPTY_STATUS,
      chain: 'base-mainnet',
      connectedNeighbors: 1,
      engineAvailable: true,
      phase: 'connected',
      routeHops: 2,
      walletAddress: '0x1234567890abcdef',
    };
    const ready: CoreStatus = {
      ...connected,
      connectedNeighbors: 0,
      phase: 'ready',
      proxyEnabled: false,
      proxyPort: null,
      routeHops: 0,
    };
    mockController.status = connected;
    jest.spyOn(masqCore, 'shutdown').mockResolvedValue(ready);
    mockController.refresh.mockImplementationOnce(async () => {
      mockController.status = ready;
      return ready;
    });
    const alert = jest.spyOn(Alert, 'alert').mockImplementation(jest.fn());
    const renderer = await renderApp();

    ReactTestRenderer.act(() =>
      findButton(renderer, 'Browse without MASQ').props.onPress(),
    );
    await ReactTestRenderer.act(async () => {
      alert.mock.calls[0][2]
        ?.find(action => action.text === 'Browse directly')
        ?.onPress?.();
      await Promise.resolve();
      await Promise.resolve();
    });
    await ReactTestRenderer.act(async () => {
      await renderer.root
        .findByProps({ testID: 'browser-screen' })
        .props.onClose();
    });
    ReactTestRenderer.act(() =>
      findButton(renderer, 'Connect to MASQ').props.onPress(),
    );

    expect(mockController.connect).toHaveBeenCalledTimes(1);
    ReactTestRenderer.act(() => renderer.unmount());
  });

  it('keeps direct browsing blocked without a teardown acknowledgement', async () => {
    mockController.status = {
      ...EMPTY_STATUS,
      chain: 'base-mainnet',
      connectedNeighbors: 1,
      engineAvailable: true,
      engineGeneration: 1,
      phase: 'connected',
      proxyPort: 44_443,
      routeHops: 2,
      walletAddress: '0x1234567890abcdef',
    };
    const shutdown = jest.spyOn(masqCore, 'shutdown').mockResolvedValueOnce({
      ...mockController.status,
      phase: 'connected',
      proxyEnabled: false,
    });
    const alert = jest.spyOn(Alert, 'alert').mockImplementation(jest.fn());
    const renderer = await renderApp();

    ReactTestRenderer.act(() =>
      findButton(renderer, 'Browse without MASQ').props.onPress(),
    );
    await ReactTestRenderer.act(async () => {
      alert.mock.calls[0][2]
        ?.find(action => action.text === 'Browse directly')
        ?.onPress?.();
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(mockController.disconnect).toHaveBeenCalledTimes(1);
    expect(shutdown).toHaveBeenCalledTimes(1);
    expect(mockPrepareBrowserSession).not.toHaveBeenCalled();
    expect(JSON.stringify(renderer.toJSON())).toContain(
      'MASQ could not confirm that its peer connection and system routing stopped.',
    );
    expect(
      renderer.root.findAllByProps({ testID: 'browser-screen' }),
    ).toHaveLength(0);
    ReactTestRenderer.act(() => renderer.unmount());
  });

  it('keeps direct browsing blocked when native teardown does not finish', async () => {
    jest.useFakeTimers();
    const shutdown = jest
      .spyOn(masqCore, 'shutdown')
      .mockReturnValue(new Promise(() => undefined));
    const alert = jest.spyOn(Alert, 'alert').mockImplementation(jest.fn());
    const renderer = await renderApp();

    ReactTestRenderer.act(() =>
      findButton(renderer, 'Browse without MASQ').props.onPress(),
    );
    ReactTestRenderer.act(() => {
      alert.mock.calls[0][2]
        ?.find(action => action.text === 'Browse directly')
        ?.onPress?.();
    });
    await ReactTestRenderer.act(async () => {
      await Promise.resolve();
      await Promise.resolve();
    });
    await ReactTestRenderer.act(async () => {
      jest.advanceTimersByTime(15_000);
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(shutdown).toHaveBeenCalledTimes(1);
    expect(mockPrepareBrowserSession).not.toHaveBeenCalled();
    expect(JSON.stringify(renderer.toJSON())).toContain(
      'MASQ shutdown did not finish. Direct browsing remains blocked.',
    );
    expect(
      renderer.root.findAllByProps({ testID: 'browser-screen' }),
    ).toHaveLength(0);
    ReactTestRenderer.act(() => renderer.unmount());
  });

  it('keeps direct-open failures out of MASQ retry classification', async () => {
    mockController.status = {
      ...EMPTY_STATUS,
      chain: 'base-mainnet',
      connectedNeighbors: 1,
      engineAvailable: true,
      phase: 'connected',
      routeHops: 2,
      walletAddress: '0x1234567890abcdef',
    };
    mockPrepareBrowserSession.mockRejectedValue(
      new Error('Turn off MASQ system routing before browsing directly.'),
    );
    const alert = jest.spyOn(Alert, 'alert').mockImplementation(jest.fn());
    const renderer = await renderApp();

    ReactTestRenderer.act(() =>
      findButton(renderer, 'Browse without MASQ').props.onPress(),
    );
    await ReactTestRenderer.act(async () => {
      alert.mock.calls[0][2]
        ?.find(action => action.text === 'Browse directly')
        ?.onPress?.();
      await Promise.resolve();
      await Promise.resolve();
    });

    const content = JSON.stringify(renderer.toJSON());
    expect(content).toContain(
      'Turn off MASQ system routing before browsing directly.',
    );
    expect(content).not.toContain('The private route was interrupted');
    expect(content).not.toContain('Retry connection');
    expect(mockController.connect).not.toHaveBeenCalled();
    ReactTestRenderer.act(() => renderer.unmount());
  });

  it('never falls back to direct when MASQ preparation fails', async () => {
    mockController.status = {
      ...EMPTY_STATUS,
      chain: 'base-mainnet',
      connectedNeighbors: 1,
      engineAvailable: true,
      phase: 'connected',
      proxyPort: 44_443,
      routeHops: 2,
      routeStage: 2,
      walletAddress: '0x1234567890abcdef',
    };
    mockPrepareBrowserSession.mockRejectedValue(
      new Error('MASQ route verification failed.'),
    );
    const renderer = await renderApp();

    await ReactTestRenderer.act(async () => {
      findButton(renderer, 'Open private browser').props.onPress();
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(mockPrepareBrowserSession.mock.calls.map(call => call[1])).toEqual([
      'masq',
    ]);
    expect(mockCloseBrowserSession).toHaveBeenCalled();
    expect(
      renderer.root.findAllByProps({ testID: 'browser-screen' }),
    ).toHaveLength(0);
    expect(JSON.stringify(renderer.toJSON())).toContain(
      'The private route was interrupted',
    );
    ReactTestRenderer.act(() => renderer.unmount());
  });

  it('clears a transient private-browser issue after the native route recovers', async () => {
    mockController.status = {
      ...EMPTY_STATUS,
      chain: 'base-mainnet',
      connectedNeighbors: 1,
      engineAvailable: true,
      engineGeneration: 1,
      phase: 'connected',
      proxyPort: 44_443,
      routeHops: 1,
      routeStage: 1,
      walletAddress: '0x1234567890abcdef',
    };
    mockPrepareBrowserSession.mockRejectedValue(
      new Error('MASQ route verification failed.'),
    );
    const renderer = await renderApp();

    await ReactTestRenderer.act(async () => {
      findButton(renderer, 'Open private browser').props.onPress();
      await Promise.resolve();
      await Promise.resolve();
    });
    expect(JSON.stringify(renderer.toJSON())).toContain(
      'The private route was interrupted',
    );

    mockController.status = {
      ...mockController.status,
      lastError: null,
      routeHops: 2,
      routeStage: 2,
    };
    await ReactTestRenderer.act(async () => {
      renderer.update(<App />);
      await Promise.resolve();
    });

    expect(JSON.stringify(renderer.toJSON())).not.toContain(
      'The private route was interrupted',
    );
    ReactTestRenderer.act(() => renderer.unmount());
  });

  it('blocks a pending direct session when the app enters the background', async () => {
    const emitAppState = mockAppStateChanges();
    let resolvePreparation!: (mode: 'direct') => void;
    mockPrepareBrowserSession.mockReturnValue(
      new Promise(resolve => {
        resolvePreparation = resolve;
      }),
    );
    const alert = jest.spyOn(Alert, 'alert').mockImplementation(jest.fn());
    const renderer = await renderApp();

    ReactTestRenderer.act(() => {
      findButton(renderer, 'Browse without MASQ').props.onPress();
    });
    const actions = alert.mock.calls[0][2] ?? [];
    ReactTestRenderer.act(() => {
      actions.find(action => action.text === 'Browse directly')?.onPress?.();
    });
    ReactTestRenderer.act(() => {
      emitAppState('background');
    });

    expect(mockCloseBrowserSession).toHaveBeenCalledTimes(1);

    await ReactTestRenderer.act(async () => {
      resolvePreparation('direct');
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(mockCloseBrowserSession).toHaveBeenCalledTimes(2);
    expect(
      renderer.root.findAllByProps({ testID: 'browser-screen' }),
    ).toHaveLength(0);
    ReactTestRenderer.act(() => renderer.unmount());
  });

  it('closes and blocks an active direct session in the background', async () => {
    const emitAppState = mockAppStateChanges();
    const alert = jest.spyOn(Alert, 'alert').mockImplementation(jest.fn());
    const renderer = await renderApp();
    ReactTestRenderer.act(() =>
      findButton(renderer, 'Browse without MASQ').props.onPress(),
    );
    await ReactTestRenderer.act(async () => {
      alert.mock.calls[0][2]
        ?.find(action => action.text === 'Browse directly')
        ?.onPress?.();
      await Promise.resolve();
      await Promise.resolve();
    });
    const browser = renderer.root.findByProps({ testID: 'browser-screen' });
    expect(browser.props.mode).toBe('direct');

    await ReactTestRenderer.act(async () => {
      emitAppState('background');
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(mockCloseBrowserSession).toHaveBeenCalledTimes(1);
    expect(
      renderer.root.findAllByProps({ testID: 'browser-screen' }),
    ).toHaveLength(0);
    const content = JSON.stringify(renderer.toJSON());
    expect(content).toContain('direct browser was closed');
    expect(content).not.toContain('private route was interrupted');
    expect(content).not.toContain('Retry connection');
    ReactTestRenderer.act(() => renderer.unmount());
  });

  it('keeps the browser route closed to navigation until blocked is acknowledged', async () => {
    mockController.status = {
      ...EMPTY_STATUS,
      chain: 'base-mainnet',
      connectedNeighbors: 1,
      engineAvailable: true,
      phase: 'connected',
      proxyEnabled: true,
      proxyPort: 44_443,
      routeHops: 2,
      walletAddress: '0x1234567890abcdef',
    };
    const renderer = await renderApp();
    await ReactTestRenderer.act(async () => {
      findButton(renderer, 'Open private browser').props.onPress();
      await Promise.resolve();
      await Promise.resolve();
    });
    const browser = renderer.root.findByProps({ testID: 'browser-screen' });
    const refreshCallsBeforeClose = mockController.refresh.mock.calls.length;
    mockCloseBrowserSession.mockRejectedValueOnce(
      new Error('Browser traffic could not be confirmed blocked.'),
    );

    await expect(browser.props.onClose()).rejects.toThrow(
      'Browser traffic could not be confirmed blocked.',
    );
    expect(
      renderer.root.findAllByProps({ testID: 'browser-screen' }),
    ).not.toHaveLength(0);
    expect(mockController.refresh).toHaveBeenCalledTimes(
      refreshCallsBeforeClose,
    );

    await ReactTestRenderer.act(async () => {
      await browser.props.onClose();
    });
    expect(
      renderer.root.findAllByProps({ testID: 'browser-screen' }),
    ).toHaveLength(0);
    ReactTestRenderer.act(() => renderer.unmount());
  });

  it('invalidates and fully settles a pending route repair before leaving the browser', async () => {
    mockController.status = {
      ...EMPTY_STATUS,
      chain: 'base-mainnet',
      connectedNeighbors: 1,
      engineAvailable: true,
      phase: 'connected',
      proxyEnabled: true,
      proxyPort: 44_443,
      routeHops: 2,
      routeStage: 2,
      walletAddress: '0x1234567890abcdef',
    };
    let resolveRepair!: () => void;
    let repairIsCurrent!: () => boolean;
    mockRepairPrivateBrowserRoute.mockImplementation(
      (_core, _reconnect, isCurrent: () => boolean) => {
        repairIsCurrent = isCurrent;
        return new Promise<void>(resolve => {
          resolveRepair = resolve;
        });
      },
    );
    const renderer = await renderApp();
    await ReactTestRenderer.act(async () => {
      findButton(renderer, 'Open private browser').props.onPress();
      await Promise.resolve();
      await Promise.resolve();
    });
    const browser = renderer.root.findByProps({ testID: 'browser-screen' });
    const repair = browser.props.onRepairRoute();
    await Promise.resolve();
    expect(repairIsCurrent()).toBe(true);

    let close!: Promise<void>;
    ReactTestRenderer.act(() => {
      close = browser.props.onClose();
    });
    await ReactTestRenderer.act(async () => {
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(repairIsCurrent()).toBe(false);
    expect(mockCloseBrowserSession).toHaveBeenCalledTimes(1);
    expect(
      renderer.root.findAllByProps({ testID: 'browser-screen' }),
    ).not.toHaveLength(0);

    await ReactTestRenderer.act(async () => {
      resolveRepair();
      await repair;
      await close;
    });

    expect(mockCloseBrowserSession).toHaveBeenCalledTimes(2);
    expect(
      renderer.root.findAllByProps({ testID: 'browser-screen' }),
    ).toHaveLength(0);
    ReactTestRenderer.act(() => renderer.unmount());
  });

  it('uses Android back for inner screens and preserves Home app exit', async () => {
    (Platform as unknown as { OS: string }).OS = 'android';
    mockController.status = {
      ...EMPTY_STATUS,
      chain: 'base-mainnet',
      engineAvailable: true,
      phase: 'ready',
      walletAddress: '0x1234567890abcdef',
    };
    mockController.systemTunnel = {
      ...UNSUPPORTED_SYSTEM_TUNNEL,
      active: true,
      mode: 'wholeDevice',
      phase: 'active',
      supported: true,
      trafficDisposition: 'masq',
    };
    let hardwareBack:
      | ((event: typeof HARDWARE_BACK_EVENT) => boolean | null | undefined)
      | undefined;
    const remove = jest.fn();
    jest
      .spyOn(BackHandler, 'addEventListener')
      .mockImplementation((_event, listener) => {
        hardwareBack = listener;
        return { remove };
      });
    const renderer = await renderApp();

    expect(hardwareBack?.(HARDWARE_BACK_EVENT)).toBe(false);

    ReactTestRenderer.act(() =>
      findButton(renderer, 'Node & wallet settings').props.onPress(),
    );
    expect(JSON.stringify(renderer.toJSON())).toContain('Set up MASQ');
    ReactTestRenderer.act(() => {
      expect(hardwareBack?.(HARDWARE_BACK_EVENT)).toBe(true);
    });
    expect(JSON.stringify(renderer.toJSON())).toContain(
      'Node & wallet settings',
    );

    ReactTestRenderer.act(() =>
      findButton(renderer, 'How MASQ handles data').props.onPress(),
    );
    expect(JSON.stringify(renderer.toJSON())).toContain('Privacy & legal');
    ReactTestRenderer.act(() => {
      expect(hardwareBack?.(HARDWARE_BACK_EVENT)).toBe(true);
    });

    ReactTestRenderer.act(() =>
      findButton(renderer, 'Whole-device HTTPS route ready').props.onPress(),
    );
    expect(JSON.stringify(renderer.toJSON())).toContain('Traffic routing');
    ReactTestRenderer.act(() => {
      expect(hardwareBack?.(HARDWARE_BACK_EVENT)).toBe(true);
    });
    expect(hardwareBack?.(HARDWARE_BACK_EVENT)).toBe(false);

    ReactTestRenderer.act(() => renderer.unmount());
    expect(remove).toHaveBeenCalled();
  });

  it('does not capture defaults before the complete saved profile is ready', async () => {
    const savedDraft = {
      ...DEFAULT_SETUP,
      rpcUrl: 'https://saved-profile.example',
      neighbors: [
        'masq://base-mainnet:key-one@198.51.100.10:443',
        'masq://base-mainnet:key-two@198.51.100.11:443',
      ],
      minHops: 5,
    };
    mockController.busy = true;
    mockController.profileReady = false;
    mockController.initializationState = 'loading';
    mockController.draft = DEFAULT_SETUP;
    const renderer = await renderApp();

    const settings = findButton(renderer, 'Node & wallet settings');
    expect(settings.props.disabled).toBe(true);
    expect(settings.props.accessibilityState).toEqual({ disabled: true });
    expect(
      findButton(renderer, 'Browse without MASQ').props.disabled,
    ).toBe(true);
    expect(
      renderer.root.findByProps({
        accessibilityLabel: 'Loading saved Node and wallet profile',
      }).props.disabled,
    ).toBe(true);

    ReactTestRenderer.act(() => settings.props.onPress());
    expect(JSON.stringify(renderer.toJSON())).not.toContain('Set up MASQ');
    expect(JSON.stringify(renderer.toJSON())).toContain(
      'Node and wallet actions stay locked until the complete saved profile is available',
    );

    mockController.busy = false;
    mockController.profileReady = true;
    mockController.initializationState = 'ready';
    mockController.draft = savedDraft;
    await ReactTestRenderer.act(async () => {
      renderer.update(<App />);
      await Promise.resolve();
    });
    ReactTestRenderer.act(() =>
      findButton(renderer, 'Node & wallet settings').props.onPress(),
    );

    expect(JSON.stringify(renderer.toJSON())).toContain('Set up MASQ');
    ReactTestRenderer.act(() =>
      findButton(renderer, 'Advanced network settings').props.onPress(),
    );
    expect(
      renderer.root
        .findAllByType(TextInput)
        .some(input => input.props.value === savedDraft.rpcUrl),
    ).toBe(true);
    ReactTestRenderer.act(() => renderer.unmount());
  });

  it('offers an explicit retry after profile initialization fails', async () => {
    mockController.profileReady = false;
    mockController.initializationState = 'error';
    const renderer = await renderApp();

    const retry = findButton(renderer, 'Retry profile loading');
    expect(retry.props.disabled).toBeFalsy();
    ReactTestRenderer.act(() => retry.props.onPress());

    expect(mockController.retryInitialization).toHaveBeenCalledTimes(1);
    expect(
      findButton(renderer, 'Node & wallet settings').props.disabled,
    ).toBe(true);
    ReactTestRenderer.act(() => renderer.unmount());
  });

  it('confirms wallet-preserving profile recovery while Direct Browse remains blocked', async () => {
    mockController.profileReady = false;
    mockController.initializationState = 'error';
    mockController.profileRecoveryAvailable = true;
    const alert = jest.spyOn(Alert, 'alert').mockImplementation(jest.fn());
    const renderer = await renderApp();

    expect(
      findButton(renderer, 'Browse without MASQ').props.disabled,
    ).toBe(true);
    ReactTestRenderer.act(() =>
      findButton(
        renderer,
        'Reset network profile · keep wallet',
      ).props.onPress(),
    );

    expect(alert).toHaveBeenCalledTimes(1);
    expect(alert.mock.calls[0][0]).toBe('Reset invalid network profile?');
    expect(alert.mock.calls[0][1]).toContain(
      'consumer wallet remains on this device',
    );
    const reset = alert.mock.calls[0][2]?.find(
      action => action.text === 'Reset network profile',
    );
    await ReactTestRenderer.act(async () => {
      reset?.onPress?.();
      await Promise.resolve();
    });
    expect(mockController.recoverNetworkProfile).toHaveBeenCalledTimes(1);
    ReactTestRenderer.act(() => renderer.unmount());
  });
});

async function renderApp() {
  let renderer!: ReactTestRenderer.ReactTestRenderer;
  await ReactTestRenderer.act(async () => {
    renderer = ReactTestRenderer.create(<App />);
    await Promise.resolve();
  });
  return renderer;
}

function findButton(
  renderer: ReactTestRenderer.ReactTestRenderer,
  label: string,
) {
  const matches = renderer.root.findAll(
    node =>
      node.props.accessibilityRole === 'button' &&
      node.findAllByType(Text).some(text => text.props.children === label),
  );
  if (matches.length === 0) {
    throw new Error(JSON.stringify(renderer.toJSON()));
  }
  return matches[0];
}

function mockAppStateChanges() {
  const listeners = new Set<(state: AppStateStatus) => void>();
  jest
    .spyOn(AppState, 'addEventListener')
    .mockImplementation((_event, listener) => {
      listeners.add(listener);
      return { remove: () => listeners.delete(listener) };
    });
  return (state: AppStateStatus) => {
    [...listeners].forEach(listener => listener(state));
  };
}
