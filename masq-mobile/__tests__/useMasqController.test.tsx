import React from 'react';
import { AppState, type AppStateStatus } from 'react-native';
import ReactTestRenderer from 'react-test-renderer';

import { masqCore, SavedProfileError } from '../src/core/masqCore';
import {
  UNSUPPORTED_SYSTEM_TUNNEL,
  type SystemTunnelStatus,
} from '../src/core/systemTunnel';
import {
  DEFAULT_SETUP,
  EMPTY_STATUS,
  type CoreStatus,
  type MasqConfig,
  type NetworkStatus,
} from '../src/core/types';
import {
  automaticReconnectDelayMs,
  isNativeMasqRecoveryInProgress,
  NATIVE_RECOVERY_OBSERVATION_MS,
  PROFILE_NOT_READY_MESSAGE,
  PROFILE_RECOVERY_NOT_AVAILABLE_MESSAGE,
  shouldAutomaticallyRetryMasqIssue,
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

const REFRESHED_PROFILE: MasqConfig = {
  ...SAVED_PROFILE,
  neighbors: [
    'masq://base-mainnet:key-three@198.51.100.12:443',
    'masq://base-mainnet:key-four@198.51.100.13:443',
    'masq://base-mainnet:key-five@198.51.100.14:443',
  ],
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
const SYSTEM_TUNNEL_OFF: SystemTunnelStatus = {
  active: false,
  appliedMode: 'off',
  appliedRevision: null,
  appliedSelectedApps: [],
  lastError: null,
  mode: 'off',
  phase: 'off',
  selectedApps: [],
  supported: true,
  trafficDisposition: 'off',
};
const SYSTEM_TUNNEL_ACTIVE: SystemTunnelStatus = {
  active: true,
  appliedMode: 'wholeDevice',
  appliedRevision: 81,
  appliedSelectedApps: [],
  lastError: null,
  mode: 'wholeDevice',
  phase: 'active',
  selectedApps: [],
  supported: true,
  trafficDisposition: 'masq',
};

describe('useMasqController profile readiness', () => {
  let current!: ReturnType<typeof useMasqController>;

  beforeEach(() => {
    jest.spyOn(masqCore, 'getNetworkStatus').mockResolvedValue({ ...NETWORK });
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
    const resetNetworkProfile = jest.spyOn(masqCore, 'resetNetworkProfile');
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
    await expect(current.connect()).rejects.toThrow(PROFILE_NOT_READY_MESSAGE);
    await expect(current.updateMinHops(4)).rejects.toThrow(
      PROFILE_NOT_READY_MESSAGE,
    );
    await expect(current.reset()).rejects.toThrow(PROFILE_NOT_READY_MESSAGE);
    await expect(current.resetNetworkProfile()).rejects.toThrow(
      PROFILE_NOT_READY_MESSAGE,
    );
    await expect(current.removeWallet()).rejects.toThrow(
      PROFILE_NOT_READY_MESSAGE,
    );
    await expect(current.updateSystemTunnel('wholeDevice', [])).rejects.toThrow(
      PROFILE_NOT_READY_MESSAGE,
    );
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
      .mockRejectedValueOnce(new SavedProfileError())
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
    jest.spyOn(masqCore, 'getSavedConfiguration').mockResolvedValue(null);
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
    jest.spyOn(masqCore, 'getSavedConfiguration').mockResolvedValue(null);
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
    const resetNetworkProfile = jest.spyOn(masqCore, 'resetNetworkProfile');
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
    const resetNetworkProfile = jest.spyOn(masqCore, 'resetNetworkProfile');
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
    const resetNetworkProfile = jest.spyOn(masqCore, 'resetNetworkProfile');
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
  ])(
    'fails closed when saved and native %s differ',
    async (_label, override) => {
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
    },
  );

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
    jest.spyOn(masqCore, 'getNetworkStatus').mockResolvedValue({ ...NETWORK });
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

  it('ignores a deferred tunnel poll after a newer OFF operation commits', async () => {
    jest.useFakeTimers();
    jest
      .spyOn(masqCore, 'getSavedConfiguration')
      .mockResolvedValue(SAVED_PROFILE);
    jest.spyOn(masqCore, 'getStatus').mockResolvedValue(CONFIGURED_STATUS);
    const staleTunnelPoll = deferred<SystemTunnelStatus>();
    const getSystemTunnelStatus = jest
      .spyOn(masqCore, 'getSystemTunnelStatus')
      .mockResolvedValueOnce(SYSTEM_TUNNEL_ACTIVE)
      .mockReturnValueOnce(staleTunnelPoll.promise)
      .mockResolvedValue(SYSTEM_TUNNEL_OFF);
    jest
      .spyOn(masqCore, 'setSystemTunnel')
      .mockResolvedValue(SYSTEM_TUNNEL_OFF);
    const renderer = await renderController(value => {
      current = value;
    });

    expect(current.profileReady).toBe(true);
    expect(current.systemTunnel).toEqual(SYSTEM_TUNNEL_ACTIVE);
    await ReactTestRenderer.act(async () => {
      jest.advanceTimersByTime(2_000);
      await Promise.resolve();
    });
    expect(getSystemTunnelStatus).toHaveBeenCalledTimes(2);

    await ReactTestRenderer.act(async () => {
      await current.updateSystemTunnel('off', []);
    });
    expect(current.systemTunnel).toEqual(SYSTEM_TUNNEL_OFF);

    await ReactTestRenderer.act(async () => {
      staleTunnelPoll.resolve(SYSTEM_TUNNEL_ACTIVE);
      await Promise.resolve();
      await Promise.resolve();
    });
    expect(current.systemTunnel).toEqual(SYSTEM_TUNNEL_OFF);
    ReactTestRenderer.act(() => renderer.unmount());
  });

  it('ignores a deferred initial tunnel snapshot after a newer operation commits', async () => {
    jest
      .spyOn(masqCore, 'getSavedConfiguration')
      .mockResolvedValue(SAVED_PROFILE);
    jest.spyOn(masqCore, 'getStatus').mockResolvedValue(CONFIGURED_STATUS);
    const staleInitialTunnel = deferred<SystemTunnelStatus>();
    jest
      .spyOn(masqCore, 'getSystemTunnelStatus')
      .mockReturnValueOnce(staleInitialTunnel.promise)
      .mockResolvedValue(SYSTEM_TUNNEL_OFF);
    jest
      .spyOn(masqCore, 'setSystemTunnel')
      .mockResolvedValue(SYSTEM_TUNNEL_OFF);
    const renderer = await renderController(value => {
      current = value;
    });

    expect(current.profileReady).toBe(true);
    await ReactTestRenderer.act(async () => {
      await current.updateSystemTunnel('off', []);
    });
    expect(current.systemTunnel).toEqual(SYSTEM_TUNNEL_OFF);

    await ReactTestRenderer.act(async () => {
      staleInitialTunnel.resolve(SYSTEM_TUNNEL_ACTIVE);
      await Promise.resolve();
      await Promise.resolve();
    });
    expect(current.systemTunnel).toEqual(SYSTEM_TUNNEL_OFF);
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
      // Let the resume path pass its idle barrier and begin the stale reads
      // before a newer initialization invalidates them.
      await Promise.resolve();
      await Promise.resolve();
    });
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

  it('routes a full reset directly to the native recovery path without requiring a refused tunnel stop', async () => {
    jest
      .spyOn(masqCore, 'getStatus')
      .mockResolvedValue({ ...CONFIGURED_STATUS });
    jest
      .spyOn(masqCore, 'getSavedConfiguration')
      .mockResolvedValue(SAVED_PROFILE);
    jest.spyOn(masqCore, 'getSystemTunnelStatus').mockResolvedValue({
      active: false,
      appliedMode: 'off',
      appliedRevision: null,
      appliedSelectedApps: [],
      lastError: 'unsupported_policy_schema',
      mode: 'off',
      phase: 'blocked',
      selectedApps: [],
      supported: true,
    });
    const setSystemTunnel = jest.spyOn(masqCore, 'setSystemTunnel');
    const reset = jest
      .spyOn(masqCore, 'reset')
      .mockResolvedValue({ ...EMPTY_STATUS });
    const renderer = await renderController(value => {
      current = value;
    });

    await ReactTestRenderer.act(async () => {
      await current.reset();
    });

    expect(reset).toHaveBeenCalledTimes(1);
    expect(setSystemTunnel).not.toHaveBeenCalled();
    expect(current.draft).toEqual(DEFAULT_SETUP);
    ReactTestRenderer.act(() => renderer.unmount());
  });
});

describe('useMasqController entry-node connection lifecycle', () => {
  let current!: ReturnType<typeof useMasqController>;

  beforeEach(() => {
    jest
      .spyOn(masqCore, 'getSavedConfiguration')
      .mockResolvedValue(SAVED_PROFILE);
    jest
      .spyOn(masqCore, 'getStatus')
      .mockResolvedValue({ ...CONFIGURED_STATUS });
    jest.spyOn(masqCore, 'getNetworkStatus').mockResolvedValue({ ...NETWORK });
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

  it('bounds automatic reconnect backoff', () => {
    expect([1, 2, 3, 4, 99].map(automaticReconnectDelayMs)).toEqual([
      2_000, 5_000, 15_000, 30_000, 30_000,
    ]);
  });

  it('automatically retries only issues with an explicit retry action', () => {
    const issue = (
      action: 'retry' | 'wallet' | 'network-profile' | 'none',
    ) => ({
      action,
      category: 'unknown' as const,
      code: null,
      message: 'Safe test diagnostic.',
    });

    expect(shouldAutomaticallyRetryMasqIssue(issue('retry'))).toBe(true);
    expect(shouldAutomaticallyRetryMasqIssue(issue('wallet'))).toBe(false);
    expect(shouldAutomaticallyRetryMasqIssue(issue('network-profile'))).toBe(
      false,
    );
    expect(shouldAutomaticallyRetryMasqIssue(issue('none'))).toBe(false);
    expect(shouldAutomaticallyRetryMasqIssue(null)).toBe(false);
  });

  it('recognizes native route progress without hiding a terminal stage-one error', () => {
    const stageOne: CoreStatus = {
      ...CONFIGURED_STATUS,
      connectedNeighbors: 1,
      engineGeneration: 70,
      phase: 'connected',
      proxyPort: 44_443,
      routeHops: 1,
      routeStage: 1,
    };

    expect(isNativeMasqRecoveryInProgress(stageOne)).toBe(true);
    expect(
      isNativeMasqRecoveryInProgress({
        ...stageOne,
        lastError: 'E_ENTRY_NO_INBOUND_BYTES: no peer reply',
      }),
    ).toBe(false);
    expect(
      isNativeMasqRecoveryInProgress({
        ...stageOne,
        engineGeneration: 0,
      }),
    ).toBe(false);
  });

  it('observes native stage-one recovery after wifi returns before starting another engine', async () => {
    jest.useFakeTimers();
    Object.assign(AppState, { currentState: 'active' });
    const offline: NetworkStatus = {
      ...NETWORK,
      available: false,
      generation: 40,
    };
    const restored: NetworkStatus = { ...NETWORK, generation: 41 };
    const nativeProgress: CoreStatus = {
      ...CONFIGURED_STATUS,
      connectedNeighbors: 1,
      engineGeneration: 90,
      phase: 'connected',
      proxyPort: 44_443,
      routeHops: 1,
      routeStage: 1,
    };
    const terminalStageOne: CoreStatus = {
      ...nativeProgress,
      lastError: 'E_ENTRY_NO_INBOUND_BYTES: no peer reply',
    };
    const recovered: CoreStatus = {
      ...nativeProgress,
      routeHops: 3,
      routeStage: 2,
    };
    let observedNetwork = offline;
    let observedStatus = nativeProgress;
    jest
      .mocked(masqCore.getNetworkStatus)
      .mockImplementation(() => Promise.resolve(observedNetwork));
    jest
      .mocked(masqCore.getStatus)
      .mockImplementation(() => Promise.resolve(observedStatus));
    const start = jest.spyOn(masqCore, 'start').mockResolvedValue(recovered);
    const renderer = await renderController(value => {
      current = value;
    });

    observedNetwork = restored;
    await ReactTestRenderer.act(async () => {
      jest.advanceTimersByTime(2_000);
      await Promise.resolve();
      await Promise.resolve();
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(start).not.toHaveBeenCalled();
    expect(current.status).toEqual(nativeProgress);

    observedStatus = terminalStageOne;
    await ReactTestRenderer.act(async () => {
      jest.advanceTimersByTime(2_000);
      await Promise.resolve();
      await Promise.resolve();
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(start).toHaveBeenCalledTimes(1);
    expect(current.status).toEqual(recovered);
    ReactTestRenderer.act(() => renderer.unmount());
  });

  it('reserves a new wifi generation while native status still briefly reports ready', async () => {
    jest.useFakeTimers();
    Object.assign(AppState, { currentState: 'active' });
    let appStateListener!: (state: AppStateStatus) => void;
    jest
      .spyOn(AppState, 'addEventListener')
      .mockImplementation((_type, listener) => {
        appStateListener = listener;
        return { remove: jest.fn() };
      });
    const offline: NetworkStatus = {
      ...NETWORK,
      available: false,
      generation: 50,
    };
    const restored: NetworkStatus = { ...NETWORK, generation: 51 };
    const priorRoute: CoreStatus = {
      ...CONFIGURED_STATUS,
      connectedNeighbors: 1,
      engineGeneration: 92,
      phase: 'connected',
      proxyPort: 44_443,
      routeHops: 1,
      routeStage: 1,
    };
    const nativeProgress: CoreStatus = {
      ...priorRoute,
      engineGeneration: 93,
      phase: 'connecting',
    };
    let observedNetwork = offline;
    let observedStatus = priorRoute;
    jest
      .mocked(masqCore.getNetworkStatus)
      .mockImplementation(() => Promise.resolve(observedNetwork));
    jest
      .mocked(masqCore.getStatus)
      .mockImplementation(() => Promise.resolve(observedStatus));
    const start = jest.spyOn(masqCore, 'start');
    const renderer = await renderController(value => {
      current = value;
    });

    observedNetwork = restored;
    observedStatus = CONFIGURED_STATUS;
    await ReactTestRenderer.act(async () => {
      jest.advanceTimersByTime(2_000);
      await Promise.resolve();
      await Promise.resolve();
      await Promise.resolve();
      await Promise.resolve();
    });
    expect(start).not.toHaveBeenCalled();

    expect(47).toBeLessThan(NATIVE_RECOVERY_OBSERVATION_MS);
    await ReactTestRenderer.act(async () => {
      jest.advanceTimersByTime(47);
      appStateListener('active');
      await Promise.resolve();
      await Promise.resolve();
      await Promise.resolve();
      await Promise.resolve();
    });
    expect(start).not.toHaveBeenCalled();

    observedStatus = nativeProgress;
    await ReactTestRenderer.act(async () => {
      jest.advanceTimersByTime(2_000);
      await Promise.resolve();
      await Promise.resolve();
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(start).not.toHaveBeenCalled();
    expect(current.status).toEqual(nativeProgress);
    ReactTestRenderer.act(() => renderer.unmount());
  });

  it('does not queue foreground recovery behind the wifi recovery flight', async () => {
    jest.useFakeTimers();
    Object.assign(AppState, { currentState: 'active' });
    let appStateListener!: (state: AppStateStatus) => void;
    jest
      .spyOn(AppState, 'addEventListener')
      .mockImplementation((_type, listener) => {
        appStateListener = listener;
        return { remove: jest.fn() };
      });
    const awaitingRoute: CoreStatus = {
      ...CONFIGURED_STATUS,
      connectedNeighbors: 1,
      engineGeneration: 91,
      phase: 'connected',
      proxyPort: 44_443,
      routeHops: 1,
      routeStage: 1,
    };
    const terminalStageOne: CoreStatus = {
      ...awaitingRoute,
      lastError: 'E_ENTRY_NO_INBOUND_BYTES: no peer reply',
    };
    const recovered: CoreStatus = {
      ...awaitingRoute,
      routeHops: 3,
      routeStage: 2,
    };
    jest
      .mocked(masqCore.getStatus)
      .mockResolvedValueOnce(awaitingRoute)
      .mockResolvedValue(terminalStageOne);
    const pendingStart = deferred<CoreStatus>();
    const start = jest
      .spyOn(masqCore, 'start')
      .mockReturnValueOnce(pendingStart.promise)
      .mockResolvedValueOnce(recovered);
    const renderer = await renderController(value => {
      current = value;
    });

    await ReactTestRenderer.act(async () => {
      jest.advanceTimersByTime(2_000);
      await Promise.resolve();
      await Promise.resolve();
      await Promise.resolve();
    });
    expect(start).toHaveBeenCalledTimes(1);

    ReactTestRenderer.act(() => appStateListener('active'));
    await ReactTestRenderer.act(async () => {
      pendingStart.reject(
        Object.assign(new Error('Transient native startup failure.'), {
          code: 'E_CORE_STARTUP_FAILED',
        }),
      );
      await Promise.resolve();
      await Promise.resolve();
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(start).toHaveBeenCalledTimes(1);

    await ReactTestRenderer.act(async () => {
      jest.advanceTimersByTime(1_999);
      await Promise.resolve();
      await Promise.resolve();
    });
    expect(start).toHaveBeenCalledTimes(1);

    await ReactTestRenderer.act(async () => {
      jest.advanceTimersByTime(1);
      await Promise.resolve();
      await Promise.resolve();
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(start).toHaveBeenCalledTimes(2);
    ReactTestRenderer.act(() => renderer.unmount());
  });

  it('preserves recovery intent after a transient automatic reconnect failure', async () => {
    jest.useFakeTimers();
    Object.assign(AppState, { currentState: 'active' });
    const awaitingRoute: CoreStatus = {
      ...CONFIGURED_STATUS,
      connectedNeighbors: 1,
      engineGeneration: 71,
      phase: 'connected',
      proxyPort: 44_443,
      routeHops: 1,
      routeStage: 1,
    };
    const recovered: CoreStatus = {
      ...awaitingRoute,
      routeHops: 3,
      routeStage: 2,
    };
    jest
      .spyOn(masqCore, 'getStatus')
      .mockResolvedValueOnce(awaitingRoute)
      .mockResolvedValue({
        ...awaitingRoute,
        lastError: 'E_ENTRY_NO_INBOUND_BYTES: no peer reply',
      });
    const start = jest
      .spyOn(masqCore, 'start')
      .mockRejectedValueOnce(
        Object.assign(new Error('Transient native startup failure.'), {
          code: 'E_CORE_STARTUP_FAILED',
        }),
      )
      .mockResolvedValueOnce(recovered);
    const shutdown = jest
      .spyOn(masqCore, 'shutdown')
      .mockResolvedValue(CONFIGURED_STATUS);
    const renderer = await renderController(value => {
      current = value;
    });

    await ReactTestRenderer.act(async () => {
      jest.advanceTimersByTime(2_000);
      await Promise.resolve();
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(start).toHaveBeenCalledTimes(1);
    expect(shutdown).not.toHaveBeenCalled();

    await ReactTestRenderer.act(async () => {
      jest.advanceTimersByTime(2_000);
      await Promise.resolve();
      await Promise.resolve();
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(start).toHaveBeenCalledTimes(2);
    expect(shutdown).not.toHaveBeenCalled();
    expect(current.status).toEqual(recovered);
    ReactTestRenderer.act(() => renderer.unmount());
  });

  it('stops background retries when the wallet requires user attention', async () => {
    jest.useFakeTimers();
    Object.assign(AppState, { currentState: 'active' });
    const awaitingRoute: CoreStatus = {
      ...CONFIGURED_STATUS,
      connectedNeighbors: 1,
      engineGeneration: 72,
      phase: 'connected',
      proxyPort: 44_443,
      routeHops: 1,
      routeStage: 1,
    };
    jest
      .spyOn(masqCore, 'getStatus')
      .mockResolvedValueOnce(awaitingRoute)
      .mockResolvedValue({
        ...awaitingRoute,
        lastError: 'E_ENTRY_NO_INBOUND_BYTES: no peer reply',
      });
    const start = jest
      .spyOn(masqCore, 'start')
      .mockRejectedValue(
        new Error('The consumer wallet keystore requires attention.'),
      );
    const shutdown = jest
      .spyOn(masqCore, 'shutdown')
      .mockResolvedValue(CONFIGURED_STATUS);
    const renderer = await renderController(value => {
      current = value;
    });

    await ReactTestRenderer.act(async () => {
      jest.advanceTimersByTime(2_000);
      await Promise.resolve();
      await Promise.resolve();
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(start).toHaveBeenCalledTimes(1);
    expect(shutdown).toHaveBeenCalledTimes(1);
    expect(current.issue).toMatchObject({
      action: 'wallet',
      category: 'wallet',
    });

    await ReactTestRenderer.act(async () => {
      jest.advanceTimersByTime(60_000);
      await Promise.resolve();
      await Promise.resolve();
    });
    expect(start).toHaveBeenCalledTimes(1);
    ReactTestRenderer.act(() => renderer.unmount());
  });

  it('coalesces a manual waiter without cancelling automatic recovery intent', async () => {
    jest.useFakeTimers();
    Object.assign(AppState, { currentState: 'active' });
    const awaitingRoute: CoreStatus = {
      ...CONFIGURED_STATUS,
      connectedNeighbors: 1,
      engineGeneration: 73,
      phase: 'connected',
      proxyPort: 44_443,
      routeHops: 1,
      routeStage: 1,
    };
    const recovered: CoreStatus = {
      ...awaitingRoute,
      routeHops: 3,
      routeStage: 2,
    };
    jest
      .spyOn(masqCore, 'getStatus')
      .mockResolvedValueOnce(awaitingRoute)
      .mockResolvedValue({
        ...awaitingRoute,
        lastError: 'E_ENTRY_NO_INBOUND_BYTES: no peer reply',
      });
    const pendingStart = deferred<CoreStatus>();
    const start = jest
      .spyOn(masqCore, 'start')
      .mockReturnValueOnce(pendingStart.promise)
      .mockResolvedValueOnce(recovered);
    const shutdown = jest
      .spyOn(masqCore, 'shutdown')
      .mockResolvedValue(CONFIGURED_STATUS);
    const renderer = await renderController(value => {
      current = value;
    });

    await ReactTestRenderer.act(async () => {
      jest.advanceTimersByTime(2_000);
      await Promise.resolve();
      await Promise.resolve();
    });
    expect(start).toHaveBeenCalledTimes(1);

    let manualConnect!: Promise<CoreStatus>;
    await ReactTestRenderer.act(async () => {
      manualConnect = current.connect();
      await Promise.resolve();
      pendingStart.reject(
        Object.assign(new Error('Transient native startup failure.'), {
          code: 'E_CORE_STARTUP_FAILED',
        }),
      );
      await manualConnect.catch(() => undefined);
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(start).toHaveBeenCalledTimes(1);
    expect(shutdown).not.toHaveBeenCalled();

    await ReactTestRenderer.act(async () => {
      jest.advanceTimersByTime(2_000);
      await Promise.resolve();
      await Promise.resolve();
      await Promise.resolve();
      await Promise.resolve();
    });
    expect(start).toHaveBeenCalledTimes(2);
    expect(shutdown).not.toHaveBeenCalled();
    expect(current.status).toEqual(recovered);
    ReactTestRenderer.act(() => renderer.unmount());
  });

  it('publishes attempt 1 before waiting for a long native start', async () => {
    const pendingStart = deferred<CoreStatus>();
    jest.spyOn(masqCore, 'start').mockReturnValue(pendingStart.promise);
    const renderer = await renderController(value => {
      current = value;
    });
    let connection!: Promise<CoreStatus>;

    await ReactTestRenderer.act(async () => {
      connection = current.connect();
      await Promise.resolve();
    });

    expect(current.status.phase).toBe('connecting');
    expect(current.entryNodeRefresh).toEqual({
      attempt: 1,
      maxAttempts: 3,
      stage: 'discovery',
    });
    expect(current.connectionProgress).toEqual({
      step: 2,
      total: 5,
      label: 'Finding reachable entry nodes (1/3)',
    });

    await ReactTestRenderer.act(async () => {
      pendingStart.resolve({
        ...CONFIGURED_STATUS,
        connectedNeighbors: 1,
        engineGeneration: 74,
        phase: 'connected',
        proxyPort: 44_443,
        routeHops: 3,
        routeStage: 2,
      });
      await connection;
    });

    expect(current.entryNodeRefresh).toBeNull();
    expect(current.status.phase).toBe('connected');
    ReactTestRenderer.act(() => renderer.unmount());
  });

  it('restores the connecting phase when automatic refresh starts another attempt', async () => {
    jest.useFakeTimers();
    Object.assign(AppState, { currentState: 'background' });
    const pendingSecondStart = deferred<CoreStatus>();
    jest
      .spyOn(masqCore, 'start')
      .mockRejectedValueOnce(
        Object.assign(new Error('Safe TCP diagnostic.'), {
          code: 'E_ENTRY_TCP_FAILED',
        }),
      )
      .mockReturnValueOnce(pendingSecondStart.promise);
    const renderer = await renderController(value => {
      current = value;
    });
    let connection!: Promise<CoreStatus>;

    await ReactTestRenderer.act(async () => {
      connection = current.connect();
      await Promise.resolve();
      await Promise.resolve();
    });

    await ReactTestRenderer.act(async () => {
      jest.advanceTimersByTime(1_500);
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(current.status.phase).toBe('connecting');
    expect(current.entryNodeRefresh).toEqual({
      attempt: 2,
      maxAttempts: 3,
      stage: 'discovery',
    });

    await ReactTestRenderer.act(async () => {
      pendingSecondStart.resolve({
        ...CONFIGURED_STATUS,
        connectedNeighbors: 1,
        engineGeneration: 75,
        phase: 'connected',
        proxyPort: 44_443,
        routeHops: 3,
        routeStage: 2,
      });
      await connection;
    });
    expect(current.entryNodeRefresh).toBeNull();
    ReactTestRenderer.act(() => renderer.unmount());
  });

  it('retries a foreground network handover without shutting down service recovery', async () => {
    jest.useFakeTimers();
    const recovered: CoreStatus = {
      ...CONFIGURED_STATUS,
      connectedNeighbors: 1,
      engineGeneration: 76,
      phase: 'connected',
      proxyPort: 44_443,
      routeHops: 3,
      routeStage: 2,
    };
    const start = jest
      .spyOn(masqCore, 'start')
      .mockRejectedValueOnce(
        Object.assign(new Error('Safe network handover diagnostic.'), {
          code: 'E_NETWORK_HANDOVER_RETRY',
        }),
      )
      .mockResolvedValueOnce(recovered);
    const shutdown = jest.spyOn(masqCore, 'shutdown');
    const renderer = await renderController(value => {
      current = value;
    });
    let connection!: Promise<CoreStatus>;

    await ReactTestRenderer.act(async () => {
      connection = current.connect();
      await Promise.resolve();
      await Promise.resolve();
    });
    expect(start).toHaveBeenCalledTimes(1);
    expect(shutdown).not.toHaveBeenCalled();

    await ReactTestRenderer.act(async () => {
      jest.advanceTimersByTime(1_500);
      await connection;
    });

    expect(start).toHaveBeenCalledTimes(2);
    expect(shutdown).not.toHaveBeenCalled();
    expect(current.status).toEqual(recovered);
    ReactTestRenderer.act(() => renderer.unmount());
  });

  it('preserves handover recovery while offline and reconnects when the network returns', async () => {
    jest.useFakeTimers();
    Object.assign(AppState, { currentState: 'active' });
    const offline: NetworkStatus = {
      ...NETWORK,
      available: false,
      generation: 2,
    };
    const restored: NetworkStatus = { ...NETWORK, generation: 3 };
    let observedNetwork = offline;
    jest
      .mocked(masqCore.getNetworkStatus)
      .mockImplementation(() => Promise.resolve(observedNetwork));
    const recovered: CoreStatus = {
      ...CONFIGURED_STATUS,
      connectedNeighbors: 1,
      engineGeneration: 77,
      phase: 'connected',
      proxyPort: 44_443,
      routeHops: 3,
      routeStage: 2,
    };
    const handoverError = Object.assign(
      new Error('Safe network handover diagnostic.'),
      { code: 'E_NETWORK_HANDOVER_RETRY' },
    );
    const start = jest
      .spyOn(masqCore, 'start')
      .mockRejectedValueOnce(handoverError)
      .mockRejectedValueOnce(handoverError)
      .mockRejectedValueOnce(handoverError)
      .mockResolvedValueOnce(recovered);
    const shutdown = jest.spyOn(masqCore, 'shutdown');
    const renderer = await renderController(value => {
      current = value;
    });
    let connection!: Promise<unknown>;

    await ReactTestRenderer.act(async () => {
      connection = current.connect().catch(error => error);
      await Promise.resolve();
      await Promise.resolve();
    });
    await ReactTestRenderer.act(async () => {
      jest.advanceTimersByTime(1_500);
      await Promise.resolve();
      await Promise.resolve();
    });
    await ReactTestRenderer.act(async () => {
      jest.advanceTimersByTime(3_000);
      await connection;
      await Promise.resolve();
    });

    expect(start).toHaveBeenCalledTimes(3);
    expect(shutdown).not.toHaveBeenCalled();
    expect(current.issue).toMatchObject({ category: 'offline' });

    observedNetwork = restored;
    await ReactTestRenderer.act(async () => {
      jest.advanceTimersByTime(2_000);
      await Promise.resolve();
      await Promise.resolve();
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(start).toHaveBeenCalledTimes(3);

    await ReactTestRenderer.act(async () => {
      jest.advanceTimersByTime(2_000);
      await Promise.resolve();
      await Promise.resolve();
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(start).toHaveBeenCalledTimes(4);
    expect(shutdown).not.toHaveBeenCalled();
    expect(current.status).toEqual(recovered);
    ReactTestRenderer.act(() => renderer.unmount());
  });

  it('preserves a manual retry intent while offline and resumes on network restoration', async () => {
    jest.useFakeTimers();
    Object.assign(AppState, { currentState: 'active' });
    const offline: NetworkStatus = {
      ...NETWORK,
      available: false,
      generation: 8,
    };
    const restored: NetworkStatus = { ...NETWORK, generation: 9 };
    let observedNetwork = offline;
    jest
      .mocked(masqCore.getNetworkStatus)
      .mockImplementation(() => Promise.resolve(observedNetwork));
    const recovered: CoreStatus = {
      ...CONFIGURED_STATUS,
      connectedNeighbors: 1,
      engineGeneration: 79,
      phase: 'connected',
      proxyPort: 44_443,
      routeHops: 3,
      routeStage: 2,
    };
    const start = jest
      .spyOn(masqCore, 'start')
      .mockRejectedValueOnce(
        Object.assign(new Error('Transient native startup failure.'), {
          code: 'E_CORE_STARTUP_FAILED',
        }),
      )
      .mockResolvedValueOnce(recovered);
    const shutdown = jest.spyOn(masqCore, 'shutdown');
    const renderer = await renderController(value => {
      current = value;
    });

    await ReactTestRenderer.act(async () => {
      await current.connect().catch(() => undefined);
    });

    expect(start).toHaveBeenCalledTimes(1);
    expect(shutdown).not.toHaveBeenCalled();
    expect(current.issue).toMatchObject({ category: 'offline' });

    await ReactTestRenderer.act(async () => {
      jest.advanceTimersByTime(2_000);
      await Promise.resolve();
      await Promise.resolve();
    });
    expect(start).toHaveBeenCalledTimes(1);

    observedNetwork = restored;
    await ReactTestRenderer.act(async () => {
      jest.advanceTimersByTime(2_000);
      await Promise.resolve();
      await Promise.resolve();
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(start).toHaveBeenCalledTimes(1);

    await ReactTestRenderer.act(async () => {
      jest.advanceTimersByTime(2_000);
      await Promise.resolve();
      await Promise.resolve();
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(start).toHaveBeenCalledTimes(2);
    expect(shutdown).not.toHaveBeenCalled();
    expect(current.status).toEqual(recovered);
    ReactTestRenderer.act(() => renderer.unmount());
  });

  it('cancels the preserved retry intent when the user disconnects during backoff', async () => {
    jest.useFakeTimers();
    Object.assign(AppState, { currentState: 'active' });
    const start = jest.spyOn(masqCore, 'start').mockRejectedValueOnce(
      Object.assign(new Error('Transient native startup failure.'), {
        code: 'E_CORE_STARTUP_FAILED',
      }),
    );
    jest.spyOn(masqCore, 'setBrowserRoutingMode').mockResolvedValue('blocked');
    jest.spyOn(masqCore, 'stop').mockResolvedValue(CONFIGURED_STATUS);
    const renderer = await renderController(value => {
      current = value;
    });

    await ReactTestRenderer.act(async () => {
      await current.connect().catch(() => undefined);
      await current.disconnect();
    });

    await ReactTestRenderer.act(async () => {
      jest.advanceTimersByTime(60_000);
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(start).toHaveBeenCalledTimes(1);
    ReactTestRenderer.act(() => renderer.unmount());
  });

  it('synchronizes refreshed native entry nodes after a successful connection', async () => {
    jest
      .mocked(masqCore.getSavedConfiguration)
      .mockResolvedValueOnce(SAVED_PROFILE)
      .mockResolvedValueOnce(REFRESHED_PROFILE);
    const connected: CoreStatus = {
      ...CONFIGURED_STATUS,
      connectedNeighbors: 1,
      engineGeneration: 80,
      phase: 'connected',
      proxyPort: 44_443,
      routeHops: 3,
      routeStage: 2,
    };
    jest.spyOn(masqCore, 'start').mockResolvedValue(connected);
    const renderer = await renderController(value => {
      current = value;
    });

    await ReactTestRenderer.act(async () => {
      await current.connect();
    });

    expect(masqCore.getSavedConfiguration).toHaveBeenCalledTimes(2);
    expect(current.draft).toMatchObject({
      ...REFRESHED_PROFILE,
      neighbors: REFRESHED_PROFILE.neighbors,
      walletSecret: '',
    });
    ReactTestRenderer.act(() => renderer.unmount());
  });

  it('does not block a mounted browser session while the app is backgrounded', async () => {
    let appStateListener!: (state: AppStateStatus) => void;
    jest
      .spyOn(AppState, 'addEventListener')
      .mockImplementation((_type, listener) => {
        appStateListener = listener;
        return { remove: jest.fn() };
      });
    const connectedStatus: CoreStatus = {
      ...CONFIGURED_STATUS,
      connectedNeighbors: 1,
      engineGeneration: 77,
      phase: 'connected',
      proxyPort: 44_443,
      routeHops: 3,
      routeStage: 2,
    };
    jest
      .spyOn(masqCore, 'getSavedConfiguration')
      .mockResolvedValue(SAVED_PROFILE);
    jest.spyOn(masqCore, 'getStatus').mockResolvedValue(connectedStatus);
    const setBrowserRoutingMode = jest.spyOn(masqCore, 'setBrowserRoutingMode');
    const renderer = await renderController(value => {
      current = value;
    }, true);
    setBrowserRoutingMode.mockClear();

    await ReactTestRenderer.act(async () => {
      appStateListener('inactive');
      appStateListener('background');
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(setBrowserRoutingMode).not.toHaveBeenCalled();

    await ReactTestRenderer.act(async () => {
      appStateListener('active');
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(setBrowserRoutingMode).not.toHaveBeenCalled();
    expect(current.status).toEqual(connectedStatus);
    ReactTestRenderer.act(() => renderer.unmount());
  });

  it('keeps an in-flight connection alive across screen lock and backgrounding', async () => {
    let appStateListener!: (state: AppStateStatus) => void;
    jest
      .spyOn(AppState, 'addEventListener')
      .mockImplementation((_type, listener) => {
        appStateListener = listener;
        return { remove: jest.fn() };
      });
    const firstNativeStart = deferred<CoreStatus>();
    const connectedStatus: CoreStatus = {
      ...CONFIGURED_STATUS,
      connectedNeighbors: 1,
      engineGeneration: 78,
      phase: 'connected',
      proxyPort: 44_443,
      routeHops: 3,
      routeStage: 2,
    };
    jest
      .mocked(masqCore.getStatus)
      .mockResolvedValueOnce({ ...CONFIGURED_STATUS })
      .mockResolvedValue(connectedStatus);
    const start = jest
      .spyOn(masqCore, 'start')
      .mockImplementationOnce(() => firstNativeStart.promise);
    const shutdown = jest.spyOn(masqCore, 'shutdown');
    const setBrowserRoutingMode = jest.spyOn(masqCore, 'setBrowserRoutingMode');
    const reset = jest.spyOn(masqCore, 'reset');
    const resetNetworkProfile = jest.spyOn(masqCore, 'resetNetworkProfile');
    const removeWallet = jest.spyOn(masqCore, 'removeWallet');
    const renderer = await renderController(value => {
      current = value;
    });
    let connection!: Promise<CoreStatus>;

    await ReactTestRenderer.act(async () => {
      connection = current.connect();
      await Promise.resolve();
    });
    expect(start).toHaveBeenCalledTimes(1);

    ReactTestRenderer.act(() => appStateListener('background'));
    await ReactTestRenderer.act(async () => {
      await Promise.resolve();
      await Promise.resolve();
    });
    expect(start).toHaveBeenCalledTimes(1);
    expect(setBrowserRoutingMode).toHaveBeenCalledWith('blocked');

    await ReactTestRenderer.act(async () => {
      firstNativeStart.resolve(connectedStatus);
      await connection;
      await Promise.resolve();
      await Promise.resolve();
    });
    expect(start).toHaveBeenCalledTimes(1);
    expect(shutdown).not.toHaveBeenCalled();

    await ReactTestRenderer.act(async () => {
      appStateListener('active');
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(start).toHaveBeenCalledTimes(1);
    expect(current.status).toEqual(connectedStatus);
    expect(current.entryNodeRefresh).toBeNull();
    expect(current.busy).toBe(false);
    expect(current.status.walletAddress).toBe(CONFIGURED_STATUS.walletAddress);
    expect(current.draft).toMatchObject({
      ...SAVED_PROFILE,
      walletSecret: '',
    });
    expect(reset).not.toHaveBeenCalled();
    expect(resetNetworkProfile).not.toHaveBeenCalled();
    expect(removeWallet).not.toHaveBeenCalled();
    ReactTestRenderer.act(() => renderer.unmount());
  });

  it('keeps a retryable failed manual start alive without resetting the wallet or profile', async () => {
    jest.useFakeTimers();
    Object.assign(AppState, { currentState: 'active' });
    const startupError = Object.assign(
      new Error('The embedded MASQ core could not start.'),
      { code: 'E_CORE_STARTUP_FAILED' },
    );
    const recovered: CoreStatus = {
      ...CONFIGURED_STATUS,
      connectedNeighbors: 1,
      engineGeneration: 81,
      phase: 'connected',
      proxyPort: 44_443,
      routeHops: 3,
      routeStage: 2,
    };
    const start = jest
      .spyOn(masqCore, 'start')
      .mockRejectedValueOnce(startupError)
      .mockResolvedValueOnce(recovered);
    const shutdown = jest
      .spyOn(masqCore, 'shutdown')
      .mockResolvedValue({ ...CONFIGURED_STATUS });
    const reset = jest.spyOn(masqCore, 'reset');
    const resetNetworkProfile = jest.spyOn(masqCore, 'resetNetworkProfile');
    const removeWallet = jest.spyOn(masqCore, 'removeWallet');
    const renderer = await renderController(value => {
      current = value;
    });
    let caught: unknown;

    await ReactTestRenderer.act(async () => {
      try {
        await current.connect();
      } catch (error) {
        caught = error;
      }
    });

    expect(caught).toBe(startupError);
    expect(shutdown).not.toHaveBeenCalled();
    expect(reset).not.toHaveBeenCalled();
    expect(resetNetworkProfile).not.toHaveBeenCalled();
    expect(removeWallet).not.toHaveBeenCalled();
    expect(current.entryNodeRefresh).toBeNull();
    expect(current.status.walletAddress).toBe(CONFIGURED_STATUS.walletAddress);
    expect(current.draft).toMatchObject({
      ...SAVED_PROFILE,
      walletSecret: '',
    });
    expect(current.issue).toMatchObject({
      action: 'retry',
      category: 'native-core',
      code: 'E_CORE_STARTUP_FAILED',
    });

    await ReactTestRenderer.act(async () => {
      jest.advanceTimersByTime(2_000);
      await Promise.resolve();
      await Promise.resolve();
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(start).toHaveBeenCalledTimes(2);
    expect(shutdown).not.toHaveBeenCalled();
    expect(current.status).toEqual(recovered);
    ReactTestRenderer.act(() => renderer.unmount());
  });
});

async function renderController(
  onRender: (value: ReturnType<typeof useMasqController>) => void,
  browserSessionActive = false,
) {
  function Harness() {
    onRender(useMasqController(browserSessionActive));
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
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((fulfill, fail) => {
    resolve = fulfill;
    reject = fail;
  });
  return { promise, reject, resolve };
}
