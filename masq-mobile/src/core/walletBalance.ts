import type { Chain } from './types';

const TOKEN_CONTRACTS: Record<Chain, string> = {
  'base-mainnet': '0x45d9c101a3870ca5024582fd788f4e1e8f7971c3',
  'base-sepolia': '0x898e1ce720084a902bc37dd822ed6d6a5f027e10',
};
const BALANCE_OF_SELECTOR = '70a08231';
const DECIMALS_SELECTOR = '313ce567';
const MINIMUM_GAS_RESERVE_UNITS = 100_000n;
const REQUEST_TIMEOUT_MS = 8000;

export interface WalletBalance {
  gasBalance: string;
  gasPriceGwei: string;
  gasReserve: string;
  lowGas: boolean;
  lowMasq: boolean;
  masqBalance: string;
  checkedAt: string;
}

export type WalletBalanceState =
  | { state: 'idle'; value: null; message: null }
  | { state: 'loading'; value: WalletBalance | null; message: null }
  | { state: 'ready'; value: WalletBalance; message: null }
  | { state: 'error'; value: WalletBalance | null; message: string };

interface WalletBalanceOptions {
  fetchImpl?: typeof fetch;
  signal?: AbortSignal;
  timeoutMs?: number;
}

export const EMPTY_WALLET_BALANCE: WalletBalanceState = {
  state: 'idle',
  value: null,
  message: null,
};

export async function fetchWalletBalance(
  chain: Chain,
  rpcUrl: string,
  walletAddress: string,
  options: WalletBalanceOptions = {},
): Promise<WalletBalance> {
  validateInputs(rpcUrl, walletAddress);
  const fetchImpl = options.fetchImpl ?? fetch;
  const addressWord = walletAddress.slice(2).toLowerCase().padStart(64, '0');
  const contract = TOKEN_CONTRACTS[chain];
  const requests = [
    rpcCall(
      fetchImpl,
      rpcUrl,
      'eth_getBalance',
      [walletAddress, 'latest'],
      options,
    ),
    rpcCall(fetchImpl, rpcUrl, 'eth_gasPrice', [], options),
    rpcCall(
      fetchImpl,
      rpcUrl,
      'eth_call',
      [
        { to: contract, data: `0x${BALANCE_OF_SELECTOR}${addressWord}` },
        'latest',
      ],
      options,
    ),
    rpcCall(
      fetchImpl,
      rpcUrl,
      'eth_call',
      [{ to: contract, data: `0x${DECIMALS_SELECTOR}` }, 'latest'],
      options,
    ),
  ] as const;
  const [gasHex, gasPriceHex, masqHex, decimalsHex] = await Promise.all(
    requests,
  );
  const gas = parseQuantity(gasHex);
  const gasPrice = parseQuantity(gasPriceHex);
  const masq = parseQuantity(masqHex);
  const decimals = Number(parseQuantity(decimalsHex));
  if (!Number.isInteger(decimals) || decimals < 0 || decimals > 36) {
    throw new Error('The MASQ token returned invalid decimal metadata.');
  }
  const reserve = gasPrice * MINIMUM_GAS_RESERVE_UNITS;

  return {
    gasBalance: formatUnits(gas, 18, 6),
    gasPriceGwei: formatUnits(gasPrice, 9, 3),
    gasReserve: formatUnits(reserve, 18, 6),
    lowGas: gas < reserve,
    lowMasq: masq === 0n,
    masqBalance: formatUnits(masq, decimals, 6),
    checkedAt: new Date().toISOString(),
  };
}

async function rpcCall(
  fetchImpl: typeof fetch,
  rpcUrl: string,
  method: string,
  params: unknown[],
  options: WalletBalanceOptions,
): Promise<string> {
  const controller = new AbortController();
  const cancel = () => controller.abort();
  options.signal?.addEventListener('abort', cancel, { once: true });
  const timeout = setTimeout(
    () => controller.abort(),
    options.timeoutMs ?? REQUEST_TIMEOUT_MS,
  );
  try {
    const response = await fetchImpl(rpcUrl, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ jsonrpc: '2.0', id: 1, method, params }),
      signal: controller.signal,
    });
    if (!response.ok) {
      throw new Error('The blockchain RPC rejected the balance request.');
    }
    const payload = (await response.json()) as {
      error?: unknown;
      result?: unknown;
    };
    if (payload.error || typeof payload.result !== 'string') {
      throw new Error(
        'The blockchain RPC returned an invalid balance response.',
      );
    }
    return payload.result;
  } finally {
    clearTimeout(timeout);
    options.signal?.removeEventListener('abort', cancel);
  }
}

function validateInputs(rpcUrl: string, walletAddress: string) {
  const parsed = new URL(rpcUrl);
  if (parsed.protocol !== 'https:' || parsed.username || parsed.password) {
    throw new Error('Wallet balances require a credential-free HTTPS RPC.');
  }
  if (!/^0x[a-fA-F0-9]{40}$/.test(walletAddress)) {
    throw new Error('The consumer wallet address is invalid.');
  }
}

function parseQuantity(value: string): bigint {
  if (!/^0x[0-9a-fA-F]+$/.test(value)) {
    throw new Error('The blockchain RPC returned an invalid quantity.');
  }
  return BigInt(value);
}

export function formatUnits(
  value: bigint,
  decimals: number,
  visibleDecimals: number,
): string {
  const base = 10n ** BigInt(decimals);
  const integer = value / base;
  const fraction = (value % base).toString().padStart(decimals, '0');
  const trimmed = fraction.slice(0, visibleDecimals).replace(/0+$/, '');
  return trimmed ? `${integer}.${trimmed}` : integer.toString();
}
