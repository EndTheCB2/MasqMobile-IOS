import type { CoreStatus } from './types';
import {
  codedMasqError,
  extractMasqErrorCode,
  extractMasqErrorMessage,
  isConnectionRetryCode,
  isEntryNodeRetryCode,
} from './errorCodes';

export const ENTRY_NODE_REFRESH_ATTEMPTS = 3;
export const CONNECTION_ATTEMPT_BUDGET_MS = 90_000;
const PROGRESS_CONNECTION_HARD_BUDGET_MS = 120_000;
const PROGRESS_BONUS_ATTEMPTS = 1;
const ENTRY_NODE_REFRESH_BASE_DELAY_MS = 1500;
const ENTRY_NODE_REFRESH_MAX_DELAY_MS = 6000;
// The embedded Node emits aggregate peer failures as soon as both attempts are
// terminal and otherwise uses activity-based 18/26-second watchdogs. This
// 35-second outer slice leaves polling/scheduler margin while the 90/120-second
// deadline remains authoritative across discovery, readiness and route proof.
const ENTRY_NODE_POST_START_READINESS_MS = 35_000;
const MINIMUM_FRESH_NODE_SET_BUDGET_MS = 20_000;

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
  now?: () => number;
  overallTimeoutMs?: number;
  signal?: AbortSignal;
  sleep?: (milliseconds: number) => Promise<void>;
}

export interface EntryNodeAttemptBudget {
  deadlineAtMs: number;
  remainingMs: number;
  readinessTimeoutMs: number;
}

export async function startWithEntryNodeRefresh(
  start: (budget: EntryNodeAttemptBudget) => Promise<CoreStatus>,
  options: RefreshOptions = {},
): Promise<CoreStatus> {
  const configuredMaxAttempts = Math.max(
    1,
    options.maxAttempts ?? ENTRY_NODE_REFRESH_ATTEMPTS,
  );
  let activeMaxAttempts = configuredMaxAttempts;
  // An explicit maximum remains a hard caller-owned cap. The production
  // default can earn one extra fresh node set only after reaching stage one.
  const progressMaxAttempts =
    options.maxAttempts === undefined
      ? configuredMaxAttempts + PROGRESS_BONUS_ATTEMPTS
      : configuredMaxAttempts;
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
  const now = options.now ?? Date.now;
  const hardTimeoutMs = Math.min(
    PROGRESS_CONNECTION_HARD_BUDGET_MS,
    Math.max(1, options.overallTimeoutMs ?? PROGRESS_CONNECTION_HARD_BUDGET_MS),
  );
  const baseTimeoutMs = Math.min(CONNECTION_ATTEMPT_BUDGET_MS, hardTimeoutMs);
  const startedAtMs = now();
  const hardDeadlineAtMs = startedAtMs + hardTimeoutMs;
  let deadlineAtMs = startedAtMs + baseTimeoutMs;
  let progressBonusGranted = false;
  let lastSafeHandshakeError: unknown;
  const budgetFailure = () =>
    options.overallTimeoutMs === undefined && lastSafeHandshakeError
      ? lastSafeHandshakeError
      : connectionBudgetError();

  for (let attempt = 1; attempt <= activeMaxAttempts; attempt += 1) {
    throwIfAborted(options.signal);
    const attemptStartedAtMs = now();
    const remainingMs = deadlineAtMs - attemptStartedAtMs;
    if (
      remainingMs <= 0 ||
      (attempt > 1 && remainingMs < MINIMUM_FRESH_NODE_SET_BUDGET_MS)
    ) {
      throw budgetFailure();
    }
    // Discovery is bounded natively but may take several seconds on a changing
    // mobile network. Pass the outer absolute cap separately so readiness can
    // measure its full window from the moment native start actually settles.
    const attemptDeadlineAtMs = deadlineAtMs;
    const attemptRemainingMs = remainingMs;
    options.onAttempt?.({
      attempt,
      maxAttempts: activeMaxAttempts,
      stage: 'discovery',
    });
    try {
      const status = await start({
        deadlineAtMs: attemptDeadlineAtMs,
        remainingMs: attemptRemainingMs,
        readinessTimeoutMs: ENTRY_NODE_POST_START_READINESS_MS,
      });
      throwIfAborted(options.signal);
      if (now() > attemptDeadlineAtMs) {
        throw connectionBudgetError();
      }
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
        isConnectionRetryCode(retryCode)
      ) {
        lastSafeHandshakeError = caught;
      }
      if (!retryable) {
        throw caught;
      }
      if (
        !progressBonusGranted &&
        isDeepRouteProgressCode(retryCode) &&
        activeMaxAttempts < progressMaxAttempts &&
        deadlineAtMs < hardDeadlineAtMs
      ) {
        // A stage-one route proof means this network can reach MASQ. Give a
        // different entry pair one bounded chance without slowing ordinary
        // success or fully unreachable networks.
        progressBonusGranted = true;
        activeMaxAttempts = progressMaxAttempts;
        deadlineAtMs = hardDeadlineAtMs;
      }
      if (now() >= deadlineAtMs) {
        throw budgetFailure();
      }
      if (attempt === activeMaxAttempts) {
        throw shouldPreserveHandshakeError(caught)
          ? lastSafeHandshakeError ?? caught
          : caught;
      }
      const remainingAfterAttemptMs = deadlineAtMs - now();
      const retryDelayBudgetMs =
        remainingAfterAttemptMs - MINIMUM_FRESH_NODE_SET_BUDGET_MS;
      if (retryDelayBudgetMs < 0) {
        throw budgetFailure();
      }
      const exponentialDelayMs = Math.min(
        baseDelayMs * 2 ** (attempt - 1),
        maxDelayMs,
        retryDelayBudgetMs,
      );
      // A structured entry diagnostic already contains a bounded native wait.
      // Yield once for cancellation, then rotate immediately to a fresh pair.
      // Discovery, route-proof, network-handover and fuzzy errors retain the
      // capped backoff so independent failure domains are not hammered.
      const delayMs =
        retryCode !== 'E_ENTRY_NODE_DISCOVERY' &&
        isEntryNodeRetryCode(retryCode)
          ? 0
          : exponentialDelayMs;
      await sleep(delayMs);
      throwIfAborted(options.signal);
    }
  }

  throw new Error('MASQ entry-node refresh stopped unexpectedly.');
}

function connectionBudgetError(): Error {
  return codedMasqError(
    'E_CONNECTION_BUDGET_EXHAUSTED',
    'MASQ could not prove a private route within the bounded connection time.',
  );
}

function isDeepRouteProgressCode(code: string | null): boolean {
  return (
    code === 'E_PRIVATE_ROUTE_FAILED' || code === 'E_PRIVATE_ROUTE_TIMEOUT'
  );
}

export function isEntryNodeRefreshError(caught: unknown): boolean {
  const code = extractMasqErrorCode(caught);
  if (code !== null) {
    return isConnectionRetryCode(code);
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
