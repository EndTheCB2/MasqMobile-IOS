import {
  classifyMasqIssue,
  reconcileMasqIssue,
  type MasqIssue,
} from '../src/core/issues';
import {EMPTY_STATUS, type NetworkStatus} from '../src/core/types';

const online: NetworkStatus = {
  available: true,
  constrained: false,
  expensive: false,
  generation: 1,
  interface: 'wifi',
};

describe('MASQ issue classification', () => {
  it.each([
    ['E_ENTRY_NODE_DISCOVERY', 'No reachable entry nodes', 'entry-nodes', 'retry'],
    ['E_CORE_UNAVAILABLE', 'Native MASQ core is missing', 'native-core', 'none'],
    ['E_KEYSTORE', 'Android Keystore failed', 'wallet', 'wallet'],
    ['E_RPC', 'RPC chain ID mismatch', 'rpc', 'network-profile'],
    ['E_PROXY_STATE', 'Private proxy failed', 'route', 'retry'],
  ])('maps %s to an actionable category', (code, message, category, action) => {
    const result = classifyMasqIssue(
      Object.assign(new Error(message), {code}),
      online,
      EMPTY_STATUS,
    );

    expect(result).toMatchObject({category, action, code});
    expect(result?.message).not.toContain(message);
  });

  it('prioritizes a known offline state over a secondary native error', () => {
    const result = classifyMasqIssue(new Error('request failed'), {
      ...online,
      available: false,
      interface: 'cellular',
    });

    expect(result).toMatchObject({category: 'offline', action: 'settings'});
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
      {...EMPTY_STATUS, phase: 'connected'},
    );

    expect(result).toMatchObject({category: 'route', action: 'none'});
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

  it('clears offline and entry-node issues after observed recovery', () => {
    expect(
      reconcileMasqIssue(issue('offline'), EMPTY_STATUS, {
        ...online,
        available: true,
      }),
    ).toBeNull();
    expect(
      reconcileMasqIssue(
        issue('entry-nodes'),
        {...EMPTY_STATUS, connectedNeighbors: 1, phase: 'connected'},
        online,
      ),
    ).toBeNull();
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
      reconcileMasqIssue(
        routeIssue,
        {
          ...EMPTY_STATUS,
          connectedNeighbors: 1,
          phase: 'connected',
          routeStage: 2,
        },
        online,
      ),
    ).toBeNull();
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
