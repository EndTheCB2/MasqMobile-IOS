import type { CoreStatus, NetworkStatus } from './types';
import { isCoreRouteReady } from './connectionReadiness';
import {
  extractMasqErrorCode,
  extractMasqErrorMessage,
  isEntryNodeRetryCode,
  isNetworkTransitionRetryCode,
  isPrivateRouteRetryCode,
} from './errorCodes';

export type MasqIssueCategory =
  | 'offline'
  | 'permission'
  | 'entry-nodes'
  | 'rpc'
  | 'wallet'
  | 'route'
  | 'native-core'
  | 'unknown';

export type MasqRecoveryAction =
  | 'retry'
  | 'settings'
  | 'network-profile'
  | 'wallet'
  | 'none';

export interface MasqIssue {
  action: MasqRecoveryAction;
  category: MasqIssueCategory;
  code: string | null;
  message: string;
}

export const SAFE_UNKNOWN_ISSUE_SUMMARY =
  'MASQ could not complete the operation. Retry once or share the redacted diagnostics.';

const ENTRY_NODE_SUMMARIES: Partial<Record<string, string>> = {
  E_ENTRY_NODE_DISCOVERY:
    'MASQ could not find two reachable entry nodes. Check the network and retry the automatic refresh.',
  E_ENTRY_TCP_FAILED:
    'MASQ could not open a transport connection to an entry peer. Automatic refresh will try another peer.',
  E_ENTRY_TCP_WAITING_GOSSIP:
    'MASQ reached an entry peer, but its private-network handshake did not finish. Automatic refresh will try another peer.',
  E_ENTRY_GOSSIP_TIMEOUT:
    'MASQ reached an entry peer, but the private-network handshake timed out. Automatic refresh will try another peer.',
  E_ENTRY_GOSSIP_PASS_LOOP:
    'MASQ received handshake traffic, but no entry peer became ready. Automatic refresh will try another peer.',
  E_ENTRY_NO_PROGRESS:
    'MASQ did not observe safe connection progress from an entry peer. Automatic refresh will try another peer.',
  E_ENTRY_DEBUT_NOT_WRITTEN:
    'TCP connected, but MASQ did not write the entry handshake. Automatic refresh will try another peer.',
  E_ENTRY_NO_INBOUND_BYTES:
    'MASQ wrote the entry handshake, but the peer sent no reply bytes. Automatic refresh will try another peer.',
  E_ENTRY_INBOUND_NOT_ACCEPTED:
    'Reply bytes arrived, but MASQ accepted no valid gossip. Automatic refresh will try another peer.',
  E_ENTRY_GOSSIP_NOT_PROMOTED:
    'MASQ accepted gossip, but did not promote the peer. Automatic refresh will try another peer.',
};

export function safeDiagnosticIssueSummary(currentIssue: MasqIssue): string {
  if (currentIssue.code && ENTRY_NODE_SUMMARIES[currentIssue.code]) {
    return ENTRY_NODE_SUMMARIES[currentIssue.code]!;
  }
  if (currentIssue.code === 'E_CORE_STARTUP_FAILED') {
    return 'The embedded MASQ core could not start safely.';
  }
  if (currentIssue.code === 'E_CORE_EARLY_EXIT') {
    return 'The embedded MASQ core stopped before an entry peer was ready.';
  }

  switch (currentIssue.category) {
    case 'offline':
      return 'No internet connection was available.';
    case 'permission':
      return 'MASQ did not have the required network access.';
    case 'entry-nodes':
      return 'MASQ could not complete a safe entry-peer handshake.';
    case 'rpc':
      return 'The blockchain RPC or network profile could not be validated.';
    case 'wallet':
      return 'The consumer wallet requires attention.';
    case 'route':
      return 'MASQ could not complete a safe private route.';
    case 'native-core':
      return 'The native MASQ core was unavailable.';
    case 'unknown':
      return SAFE_UNKNOWN_ISSUE_SUMMARY;
  }
}

export function reconcileMasqIssue(
  current: MasqIssue | null,
  status: CoreStatus,
  network: NetworkStatus,
): MasqIssue | null {
  if (!current) return null;

  const connected = isCoreRouteReady(status);
  switch (current.category) {
    case 'offline':
      return network.available ? null : current;
    case 'entry-nodes':
    case 'permission':
      return connected ? null : current;
    case 'route':
      return connected ? null : current;
    case 'unknown':
      return ['ready', 'connected'].includes(status.phase) && !status.lastError
        ? null
        : current;
    default:
      return current;
  }
}

