import {
  closeBrowserSession,
  prepareBrowserSession,
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

  it('starts a MASQ session from blocked mode and preflights it', async () => {
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
    expect(calls).toEqual(['blocked', 'masq', 'preflight']);
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
    expect(calls).toEqual(['blocked', 'masq', 'blocked']);
    expect(calls).not.toContain('direct');
    expect(core.preflightBrowserProxy).not.toHaveBeenCalled();
  });

  it('returns to blocked mode when MASQ preflight fails', async () => {
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

    await expect(prepareBrowserSession(core, 'masq')).rejects.toThrow(
      'preflight failed',
    );
    expect(calls).toEqual(['blocked', 'masq', 'blocked']);
    expect(calls).not.toContain('direct');
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

    await expect(prepareBrowserSession(core, 'masq')).rejects.toThrow(
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
