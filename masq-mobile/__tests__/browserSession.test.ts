import {
  BrowserSessionOperationCancelledError,
  closeBrowserSession,
  prepareBrowserSession,
  repairPrivateBrowserRoute,
} from '../src/core/browserSession';
import { masqCore, type BrowserRoutingMode } from '../src/core/masqCore';
import { EMPTY_STATUS } from '../src/core/types';

describe('browser routing sessions', () => {
  it('allows safe and direct routing without a native core but rejects MASQ', async () => {
    await expect(masqCore.setBrowserRoutingMode('blocked')).resolves.toBe(
      'blocked',
    );
    await expect(masqCore.setBrowserRoutingMode('direct')).resolves.toBe(
      'direct',
    );
    await expect(masqCore.setBrowserRoutingMode('masq')).rejects.toThrow(
      'native MASQ core',
    );
  });

  it('starts a MASQ session through the native fail-closed transition without a duplicate preflight', async () => {
    const calls: string[] = [];
    const core = {
      setBrowserRoutingMode: jest.fn(async (mode: BrowserRoutingMode) => {
        calls.push(mode);
        return mode;
      }),
      preflightBrowserProxy: jest.fn(async () => {
        calls.push('preflight');
        return EMPTY_STATUS;
      }),
    };

    await expect(prepareBrowserSession(core, 'masq')).resolves.toBe('masq');
    expect(calls).toEqual(['masq']);
    expect(core.preflightBrowserProxy).not.toHaveBeenCalled();
  });

  it('starts a direct session from blocked mode without a MASQ preflight', async () => {
    const calls: string[] = [];
    const core = {
      setBrowserRoutingMode: jest.fn(async (mode: BrowserRoutingMode) => {
        calls.push(mode);
        return mode;
      }),
      preflightBrowserProxy: jest.fn().mockResolvedValue(EMPTY_STATUS),
    };

    await expect(prepareBrowserSession(core, 'direct')).resolves.toBe('direct');
    expect(calls).toEqual(['blocked', 'direct']);
    expect(core.preflightBrowserProxy).not.toHaveBeenCalled();
  });

  it('returns to blocked mode when MASQ routing fails without trying direct', async () => {
    const calls: string[] = [];
    const core = {
      setBrowserRoutingMode: jest.fn(async (mode: BrowserRoutingMode) => {
        calls.push(mode);
        if (mode === 'masq') {
          throw new Error('MASQ unavailable');
        }
        return mode;
      }),
      preflightBrowserProxy: jest.fn().mockResolvedValue(EMPTY_STATUS),
    };

    await expect(prepareBrowserSession(core, 'masq')).rejects.toThrow(
      'MASQ unavailable',
    );
    expect(calls).toEqual(['masq', 'blocked']);
    expect(calls).not.toContain('direct');
    expect(core.preflightBrowserProxy).not.toHaveBeenCalled();
  });

  it('does not repeat the end-to-end route proof while opening MASQ', async () => {
    const calls: string[] = [];
    const core = {
      setBrowserRoutingMode: jest.fn(async (mode: BrowserRoutingMode) => {
        calls.push(mode);
        return mode;
      }),
      preflightBrowserProxy: jest
        .fn()
        .mockRejectedValue(new Error('preflight failed')),
    };

    await expect(prepareBrowserSession(core, 'masq')).resolves.toBe('masq');
    expect(calls).toEqual(['masq']);
    expect(calls).not.toContain('direct');
    expect(core.preflightBrowserProxy).not.toHaveBeenCalled();
  });

  it('refreshes entries through reconnect after a transient route proof fails', async () => {
    const calls: string[] = [];
    const core = {
      setBrowserRoutingMode: jest.fn(async (mode: BrowserRoutingMode) => {
        calls.push(mode);
        return mode;
      }),
      preflightBrowserProxy: jest.fn().mockImplementationOnce(async () => {
        calls.push('failed-preflight');
        throw new Error('route interrupted');
      }),
    };
    const reconnect = jest.fn(async () => {
      calls.push('reconnect');
    });

    await expect(
      repairPrivateBrowserRoute(core, reconnect),
    ).resolves.toBeUndefined();

    expect(calls).toEqual([
      'failed-preflight',
      'blocked',
      'reconnect',
      'masq',
    ]);
    expect(reconnect).toHaveBeenCalledTimes(1);
    expect(calls).not.toContain('direct');
  });

  it('keeps the current entries when the short route proof still succeeds', async () => {
    const core = {
      setBrowserRoutingMode: jest.fn(),
      preflightBrowserProxy: jest.fn().mockResolvedValue(EMPTY_STATUS),
    };
    const reconnect = jest.fn();

    await repairPrivateBrowserRoute(core, reconnect);

    expect(reconnect).not.toHaveBeenCalled();
    expect(core.setBrowserRoutingMode).not.toHaveBeenCalled();
  });

  it('blocks a stale repair after a pending route proof completes', async () => {
    let resolveProof!: (status: typeof EMPTY_STATUS) => void;
    let current = true;
    const core = {
      setBrowserRoutingMode: jest
        .fn()
        .mockImplementation(async (mode: BrowserRoutingMode) => mode),
      preflightBrowserProxy: jest.fn(
        () =>
          new Promise<typeof EMPTY_STATUS>(resolve => {
            resolveProof = resolve;
          }),
      ),
    };
    const reconnect = jest.fn();
    const repair = repairPrivateBrowserRoute(
      core,
      reconnect,
      () => current,
    );
    await flushMicrotasks();
    expect(core.preflightBrowserProxy).toHaveBeenCalledTimes(1);

    current = false;
    resolveProof(EMPTY_STATUS);

    await expect(repair).rejects.toBeInstanceOf(
      BrowserSessionOperationCancelledError,
    );
    expect(core.setBrowserRoutingMode).toHaveBeenCalledTimes(1);
    expect(core.setBrowserRoutingMode).toHaveBeenCalledWith('blocked');
    expect(reconnect).not.toHaveBeenCalled();
  });

  it('does not prepare MASQ after a reconnect becomes stale', async () => {
    let resolveReconnect!: () => void;
    let current = true;
    const calls: string[] = [];
    const core = {
      setBrowserRoutingMode: jest.fn(async (mode: BrowserRoutingMode) => {
        calls.push(mode);
        return mode;
      }),
      preflightBrowserProxy: jest.fn(async () => {
        calls.push('failed-preflight');
        throw new Error('route interrupted');
      }),
    };
    const reconnect = jest.fn(
      () =>
        new Promise<void>(resolve => {
          calls.push('reconnect');
          resolveReconnect = resolve;
        }),
    );
    const repair = repairPrivateBrowserRoute(
      core,
      reconnect,
      () => current,
    );
    await flushMicrotasks();
    expect(reconnect).toHaveBeenCalledTimes(1);

    current = false;
    resolveReconnect();

    await expect(repair).rejects.toBeInstanceOf(
      BrowserSessionOperationCancelledError,
    );
    expect(calls).toEqual([
      'failed-preflight',
      'blocked',
      'reconnect',
      'blocked',
    ]);
    expect(calls).not.toContain('masq');
  });

  it('re-blocks when an in-flight MASQ preparation completes after cancellation', async () => {
    let resolveMasqMode!: (mode: BrowserRoutingMode) => void;
    let current = true;
    const calls: string[] = [];
    const core = {
      setBrowserRoutingMode: jest.fn((mode: BrowserRoutingMode) => {
        calls.push(mode);
        if (mode === 'masq') {
          return new Promise<BrowserRoutingMode>(resolve => {
            resolveMasqMode = resolve;
          });
        }
        return Promise.resolve(mode);
      }),
      preflightBrowserProxy: jest
        .fn()
        .mockRejectedValueOnce(new Error('route interrupted'))
        .mockResolvedValueOnce(EMPTY_STATUS),
    };
    const repair = repairPrivateBrowserRoute(
      core,
      jest.fn().mockResolvedValue(undefined),
      () => current,
    );
    await flushMicrotasks(20);
    expect(calls).toEqual(['blocked', 'masq']);

    current = false;
    resolveMasqMode('masq');

    await expect(repair).rejects.toBeInstanceOf(
      BrowserSessionOperationCancelledError,
    );
    expect(calls.at(-1)).toBe('blocked');
    expect(core.preflightBrowserProxy).toHaveBeenCalledTimes(1);
  });

  it('returns to blocked mode when direct routing fails', async () => {
    const calls: string[] = [];
    const core = {
      setBrowserRoutingMode: jest.fn(async (mode: BrowserRoutingMode) => {
        calls.push(mode);
        if (mode === 'direct') {
          throw new Error('direct unavailable');
        }
        return mode;
      }),
      preflightBrowserProxy: jest.fn().mockResolvedValue(EMPTY_STATUS),
    };

    await expect(prepareBrowserSession(core, 'direct')).rejects.toThrow(
      'direct unavailable',
    );
    expect(calls).toEqual(['blocked', 'direct', 'blocked']);
  });

  it('blocks browser traffic when a session closes', async () => {
    const core = {
      setBrowserRoutingMode: jest.fn().mockResolvedValue('blocked'),
    };

    await expect(closeBrowserSession(core)).resolves.toBe('blocked');
    expect(core.setBrowserRoutingMode).toHaveBeenCalledWith('blocked');
  });

  it('rejects a close when native code does not acknowledge blocked mode', async () => {
    const core = {
      setBrowserRoutingMode: jest.fn().mockResolvedValue('direct'),
    };

    await expect(closeBrowserSession(core)).rejects.toThrow(
      'Browser traffic could not be confirmed blocked.',
    );
    expect(core.setBrowserRoutingMode).toHaveBeenCalledTimes(1);
  });

  it('does not open a session before blocked mode is acknowledged', async () => {
    const calls: string[] = [];
    let blockedAttempts = 0;
    const core = {
      setBrowserRoutingMode: jest.fn(async (mode: BrowserRoutingMode) => {
        calls.push(mode);
        if (mode === 'blocked' && blockedAttempts++ === 0) {
          return 'direct' as BrowserRoutingMode;
        }
        return mode;
      }),
      preflightBrowserProxy: jest.fn().mockResolvedValue(EMPTY_STATUS),
    };

    await expect(prepareBrowserSession(core, 'direct')).rejects.toThrow(
      'Browser traffic could not be confirmed blocked.',
    );
    expect(calls).toEqual(['blocked', 'blocked']);
    expect(core.preflightBrowserProxy).not.toHaveBeenCalled();
  });

  it('surfaces a failed fail-closed rollback instead of the preparation error', async () => {
    const core = {
      setBrowserRoutingMode: jest.fn(async (mode: BrowserRoutingMode) =>
        mode === 'blocked' ? ('direct' as BrowserRoutingMode) : mode,
      ),
      preflightBrowserProxy: jest.fn().mockResolvedValue(EMPTY_STATUS),
    };

    await expect(prepareBrowserSession(core, 'direct')).rejects.toThrow(
      'browser traffic could not be confirmed blocked',
    );
    expect(core.setBrowserRoutingMode).toHaveBeenCalledTimes(2);
  });
});

async function flushMicrotasks(count = 10) {
  for (let index = 0; index < count; index += 1) {
    await Promise.resolve();
  }
}
