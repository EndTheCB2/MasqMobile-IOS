import { chooseReachableRpc } from '../src/core/rpcHealth';

describe('RPC fallback selection', () => {
  it('keeps the preferred endpoint when it reports the expected chain', async () => {
    const fetchImpl = jest.fn().mockResolvedValue({
      ok: true,
      json: async () => ({ result: '0x2105' }),
    });

    await expect(
      chooseReachableRpc(
        'base-mainnet',
        'https://preferred.example',
        fetchImpl as unknown as typeof fetch,
      ),
    ).resolves.toBe('https://preferred.example');
    expect(fetchImpl).toHaveBeenCalledTimes(1);
  });

  it('falls back when the preferred endpoint is offline or on the wrong chain', async () => {
    const fetchImpl = jest
      .fn()
      .mockRejectedValueOnce(new Error('offline'))
      .mockResolvedValueOnce({
        ok: true,
        json: async () => ({ result: '0x1' }),
      })
      .mockResolvedValueOnce({
        ok: true,
        json: async () => ({ result: '0x2105' }),
      });

    await expect(
      chooseReachableRpc(
        'base-mainnet',
        'https://preferred.example',
        fetchImpl as unknown as typeof fetch,
      ),
    ).resolves.toBe('https://mainnet.base.org');
  });

  it('rejects when no endpoint reports the selected chain', async () => {
    const fetchImpl = jest.fn().mockResolvedValue({
      ok: true,
      json: async () => ({ result: '0x1' }),
    });
    await expect(
      chooseReachableRpc(
        'base-sepolia',
        'https://wrong.example',
        fetchImpl as unknown as typeof fetch,
      ),
    ).rejects.toThrow('No compatible blockchain RPC');
  });
});
