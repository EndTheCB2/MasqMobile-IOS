import React from 'react';
import { AppState, type AppStateStatus } from 'react-native';
import ReactTestRenderer from 'react-test-renderer';

import { masqCore, SavedProfileError } from '../src/core/masqCore';
import { UNSUPPORTED_SYSTEM_TUNNEL } from '../src/core/systemTunnel';
import {
  DEFAULT_SETUP,
  EMPTY_STATUS,
  type CoreStatus,
  type MasqConfig,
  type NetworkStatus,
} from '../src/core/types';
import {
  PROFILE_NOT_READY_MESSAGE,
  PROFILE_RECOVERY_NOT_AVAILABLE_MESSAGE,
  useMasqController,
} from '../src/hooks/useMasqController';

const NETWORK: NetworkStatus = {
  available: true,
  constrained: false,
  expensive: false,
  generation: 1,
  interface: 'wifi',
};
const INITIAL_APP_STATE = AppState.currentState;

const SAVED_PROFILE: MasqConfig = {
  configVersion: 2,
  chain: 'base-mainnet',
  rpcUrl: 'https://saved-profile.example',
  neighbors: [
    'masq://base-mainnet:key-one@198.51.100.10:443',
    'masq://base-mainnet:key-two@198.51.100.11:443',
  ],
  minHops: 5,
  exitCountry: 'BE',
  exitCountryFallback: false,
};

const CONFIGURED_STATUS: CoreStatus = {
  ...EMPTY_STATUS,
  chain: SAVED_PROFILE.chain,
  engineAvailable: true,
  exitCountry: SAVED_PROFILE.exitCountry,
  exitCountryFallback: SAVED_PROFILE.exitCountryFallback,
  minHops: SAVED_PROFILE.minHops,
  phase: 'ready',
  walletAddress: '0x1234567890abcdef',
};

