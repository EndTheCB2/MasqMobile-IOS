import {
  classifyMasqIssue,
  reconcileMasqIssue,
  type MasqIssue,
} from '../src/core/issues';
import { EMPTY_STATUS, type NetworkStatus } from '../src/core/types';

const online: NetworkStatus = {
  available: true,
  constrained: false,
  expensive: false,
  generation: 1,
  interface: 'wifi',
};

describe('MASQ issue classification', () => {
  it.each([
    [
      'E_ENTRY_NODE_DISCOVERY',
      'No reachable entry nodes',
      'entry-nodes',
      'retry',
    ],
    [
      'E_CORE_UNAVAILABLE',
      'Native MASQ core is missing',
      'native-core',
      'none',
    ],
    ['E_KEYSTORE', 'Android Keystore failed', 'wallet', 'wallet'],
    ['E_RPC', 'RPC chain ID mismatch', 'rpc', 'network-profile'],
    ['E_PROXY_STATE', 'Private proxy failed', 'route', 'retry'],
    ['E_PRIVATE_ROUTE_FAILED', 'Private route proof failed', 'route', 'retry'],
    [
      'E_PRIVATE_ROUTE_TIMEOUT',
      'Private route proof timed out',
      'route',
      'retry',
    ],
    [
      'E_CONNECTION_BUDGET_EXHAUSTED',
      'Connection budget exhausted',
      'route',
      'retry',
    ],
  ])('maps %s to an actionable category', (code, message, category, action) => {
    const result = classifyMasqIssue(
      Object.assign(new Error(message), { code }),
      online,
      EMPTY_STATUS,
    );

    expect(result).toMatchObject({ category, action, code });
    expect(result?.message).not.toContain(message);
  });

  it.each([
    [
      'E_ENTRY_DEBUT_NOT_WRITTEN',
      'TCP connected, but MASQ did not write the entry handshake.',
    ],
    [
      'E_ENTRY_NO_INBOUND_BYTES',
      'MASQ wrote the entry handshake, but the peer sent no reply bytes.',
    ],
    [
      'E_ENTRY_INBOUND_NOT_ACCEPTED',
      'Reply bytes arrived, but MASQ accepted no valid gossip.',
    ],
    [
      'E_ENTRY_GOSSIP_NOT_PROMOTED',
      'MASQ accepted gossip, but did not promote the peer.',
    ],
  ])('uses a fixed safe summary for milestone %s', (code, summary) => {
    const privateNativeDetail =
      'private-key material at masq://base-mainnet:key@198.51.100.2:443';
    const result = classifyMasqIssue(
      Object.assign(new Error(privateNativeDetail), { code }),
      online,
      EMPTY_STATUS,
    );

    expect(result).toMatchObject({
      action: 'retry',
      category: 'entry-nodes',
      code,
    });
    expect(result?.message).toContain(summary);
    expect(result?.message).not.toContain(privateNativeDetail);
  });

  it.each([
    ['E_CORE_STARTUP_FAILED', 'native-core'],
    ['E_CORE_EARLY_EXIT', 'route'],
    ['E_NETWORK_HANDOVER_RETRY', 'route'],
  ])('classifies stable core lifecycle code %s', (code, category) => {
    expect(
      classifyMasqIssue(
        Object.assign(new Error('Internal native detail.'), { code }),
        online,
        EMPTY_STATUS,
      ),
    ).toMatchObject({ action: 'retry', category, code });
  });

  it('prioritizes a known offline state over a secondary native error', () => {
    const result = classifyMasqIssue(new Error('request failed'), {
      ...online,
      available: false,
      interface: 'cellular',
    });

    expect(result).toMatchObject({ category: 'offline', action: 'settings' });
  });

  it('does not surface intentional cancellation as an error', () => {
    const aborted = new Error('cancelled');
    aborted.name = 'AbortError';

    expect(classifyMasqIssue(aborted, online)).toBeNull();
  });

  it('does not misclassify the active-settings guard as a broken route', () => {
    const result = classifyMasqIssue(
      new Error('Fully restart the app before changing active Node settings.'),
      online,
      { ...EMPTY_STATUS, phase: 'connected' },
    );

    expect(result).toMatchObject({ category: 'route', action: 'none' });
    expect(result?.message).toContain('changing active Node settings');
  });
});

describe('MASQ issue recovery', () => {
  const issue = (category: MasqIssue['category']): MasqIssue => ({
    action: 'retry',
    category,
    code: null,
    message: 'Temporary problem',
  });
  const routeReady = {
    ...EMPTY_STATUS,
    connectedNeighbors: 1,
    engineAvailable: true,
    engineGeneration: 4,
    phase: 'connected' as const,
    proxyPort: 44_443,
    routeStage: 2,
  };

  it('clears offline and entry-node issues after observed recovery', () => {
    expect(
      reconcileMasqIssue(issue('offline'), EMPTY_STATUS, {
        ...online,
        available: true,
      }),
    ).toBeNull();
    expect(
      reconcileMasqIssue(issue('entry-nodes'), routeReady, online),
    ).toBeNull();
  });

  it('keeps entry-node and permission issues through legacy stage one', () => {
    const legacyStageOne = {
      ...routeReady,
      routeStage: 1,
    };

    (['entry-nodes', 'permission'] as const).forEach(category => {
      const pending = issue(category);
      expect(reconcileMasqIssue(pending, legacyStageOne, online)).toBe(
        pending,
      );
    });
  });

  it('keeps a route issue until an exit route is actually ready', () => {
    const routeIssue = issue('route');
    expect(
      reconcileMasqIssue(
        routeIssue,
        {
          ...EMPTY_STATUS,
          connectedNeighbors: 1,
          phase: 'connected',
          routeStage: 1,
        },
        online,
      ),
    ).toBe(routeIssue);
    expect(
      reconcileMasqIssue(routeIssue, routeReady, online),
    ).toBeNull();
  });

  it.each([
    ['unavailable engine', { engineAvailable: false }],
    ['missing engine generation', { engineGeneration: 0 }],
    ['stale native error', { lastError: 'E_PRIVATE_ROUTE_FAILED: stale' }],
  ])('does not clear route issues for %s', (_label, override) => {
    const pending = issue('route');
    expect(
      reconcileMasqIssue(pending, { ...routeReady, ...override }, online),
    ).toBe(pending);
  });

  it.each(['wallet', 'rpc', 'native-core'] as const)(
    'does not hide an unresolved %s issue',
    category => {
      const persistent = issue(category);
      expect(
        reconcileMasqIssue(
          persistent,
          {
            ...EMPTY_STATUS,
            connectedNeighbors: 1,
            phase: 'connected',
            routeStage: 2,
          },
          online,
        ),
      ).toBe(persistent);
    },
  );
});
