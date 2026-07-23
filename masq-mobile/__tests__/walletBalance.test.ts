import { fetchWalletBalance, formatUnits } from '../src/core/walletBalance';

function rpcResponse(result: string): Response {
  return {
    ok: true,
    json: async () => ({ jsonrpc: '2.0', id: 1, result }),
  } as Response;
}

describe('consumer wallet balance guardrails', () => {
  it('loads gas, MASQ and a current fee reserve from the configured RPC', async () => {
    const fetchImpl = jest.fn(async (_url: unknown, init?: RequestInit) => {
      const request = JSON.parse(String(init?.body)) as {
        method: string;
        params: Array<{ data?: string }>;
      };
      if (request.method === 'eth_getBalance')
        return rpcResponse('0xde0b6b3a7640000');
      if (request.method === 'eth_gasPrice') return rpcResponse('0x3b9aca00');
      if (request.params[0]?.data === '0x313ce567') return rpcResponse('0x12');
      return rpcResponse('0x4563918244f40000');
    }) as unknown as typeof fetch;

    const balance = await fetchWalletBalance(
      'base-mainnet',
      'https://rpc.example',
      '0x1111111111111111111111111111111111111111',
      { fetchImpl },
    );

    expect(balance).toMatchObject({
      gasBalance: '1',
      gasPriceGwei: '1',
      gasReserve: '0.0001',
      lowGas: false,
      lowMasq: false,
      masqBalance: '5',
    });
    expect(fetchImpl).toHaveBeenCalledTimes(4);
  });

  it('warns when there is no MASQ and not enough gas reserve', async () => {
    const results = ['0x1', '0x3b9aca00', '0x0', '0x12'];
    const fetchImpl = jest
      .fn()
      .mockImplementation(async () =>
        rpcResponse(results.shift()!),
      ) as unknown as typeof fetch;

    const balance = await fetchWalletBalance(
      'base-sepolia',
      'https://rpc.example',
      '0x2222222222222222222222222222222222222222',
      { fetchImpl },
    );

    expect(balance).toMatchObject({ lowGas: true, lowMasq: true });
  });

  it('never accepts a credential-bearing or non-HTTPS RPC', async () => {
    await expect(
      fetchWalletBalance(
        'base-mainnet',
        'http://user:secret@rpc.example',
        '0x1111111111111111111111111111111111111111',
      ),
    ).rejects.toThrow(/HTTPS RPC/);
  });

  it('formats large integer quantities without floating-point loss', () => {
    expect(formatUnits(1234567890123456789n, 18, 6)).toBe('1.234567');
  });
});
