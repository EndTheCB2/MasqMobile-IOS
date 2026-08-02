import type { CoreStatus } from './types';
import {
  codedMasqError,
  extractMasqErrorCode,
  extractMasqErrorMessage,
  isEntryNodeRetryCode,
} from './errorCodes';

interface ConnectionReadinessOptions {
  pollIntervalMs?: number;
  timeoutMs?: number;
  signal?: AbortSignal;
  sleep?: (milliseconds: number) => Promise<void>;
  onStatus?: (status: CoreStatus) => void;
}

export function isCoreReadyForSystemRouting(status: CoreStatus): boolean {
  return (
    status.phase === 'connected' &&
    status.connectedNeighbors > 0 &&
    status.routeStage > 0 &&
    status.proxyPort !== null &&
    Number.isInteger(status.proxyPort) &&
    status.proxyPort > 0 &&
    status.proxyPort <= 65_535
  );
}

export async function startAndAwaitMasqConnection(
  start: () => Promise<CoreStatus>,
  getStatus: () => Promise<CoreStatus>,
  options: ConnectionReadinessOptions = {},
): Promise<CoreStatus> {
  const timeoutMs = Math.max(1, options.timeoutMs ?? 35_000);
  const pollIntervalMs = Math.max(1, options.pollIntervalMs ?? 500);
  const sleep =
    options.sleep ?? (milliseconds => wait(milliseconds, options.signal));

  let status: CoreStatus;
  try {
    status = await start();
  } catch (caught) {
    throwIfAborted(options.signal);
    if (extractMasqErrorCode(caught)) {
      throw caught;
    }
    throw codedMasqError(
      'E_CORE_STARTUP_FAILED',
      extractMasqErrorMessage(caught) ||
        'The embedded MASQ core could not start.',
    );
  }
  throwIfAborted(options.signal);
  options.onStatus?.(status);
  throwForTerminalStatus(status, true);

  const startedAt = Date.now();
  while (status.phase !== 'connected') {
    throwIfAborted(options.signal);
    if (Date.now() - startedAt >= timeoutMs) {
      throw codedMasqError(
        'E_ENTRY_GOSSIP_TIMEOUT',
        'The MASQ entry nodes did not complete a handshake in time.',
      );
    }
    await sleep(pollIntervalMs);
    throwIfAborted(options.signal);
    status = await getStatus();
    throwIfAborted(options.signal);
    options.onStatus?.(status);
    throwForTerminalStatus(status, false);
  }

  return status;
}

function throwForTerminalStatus(status: CoreStatus, startup: boolean): void {
  if (status.phase === 'connected') {
    return;
  }

  if (status.lastError) {
    const code = extractMasqErrorCode(status.lastError);
    if (isEntryNodeRetryCode(code)) {
      throw codedMasqError(code, status.lastError);
    }
    throw codedMasqError(
      startup ? 'E_CORE_STARTUP_FAILED' : 'E_CORE_EARLY_EXIT',
      status.lastError,
    );
  }

  const expectedPhase = ['connecting', 'connected'];
  if (!expectedPhase.includes(status.phase)) {
    throw codedMasqError(
      startup ? 'E_CORE_STARTUP_FAILED' : 'E_CORE_EARLY_EXIT',
      startup
        ? 'The embedded MASQ core did not enter its connection phase.'
        : 'The embedded MASQ core exited before a peer connection was ready.',
    );
  }
}

function throwIfAborted(signal?: AbortSignal) {
  if (signal?.aborted) {
    const error = new Error('The MASQ connection attempt was cancelled.');
    error.name = 'AbortError';
    throw error;
  }
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
      const error = new Error('The MASQ connection attempt was cancelled.');
      error.name = 'AbortError';
      reject(error);
    };
    signal?.addEventListener('abort', cancel, { once: true });
  });
}
