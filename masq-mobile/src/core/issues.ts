import type {CoreStatus, NetworkStatus} from './types';

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

export function reconcileMasqIssue(
  current: MasqIssue | null,
  status: CoreStatus,
  network: NetworkStatus,
): MasqIssue | null {
  if (!current) return null;

  const connected =
    status.phase === 'connected' && status.connectedNeighbors > 0;
  switch (current.category) {
    case 'offline':
      return network.available ? null : current;
    case 'entry-nodes':
    case 'permission':
      return connected ? null : current;
    case 'route':
      return connected && status.routeStage >= 2 && !status.lastError
        ? null
        : current;
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

  const code = extractCode(caught);
  const technicalMessage = extractMessage(caught);
  const searchable = `${code || ''} ${technicalMessage}`.toLowerCase();

  if (
    network &&
    !network.available &&
    network.interface !== 'unknown'
  ) {
    return issue(
      'offline',
      'settings',
      code,
      'No internet connection is available. Reconnect Wi-Fi or mobile data, then try again.',
    );
  }
  if (/core_unavailable|native masq core|core is missing|not included/.test(searchable)) {
    return issue(
      'native-core',
      'none',
      code,
      'This installation does not contain the native MASQ core. Install a complete signed build.',
    );
  }
  if (/permission|operation not permitted|local network|network access/.test(searchable)) {
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
  if (/rpc|chain id|network profile|saved masq configuration/.test(searchable)) {
    return issue(
      'rpc',
      'network-profile',
      code,
      'The blockchain RPC or network profile could not be validated. Review the connection profile.',
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
    ) || status?.phase === 'connected'
  ) {
    return issue(
      'route',
      'retry',
      code,
      'The private route was interrupted before MASQ could safely carry browser traffic. Retry the route check.',
    );
  }

  return issue(
    'unknown',
    'retry',
    code,
    'MASQ could not complete the operation. Retry once or share the redacted diagnostics.',
  );
}

function issue(
  category: MasqIssueCategory,
  action: MasqRecoveryAction,
  code: string | null,
  message: string,
): MasqIssue {
  return {action, category, code, message};
}

function extractCode(caught: unknown): string | null {
  if (!caught || typeof caught !== 'object') return null;
  const code = (caught as {code?: unknown}).code;
  return typeof code === 'string' ? code : null;
}

function extractMessage(caught: unknown): string {
  if (typeof caught === 'string') return caught;
  if (caught instanceof Error) return caught.message;
  if (caught && typeof caught === 'object') {
    const message = (caught as {message?: unknown}).message;
    if (typeof message === 'string') return message;
  }
  return '';
}

function isAbort(caught: unknown): boolean {
  return caught instanceof Error && caught.name === 'AbortError';
}
