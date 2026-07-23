import { stopMasqSafely } from '../src/core/connectionLifecycle';
import { EMPTY_STATUS } from '../src/core/types';

describe('MASQ connection shutdown', () => {
  it('blocks browser traffic before stopping the core', async () => {
    const calls: string[] = [];
    const core = {
      setBrowserRoutingMode: jest.fn(async () => {
        calls.push('blocked');
        return 'blocked' as const;
      }),
      stop: jest.fn(async () => {
        calls.push('stop');
        return EMPTY_STATUS;
      }),
    };

    await expect(stopMasqSafely(core)).resolves.toBe(EMPTY_STATUS);
    expect(calls).toEqual(['blocked', 'stop']);
    expect(core.setBrowserRoutingMode).toHaveBeenCalledWith('blocked');
  });

  it('still stops the core when browser isolation fails', async () => {
    const core = {
      setBrowserRoutingMode: jest
        .fn()
        .mockRejectedValue(new Error('browser isolation failed')),
      stop: jest.fn().mockResolvedValue(EMPTY_STATUS),
    };

    await expect(stopMasqSafely(core)).resolves.toBe(EMPTY_STATUS);
    expect(core.setBrowserRoutingMode).toHaveBeenCalledWith('blocked');
    expect(core.stop).toHaveBeenCalledTimes(1);
  });
});
