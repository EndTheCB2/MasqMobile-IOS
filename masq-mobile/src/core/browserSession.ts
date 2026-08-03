import type { BrowserRoutingMode } from './masqCore';
import type { CoreStatus } from './types';

export type BrowserSessionMode = Exclude<BrowserRoutingMode, 'blocked'>;

interface BrowserSessionCore {
  setBrowserRoutingMode(mode: BrowserRoutingMode): Promise<BrowserRoutingMode>;
  preflightBrowserProxy(): Promise<CoreStatus>;
}

export class BrowserSessionOperationCancelledError extends Error {
  constructor() {
    super('The browser session operation was cancelled.');
    this.name = 'BrowserSessionOperationCancelledError';
  }
}

async function requireCurrentRepair(
  core: BrowserSessionCore,
  isCurrent: () => boolean,
  alreadyBlocked = false,
): Promise<void> {
  if (isCurrent()) {
    return;
  }
  // Native work cannot be interrupted while its Promise is pending. If a
  // close, background transition, or routing-mode switch invalidated this
  // repair, re-assert blocked mode after that pending work settles so a stale
  // completion can never leave MASQ browser routing enabled.
  if (!alreadyBlocked) {
    await closeBrowserSession(core);
  }
  throw new BrowserSessionOperationCancelledError();
}

export async function repairPrivateBrowserRoute(
  core: BrowserSessionCore,
  reconnect: () => Promise<unknown>,
  isCurrent: () => boolean = () => true,
): Promise<void> {
  await requireCurrentRepair(core, isCurrent);
  let proofFailed = false;
  try {
    // A short proof may refresh an otherwise healthy route without changing
    // entry peers. A failed proof moves native status out of Connected, so the
    // normal reconnect path can safely refresh entries and retry in place.
    await core.preflightBrowserProxy();
  } catch {
    proofFailed = true;
  }
  if (!proofFailed) {
    await requireCurrentRepair(core, isCurrent);
    return;
  }

  await closeBrowserSession(core);
  await requireCurrentRepair(core, isCurrent, true);
  await reconnect();
  await requireCurrentRepair(core, isCurrent);
  await prepareBrowserSession(core, 'masq');
  await requireCurrentRepair(core, isCurrent);
}

export async function prepareBrowserSession(
  core: BrowserSessionCore,
  mode: BrowserSessionMode,
): Promise<BrowserRoutingMode> {
  try {
    // MASQ's native transition installs its blocked sink before it reads or
    // applies the live proxy port. Calling blocked separately here duplicated
    // Android cookie/storage cleanup and immediately repeated the expensive
    // end-to-end route proof that connect() already completed. Direct mode
    // retains an explicit blocked barrier because it clears the proxy.
    if (mode === 'direct') {
      await closeBrowserSession(core);
    }
    const appliedMode = await core.setBrowserRoutingMode(mode);
    if (appliedMode !== mode) {
      throw new Error('The browser routing mode could not be confirmed.');
    }
    return appliedMode;
  } catch (error) {
    try {
      await closeBrowserSession(core);
    } catch {
      throw new Error(
        'The browser session failed and browser traffic could not be confirmed blocked.',
      );
    }
    throw error;
  }
}

export async function closeBrowserSession(
  core: Pick<BrowserSessionCore, 'setBrowserRoutingMode'>,
): Promise<BrowserRoutingMode> {
  const appliedMode = await core.setBrowserRoutingMode('blocked');
  if (appliedMode !== 'blocked') {
    throw new Error('Browser traffic could not be confirmed blocked.');
  }
  return appliedMode;
}
