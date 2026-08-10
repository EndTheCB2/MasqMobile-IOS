import {
  browserLoadWatchdogMs,
  buildBrowserNavigationMetric,
  reportBrowserNavigationMetric,
} from '../src/core/browserPerformance';

describe('browser performance telemetry', () => {
  afterEach(() => {
    jest.restoreAllMocks();
  });

  it('uses a bounded watchdog with additional time for a multi-hop route', () => {
    expect(browserLoadWatchdogMs('direct')).toBe(20_000);
    expect(browserLoadWatchdogMs('masq')).toBe(35_000);
  });

  it('rounds and bounds duration without including navigation identity', () => {
    expect(
      buildBrowserNavigationMetric('masq', 'completed', 1_000, 3_349),
    ).toEqual({
      component: 'browser',
      durationMs: 2_300,
      mode: 'masq',
      outcome: 'completed',
      version: 1,
    });
    expect(
      buildBrowserNavigationMetric('direct', 'timed_out', 0, 999_999)
        .durationMs,
    ).toBe(120_000);
  });

  it('emits only the deliberately URL-free metric payload', () => {
    const info = jest
      .spyOn(console, 'info')
      .mockImplementation(() => undefined);
    const metric = buildBrowserNavigationMetric('masq', 'failed', 10, 810);

    reportBrowserNavigationMetric(metric);

    expect(info).toHaveBeenCalledWith(
      '[MASQ_BROWSER_PERF]',
      '{"component":"browser","durationMs":800,"mode":"masq","outcome":"failed","version":1}',
    );
    const emitted = JSON.stringify(info.mock.calls);
    expect(emitted).not.toContain('http');
    expect(emitted).not.toContain('hostname');
    expect(emitted).not.toContain('wallet');
  });
});
