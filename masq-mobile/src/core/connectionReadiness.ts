import type { CoreStatus } from './types';
import {
  codedMasqError,
  extractMasqErrorCode,
  extractMasqErrorMessage,
  isConnectionRetryCode,
  isPrivateRouteRetryCode,
} from './errorCodes';

interface ConnectionReadinessOptions {
  deadlineAtMs?: number;
  pollIntervalMs?: number;
  timeoutMs?: number;
  signal?: AbortSignal;
  sleep?: (milliseconds: number) => Promise<void>;
  onStatus?: (status: CoreStatus) => void;
  verifyRoute?: () => Promise<CoreStatus>;
}

const MAX_ROUTE_PROOF_ATTEMPTS_PER_ENTRY_SET = 2;

export function isCoreRouteReady(status: CoreStatus): boolean {
  return (
    status.engineAvailable &&
    status.engineGeneration > 0 &&
    status.phase === 'connected' &&
    status.connectedNeighbors > 0 &&
    status.routeStage >= 2 &&
    status.proxyPort !== null &&
    Number.isInteger(status.proxyPort) &&
    status.proxyPort > 0 &&
    status.proxyPort <= 65_535 &&
    status.lastError === null
  );
}

export function isCoreReadyForSystemRouting(status: CoreStatus): boolean {
  return isCoreRouteReady(status);
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
    status = await startWithinDeadline(
      start,
      options.deadlineAtMs,
      options.signal,
    );
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
  const readinessDeadlineAtMs = startedAt + timeoutMs;
  const deadlineAtMs = Math.min(
    options.deadlineAtMs ?? Number.POSITIVE_INFINITY,
    readinessDeadlineAtMs,
  );
  let routeVerificationAttempts = 0;
  while (!isCoreRouteReady(status)) {
    throwIfAborted(options.signal);
    if (Date.now() >= deadlineAtMs) {
      if (status.connectedNeighbors > 0 && status.routeStage === 1) {
        throw codedMasqError(
          'E_PRIVATE_ROUTE_TIMEOUT',
          'The MASQ entry peer connected, but no private exit route was proven in time.',
        );
      }
      throw codedMasqError(
        'E_ENTRY_GOSSIP_TIMEOUT',
        'The MASQ entry nodes did not complete a handshake in time.',
      );
    }
    if (
      routeVerificationAttempts < MAX_ROUTE_PROOF_ATTEMPTS_PER_ENTRY_SET &&
      options.verifyRoute &&
      status.connectedNeighbors > 0 &&
      status.routeStage === 1 &&
      status.proxyPort !== null
    ) {
      routeVerificationAttempts += 1;
      try {
        status = await options.verifyRoute();
      } catch (caught) {
        throwIfAborted(options.signal);
        const routeErrorCode = extractMasqErrorCode(caught);
        if (routeErrorCode && !isPrivateRouteRetryCode(routeErrorCode)) {
          throw caught;
        }
        const routeError = routeErrorCode
          ? caught
          : codedMasqError(
              'E_PRIVATE_ROUTE_FAILED',
              extractMasqErrorMessage(caught) ||
                'MASQ could not prove an end-to-end private exit route.',
            );
        if (
          routeVerificationAttempts >=
          MAX_ROUTE_PROOF_ATTEMPTS_PER_ENTRY_SET
        ) {
          throw routeError;
        }
        // The entry neighborship can become usable while topology gossip is still arriving. The
        // native preflight is observational, so re-read live state and allow one bounded retry.
        await sleep(pollIntervalMs);
        throwIfAborted(options.signal);
        status = await getStatus();
        throwIfAborted(options.signal);
        options.onStatus?.(status);
        throwForTerminalStatus(status, false);
        continue;
      }
      throwIfAborted(options.signal);
      if (Date.now() > deadlineAtMs) {
        throw codedMasqError(
          'E_PRIVATE_ROUTE_TIMEOUT',
          'The MASQ exit-route proof exceeded the bounded connection time.',
        );
      }
      options.onStatus?.(status);
      throwForTerminalStatus(status, false);
      continue;
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
  if (status.lastError) {
    const code = extractMasqErrorCode(status.lastError);
    if (isConnectionRetryCode(code)) {
      throw codedMasqError(code, status.lastError);
    }
    throw codedMasqError(
      startup ? 'E_CORE_STARTUP_FAILED' : 'E_CORE_EARLY_EXIT',
      status.lastError,
    );
  }

  if (status.phase === 'connected') {
    return;
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
    throw abortError();
  }
}

function startWithinDeadline(
  start: () => Promise<CoreStatus>,
  deadlineAtMs?: number,
  signal?: AbortSignal,
): Promise<CoreStatus> {
  throwIfAborted(signal);
  const remainingMs =
    deadlineAtMs === undefined ? null : deadlineAtMs - Date.now();
  if (remainingMs !== null && remainingMs <= 0) {
    throw startupDeadlineError();
  }

  const operation = start();
  if (remainingMs === null && !signal) {
    return operation;
  }

  return new Promise((resolve, reject) => {
    let settled = false;
    let deadlineTimer: ReturnType<typeof setTimeout> | undefined;

    const finish = (complete: () => void) => {
      if (settled) return;
      settled = true;
      if (deadlineTimer !== undefined) clearTimeout(deadlineTimer);
      signal?.removeEventListener('abort', cancel);
      complete();
    };
    const cancel = () => finish(() => reject(abortError()));

    signal?.addEventListener('abort', cancel, { once: true });
    if (signal?.aborted) {
      cancel();
    }
    if (!settled && deadlineAtMs !== undefined) {
      deadlineTimer = setTimeout(
        () => finish(() => reject(startupDeadlineError())),
        Math.max(0, deadlineAtMs - Date.now()),
      );
    }

    // Keep both handlers attached after timeout/abort. A native Promise cannot
    // be cancelled, but its eventual rejection must stay handled and its late
    // status must never escape into onStatus or readiness polling.
    operation.then(
      value => {
        if (deadlineAtMs !== undefined && Date.now() >= deadlineAtMs) {
          finish(() => reject(startupDeadlineError()));
          return;
        }
        finish(() => resolve(value));
      },
      caught => {
        if (deadlineAtMs !== undefined && Date.now() >= deadlineAtMs) {
          finish(() => reject(startupDeadlineError()));
          return;
        }
        finish(() => reject(caught));
      },
    );
  });
}

function startupDeadlineError(): Error {
  return codedMasqError(
    'E_ENTRY_NODE_DISCOVERY',
    'MASQ entry-node discovery and native startup exceeded the bounded connection time.',
  );
}

function abortError(): Error {
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
    signal?.addEventListener('abort', cancel, { once: true });
  });
}
