export type Chain = 'base-mainnet' | 'base-sepolia';
export type WalletImportMode = 'seedPhrase' | 'privateKey';
export type ExitCountry = string | null;

export type CorePhase =
  | 'unconfigured'
  | 'ready'
  | 'connecting'
  | 'connected'
  | 'paused'
  | 'stopping'
  | 'blocked'
  | 'error';

export interface MasqConfig {
  configVersion: number;
  chain: Chain;
  rpcUrl: string;
  neighbors: string[];
  minHops: number;
  exitCountry: ExitCountry;
  exitCountryFallback: boolean;
}

export interface CoreStatus {
  phase: CorePhase;
  engineAvailable: boolean;
  engineGeneration: number;
  proxyEnabled: boolean;
  proxyPort: number | null;
  chain: Chain | null;
  walletAddress: string | null;
  connectedNeighbors: number;
  routeStage: number;
  routeHops: number;
  minHops: number;
  exitCountry: ExitCountry;
  exitCountryFallback: boolean;
  availableExitCountries: string[];
  bytesUp: number;
  bytesDown: number;
  lastError: string | null;
}

export interface NetworkStatus {
  available: boolean;
  interface: 'wifi' | 'cellular' | 'wired' | 'other' | 'unknown';
  expensive: boolean;
  constrained: boolean;
  generation: number;
}

export interface DebtSummary {
  totalMasqWei: string;
  creditorCount: number;
  settlementInProgress: boolean;
}

export interface DebtSettlementQuote {
  quoteId: string;
  createdAtUnixSeconds: number;
  expiresAtUnixSeconds: number;
  totalMasqWei: string;
  estimatedL2FeeWei: string;
  masqBalanceWei: string;
  baseEthBalanceWei: string;
  creditorCount: number;
  hasMoreCreditors: boolean;
  feeEstimateIncludesL1DataFee: false;
  requiresDeviceAuthentication: false;
  requiresExplicitConfirmation: true;
}

export type DebtSettlementPhase =
  | 'idle'
  | 'reserved'
  | 'submitted'
  | 'attention'
  | 'failed'
  | 'completed';

export interface DebtSettlementStatus {
  operationId: string | null;
  phase: DebtSettlementPhase;
  totalMasqWei: string;
  estimatedL2FeeWei: string;
  transactionCount: number;
  confirmedTransactionCount: number;
  transactionHashes: string[];
  errorCode: string | null;
}

export interface SetupDraft extends Omit<MasqConfig, 'configVersion'> {
  configVersion?: number;
  walletImportMode: WalletImportMode;
  walletSecret: string;
}

export const DEFAULT_RPC_URLS: Record<Chain, string> = {
  'base-mainnet': 'https://base-pokt.nodies.app',
  'base-sepolia': 'https://base-sepolia-rpc.publicnode.com',
};

export const EMPTY_STATUS: CoreStatus = {
  phase: 'unconfigured',
  engineAvailable: false,
  engineGeneration: 0,
  proxyEnabled: false,
  proxyPort: null,
  chain: null,
  walletAddress: null,
  connectedNeighbors: 0,
  routeStage: 0,
  routeHops: 0,
  minHops: 1,
  exitCountry: null,
  exitCountryFallback: true,
  availableExitCountries: [],
  bytesUp: 0,
  bytesDown: 0,
  lastError: null,
};

export const DEFAULT_SETUP: SetupDraft = {
  configVersion: 2,
  chain: 'base-mainnet',
  rpcUrl: DEFAULT_RPC_URLS['base-mainnet'],
  neighbors: [],
  minHops: 1,
  exitCountry: null,
  exitCountryFallback: true,
  walletImportMode: 'seedPhrase',
  walletSecret: '',
};
