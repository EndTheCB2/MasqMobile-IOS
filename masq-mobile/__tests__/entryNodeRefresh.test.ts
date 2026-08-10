import { startWithEntryNodeRefresh } from '../src/core/entryNodeRefresh';
import { EMPTY_STATUS, type CoreStatus } from '../src/core/types';

const connected: CoreStatus = { ...EMPTY_STATUS, phase: 'connecting' };

interface AttemptBudget {
  deadlineAtMs: number;
  remainingMs: number;
  readinessTimeoutMs: number;
}

function discoveryError(
  message = 'MASQ could not find two reachable entry nodes.',
) {
  return Object.assign(new Error(message), { code: 'E_ENTRY_NODE_DISCOVERY' });
}

function codedConnectionError(code: string) {
  return Object.assign(new Error('Safe native diagnostic.'), { code });
}

describe('automatic entry-node refresh', () => {
  it('returns immediately when the first discovery succeeds', async () => {
    const start = jest.fn().mockResolvedValue(connected);
    const onAttempt = jest.fn();

    await expect(
      startWithEntryNodeRefresh(start, { now: () => 10_000, onAttempt }),
    ).resolves.toBe(connected);
    expect(start).toHaveBeenCalledTimes(1);
    expect(start).toHaveBeenCalledWith({
      deadlineAtMs: 100_000,
      remainingMs: 90_000,
      readinessTimeoutMs: 35_000,
    });
    expect(onAttempt).toHaveBeenCalledWith({
      attempt: 1,
      maxAttempts: 3,
      stage: 'discovery',
    });
  });

  it('refreshes automatically with capped exponential backoff', async () => {
    const start = jest
      .fn()
      .mockRejectedValueOnce(discoveryError())
      .mockRejectedValueOnce(discoveryError())
      .mockResolvedValue(connected);
    const sleep = jest.fn().mockResolvedValue(undefined);
    const onAttempt = jest.fn();

    await expect(
      startWithEntryNodeRefresh(start, {
        baseDelayMs: 1000,
        maxAttempts: 4,
        maxDelayMs: 1500,
        onAttempt,
        sleep,
      }),
    ).resolves.toBe(connected);

    expect(start).toHaveBeenCalledTimes(3);
    expect(sleep.mock.calls).toEqual([[1000], [1500]]);
    expect(onAttempt.mock.calls).toEqual([
      [{ attempt: 1, maxAttempts: 4, stage: 'discovery' }],
      [{ attempt: 2, maxAttempts: 4, stage: 'discovery' }],
      [{ attempt: 3, maxAttempts: 4, stage: 'discovery' }],
    ]);
  });

  it.each([
    'E_ENTRY_TCP_FAILED',
    'E_ENTRY_TCP_WAITING_GOSSIP',
    'E_ENTRY_GOSSIP_TIMEOUT',
    'E_ENTRY_GOSSIP_PASS_LOOP',
    'E_ENTRY_NO_PROGRESS',
    'E_ENTRY_DEBUT_NOT_WRITTEN',
    'E_ENTRY_NO_INBOUND_BYTES',
    'E_ENTRY_INBOUND_NOT_ACCEPTED',
    'E_ENTRY_GOSSIP_NOT_PROMOTED',
    'E_PRIVATE_ROUTE_FAILED',
    'E_PRIVATE_ROUTE_TIMEOUT',
    'E_NETWORK_HANDOVER_RETRY',
  ])('retries the safe native diagnostic %s', async code => {
    const diagnostic = Object.assign(new Error('Safe native diagnostic.'), {
      code,
    });
    const start = jest
      .fn()
      .mockRejectedValueOnce(diagnostic)
      .mockResolvedValue(connected);
    const sleep = jest.fn().mockResolvedValue(undefined);

    await expect(startWithEntryNodeRefresh(start, { sleep })).resolves.toBe(
      connected,
    );
    expect(start).toHaveBeenCalledTimes(2);
  });

  it.each([
    'E_ENTRY_TCP_FAILED',
    'E_ENTRY_GOSSIP_TIMEOUT',
    'E_ENTRY_GOSSIP_PASS_LOOP',
    'E_ENTRY_NO_PROGRESS',
    'E_ENTRY_NO_INBOUND_BYTES',
    'E_ENTRY_INBOUND_NOT_ACCEPTED',
  ])('rotates %s to a fresh pair without stacking another backoff', async code => {
    const start = jest
      .fn()
      .mockRejectedValueOnce(codedConnectionError(code))
      .mockResolvedValue(connected);
    const sleep = jest.fn().mockResolvedValue(undefined);

    await expect(startWithEntryNodeRefresh(start, { sleep })).resolves.toBe(
      connected,
    );

    expect(sleep.mock.calls).toEqual([[0]]);
  });

  it.each([
    'E_ENTRY_NODE_DISCOVERY',
    'E_PRIVATE_ROUTE_FAILED',
    'E_NETWORK_HANDOVER_RETRY',
  ])('retains bounded backoff for the independent failure domain %s', async code => {
    const start = jest
      .fn()
      .mockRejectedValueOnce(codedConnectionError(code))
      .mockResolvedValue(connected);
    const sleep = jest.fn().mockResolvedValue(undefined);

    await expect(startWithEntryNodeRefresh(start, { sleep })).resolves.toBe(
      connected,
    );

    expect(sleep.mock.calls).toEqual([[1_500]]);
  });

  it('does not retry unrelated native errors', async () => {
    const error = Object.assign(new Error('The native MASQ core is missing.'), {
      code: 'E_CORE_UNAVAILABLE',
    });
    const start = jest.fn().mockRejectedValue(error);
    const sleep = jest.fn().mockResolvedValue(undefined);

    await expect(startWithEntryNodeRefresh(start, { sleep })).rejects.toBe(
      error,
    );
    expect(start).toHaveBeenCalledTimes(1);
    expect(sleep).not.toHaveBeenCalled();
  });

  it('does not reinterpret a stable core lifecycle code as an entry-node retry', async () => {
    const error = Object.assign(
      new Error('The core stopped before an entry peer connection was ready.'),
      { code: 'E_CORE_EARLY_EXIT' },
    );
    const start = jest.fn().mockRejectedValue(error);
    const sleep = jest.fn().mockResolvedValue(undefined);

    await expect(startWithEntryNodeRefresh(start, { sleep })).rejects.toBe(
      error,
    );
    expect(start).toHaveBeenCalledTimes(1);
    expect(sleep).not.toHaveBeenCalled();
  });

  it('retries a handshake timeout reported after native startup', async () => {
    const start = jest
      .fn()
      .mockRejectedValueOnce(
        new Error('The MASQ entry nodes did not complete a handshake in time.'),
      )
      .mockResolvedValue(connected);
    const sleep = jest.fn().mockResolvedValue(undefined);

    await expect(startWithEntryNodeRefresh(start, { sleep })).resolves.toBe(
      connected,
    );
    expect(start).toHaveBeenCalledTimes(2);
    expect(sleep.mock.calls).toEqual([[1_500]]);
  });

  it('returns the final discovery error after all refresh rounds', async () => {
    const error = discoveryError(
      'Automatic refresh found 0 reachable entry nodes.',
    );
    const start = jest.fn().mockRejectedValue(error);
    const sleep = jest.fn().mockResolvedValue(undefined);

    await expect(
      startWithEntryNodeRefresh(start, { maxAttempts: 3, sleep }),
    ).rejects.toBe(error);
    expect(start).toHaveBeenCalledTimes(3);
    expect(sleep).toHaveBeenCalledTimes(2);
  });

  it('preserves the last specific handshake diagnostic when a generic discovery error ends the refresh', async () => {
    const diagnostic = Object.assign(
      new Error('The peer returned no handshake bytes.'),
      { code: 'E_ENTRY_NO_INBOUND_BYTES' },
    );
    const generic = discoveryError();
    const start = jest
      .fn()
      .mockRejectedValueOnce(diagnostic)
      .mockRejectedValue(generic);

    await expect(
      startWithEntryNodeRefresh(start, {
        maxAttempts: 3,
        sleep: () => Promise.resolve(),
      }),
    ).rejects.toBe(diagnostic);
    expect(start).toHaveBeenCalledTimes(3);
  });

  it('does not start another fresh node set when the overall budget is too small', async () => {
    let clockMs = 0;
    const observedBudgets: number[] = [];
    const start = jest.fn(async (budget: { remainingMs: number }) => {
      observedBudgets.push(budget.remainingMs);
      clockMs += 35_000;
      throw discoveryError();
    });
    const sleep = jest.fn(async (milliseconds: number) => {
      clockMs += milliseconds;
    });

    await expect(
      startWithEntryNodeRefresh(start, {
        now: () => clockMs,
        overallTimeoutMs: 90_000,
        sleep,
      }),
    ).rejects.toMatchObject({ code: 'E_CONNECTION_BUDGET_EXHAUSTED' });

    expect(start).toHaveBeenCalledTimes(2);
    expect(observedBudgets).toEqual([90_000, 53_500]);
    expect(sleep.mock.calls).toEqual([[1_500]]);
    expect(clockMs).toBe(71_500);
    expect(clockMs).toBeLessThanOrEqual(90_000);
  });

  it('caps the default connection action at three fresh node sets', async () => {
    const start = jest.fn().mockRejectedValue(discoveryError());

    await expect(
      startWithEntryNodeRefresh(start, {
        baseDelayMs: 0,
        now: () => 0,
        sleep: () => Promise.resolve(),
      }),
    ).rejects.toMatchObject({ code: 'E_ENTRY_NODE_DISCOVERY' });

    expect(start).toHaveBeenCalledTimes(3);
  });

  it('does not extend ordinary no-inbound failures beyond three attempts or 90 seconds', async () => {
    const noInbound = codedConnectionError('E_ENTRY_NO_INBOUND_BYTES');
    const observedDeadlines: number[] = [];
    const start = jest.fn(
      async ({ deadlineAtMs }: { deadlineAtMs: number }) => {
        observedDeadlines.push(deadlineAtMs);
        throw noInbound;
      },
    );

    await expect(
      startWithEntryNodeRefresh(start, {
        baseDelayMs: 0,
        now: () => 0,
        sleep: () => Promise.resolve(),
      }),
    ).rejects.toBe(noInbound);

    expect(start).toHaveBeenCalledTimes(3);
    expect(observedDeadlines).toEqual([90_000, 90_000, 90_000]);
  });

  it('grants one fourth fresh node set after deep route progress', async () => {
    let clockMs = 0;
    const observedBudgets: AttemptBudget[] = [];
    const routeFailure = codedConnectionError('E_PRIVATE_ROUTE_FAILED');
    const noInbound = codedConnectionError('E_ENTRY_NO_INBOUND_BYTES');
    const start = jest.fn(async (budget: AttemptBudget) => {
      observedBudgets.push(budget);
      switch (observedBudgets.length) {
        case 1:
          clockMs += 10_000;
          throw routeFailure;
        case 2:
        case 3:
          clockMs += 25_000;
          throw noInbound;
        default:
          return connected;
      }
    });
    const sleep = jest.fn(async (milliseconds: number) => {
      clockMs += milliseconds;
    });
    const onAttempt = jest.fn();

    await expect(
      startWithEntryNodeRefresh(start, {
        now: () => clockMs,
        onAttempt,
        sleep,
      }),
    ).resolves.toBe(connected);

    expect(start).toHaveBeenCalledTimes(4);
    expect(observedBudgets).toEqual([
      {
        deadlineAtMs: 90_000,
        remainingMs: 90_000,
        readinessTimeoutMs: 35_000,
      },
      {
        deadlineAtMs: 120_000,
        remainingMs: 108_500,
        readinessTimeoutMs: 35_000,
      },
      {
        deadlineAtMs: 120_000,
        remainingMs: 83_500,
        readinessTimeoutMs: 35_000,
      },
      {
        deadlineAtMs: 120_000,
        remainingMs: 58_500,
        readinessTimeoutMs: 35_000,
      },
    ]);
    expect(sleep.mock.calls).toEqual([[1_500], [0], [0]]);
    expect(onAttempt.mock.calls).toEqual([
      [{ attempt: 1, maxAttempts: 3, stage: 'discovery' }],
      [{ attempt: 2, maxAttempts: 4, stage: 'discovery' }],
      [{ attempt: 3, maxAttempts: 4, stage: 'discovery' }],
      [{ attempt: 4, maxAttempts: 4, stage: 'discovery' }],
    ]);
  });

  it('grants a clipped third attempt when deep progress arrives near the soft deadline', async () => {
    let clockMs = 0;
    const observedBudgets: AttemptBudget[] = [];
    const noInbound = codedConnectionError('E_ENTRY_NO_INBOUND_BYTES');
    const routeTimeout = codedConnectionError('E_PRIVATE_ROUTE_TIMEOUT');
    const start = jest.fn(async (budget: AttemptBudget) => {
      observedBudgets.push(budget);
      if (observedBudgets.length === 1) {
        clockMs += 35_000;
        throw noInbound;
      }
      if (observedBudgets.length === 2) {
        clockMs += 35_000;
        throw routeTimeout;
      }
      return connected;
    });
    const sleep = jest.fn(async (milliseconds: number) => {
      clockMs += milliseconds;
    });

    await expect(
      startWithEntryNodeRefresh(start, {
        now: () => clockMs,
        sleep,
      }),
    ).resolves.toBe(connected);

    expect(start).toHaveBeenCalledTimes(3);
    expect(observedBudgets).toEqual([
      {
        deadlineAtMs: 90_000,
        remainingMs: 90_000,
        readinessTimeoutMs: 35_000,
      },
      {
        deadlineAtMs: 90_000,
        remainingMs: 55_000,
        readinessTimeoutMs: 35_000,
      },
      {
        deadlineAtMs: 120_000,
        remainingMs: 47_000,
        readinessTimeoutMs: 35_000,
      },
    ]);
    expect(sleep.mock.calls).toEqual([[0], [3_000]]);
    expect(clockMs).toBe(73_000);
  });

  it('rotates away from a silent first entry set after one bounded slice', async () => {
    let clockMs = 0;
    const observedBudgets: AttemptBudget[] = [];
    const timeout = codedConnectionError('E_ENTRY_GOSSIP_TIMEOUT');
    const start = jest.fn(async (budget: AttemptBudget) => {
      observedBudgets.push(budget);
      if (observedBudgets.length === 1) {
        clockMs += 35_000;
        throw timeout;
      }
      return connected;
    });
    const sleep = jest.fn(async (milliseconds: number) => {
      clockMs += milliseconds;
    });

    await expect(
      startWithEntryNodeRefresh(start, {
        now: () => clockMs,
        sleep,
      }),
    ).resolves.toBe(connected);

    expect(observedBudgets).toEqual([
      {
        deadlineAtMs: 90_000,
        remainingMs: 90_000,
        readinessTimeoutMs: 35_000,
      },
      {
        deadlineAtMs: 90_000,
        remainingMs: 55_000,
        readinessTimeoutMs: 35_000,
      },
    ]);
    expect(sleep).toHaveBeenCalledWith(0);
  });

  it('uses the remaining normal budget for one clipped third entry attempt', async () => {
    let clockMs = 0;
    const observedBudgets: AttemptBudget[] = [];
    const timeout = codedConnectionError('E_ENTRY_GOSSIP_TIMEOUT');
    const start = jest.fn(async (budget: AttemptBudget) => {
      observedBudgets.push(budget);
      clockMs = Math.min(
        clockMs + budget.readinessTimeoutMs,
        budget.deadlineAtMs,
      );
      throw timeout;
    });
    const sleep = jest.fn(async (milliseconds: number) => {
      clockMs += milliseconds;
    });

    await expect(
      startWithEntryNodeRefresh(start, {
        now: () => clockMs,
        sleep,
      }),
    ).rejects.toBe(timeout);

    expect(observedBudgets).toEqual([
      {
        deadlineAtMs: 90_000,
        remainingMs: 90_000,
        readinessTimeoutMs: 35_000,
      },
      {
        deadlineAtMs: 90_000,
        remainingMs: 55_000,
        readinessTimeoutMs: 35_000,
      },
      {
        deadlineAtMs: 90_000,
        remainingMs: 20_000,
        readinessTimeoutMs: 35_000,
      },
    ]);
    expect(sleep.mock.calls).toEqual([[0], [0]]);
    expect(clockMs).toBe(90_000);
    expect(clockMs).toBeLessThanOrEqual(90_000);
  });

  it('clips a tiny caller-owned timeout without starting another node set', async () => {
    let clockMs = 0;
    const observedBudgets: AttemptBudget[] = [];
    const timeout = codedConnectionError('E_ENTRY_GOSSIP_TIMEOUT');
    const start = jest.fn(async (budget: AttemptBudget) => {
      observedBudgets.push(budget);
      clockMs = budget.deadlineAtMs;
      throw timeout;
    });
    const sleep = jest.fn(async (milliseconds: number) => {
      clockMs += milliseconds;
    });

    await expect(
      startWithEntryNodeRefresh(start, {
        now: () => clockMs,
        overallTimeoutMs: 5_000,
        sleep,
      }),
    ).rejects.toMatchObject({ code: 'E_CONNECTION_BUDGET_EXHAUSTED' });

    expect(observedBudgets).toEqual([
      {
        deadlineAtMs: 5_000,
        remainingMs: 5_000,
        readinessTimeoutMs: 35_000,
      },
    ]);
    expect(start).toHaveBeenCalledTimes(1);
    expect(sleep).not.toHaveBeenCalled();
    expect(clockMs).toBe(5_000);
  });

  it('can grant the fourth attempt when deep progress arrives near the soft deadline with custom timing', async () => {
    let clockMs = 0;
    const observedRemaining: number[] = [];
    const noInbound = codedConnectionError('E_ENTRY_NO_INBOUND_BYTES');
    const routeTimeout = codedConnectionError('E_PRIVATE_ROUTE_TIMEOUT');
    const start = jest.fn(async ({ remainingMs }: { remainingMs: number }) => {
      observedRemaining.push(remainingMs);
      if (observedRemaining.length <= 2) {
        clockMs += 29_000;
        throw noInbound;
      }
      if (observedRemaining.length === 3) {
        clockMs += 27_000;
        throw routeTimeout;
      }
      return connected;
    });
    const sleep = jest.fn(async (milliseconds: number) => {
      clockMs += milliseconds;
    });

    await expect(
      startWithEntryNodeRefresh(start, {
        now: () => clockMs,
        sleep,
      }),
    ).resolves.toBe(connected);

    expect(start).toHaveBeenCalledTimes(4);
    expect(observedRemaining).toEqual([90_000, 61_000, 32_000, 29_000]);
    expect(sleep.mock.calls).toEqual([[0], [0], [6_000]]);
    expect(clockMs).toBeLessThanOrEqual(120_000);
  });

  it('never stacks deep-progress credits beyond four attempts and 120 seconds', async () => {
    const routeFailure = codedConnectionError('E_PRIVATE_ROUTE_FAILED');
    const observedDeadlines: number[] = [];
    const start = jest.fn(
      async ({ deadlineAtMs }: { deadlineAtMs: number }) => {
        observedDeadlines.push(deadlineAtMs);
        throw routeFailure;
      },
    );

    await expect(
      startWithEntryNodeRefresh(start, {
        baseDelayMs: 0,
        now: () => 0,
        sleep: () => Promise.resolve(),
      }),
    ).rejects.toBe(routeFailure);

    expect(start).toHaveBeenCalledTimes(4);
    expect(observedDeadlines).toEqual([90_000, 120_000, 120_000, 120_000]);
  });

  it('honours an explicit attempt cap', async () => {
    const routeFailure = codedConnectionError('E_PRIVATE_ROUTE_FAILED');
    const start = jest.fn().mockRejectedValue(routeFailure);

    await expect(
      startWithEntryNodeRefresh(start, {
        baseDelayMs: 0,
        maxAttempts: 2,
        now: () => 0,
        sleep: () => Promise.resolve(),
      }),
    ).rejects.toBe(routeFailure);

    expect(start).toHaveBeenCalledTimes(2);
  });

  it('honours an explicit overall timeout cap', async () => {
    const routeFailure = codedConnectionError('E_PRIVATE_ROUTE_FAILED');
    const observedDeadlines: number[] = [];
    const start = jest.fn(
      async ({ deadlineAtMs }: { deadlineAtMs: number }) => {
        observedDeadlines.push(deadlineAtMs);
        throw routeFailure;
      },
    );

    await expect(
      startWithEntryNodeRefresh(start, {
        baseDelayMs: 0,
        now: () => 0,
        overallTimeoutMs: 90_000,
        sleep: () => Promise.resolve(),
      }),
    ).rejects.toBe(routeFailure);

    expect(start).toHaveBeenCalledTimes(3);
    expect(observedDeadlines).toEqual([90_000, 90_000, 90_000]);
  });

  it('stops during the bonus backoff when the connection is cancelled', async () => {
    const controller = new AbortController();
    const routeFailure = codedConnectionError('E_PRIVATE_ROUTE_FAILED');
    const start = jest.fn().mockRejectedValue(routeFailure);
    const sleep = jest.fn().mockImplementation(async () => {
      controller.abort();
    });

    await expect(
      startWithEntryNodeRefresh(start, {
        signal: controller.signal,
        sleep,
      }),
    ).rejects.toMatchObject({ name: 'AbortError' });

    expect(start).toHaveBeenCalledTimes(1);
    expect(sleep).toHaveBeenCalledTimes(1);
  });

  it('honours cancellation during a zero-delay structured entry rotation', async () => {
    const controller = new AbortController();
    const start = jest
      .fn()
      .mockRejectedValue(codedConnectionError('E_ENTRY_NO_PROGRESS'));
    const sleep = jest.fn().mockImplementation(async () => {
      controller.abort();
    });

    await expect(
      startWithEntryNodeRefresh(start, {
        signal: controller.signal,
        sleep,
      }),
    ).rejects.toMatchObject({ name: 'AbortError' });

    expect(start).toHaveBeenCalledTimes(1);
    expect(sleep.mock.calls).toEqual([[0]]);
  });

  it('does not start when the connection attempt was already cancelled', async () => {
    const controller = new AbortController();
    controller.abort();
    const start = jest.fn().mockResolvedValue(connected);

    await expect(
      startWithEntryNodeRefresh(start, { signal: controller.signal }),
    ).rejects.toMatchObject({ name: 'AbortError' });
    expect(start).not.toHaveBeenCalled();
  });

  it('stops retrying when cancellation happens during backoff', async () => {
    const controller = new AbortController();
    const start = jest.fn().mockRejectedValue(discoveryError());
    const sleep = jest.fn().mockImplementation(async () => {
      controller.abort();
    });

    await expect(
      startWithEntryNodeRefresh(start, {
        signal: controller.signal,
        sleep,
      }),
    ).rejects.toMatchObject({ name: 'AbortError' });
    expect(start).toHaveBeenCalledTimes(1);
    expect(sleep).toHaveBeenCalledTimes(1);
  });
});
