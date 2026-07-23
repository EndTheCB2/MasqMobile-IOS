import type {CoreStatus} from './types';

export const ENTRY_NODE_REFRESH_ATTEMPTS = 6;
const ENTRY_NODE_REFRESH_BASE_DELAY_MS = 1500;
const ENTRY_NODE_REFRESH_MAX_DELAY_MS = 6000;

interface RefreshProgress {
  attempt: number;
  maxAttempts: number;
}

interface RefreshOptions {
  baseDelayMs?: number;
  maxAttempts?: number;
  maxDelayMs?: number;
  onAttempt?: (progress: RefreshProgress) => void;
  signal?: AbortSignal;
  sleep?: (milliseconds: number) => Promise<void>;
}

export async function startWithEntryNodeRefresh(
  start: () => Promise<CoreStatus>,
  options: RefreshOptions = {},
): Promise<CoreStatus> {
  const maxAttempts = Math.max(
    1,
    options.maxAttempts ?? ENTRY_NODE_REFRESH_ATTEMPTS,
  );
  const baseDelayMs = Math.max(
    0,
    options.baseDelayMs ?? ENTRY_NODE_REFRESH_BASE_DELAY_MS,
  );
  const maxDelayMs = Math.max(
    baseDelayMs,
    options.maxDelayMs ?? ENTRY_NODE_REFRESH_MAX_DELAY_MS,
  );
  const sleep = options.sleep ?? (milliseconds => wait(milliseconds, options.signal));

  for (let attempt = 1; attempt <= maxAttempts; attempt += 1) {
    throwIfAborted(options.signal);
    options.onAttempt?.({attempt, maxAttempts});
    try {
      const status = await start();
      throwIfAborted(options.signal);
      return status;
    } catch (caught) {
      if (options.signal?.aborted) {
        throw abortError();
      }
      if (!isEntryNodeDiscoveryError(caught) || attempt === maxAttempts) {
        throw caught;
      }
      const delayMs = Math.min(
        baseDelayMs * 2 ** (attempt - 1),
        maxDelayMs,
      );
      await sleep(delayMs);
      throwIfAborted(options.signal);
    }
  }

  throw new Error('MASQ entry-node refresh stopped unexpectedly.');
}

function isEntryNodeDiscoveryError(caught: unknown): boolean {
  if (!caught || typeof caught !== 'object') {
    return false;
  }
  const candidate = caught as {code?: unknown; message?: unknown};
  if (candidate.code === 'E_ENTRY_NODE_DISCOVERY') {
    return true;
  }
  if (typeof candidate.message !== 'string') {
    return false;
  }
  return /entry nodes?|node-finder/i.test(candidate.message) &&
    /reachable|refresh|discover|find|handshake|connect|tim(?:e|ed)/i.test(
      candidate.message,
    );
}

export function isAbortError(caught: unknown): boolean {
  return caught instanceof Error && caught.name === 'AbortError';
}

function throwIfAborted(signal?: AbortSignal) {
  if (signal?.aborted) {
    throw abortError();
  }
}

function abortError() {
  const error = new Error('The MASQ connection attempt was cancelled.');
  error.name = 'AbortError';
  return error;
}

function wait(milliseconds: number, signal?: AbortSignal): Promise<void> {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      signal?.removeEventListener('abort', cancel);
      resolve();
    }, milliseconds);
    const cancel = () => {
      clearTimeout(timer);
      signal?.removeEventListener('abort', cancel);
      reject(abortError());
    };
    signal?.addEventListener('abort', cancel, {once: true});
  });
}
