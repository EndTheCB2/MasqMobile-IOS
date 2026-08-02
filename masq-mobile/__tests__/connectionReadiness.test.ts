import {
  isCoreReadyForSystemRouting,
  startAndAwaitMasqConnection,
} from '../src/core/connectionReadiness';
import { EMPTY_STATUS, type CoreStatus } from '../src/core/types';

function status(overrides: Partial<CoreStatus>): CoreStatus {
  return { ...EMPTY_STATUS, engineAvailable: true, ...overrides };
}

describe('MASQ connection readiness', () => {
  it('requires a peer, route progress, and proxy port for system routing', () => {
    const ready = status({
      phase: 'connected',
      connectedNeighbors: 1,
      routeStage: 1,
      proxyPort: 44_443,
    });
    expect(isCoreReadyForSystemRouting(ready)).toBe(true);
    expect(
      isCoreReadyForSystemRouting({ ...ready, connectedNeighbors: 0 }),
    ).toBe(false);
    expect(isCoreReadyForSystemRouting({ ...ready, routeStage: 0 })).toBe(
      false,
    );
    expect(isCoreReadyForSystemRouting({ ...ready, proxyPort: null })).toBe(
      false,
    );
  });

  it('waits for a confirmed neighbor instead of treating start as connected', async () => {
    const onStatus = jest.fn();
    const getStatus = jest
      .fn<Promise<CoreStatus>, []>()
      .mockResolvedValueOnce(status({ phase: 'connecting' }))
      .mockResolvedValueOnce(
        status({ phase: 'connected', connectedNeighbors: 1 }),
      );

    const result = await startAndAwaitMasqConnection(
      () => Promise.resolve(status({ phase: 'connecting' })),
      getStatus,
      { onStatus, sleep: () => Promise.resolve() },
    );

    expect(result.phase).toBe('connected');
    expect(getStatus).toHaveBeenCalledTimes(2);
    expect(onStatus).toHaveBeenLastCalledWith(result);
  });

  it('surfaces the delayed native handshake error so retry can run again', async () => {
    await expect(
      startAndAwaitMasqConnection(
        () => Promise.resolve(status({ phase: 'connecting' })),
        () =>
          Promise.resolve(
            status({
              phase: 'connecting',
              lastError: 'The MASQ entry-node handshake failed.',
            }),
          ),
        { sleep: () => Promise.resolve() },
      ),
    ).rejects.toThrow('entry-node handshake failed');
  });

  it('passes a coded entry diagnostic from native status directly to refresh', async () => {
    const connection = startAndAwaitMasqConnection(
      () => Promise.resolve(status({ phase: 'connecting' })),
      () =>
        Promise.resolve(
          status({
            phase: 'connecting',
            lastError:
              'E_ENTRY_NO_INBOUND_BYTES: the peer sent no response bytes.',
          }),
        ),
      { sleep: () => Promise.resolve() },
    );

    await expect(connection).rejects.toMatchObject({
      code: 'E_ENTRY_NO_INBOUND_BYTES',
    });
  });

  it('assigns a stable startup code when native start returns an error phase', async () => {
    await expect(
      startAndAwaitMasqConnection(
        () =>
          Promise.resolve(
            status({
              phase: 'error',
              lastError: 'The embedded engine rejected startup.',
            }),
          ),
        jest.fn(),
      ),
    ).rejects.toMatchObject({ code: 'E_CORE_STARTUP_FAILED' });
  });

  it('assigns a stable early-exit code when the embedded engine leaves connecting', async () => {
    await expect(
      startAndAwaitMasqConnection(
        () => Promise.resolve(status({ phase: 'connecting' })),
        () => Promise.resolve(status({ phase: 'ready' })),
        { sleep: () => Promise.resolve() },
      ),
    ).rejects.toMatchObject({ code: 'E_CORE_EARLY_EXIT' });
  });

  it('uses a 35-second JS boundary measured after native start settles', async () => {
    const dateNow = jest.spyOn(Date, 'now').mockReturnValueOnce(1000);
    const controller = new AbortController();
    const start = deferred<CoreStatus>();
    const connection = startAndAwaitMasqConnection(
      () => start.promise,
      jest.fn(),
      { signal: controller.signal },
    );

    expect(dateNow).not.toHaveBeenCalled();
    start.resolve(status({ phase: 'connected', connectedNeighbors: 1 }));
    await expect(connection).resolves.toMatchObject({ phase: 'connected' });
    expect(dateNow).toHaveBeenCalledTimes(1);
    dateNow.mockRestore();
  });

  it('reports the JS handshake boundary with a retryable safe code', async () => {
    const dateNow = jest
      .spyOn(Date, 'now')
      .mockReturnValueOnce(1000)
      .mockReturnValueOnce(36_000);

    await expect(
      startAndAwaitMasqConnection(
        () => Promise.resolve(status({ phase: 'connecting' })),
        jest.fn(),
        { sleep: () => Promise.resolve() },
      ),
    ).rejects.toMatchObject({ code: 'E_ENTRY_GOSSIP_TIMEOUT' });
    dateNow.mockRestore();
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
      { onStatus, signal: controller.signal },
    );
    controller.abort();
    resolveStart(status({ phase: 'connecting' }));

    await expect(connection).rejects.toMatchObject({ name: 'AbortError' });
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
      () => Promise.resolve(status({ phase: 'connecting' })),
      getStatus,
      {
        onStatus,
        signal: controller.signal,
        sleep: () => Promise.resolve(),
      },
    );

    await expect(connection).rejects.toMatchObject({ name: 'AbortError' });
    expect(getStatus).toHaveBeenCalledTimes(1);
    expect(onStatus).toHaveBeenCalledTimes(1);
    expect(onStatus).not.toHaveBeenCalledWith(connected);
  });
});

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>(fulfill => {
    resolve = fulfill;
  });
  return { promise, resolve };
}