export function classifyMasqIssue(
  caught: unknown,
  network?: NetworkStatus,
  status?: CoreStatus,
): MasqIssue | null {
  if (caught === null || caught === undefined || isAbort(caught)) {
    return null;
  }

  const code = extractMasqErrorCode(caught);
  const technicalMessage = extractMasqErrorMessage(caught);
  const searchable = `${code || ''} ${technicalMessage}`.toLowerCase();

  if (network && !network.available && network.interface !== 'unknown') {
    return issue(
      'offline',
      'settings',
      code,
      'No internet connection is available. Reconnect Wi-Fi or mobile data, then try again.',
    );
  }
  if (isNetworkTransitionRetryCode(code)) {
    return issue(
      'route',
      'retry',
      code,
      'The active network changed while MASQ was connecting. MASQ will retry safely on the current network.',
    );
  }
  if (isPrivateRouteRetryCode(code)) {
    return issue(
      'route',
      'retry',
      code,
      code === 'E_PRIVATE_ROUTE_TIMEOUT'
        ? 'MASQ connected to an entry peer, but could not prove a private exit route in time. Retry with refreshed entry nodes.'
        : 'MASQ could not prove an end-to-end private exit route. Retry with refreshed entry nodes.',
    );
  }
  if (code === 'E_CONNECTION_BUDGET_EXHAUSTED') {
    return issue(
      'route',
      'retry',
      code,
      'MASQ could not prove a private route within the bounded connection time. Retry to test fresh entry nodes.',
    );
  }
  if (isEntryNodeRetryCode(code)) {
    return issue(
      'entry-nodes',
      'retry',
      code,
      ENTRY_NODE_SUMMARIES[code] ??
        'MASQ could not complete a safe entry-peer handshake. Retry the automatic refresh.',
    );
  }
  if (
    /core_unavailable|native masq core|core is missing|not included/.test(
      searchable,
    )
  ) {
    return issue(
      'native-core',
      'none',
      code,
      'This installation does not contain the native MASQ core. Install a complete signed build.',
    );
  }
  if (
    /permission|operation not permitted|local network|network access/.test(
      searchable,
    )
  ) {
    return issue(
      'permission',
      'settings',
      code,
      'MASQ does not have the required network access. Review the app permissions in device settings.',
    );
  }
  if (/keystore|keychain|wallet|recovery phrase|private key/.test(searchable)) {
    return issue(
      'wallet',
      'wallet',
      code,
      'The consumer wallet needs attention. Review or re-import it before connecting.',
    );
  }
  if (
    /rpc|chain id|network profile|saved masq configuration/.test(searchable)
  ) {
    return issue(
      'rpc',
      'network-profile',
      code,
      'The blockchain RPC or network profile could not be validated. Review the connection profile.',
    );
  }
  if (code === 'E_CORE_STARTUP_FAILED') {
    return issue(
      'native-core',
      'retry',
      code,
      'The embedded MASQ core could not start safely. Retry the connection.',
    );
  }
  if (code === 'E_CORE_EARLY_EXIT') {
    return issue(
      'route',
      'retry',
      code,
      'The embedded MASQ core stopped before an entry peer was ready. Retry the connection.',
    );
  }
  if (/entry.node|node.finder|reachable entry|e_entry_node/.test(searchable)) {
    return issue(
      'entry-nodes',
      'retry',
      code,
      'MASQ could not find two reachable entry nodes. Check the network and retry the automatic refresh.',
    );
  }
  if (/restart the app|changing active node settings/.test(searchable)) {
    return issue(
      'route',
      'none',
      code,
      'Stop MASQ and restart the app before changing active Node settings.',
    );
  }
  if (
    /proxy|private exit route|not connected|connection was lost|network connection was lost|-1005/.test(
      searchable,
    ) ||
    status?.phase === 'connected'
  ) {
    return issue(
      'route',
      'retry',
      code,
      'The private route was interrupted before MASQ could safely carry browser traffic. Retry the route check.',
    );
  }

  return issue('unknown', 'retry', code, SAFE_UNKNOWN_ISSUE_SUMMARY);
}

function issue(
  category: MasqIssueCategory,
  action: MasqRecoveryAction,
  code: string | null,
  message: string,
): MasqIssue {
  return { action, category, code, message };
}

function isAbort(caught: unknown): boolean {
  return caught instanceof Error && caught.name === 'AbortError';
}
