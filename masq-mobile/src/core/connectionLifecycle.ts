import type { BrowserRoutingMode } from './masqCore';
import type { CoreStatus } from './types';

interface StoppableMasqCore {
  setBrowserRoutingMode(
    mode: BrowserRoutingMode,
  ): Promise<BrowserRoutingMode>;
  stop(): Promise<CoreStatus>;
}

export async function stopMasqSafely(
  core: StoppableMasqCore,
): Promise<CoreStatus> {
  try {
    await core.setBrowserRoutingMode('blocked');
  } catch {
    // Stopping the core remains mandatory if browser isolation reports an error.
  }
  return core.stop();
}
