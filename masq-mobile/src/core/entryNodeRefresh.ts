import type { CoreStatus } from './types';
import {
  extractMasqErrorCode,
  extractMasqErrorMessage,
  isEntryNodeRetryCode,
} from './errorCodes';

export const ENTRY_NODE_REFRESH_ATTEMPTS = 6;
const ENTRY_NODE_REFRESH_BASE_DELAY_MS = 1500;
const ENTRY_NODE_REFRESH_MAX_DELAY_MS = 6000;

export interface EntryNodeRefreshProgress {
  attempt: number;
  maxAttempts: number;
  stage: 'discovery' | 'handshake';
}

interface RefreshOptions {
  baseDelayMs?: number;
  maxAttempts?: number;
  maxDelayMs?: number;
  onAttempt?: (progress: EntryNodeRefreshProgress) => void;
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
  const sleep =
    options.sleep ?? (milliseconds => wait(milliseconds, options.signal));
  let lastSafeHandshakeError: unknown;

  for (let attempt = 1; attempt <= maxAttempts; attempt += 1) {
    throwIfAborted(options.signal);
    options.onAttempt?.({ attempt, maxAttempts, stage: 'discovery' });
    try {
      const status = await start();
      throwIfAborted(options.signal);
      return status;
    } catch (caught) {
      if (options.signal?.aborted) {
        throw abortError();
      }
      const retryCode = extractMasqErrorCode(caught);
      const retryable = isEntryNodeRefreshError(caught);
      if (
        retryCode &&
        retryCode !== 'E_ENTRY_NODE_DISCOVERY' &&
        isEntryNodeRetryCode(retryCode)
      ) {
        lastSafeHandshakeError = caught;
      }
      if (!retryable) {
        throw caught;
      }
      if (attempt === maxAttempts) {
        throw shouldPreserveHandshakeError(caught)
          ? lastSafeHandshakeError ?? caught
          : caught;
      }
      const delayMs = Math.min(baseDelayMs * 2 ** (attempt - 1), maxDelayMs);
      await sleep(delayMs);
      throwIfAborted(options.signal);
    }
  }

  throw new Error('MASQ entry-node refresh stopped unexpectedly.');
}

export function isEntryNodeRefreshError(caught: unknown): boolean {
  const code = extractMasqErrorCode(caught);
  if (code !== null) {
    return isEntryNodeRetryCode(code);
  }

  const message = extractMasqErrorMessage(caught);
  return (
    /entry nodes?|entry peer|node-finder/i.test(message) &&
    /reachable|refresh|discover|find|handshake|connect|gossip|tim(?:e|ed)/i.test(
      message,
    )
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

function shouldPreserveHandshakeError(caught: unknown): boolean {
  const code = extractMasqErrorCode(caught);
  return code === 'E_ENTRY_NODE_DISCOVERY' || code === null;
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
    signal?.addEventListener('abort', cancel, { once: true });
  });
}
