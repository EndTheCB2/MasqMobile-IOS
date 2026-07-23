import { DEFAULT_RPC_URLS, type Chain } from './types';

export const RPC_FALLBACK_URLS: Record<Chain, readonly string[]> = {
  'base-mainnet': [
    DEFAULT_RPC_URLS['base-mainnet'],
    'https://mainnet.base.org',
    'https://base-rpc.publicnode.com',
  ],
  'base-sepolia': [
    DEFAULT_RPC_URLS['base-sepolia'],
    'https://sepolia.base.org',
  ],
};

const EXPECTED_CHAIN_IDS: Record<Chain, string> = {
  'base-mainnet': '0x2105',
  'base-sepolia': '0x14a34',
};

export async function chooseReachableRpc(
  chain: Chain,
  preferred: string,
  fetchImpl: typeof fetch = fetch,
): Promise<string> {
  const candidates = [
    ...new Set([preferred.trim(), ...RPC_FALLBACK_URLS[chain]]),
  ];
  for (const candidate of candidates) {
    if (!candidate) continue;
    const controller = new AbortController();
    const timeout = setTimeout(() => controller.abort(), 4500);
    try {
      const response = await fetchImpl(candidate, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          jsonrpc: '2.0',
          id: 1,
          method: 'eth_chainId',
          params: [],
        }),
        signal: controller.signal,
      });
      if (!response.ok) continue;
      const payload = (await response.json()) as { result?: unknown };
      if (
        typeof payload.result === 'string' &&
        payload.result.toLowerCase() === EXPECTED_CHAIN_IDS[chain]
      ) {
        return candidate;
      }
    } catch {
      // Try the next privacy-conscious public endpoint.
    } finally {
      clearTimeout(timeout);
    }
  }
  throw new Error(
    'No compatible blockchain RPC is reachable. Check the internet connection or enter another HTTPS RPC in Advanced settings.',
  );
}
