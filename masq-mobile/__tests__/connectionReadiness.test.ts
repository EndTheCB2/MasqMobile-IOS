import {
  isCoreReadyForSystemRouting,
  startAndAwaitMasqConnection,
} from '../src/core/connectionReadiness';
import { EMPTY_STATUS, type CoreStatus } from '../src/core/types';

function status(overrides: Partial<CoreStatus>): CoreStatus {
  return { ...EMPTY_STATUS, engineAvailable: true, ...overrides };
}

function routeReady(overrides: Partial<CoreStatus> = {}): CoreStatus {
  return status({
    engineGeneration: 1,
    phase: 'connected',
    connectedNeighbors: 1,
    routeStage: 2,
    routeHops: 3,
    proxyPort: 44_443,
    ...overrides,
  });
}

describe('MASQ connection readiness', () => {
  it('requires a peer, route progress, and proxy port for system routing', () => {
    const ready = routeReady();
    expect(isCoreReadyForSystemRouting(ready)).toBe(true);
    expect(
      isCoreReadyForSystemRouting({ ...ready, connectedNeighbors: 0 }),
    ).toBe(false);
    expect(isCoreReadyForSystemRouting({ ...ready, routeStage: 0 })).toBe(
      false,
    );
    expect(isCoreReadyForSystemRouting({ ...ready, routeStage: 1 })).toBe(
      false,
    );
    expect(isCoreReadyForSystemRouting({ ...ready, proxyPort: null })).toBe(
      false,
    );
    expect(
      isCoreReadyForSystemRouting({ ...ready, engineAvailable: false }),
    ).toBe(false);
    expect(
      isCoreReadyForSystemRouting({ ...ready, engineGeneration: 0 }),
    ).toBe(false);
    expect(
      isCoreReadyForSystemRouting({
        ...ready,
        lastError: 'E_PRIVATE_ROUTE_FAILED: stale route proof',
      }),
    ).toBe(false);
  });

  it('keeps stage one connecting and actively verifies an end-to-end route', async () => {
    const onStatus = jest.fn();
    const entryConnected = status({
      phase: 'connecting',
      connectedNeighbors: 1,
      routeStage: 1,
      proxyPort: 44_443,
    });
    const getStatus = jest
      .fn<Promise<CoreStatus>, []>()
      .mockResolvedValueOnce(entryConnected);
    const verifyRoute = jest.fn().mockResolvedValue(routeReady());

    const result = await startAndAwaitMasqConnection(
      () => Promise.resolve(status({ phase: 'connecting' })),
      getStatus,
      { onStatus, sleep: () => Promise.resolve(), verifyRoute },
    );

    expect(result.phase).toBe('connected');
    expect(result.routeStage).toBe(2);
    expect(getStatus).toHaveBeenCalledTimes(1);
    expect(verifyRoute).toHaveBeenCalledTimes(1);
    expect(onStatus).toHaveBeenLastCalledWith(result);
  });

  it('does not accept a legacy connected label while only stage one exists', async () => {
    const dateNow = jest
      .spyOn(Date, 'now')
      .mockReturnValueOnce(1_000)
      .mockReturnValueOnce(1_000)
      .mockReturnValueOnce(36_000);
    const stageOne = routeReady({ routeStage: 1, routeHops: 0 });

    await expect(
      startAndAwaitMasqConnection(
        () => Promise.resolve(stageOne),
        () => Promise.resolve(stageOne),
        { sleep: () => Promise.resolve() },
      ),
    ).rejects.toMatchObject({ code: 'E_PRIVATE_ROUTE_TIMEOUT' });
    dateNow.mockRestore();
  });

  it('retries one observational route-proof failure on the same entry set', async () => {
    const stageOne = status({
      engineGeneration: 7,
      phase: 'connecting',
      connectedNeighbors: 1,
      routeStage: 1,
      proxyPort: 44_443,
    });
    const getStatus = jest.fn().mockResolvedValue(stageOne);
    const sleep = jest.fn().mockResolvedValue(undefined);
    const firstFailure = Object.assign(new Error('TLS probe failed'), {
      code: 'E_PRIVATE_ROUTE_FAILED',
    });
    const verifyRoute = jest
      .fn()
      .mockRejectedValueOnce(firstFailure)
      .mockResolvedValueOnce(routeReady({ engineGeneration: 7 }));

    await expect(
      startAndAwaitMasqConnection(
        () => Promise.resolve(stageOne),
        getStatus,
        { sleep, verifyRoute },
      ),
    ).resolves.toMatchObject({
      engineGeneration: 7,
      phase: 'connected',
      routeStage: 2,
    });

    expect(verifyRoute).toHaveBeenCalledTimes(2);
    expect(getStatus).toHaveBeenCalledTimes(1);
    expect(sleep).toHaveBeenCalledTimes(1);
    expect(sleep).toHaveBeenCalledWith(500);
  });

  it('surfaces a second failed end-to-end route proof without claiming connected', async () => {
    const stageOne = status({
      engineGeneration: 8,
      phase: 'connecting',
      connectedNeighbors: 1,
      routeStage: 1,
      proxyPort: 44_443,
    });
    const getStatus = jest.fn().mockResolvedValue(stageOne);
    const sleep = jest.fn().mockResolvedValue(undefined);
    const verifyRoute = jest
      .fn()
      .mockRejectedValue(new Error('TLS probe failed'));

    await expect(
      startAndAwaitMasqConnection(
        () => Promise.resolve(stageOne),
        getStatus,
        {
          sleep,
          verifyRoute,
        },
      ),
    ).rejects.toMatchObject({ code: 'E_PRIVATE_ROUTE_FAILED' });

    expect(verifyRoute).toHaveBeenCalledTimes(2);
    expect(getStatus).toHaveBeenCalledTimes(1);
    expect(sleep).toHaveBeenCalledTimes(1);
  });

  it('does not retry a non-route error returned by route verification', async () => {
    const stageOne = status({
      engineGeneration: 9,
      phase: 'connecting',
      connectedNeighbors: 1,
      routeStage: 1,
      proxyPort: 44_443,
    });
    const walletError = Object.assign(new Error('Wallet unavailable.'), {
      code: 'E_WALLET_STORAGE_UNREADABLE',
    });
    const verifyRoute = jest.fn().mockRejectedValue(walletError);
    const getStatus = jest.fn();

    await expect(
      startAndAwaitMasqConnection(
        () => Promise.resolve(stageOne),
        getStatus,
        { verifyRoute },
      ),
    ).rejects.toBe(walletError);

    expect(verifyRoute).toHaveBeenCalledTimes(1);
    expect(getStatus).not.toHaveBeenCalled();
  });

  it('surfaces a connected-labelled stage-one terminal error immediately', async () => {
    const terminalStageOne = status({
      engineGeneration: 10,
      phase: 'connected',
      connectedNeighbors: 1,
      routeStage: 1,
      proxyPort: 44_443,
      lastError:
        'E_ENTRY_NO_INBOUND_BYTES: the selected peers returned no bytes.',
    });

    await expect(
      startAndAwaitMasqConnection(
        () => Promise.resolve(terminalStageOne),
        jest.fn(),
      ),
    ).rejects.toMatchObject({ code: 'E_ENTRY_NO_INBOUND_BYTES' });
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

  it('uses a full 35-second JS boundary measured after native start settles', async () => {
    const dateNow = jest
      .spyOn(Date, 'now')
      .mockReturnValueOnce(8_000)
      .mockReturnValueOnce(42_999)
      .mockReturnValueOnce(43_000);
    const start = deferred<CoreStatus>();
    const getStatus = jest
      .fn<Promise<CoreStatus>, []>()
      .mockResolvedValue(status({ phase: 'connecting' }));
    const connection = startAndAwaitMasqConnection(
      () => start.promise,
      getStatus,
      { sleep: () => Promise.resolve() },
    );

    expect(dateNow).not.toHaveBeenCalled();
    start.resolve(status({ phase: 'connecting' }));
    await expect(connection).rejects.toMatchObject({
      code: 'E_ENTRY_GOSSIP_TIMEOUT',
    });
    expect(getStatus).toHaveBeenCalledTimes(1);
    expect(dateNow).toHaveBeenCalledTimes(3);
    dateNow.mockRestore();
  });

  it('rejects a route proof that settles after the outer deadline', async () => {
    const dateNow = jest
      .spyOn(Date, 'now')
      .mockReturnValueOnce(0)
      .mockReturnValueOnce(0)
      .mockReturnValueOnce(0)
      .mockReturnValueOnce(10_000)
      .mockReturnValueOnce(10_000)
      .mockReturnValueOnce(20_001);
    const stageOne = status({
      phase: 'connecting',
      connectedNeighbors: 1,
      routeStage: 1,
      proxyPort: 44_443,
    });
    const verifyRoute = jest.fn().mockResolvedValue(routeReady());

    await expect(
      startAndAwaitMasqConnection(() => Promise.resolve(stageOne), jest.fn(), {
        deadlineAtMs: 20_000,
        verifyRoute,
      }),
    ).rejects.toMatchObject({ code: 'E_PRIVATE_ROUTE_TIMEOUT' });

    expect(verifyRoute).toHaveBeenCalledTimes(1);
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

  it('honours the outer absolute deadline across native startup and polling', async () => {
    const dateNow = jest
      .spyOn(Date, 'now')
      .mockReturnValueOnce(90_000)
      .mockReturnValueOnce(90_000)
      .mockReturnValueOnce(95_000)
      .mockReturnValueOnce(95_000)
      .mockReturnValueOnce(100_000);
    const stageOne = status({
      phase: 'connecting',
      connectedNeighbors: 1,
      routeStage: 1,
      proxyPort: 44_443,
    });
    const getStatus = jest.fn();

    await expect(
      startAndAwaitMasqConnection(() => Promise.resolve(stageOne), getStatus, {
        deadlineAtMs: 100_000,
      }),
    ).rejects.toMatchObject({ code: 'E_PRIVATE_ROUTE_TIMEOUT' });

    expect(getStatus).not.toHaveBeenCalled();
    dateNow.mockRestore();
  });

  it('hard-times out a native start that never settles', async () => {
    jest.useFakeTimers();
    jest.setSystemTime(10_000);
    const pendingStart = deferred<CoreStatus>();
    const getStatus = jest.fn();
    const onStatus = jest.fn();
    const connection = startAndAwaitMasqConnection(
      () => pendingStart.promise,
      getStatus,
      { deadlineAtMs: 12_000, onStatus },
    );
    const outcome = connection.catch(caught => caught);

    await jest.advanceTimersByTimeAsync(1_999);
    expect(getStatus).not.toHaveBeenCalled();
    expect(onStatus).not.toHaveBeenCalled();
    await jest.advanceTimersByTimeAsync(1);

    await expect(outcome).resolves.toMatchObject({
      code: 'E_ENTRY_NODE_DISCOVERY',
    });
    expect(getStatus).not.toHaveBeenCalled();
    expect(onStatus).not.toHaveBeenCalled();
    expect(jest.getTimerCount()).toBe(0);
    jest.useRealTimers();
  });

  it('ignores a native start status that resolves after its deadline', async () => {
    jest.useFakeTimers();
    jest.setSystemTime(20_000);
    const pendingStart = deferred<CoreStatus>();
    const getStatus = jest.fn();
    const onStatus = jest.fn();
    const connection = startAndAwaitMasqConnection(
      () => pendingStart.promise,
      getStatus,
      { deadlineAtMs: 21_000, onStatus },
    );
    const outcome = connection.catch(caught => caught);

    await jest.advanceTimersByTimeAsync(1_000);
    await expect(outcome).resolves.toMatchObject({
      code: 'E_ENTRY_NODE_DISCOVERY',
    });

    pendingStart.resolve(routeReady());
    await Promise.resolve();
    await Promise.resolve();
    expect(getStatus).not.toHaveBeenCalled();
    expect(onStatus).not.toHaveBeenCalled();
    expect(jest.getTimerCount()).toBe(0);
    jest.useRealTimers();
  });

  it('consumes a late native start rejection after the deadline', async () => {
    jest.useFakeTimers();
    jest.setSystemTime(30_000);
    const pendingStart = deferred<CoreStatus>();
    const onStatus = jest.fn();
    const connection = startAndAwaitMasqConnection(
      () => pendingStart.promise,
      jest.fn(),
      { deadlineAtMs: 31_000, onStatus },
    );
    const outcome = connection.catch(caught => caught);

    await jest.advanceTimersByTimeAsync(1_000);
    await expect(outcome).resolves.toMatchObject({
      code: 'E_ENTRY_NODE_DISCOVERY',
    });

    pendingStart.reject(new Error('Late native rejection.'));
    await Promise.resolve();
    await Promise.resolve();
    expect(onStatus).not.toHaveBeenCalled();
    expect(jest.getTimerCount()).toBe(0);
    jest.useRealTimers();
  });

  it('aborts a never-settling native start and consumes its late rejection', async () => {
    jest.useFakeTimers();
    jest.setSystemTime(40_000);
    const controller = new AbortController();
    const pendingStart = deferred<CoreStatus>();
    const onStatus = jest.fn();
    const connection = startAndAwaitMasqConnection(
      () => pendingStart.promise,
      jest.fn(),
      {
        deadlineAtMs: 50_000,
        onStatus,
        signal: controller.signal,
      },
    );

    controller.abort();
    await expect(connection).rejects.toMatchObject({ name: 'AbortError' });
    expect(jest.getTimerCount()).toBe(0);

    pendingStart.reject(new Error('Late rejection after cancellation.'));
    await Promise.resolve();
    await Promise.resolve();
    expect(onStatus).not.toHaveBeenCalled();
    jest.useRealTimers();
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
    const connected = routeReady();
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
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((fulfill, fail) => {
    resolve = fulfill;
    reject = fail;
  });
  return { promise, reject, resolve };
}
