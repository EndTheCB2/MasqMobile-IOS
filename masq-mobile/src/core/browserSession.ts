import type { BrowserRoutingMode } from './masqCore';
import type { CoreStatus } from './types';

export type BrowserSessionMode = Exclude<BrowserRoutingMode, 'blocked'>;

interface BrowserSessionCore {
  setBrowserRoutingMode(
    mode: BrowserRoutingMode,
  ): Promise<BrowserRoutingMode>;
  preflightBrowserProxy(): Promise<CoreStatus>;
}

export async function prepareBrowserSession(
  core: BrowserSessionCore,
  mode: BrowserSessionMode,
): Promise<BrowserRoutingMode> {
  try {
    await core.setBrowserRoutingMode('blocked');
    const appliedMode = await core.setBrowserRoutingMode(mode);
    if (appliedMode !== mode) {
      throw new Error('The browser routing mode could not be confirmed.');
    }
    if (mode === 'masq') {
      await core.preflightBrowserProxy();
    }
    return appliedMode;
  } catch (error) {
    await core.setBrowserRoutingMode('blocked').catch(() => 'blocked');
    throw error;
  }
}

export async function closeBrowserSession(
  core: Pick<BrowserSessionCore, 'setBrowserRoutingMode'>,
): Promise<BrowserRoutingMode> {
  return core.setBrowserRoutingMode('blocked');
}