describe('useMasqController profile readiness', () => {
  let current!: ReturnType<typeof useMasqController>;

  beforeEach(() => {
    jest
      .spyOn(masqCore, 'getNetworkStatus')
      .mockResolvedValue({ ...NETWORK });
    jest
      .spyOn(masqCore, 'getSystemTunnelStatus')
      .mockResolvedValue({ ...UNSUPPORTED_SYSTEM_TUNNEL });
    jest.spyOn(masqCore, 'getRoutableApps').mockResolvedValue([]);
  });

  afterEach(() => {
    jest.restoreAllMocks();
    jest.useRealTimers();
    Object.assign(AppState, { currentState: INITIAL_APP_STATE });
  });

  it('rejects profile mutations until status and saved configuration are both loaded', async () => {
    const status = deferred<CoreStatus>();
    const saved = deferred<MasqConfig | null>();
    jest.spyOn(masqCore, 'getStatus').mockReturnValue(status.promise);
    jest
      .spyOn(masqCore, 'getSavedConfiguration')
      .mockReturnValue(saved.promise);
    const configure = jest.spyOn(masqCore, 'configure');
    const importWallet = jest.spyOn(masqCore, 'importWallet');
    const updateMinHops = jest
      .spyOn(masqCore, 'updateMinHops')
      .mockResolvedValue({ ...EMPTY_STATUS, minHops: 4 });
    const reset = jest.spyOn(masqCore, 'reset');
    const resetNetworkProfile = jest.spyOn(
      masqCore,
      'resetNetworkProfile',
    );
    const removeWallet = jest.spyOn(masqCore, 'removeWallet');
    const start = jest.spyOn(masqCore, 'start');
    const setSystemTunnel = jest.spyOn(masqCore, 'setSystemTunnel');
    const renderer = await renderController(value => {
      current = value;
    });

    expect(current.profileReady).toBe(false);
    expect(current.initializationState).toBe('loading');
    expect(current.profileRecoveryAvailable).toBe(false);
    await expect(
      current.saveSetup({ ...DEFAULT_SETUP, neighbors: [] }),
    ).rejects.toThrow(PROFILE_NOT_READY_MESSAGE);
    await expect(current.connect()).rejects.toThrow(
      PROFILE_NOT_READY_MESSAGE,
    );
    await expect(current.updateMinHops(4)).rejects.toThrow(
      PROFILE_NOT_READY_MESSAGE,
    );
    await expect(current.reset()).rejects.toThrow(
      PROFILE_NOT_READY_MESSAGE,
    );
    await expect(current.resetNetworkProfile()).rejects.toThrow(
      PROFILE_NOT_READY_MESSAGE,
    );
    await expect(current.removeWallet()).rejects.toThrow(
      PROFILE_NOT_READY_MESSAGE,
    );
    await expect(
      current.updateSystemTunnel('wholeDevice', []),
    ).rejects.toThrow(PROFILE_NOT_READY_MESSAGE);
    await expect(current.refreshWalletBalance()).rejects.toThrow(
      PROFILE_NOT_READY_MESSAGE,
    );

    expect(configure).not.toHaveBeenCalled();
    expect(importWallet).not.toHaveBeenCalled();
    expect(updateMinHops).not.toHaveBeenCalled();
    expect(reset).not.toHaveBeenCalled();
    expect(resetNetworkProfile).not.toHaveBeenCalled();
    expect(removeWallet).not.toHaveBeenCalled();
    expect(start).not.toHaveBeenCalled();
    expect(setSystemTunnel).not.toHaveBeenCalled();

    await ReactTestRenderer.act(async () => {
      status.resolve({ ...CONFIGURED_STATUS });
      saved.resolve(SAVED_PROFILE);
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(current.profileReady).toBe(true);
    expect(current.initializationState).toBe('ready');
    expect(current.profileRecoveryAvailable).toBe(false);
    expect(current.draft).toMatchObject({
      ...SAVED_PROFILE,
      walletSecret: '',
    });

    await ReactTestRenderer.act(async () => {
      await current.updateMinHops(4);
    });
    expect(updateMinHops).toHaveBeenCalledWith(4);
    ReactTestRenderer.act(() => renderer.unmount());
  });

  it('keeps the prior draft untouched after failure and loads it on explicit retry', async () => {
    jest.spyOn(masqCore, 'getStatus').mockResolvedValue(CONFIGURED_STATUS);
    jest
      .spyOn(masqCore, 'getSavedConfiguration')
      .mockRejectedValueOnce(
        new SavedProfileError(),
      )
      .mockResolvedValueOnce(SAVED_PROFILE);
    const renderer = await renderController(value => {
      current = value;
    });

    expect(current.profileReady).toBe(false);
    expect(current.initializationState).toBe('error');
    expect(current.profileRecoveryAvailable).toBe(true);
    expect(current.draft).toEqual(DEFAULT_SETUP);
    expect(current.issue?.message).toContain(
      'network profile could not be validated',
    );

    await ReactTestRenderer.act(async () => {
      await current.retryInitialization();
    });

    expect(current.profileReady).toBe(true);
    expect(current.initializationState).toBe('ready');
    expect(current.profileRecoveryAvailable).toBe(false);
    expect(current.draft).toMatchObject({
      ...SAVED_PROFILE,
      walletSecret: '',
    });
    ReactTestRenderer.act(() => renderer.unmount());
  });

  it('fails closed when native status is configured without a matching saved profile', async () => {
    jest.spyOn(masqCore, 'getStatus').mockResolvedValue(CONFIGURED_STATUS);
    jest
      .spyOn(masqCore, 'getSavedConfiguration')
      .mockResolvedValue(null);
    const configure = jest.spyOn(masqCore, 'configure');
    const renderer = await renderController(value => {
      current = value;
    });

    expect(current.profileReady).toBe(false);
    expect(current.initializationState).toBe('error');
    expect(current.profileRecoveryAvailable).toBe(true);
    expect(current.draft).toEqual(DEFAULT_SETUP);
    expect(current.issue?.message).toContain(
      'network profile could not be validated',
    );
    await expect(
      current.saveSetup({ ...DEFAULT_SETUP, neighbors: [] }),
    ).rejects.toThrow(PROFILE_NOT_READY_MESSAGE);
    expect(configure).not.toHaveBeenCalled();
    ReactTestRenderer.act(() => renderer.unmount());
  });

  it('replaces a previously loaded draft with fresh defaults when retry finds no saved profile', async () => {
    jest
      .spyOn(masqCore, 'getStatus')
      .mockResolvedValueOnce(CONFIGURED_STATUS)
      .mockResolvedValueOnce({ ...EMPTY_STATUS });
    jest
      .spyOn(masqCore, 'getSavedConfiguration')
      .mockResolvedValueOnce(SAVED_PROFILE)
      .mockResolvedValueOnce(null);
    const renderer = await renderController(value => {
      current = value;
    });

    expect(current.profileReady).toBe(true);
    expect(current.draft).toMatchObject({
      ...SAVED_PROFILE,
      walletSecret: '',
    });

    await ReactTestRenderer.act(async () => {
      await current.retryInitialization();
    });

    expect(current.profileReady).toBe(true);
    expect(current.initializationState).toBe('ready');
    expect(current.draft).toEqual(DEFAULT_SETUP);
    ReactTestRenderer.act(() => renderer.unmount());
  });

  it('restores readiness by resetting only the invalid network profile and preserving the wallet', async () => {
    const resetStatus: CoreStatus = {
      ...EMPTY_STATUS,
      engineAvailable: true,
      walletAddress: CONFIGURED_STATUS.walletAddress,
    };
    jest
      .spyOn(masqCore, 'getSavedConfiguration')
      .mockResolvedValue(null);
    jest
      .spyOn(masqCore, 'getStatus')
      .mockResolvedValueOnce(CONFIGURED_STATUS)
      .mockResolvedValueOnce(resetStatus);
    const resetNetworkProfile = jest
      .spyOn(masqCore, 'resetNetworkProfile')
      .mockResolvedValue(resetStatus);
    const resetEverything = jest.spyOn(masqCore, 'reset');
    const removeWallet = jest.spyOn(masqCore, 'removeWallet');
    const renderer = await renderController(value => {
      current = value;
    });

    expect(current.initializationState).toBe('error');
    expect(current.profileRecoveryAvailable).toBe(true);

    await ReactTestRenderer.act(async () => {
      await current.recoverNetworkProfile();
    });

    expect(resetNetworkProfile).toHaveBeenCalledTimes(1);
    expect(resetEverything).not.toHaveBeenCalled();
    expect(removeWallet).not.toHaveBeenCalled();
    expect(current.initializationState).toBe('ready');
    expect(current.profileReady).toBe(true);
    expect(current.profileRecoveryAvailable).toBe(false);
    expect(current.status.walletAddress).toBe(CONFIGURED_STATUS.walletAddress);
    expect(current.status.chain).toBeNull();
    expect(current.draft).toEqual(DEFAULT_SETUP);
    ReactTestRenderer.act(() => renderer.unmount());
  });

  it('keeps recovery fail-closed when native reset is not confirmed', async () => {
    jest.spyOn(masqCore, 'getSavedConfiguration').mockResolvedValue(null);
    jest.spyOn(masqCore, 'getStatus').mockResolvedValue(CONFIGURED_STATUS);
    jest
      .spyOn(masqCore, 'resetNetworkProfile')
      .mockResolvedValue({ ...CONFIGURED_STATUS, phase: 'error' });
    const renderer = await renderController(value => {
      current = value;
    });

    await expect(
      ReactTestRenderer.act(async () => {
        await current.recoverNetworkProfile();
      }),
    ).rejects.toThrow(/could not confirm/i);

    expect(current.profileReady).toBe(false);
    expect(current.initializationState).toBe('error');
    expect(current.profileRecoveryAvailable).toBe(true);
    ReactTestRenderer.act(() => renderer.unmount());
  });

  it.each<CoreStatus['phase']>(['connecting', 'paused'])(
    'rejects a network-profile reset that returns nonterminal phase %s',
    async phase => {
      jest.spyOn(masqCore, 'getSavedConfiguration').mockResolvedValue(null);
      jest.spyOn(masqCore, 'getStatus').mockResolvedValue(CONFIGURED_STATUS);
      jest.spyOn(masqCore, 'resetNetworkProfile').mockResolvedValue({
        ...EMPTY_STATUS,
        engineAvailable: true,
        phase,
        walletAddress: null,
      });
      const renderer = await renderController(value => {
        current = value;
      });

      await expect(
        ReactTestRenderer.act(async () => {
          await current.recoverNetworkProfile();
        }),
      ).rejects.toThrow(/could not confirm/i);

      expect(current.profileReady).toBe(false);
      expect(current.initializationState).toBe('error');
      expect(current.profileRecoveryAvailable).toBe(true);
      ReactTestRenderer.act(() => renderer.unmount());
    },
  );

  it('does not reset the profile unless system routing is confirmed off', async () => {
    jest.spyOn(masqCore, 'getSavedConfiguration').mockResolvedValue(null);
    jest.spyOn(masqCore, 'getStatus').mockResolvedValue(CONFIGURED_STATUS);
    jest.spyOn(masqCore, 'getSystemTunnelStatus').mockResolvedValue({
      active: true,
      lastError: 'A prior tunnel still owns the device route.',
      mode: 'wholeDevice',
      phase: 'blocked',
      selectedApps: [],
      supported: false,
    });
    const resetNetworkProfile = jest.spyOn(
      masqCore,
      'resetNetworkProfile',
    );
    const renderer = await renderController(value => {
      current = value;
    });

    await expect(
      ReactTestRenderer.act(async () => {
        await current.recoverNetworkProfile();
      }),
    ).rejects.toThrow(/system routing stopped/i);

    expect(resetNetworkProfile).not.toHaveBeenCalled();
    expect(current.profileReady).toBe(false);
    expect(current.initializationState).toBe('error');
    expect(current.profileRecoveryAvailable).toBe(true);
    ReactTestRenderer.act(() => renderer.unmount());
  });

  it('does not expose destructive recovery for unrelated initialization errors', async () => {
    jest
      .spyOn(masqCore, 'getSavedConfiguration')
      .mockRejectedValue(new Error('Temporary native bridge timeout.'));
    const resetNetworkProfile = jest.spyOn(
      masqCore,
      'resetNetworkProfile',
    );
    const renderer = await renderController(value => {
      current = value;
    });

    expect(current.initializationState).toBe('error');
    expect(current.profileRecoveryAvailable).toBe(false);
    await expect(current.recoverNetworkProfile()).rejects.toThrow(
      PROFILE_RECOVERY_NOT_AVAILABLE_MESSAGE,
    );
    expect(resetNetworkProfile).not.toHaveBeenCalled();
    ReactTestRenderer.act(() => renderer.unmount());
  });

  it('keeps an unreadable encrypted wallet blocked without offering profile reset', async () => {
    jest
      .spyOn(masqCore, 'getSavedConfiguration')
      .mockResolvedValue(SAVED_PROFILE);
    jest.spyOn(masqCore, 'getStatus').mockRejectedValue({
      code: 'E_WALLET_STORAGE_UNREADABLE',
      message:
        'The encrypted consumer wallet could not be read safely. Unlock the device and retry without resetting or re-importing the wallet.',
    });
    const resetNetworkProfile = jest.spyOn(
      masqCore,
      'resetNetworkProfile',
    );
    const renderer = await renderController(value => {
      current = value;
    });

    expect(current.profileReady).toBe(false);
    expect(current.initializationState).toBe('error');
    expect(current.profileRecoveryAvailable).toBe(false);
    expect(current.issue?.category).toBe('wallet');
    await expect(current.recoverNetworkProfile()).rejects.toThrow(
      PROFILE_RECOVERY_NOT_AVAILABLE_MESSAGE,
    );
    expect(resetNetworkProfile).not.toHaveBeenCalled();
    ReactTestRenderer.act(() => renderer.unmount());
  });

  it.each(['E_SAVED_CONFIG', 'E_SAVED_CONFIG_INVALID'])(
    'offers recovery for native saved-profile error code %s without inspecting its message',
    async code => {
      jest.spyOn(masqCore, 'getSavedConfiguration').mockRejectedValue({
        code,
        message: 'Native decoder failure.',
      });
      const getStatus = jest.spyOn(masqCore, 'getStatus');
      const renderer = await renderController(value => {
        current = value;
      });

      expect(getStatus).not.toHaveBeenCalled();
      expect(current.profileReady).toBe(false);
      expect(current.initializationState).toBe('error');
      expect(current.profileRecoveryAvailable).toBe(true);
      expect(current.issue?.message).toContain(
        'network profile could not be validated',
      );
      ReactTestRenderer.act(() => renderer.unmount());
    },
  );

  it('waits for saved configuration and suppresses status polls until the profile snapshot is ready', async () => {
    jest.useFakeTimers();
    const saved = deferred<MasqConfig | null>();
    const getSavedConfiguration = jest
      .spyOn(masqCore, 'getSavedConfiguration')
      .mockReturnValue(saved.promise);
    const getStatus = jest
      .spyOn(masqCore, 'getStatus')
      .mockResolvedValue(CONFIGURED_STATUS);
    const getNetworkStatus = jest
      .spyOn(masqCore, 'getNetworkStatus')
      .mockResolvedValue({ ...NETWORK });
    const renderer = await renderController(value => {
      current = value;
    });

    await ReactTestRenderer.act(async () => {
      jest.advanceTimersByTime(5_000);
      await Promise.resolve();
    });
    expect(getSavedConfiguration).toHaveBeenCalledTimes(1);
    expect(getStatus).not.toHaveBeenCalled();
    expect(getNetworkStatus).not.toHaveBeenCalled();
    expect(current.initializationState).toBe('loading');

    await ReactTestRenderer.act(async () => {
      saved.resolve(SAVED_PROFILE);
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(getStatus).toHaveBeenCalledTimes(1);
    expect(getNetworkStatus).toHaveBeenCalledTimes(1);
    expect(current.status).toEqual(CONFIGURED_STATUS);
    expect(current.initializationState).toBe('ready');
    ReactTestRenderer.act(() => renderer.unmount());
  });

  it.each([
    ['chain', { chain: 'base-sepolia' as const }],
    ['minimum hops', { minHops: 4 }],
    ['exit country', { exitCountry: 'NL' }],
    ['exit fallback', { exitCountryFallback: true }],
  ])('fails closed when saved and native %s differ', async (_label, override) => {
    jest
      .spyOn(masqCore, 'getSavedConfiguration')
      .mockResolvedValue(SAVED_PROFILE);
    jest
      .spyOn(masqCore, 'getStatus')
      .mockResolvedValue({ ...CONFIGURED_STATUS, ...override });
    const renderer = await renderController(value => {
      current = value;
    });

    expect(current.profileReady).toBe(false);
    expect(current.initializationState).toBe('error');
    expect(current.profileRecoveryAvailable).toBe(true);
    expect(current.issue?.message).toContain(
      'network profile could not be validated',
    );
    ReactTestRenderer.act(() => renderer.unmount());
  });

  it.each([
    ['unconfigured', { ...EMPTY_STATUS }],
    [
      'error',
      {
        ...EMPTY_STATUS,
        engineAvailable: true,
        phase: 'error' as const,
        lastError: 'Native restore failed.',
      },
    ],
  ])(
    'fails closed when a saved profile exists but native status is %s',
    async (_label, nativeStatus) => {
      jest
        .spyOn(masqCore, 'getSavedConfiguration')
        .mockResolvedValue(SAVED_PROFILE);
      jest.spyOn(masqCore, 'getStatus').mockResolvedValue(nativeStatus);
      const renderer = await renderController(value => {
        current = value;
      });

      expect(current.profileReady).toBe(false);
      expect(current.initializationState).toBe('error');
      expect(current.profileRecoveryAvailable).toBe(true);
      expect(current.issue?.message).toContain(
        'network profile could not be validated',
      );
      ReactTestRenderer.act(() => renderer.unmount());
    },
  );

  it.each([
    ['config version', { configVersion: 1 }],
    ['RPC scheme', { rpcUrl: 'http://saved-profile.example' }],
    ['entry nodes', { neighbors: [] }],
    ['minimum hops', { minHops: 0 }],
    ['exit country', { exitCountry: 'Belgium' }],
  ])('rejects a semantically invalid saved %s', async (_label, override) => {
    const getStatus = jest.spyOn(masqCore, 'getStatus');
    jest
      .spyOn(masqCore, 'getSavedConfiguration')
      .mockResolvedValue({ ...SAVED_PROFILE, ...override });
    const renderer = await renderController(value => {
      current = value;
    });

    expect(getStatus).not.toHaveBeenCalled();
    expect(current.profileReady).toBe(false);
    expect(current.initializationState).toBe('error');
    expect(current.profileRecoveryAvailable).toBe(true);
    ReactTestRenderer.act(() => renderer.unmount());
  });

  it('ignores a deferred status poll after a newer initialization snapshot commits', async () => {
    jest.useFakeTimers();
    Object.assign(AppState, { currentState: 'active' });
    const staleStatus = deferred<CoreStatus>();
    const freshStatus = {
      ...CONFIGURED_STATUS,
      bytesDown: 200,
      bytesUp: 100,
    };
    jest
      .spyOn(masqCore, 'getSavedConfiguration')
      .mockResolvedValue(SAVED_PROFILE);
    const getStatus = jest
      .spyOn(masqCore, 'getStatus')
      .mockResolvedValueOnce({ ...CONFIGURED_STATUS, bytesUp: 10 })
      .mockReturnValueOnce(staleStatus.promise)
      .mockResolvedValueOnce(freshStatus);
    jest
      .spyOn(masqCore, 'getNetworkStatus')
      .mockResolvedValue({ ...NETWORK });
    const renderer = await renderController(value => {
      current = value;
    });

    await ReactTestRenderer.act(async () => {
      jest.advanceTimersByTime(1_000);
      await Promise.resolve();
    });
    expect(getStatus).toHaveBeenCalledTimes(2);

    await ReactTestRenderer.act(async () => {
      await current.retryInitialization();
    });
    expect(current.status).toEqual(freshStatus);

    await ReactTestRenderer.act(async () => {
      staleStatus.resolve({ ...CONFIGURED_STATUS, bytesUp: 20 });
      await Promise.resolve();
      await Promise.resolve();
    });
    expect(current.status).toEqual(freshStatus);
    ReactTestRenderer.act(() => renderer.unmount());
  });

  it('ignores a deferred network poll after a newer initialization snapshot commits', async () => {
    jest.useFakeTimers();
    Object.assign(AppState, { currentState: 'active' });
    const staleNetwork = deferred<NetworkStatus>();
    const freshNetwork: NetworkStatus = {
      ...NETWORK,
      generation: 30,
      interface: 'cellular',
    };
    jest
      .spyOn(masqCore, 'getSavedConfiguration')
      .mockResolvedValue(SAVED_PROFILE);
    jest.spyOn(masqCore, 'getStatus').mockResolvedValue(CONFIGURED_STATUS);
    const getNetworkStatus = jest
      .spyOn(masqCore, 'getNetworkStatus')
      .mockResolvedValueOnce({ ...NETWORK, generation: 10 })
      .mockReturnValueOnce(staleNetwork.promise)
      .mockResolvedValueOnce(freshNetwork);
    const renderer = await renderController(value => {
      current = value;
    });

    await ReactTestRenderer.act(async () => {
      jest.advanceTimersByTime(2_000);
      await Promise.resolve();
    });
    expect(getNetworkStatus).toHaveBeenCalledTimes(2);

    await ReactTestRenderer.act(async () => {
      await current.retryInitialization();
    });
    expect(current.network).toEqual(freshNetwork);

    await ReactTestRenderer.act(async () => {
      staleNetwork.resolve({ ...NETWORK, generation: 20 });
      await Promise.resolve();
      await Promise.resolve();
    });
    expect(current.network).toEqual(freshNetwork);
    ReactTestRenderer.act(() => renderer.unmount());
  });

  it('ignores a deferred AppState refresh after a newer initialization snapshot commits', async () => {
    let appStateListener!: (state: AppStateStatus) => void;
    jest
      .spyOn(AppState, 'addEventListener')
      .mockImplementation((_type, listener) => {
        appStateListener = listener;
        return { remove: jest.fn() };
      });
    const staleStatus = deferred<CoreStatus>();
    const staleNetwork = deferred<NetworkStatus>();
    const freshStatus = {
      ...CONFIGURED_STATUS,
      bytesDown: 400,
      bytesUp: 300,
    };
    const freshNetwork: NetworkStatus = {
      ...NETWORK,
      generation: 40,
      interface: 'cellular',
    };
    jest
      .spyOn(masqCore, 'getSavedConfiguration')
      .mockResolvedValue(SAVED_PROFILE);
    jest
      .spyOn(masqCore, 'getStatus')
      .mockResolvedValueOnce(CONFIGURED_STATUS)
      .mockReturnValueOnce(staleStatus.promise)
      .mockResolvedValueOnce(freshStatus);
    jest
      .spyOn(masqCore, 'getNetworkStatus')
      .mockResolvedValueOnce(NETWORK)
      .mockReturnValueOnce(staleNetwork.promise)
      .mockResolvedValueOnce(freshNetwork);
    const renderer = await renderController(value => {
      current = value;
    });

    ReactTestRenderer.act(() => appStateListener('active'));
    await ReactTestRenderer.act(async () => {
      await current.retryInitialization();
    });
    expect(current.status).toEqual(freshStatus);
    expect(current.network).toEqual(freshNetwork);

    await ReactTestRenderer.act(async () => {
      staleStatus.resolve({ ...CONFIGURED_STATUS, bytesUp: 30 });
      staleNetwork.resolve({ ...NETWORK, generation: 30 });
      await Promise.resolve();
      await Promise.resolve();
    });
    expect(current.status).toEqual(freshStatus);
    expect(current.network).toEqual(freshNetwork);
    ReactTestRenderer.act(() => renderer.unmount());
  });
});

async function renderController(
  onRender: (value: ReturnType<typeof useMasqController>) => void,
) {
  function Harness() {
    onRender(useMasqController());
    return null;
  }

  let renderer!: ReactTestRenderer.ReactTestRenderer;
  await ReactTestRenderer.act(async () => {
    renderer = ReactTestRenderer.create(<Harness />);
    await Promise.resolve();
    await Promise.resolve();
  });
  return renderer;
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>(fulfill => {
    resolve = fulfill;
  });
  return { promise, resolve };
}
