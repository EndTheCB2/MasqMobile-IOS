import type {CoreStatus} from './types';

interface ConnectionReadinessOptions {
  pollIntervalMs?: number;
  timeoutMs?: number;
  signal?: AbortSignal;
  sleep?: (milliseconds: number) => Promise<void>;
  onStatus?: (status: CoreStatus) => void;
}

export async function startAndAwaitMasqConnection(
  start: () => Promise<CoreStatus>,
  getStatus: () => Promise<CoreStatus>,
  options: ConnectionReadinessOptions = {},
): Promise<CoreStatus> {
  const timeoutMs = Math.max(1, options.timeoutMs ?? 18_000);
  const pollIntervalMs = Math.max(1, options.pollIntervalMs ?? 500);
  const sleep = options.sleep ?? (milliseconds => wait(milliseconds, options.signal));
  const startedAt = Date.now();

  let status = await start();
  throwIfAborted(options.signal);
  options.onStatus?.(status);

  while (status.phase !== 'connected') {
    throwIfAborted(options.signal);
    if (status.phase === 'error' || status.lastError) {
      throw new Error(
        status.lastError || 'The MASQ connection attempt stopped unexpectedly.',
      );
    }
    if (Date.now() - startedAt >= timeoutMs) {
      throw new Error(
        'The MASQ entry nodes did not complete a handshake in time.',
      );
    }
    await sleep(pollIntervalMs);
    throwIfAborted(options.signal);
    status = await getStatus();
    throwIfAborted(options.signal);
    options.onStatus?.(status);
  }

  return status;
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
    signal?.addEventListener('abort', cancel, {once: true});
  });
}
