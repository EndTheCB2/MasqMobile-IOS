import {startAndAwaitMasqConnection} from '../src/core/connectionReadiness';
import {EMPTY_STATUS, type CoreStatus} from '../src/core/types';

function status(overrides: Partial<CoreStatus>): CoreStatus {
  return {...EMPTY_STATUS, engineAvailable: true, ...overrides};
}

describe('MASQ connection readiness', () => {
  it('waits for a confirmed neighbor instead of treating start as connected', async () => {
    const onStatus = jest.fn();
    const getStatus = jest
      .fn<Promise<CoreStatus>, []>()
      .mockResolvedValueOnce(status({phase: 'connecting'}))
      .mockResolvedValueOnce(
        status({phase: 'connected', connectedNeighbors: 1}),
      );

    const result = await startAndAwaitMasqConnection(
      () => Promise.resolve(status({phase: 'connecting'})),
      getStatus,
      {onStatus, sleep: () => Promise.resolve()},
    );

    expect(result.phase).toBe('connected');
    expect(getStatus).toHaveBeenCalledTimes(2);
    expect(onStatus).toHaveBeenLastCalledWith(result);
  });

  it('surfaces the delayed native handshake error so retry can run again', async () => {
    await expect(
      startAndAwaitMasqConnection(
        () => Promise.resolve(status({phase: 'connecting'})),
        () =>
          Promise.resolve(
            status({
              phase: 'connecting',
              lastError: 'The MASQ entry-node handshake failed.',
            }),
          ),
        {sleep: () => Promise.resolve()},
      ),
    ).rejects.toThrow('entry-node handshake failed');
  });

  it('aborts immediately after an awaited native start settles', async () => {
    const controller = new AbortController();
    const onStatus = jest.fn();
    let resolveStart!: (value: CoreStatus) => void;
    const pendingStart = new Promise<CoreStatus>(resolve => {
      resolveStart = resolve;
    });

    const connection = startAndAwaitMasqConnection(
      () => pendingStart,
      jest.fn(),
      {onStatus, signal: controller.signal},
    );
    controller.abort();
    resolveStart(status({phase: 'connecting'}));

    await expect(connection).rejects.toMatchObject({name: 'AbortError'});
    expect(onStatus).not.toHaveBeenCalled();
  });

  it('aborts immediately after an awaited status poll settles', async () => {
    const controller = new AbortController();
    const onStatus = jest.fn();
    const connected = status({
      phase: 'connected',
      connectedNeighbors: 1,
    });
    const getStatus = jest.fn(async () => {
      controller.abort();
      return connected;
    });

    const connection = startAndAwaitMasqConnection(
      () => Promise.resolve(status({phase: 'connecting'})),
      getStatus,
      {
        onStatus,
        signal: controller.signal,
        sleep: () => Promise.resolve(),
      },
    );

    await expect(connection).rejects.toMatchObject({name: 'AbortError'});
    expect(getStatus).toHaveBeenCalledTimes(1);
    expect(onStatus).toHaveBeenCalledTimes(1);
    expect(onStatus).not.toHaveBeenCalledWith(connected);
  });
});
