export const ENTRY_NODE_RETRY_CODES = [
  'E_ENTRY_NODE_DISCOVERY',
  'E_ENTRY_TCP_FAILED',
  'E_ENTRY_TCP_WAITING_GOSSIP',
  'E_ENTRY_GOSSIP_TIMEOUT',
  'E_ENTRY_GOSSIP_PASS_LOOP',
  'E_ENTRY_NO_PROGRESS',
  'E_ENTRY_DEBUT_NOT_WRITTEN',
  'E_ENTRY_NO_INBOUND_BYTES',
  'E_ENTRY_INBOUND_NOT_ACCEPTED',
  'E_ENTRY_GOSSIP_NOT_PROMOTED',
] as const;

export type EntryNodeRetryCode = (typeof ENTRY_NODE_RETRY_CODES)[number];

export const PRIVATE_ROUTE_RETRY_CODES = [
  'E_PRIVATE_ROUTE_FAILED',
  'E_PRIVATE_ROUTE_TIMEOUT',
] as const;

export type PrivateRouteRetryCode = (typeof PRIVATE_ROUTE_RETRY_CODES)[number];

export const NETWORK_TRANSITION_RETRY_CODES = [
  'E_NETWORK_HANDOVER_RETRY',
] as const;

export type NetworkTransitionRetryCode =
  (typeof NETWORK_TRANSITION_RETRY_CODES)[number];

const ENTRY_NODE_RETRY_CODE_SET = new Set<string>(ENTRY_NODE_RETRY_CODES);
const PRIVATE_ROUTE_RETRY_CODE_SET = new Set<string>(PRIVATE_ROUTE_RETRY_CODES);
const NETWORK_TRANSITION_RETRY_CODE_SET = new Set<string>(
  NETWORK_TRANSITION_RETRY_CODES,
);
const MASQ_ERROR_CODE_PATTERN = /\b(E_[A-Z][A-Z0-9_]{1,63})\b/;

export function extractMasqErrorCode(caught: unknown): string | null {
  if (caught && typeof caught === 'object') {
    const code = (caught as { code?: unknown }).code;
    if (typeof code === 'string' && isSafeMasqErrorCode(code)) {
      return code;
    }
  }

  const message = extractMasqErrorMessage(caught);
  return message.match(MASQ_ERROR_CODE_PATTERN)?.[1] ?? null;
}

export function extractMasqErrorMessage(caught: unknown): string {
  if (typeof caught === 'string') return caught;
  if (caught instanceof Error) return caught.message;
  if (caught && typeof caught === 'object') {
    const message = (caught as { message?: unknown }).message;
    if (typeof message === 'string') return message;
  }
  return '';
}

export function isEntryNodeRetryCode(
  code: string | null,
): code is EntryNodeRetryCode {
  return code !== null && ENTRY_NODE_RETRY_CODE_SET.has(code);
}

export function isPrivateRouteRetryCode(
  code: string | null,
): code is PrivateRouteRetryCode {
  return code !== null && PRIVATE_ROUTE_RETRY_CODE_SET.has(code);
}

export function isNetworkTransitionRetryCode(
  code: string | null,
): code is NetworkTransitionRetryCode {
  return code !== null && NETWORK_TRANSITION_RETRY_CODE_SET.has(code);
}

export function isConnectionRetryCode(
  code: string | null,
): code is
  | EntryNodeRetryCode
  | PrivateRouteRetryCode
  | NetworkTransitionRetryCode {
  return (
    isEntryNodeRetryCode(code) ||
    isPrivateRouteRetryCode(code) ||
    isNetworkTransitionRetryCode(code)
  );
}

export function codedMasqError(code: string, message: string): Error {
  return Object.assign(new Error(message), { code });
}

function isSafeMasqErrorCode(code: string): boolean {
  return /^E_[A-Z][A-Z0-9_]{1,63}$/.test(code);
}
