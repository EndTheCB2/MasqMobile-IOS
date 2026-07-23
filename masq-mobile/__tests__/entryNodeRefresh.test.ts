import {startWithEntryNodeRefresh} from '../src/core/entryNodeRefresh';
import {EMPTY_STATUS, type CoreStatus} from '../src/core/types';

const connected: CoreStatus = {...EMPTY_STATUS, phase: 'connecting'};

function discoveryError(message = 'MASQ could not find two reachable entry nodes.') {
  return Object.assign(new Error(message), {code: 'E_ENTRY_NODE_DISCOVERY'});
}

describe('automatic entry-node refresh', () => {
  it('returns immediately when the first discovery succeeds', async () => {
    const start = jest.fn().mockResolvedValue(connected);
    const onAttempt = jest.fn();

    await expect(startWithEntryNodeRefresh(start, {onAttempt})).resolves.toBe(
      connected,
    );
    expect(start).toHaveBeenCalledTimes(1);
    expect(onAttempt).toHaveBeenCalledWith({attempt: 1, maxAttempts: 6});
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
      [{attempt: 1, maxAttempts: 4}],
      [{attempt: 2, maxAttempts: 4}],
      [{attempt: 3, maxAttempts: 4}],
    ]);
  });

  it('does not retry unrelated native errors', async () => {
    const error = Object.assign(new Error('The native MASQ core is missing.'), {
      code: 'E_CORE_UNAVAILABLE',
    });
    const start = jest.fn().mockRejectedValue(error);
    const sleep = jest.fn().mockResolvedValue(undefined);

    await expect(
      startWithEntryNodeRefresh(start, {sleep}),
    ).rejects.toBe(error);
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

    await expect(
      startWithEntryNodeRefresh(start, {sleep}),
    ).resolves.toBe(connected);
    expect(start).toHaveBeenCalledTimes(2);
  });

  it('returns the final discovery error after all refresh rounds', async () => {
    const error = discoveryError(
      'Automatic refresh found 0 reachable entry nodes.',
    );
    const start = jest.fn().mockRejectedValue(error);
    const sleep = jest.fn().mockResolvedValue(undefined);

    await expect(
      startWithEntryNodeRefresh(start, {maxAttempts: 3, sleep}),
    ).rejects.toBe(error);
    expect(start).toHaveBeenCalledTimes(3);
    expect(sleep).toHaveBeenCalledTimes(2);
  });

  it('does not start when the connection attempt was already cancelled', async () => {
    const controller = new AbortController();
    controller.abort();
    const start = jest.fn().mockResolvedValue(connected);

    await expect(
      startWithEntryNodeRefresh(start, {signal: controller.signal}),
    ).rejects.toMatchObject({name: 'AbortError'});
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
    ).rejects.toMatchObject({name: 'AbortError'});
    expect(start).toHaveBeenCalledTimes(1);
    expect(sleep).toHaveBeenCalledTimes(1);
  });
});
