export type BrowserPerformanceMode = 'masq' | 'direct';

export type BrowserNavigationOutcome =
  | 'completed'
  | 'failed'
  | 'http_error'
  | 'timed_out'
  | 'cancelled';

export interface BrowserNavigationMetric {
  component: 'browser';
  durationMs: number;
  mode: BrowserPerformanceMode;
  outcome: BrowserNavigationOutcome;
  version: 1;
}

const DIRECT_LOAD_WATCHDOG_MS = 20_000;
const MASQ_LOAD_WATCHDOG_MS = 35_000;
const MAX_RECORDED_DURATION_MS = 120_000;
const DURATION_PRECISION_MS = 100;

export function browserLoadWatchdogMs(mode: BrowserPerformanceMode): number {
  return mode === 'masq' ? MASQ_LOAD_WATCHDOG_MS : DIRECT_LOAD_WATCHDOG_MS;
}

export function browserMonotonicNow(): number {
  const runtime = globalThis as unknown as {
    performance?: { now?: () => number };
  };
  if (
    typeof runtime.performance !== 'undefined' &&
    typeof runtime.performance.now === 'function'
  ) {
    return runtime.performance.now();
  }
  return Date.now();
}

export function buildBrowserNavigationMetric(
  mode: BrowserPerformanceMode,
  outcome: BrowserNavigationOutcome,
  startedAtMs: number,
  finishedAtMs: number,
): BrowserNavigationMetric {
  const measuredDuration = Number.isFinite(finishedAtMs - startedAtMs)
    ? finishedAtMs - startedAtMs
    : 0;
  const boundedDuration = Math.min(
    MAX_RECORDED_DURATION_MS,
    Math.max(0, measuredDuration),
  );

  return {
    component: 'browser',
    durationMs:
      Math.round(boundedDuration / DURATION_PRECISION_MS) *
      DURATION_PRECISION_MS,
    mode,
    outcome,
    version: 1,
  };
}

/**
 * Emits a deliberately URL-free local metric. It contains no hostname, IP,
 * node identity, wallet identifier, search text or wall-clock timestamp.
 */
export function reportBrowserNavigationMetric(
  metric: BrowserNavigationMetric,
): void {
  console.info('[MASQ_BROWSER_PERF]', JSON.stringify(metric));
}
