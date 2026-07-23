export interface BrowserFailure {
  code: number;
  description: string;
  domain: string;
}

export interface BrowserRecovery {
  delayMs: number | null;
  message: string;
  nextAttempt: number;
  retry: boolean;
}

export type BrowserRecoveryMode = 'masq' | 'direct';

const TRANSIENT_CODES = new Set([-1001, -1003, -1004, -1005, -2, -6, -7, -8]);
const OFFLINE_CODES = new Set([-1009]);

export function decideBrowserRecovery(
  failure: BrowserFailure,
  completedAttempts: number,
  maxAttempts: number,
  mode: BrowserRecoveryMode = 'masq',
): BrowserRecovery {
  const safeMax = Math.max(0, maxAttempts);
  const safeCompleted = Math.max(0, completedAttempts);

  if (OFFLINE_CODES.has(failure.code)) {
    return noRetry(
      safeCompleted,
      'The device is offline. Reconnect Wi-Fi or mobile data before retrying.',
    );
  }
  if (isCertificateFailure(failure)) {
    return noRetry(
      safeCompleted,
      'The browser blocked a connection that could not establish trusted HTTPS security.',
    );
  }
  if (TRANSIENT_CODES.has(failure.code)) {
    if (safeCompleted < safeMax) {
      const nextAttempt = safeCompleted + 1;
      return {
        delayMs: 600 * 2 ** safeCompleted,
        message:
          mode === 'masq'
            ? `The private route changed. Retrying safely (${nextAttempt}/${safeMax})…`
            : `The normal internet connection was interrupted. Retrying directly (${nextAttempt}/${safeMax})…`,
        nextAttempt,
        retry: true,
      };
    }
    return noRetry(
      safeCompleted,
      mode === 'masq'
        ? 'The private route remained unavailable after automatic recovery. Reconnect MASQ and try again.'
        : 'The direct connection remained unavailable after automatic recovery. Check your internet connection and try again.',
    );
  }

  return noRetry(
    safeCompleted,
    mode === 'masq'
      ? 'The website could not be loaded through the verified MASQ route.'
      : 'The website could not be loaded using your normal internet connection.',
  );
}

function noRetry(nextAttempt: number, message: string): BrowserRecovery {
  return {delayMs: null, message, nextAttempt, retry: false};
}

function isCertificateFailure(failure: BrowserFailure): boolean {
  const searchable = `${failure.domain} ${failure.description}`.toLowerCase();
  return (
    (failure.code <= -1200 && failure.code >= -1206) ||
    /certificate|ssl|tls|untrusted/.test(searchable)
  );
}
