import React from 'react';
import ReactTestRenderer from 'react-test-renderer';

import { masqCore } from '../src/core/masqCore';
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
  useMasqController,
} from '../src/hooks/useMasqController';

const NETWORK: NetworkStatus = {
  available: true,
  constrained: false,
  expensive: false,
  generation: 1,
  interface: 'wifi',
};

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
      status.resolve({ ...EMPTY_STATUS });
      saved.resolve(SAVED_PROFILE);
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(current.profileReady).toBe(true);
    expect(current.initializationState).toBe('ready');
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
    jest.spyOn(masqCore, 'getStatus').mockResolvedValue({ ...EMPTY_STATUS });
    jest
      .spyOn(masqCore, 'getSavedConfiguration')
      .mockRejectedValueOnce(
        new Error('The saved MASQ configuration is invalid.'),
      )
      .mockResolvedValueOnce(SAVED_PROFILE);
    const renderer = await renderController(value => {
      current = value;
    });

    expect(current.profileReady).toBe(false);
    expect(current.initializationState).toBe('error');
    expect(current.draft).toEqual(DEFAULT_SETUP);
    expect(current.issue?.message).toContain(
      'network profile could not be validated',
    );

    await ReactTestRenderer.act(async () => {
      await current.retryInitialization();
    });

    expect(current.profileReady).toBe(true);
    expect(current.initializationState).toBe('ready');
    expect(current.draft).toMatchObject({
      ...SAVED_PROFILE,
      walletSecret: '',
    });
    ReactTestRenderer.act(() => renderer.unmount());
  });

  it('fails closed when native status is configured without a matching saved profile', async () => {
    jest.spyOn(masqCore, 'getStatus').mockResolvedValue({
      ...EMPTY_STATUS,
      chain: 'base-mainnet',
      walletAddress: '0x1234567890abcdef',
      phase: 'ready',
      engineAvailable: true,
    });
    jest
      .spyOn(masqCore, 'getSavedConfiguration')
      .mockResolvedValue(null);
    const configure = jest.spyOn(masqCore, 'configure');
    const renderer = await renderController(value => {
      current = value;
    });

    expect(current.profileReady).toBe(false);
    expect(current.initializationState).toBe('error');
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
    jest.spyOn(masqCore, 'getStatus').mockResolvedValue({ ...EMPTY_STATUS });
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
