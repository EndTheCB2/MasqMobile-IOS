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
});
