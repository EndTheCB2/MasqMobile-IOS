import {
  appliedSystemTunnelScope,
  appliedSystemTunnelScopeKey,
  decodeRoutableApps,
  decodeSystemTunnelStatus,
  desiredSystemTunnelScope,
  desiredSystemTunnelScopeKey,
  isVerifiedSystemTunnelRoute,
  systemTunnelTrafficDisposition,
} from '../src/core/systemTunnel';

const { readFileSync } = require('fs');
const path = require('path');

describe('system tunnel native decoding', () => {
  it('accepts active selected-app routing', () => {
    expect(
      decodeSystemTunnelStatus(
        JSON.stringify({
          active: true,
          lastError: null,
          mode: 'selectedApps',
          phase: 'active',
          selectedApps: ['org.example.browser'],
          supported: true,
        }),
      ).mode,
    ).toBe('selectedApps');
  });

  it('accepts a privacy-minimized app inventory', () => {
    expect(
      decodeRoutableApps(
        JSON.stringify([{ id: 'org.example.browser', label: 'Browser' }]),
      ),
    ).toEqual([{ id: 'org.example.browser', label: 'Browser' }]);
  });

  it('rejects malformed tunnel state', () => {
    expect(() =>
      decodeSystemTunnelStatus(JSON.stringify({ supported: true })),
    ).toThrow('invalid data');
  });

  it('accepts the versioned composite status through the legacy wire fields', () => {
    const fixture = readFileSync(
      path.resolve(
        __dirname,
        '../android/app/src/test/resources/system-routing-status-v2.json',
      ),
      'utf8',
    ).trim();

    const decoded = decodeSystemTunnelStatus(fixture);
    const versioned = JSON.parse(fixture);

    expect(decoded).toMatchObject({
      active: true,
      appliedMode: 'selectedApps',
      appliedRevision: 8,
      appliedSelectedApps: ['Com.Example.Video', 'org.example.browser'],
      lastError: null,
      mode: 'selectedApps',
      phase: 'active',
      selectedApps: ['Com.Example.Video', 'org.example.browser'],
      supported: true,
      trafficDisposition: 'masq',
    });
    expect(versioned.schemaVersion).toBe(2);
    expect(versioned.routingPhase).toBe('active');
    expect(versioned.trafficDisposition).toBe('masq');
    expect(decoded.trafficObserved).toBe(true);
    expect(isVerifiedSystemTunnelRoute(decoded)).toBe(true);
  });

  it.each(['masq', 'blocked', 'directRisk', 'off'] as const)(
    'preserves the native %s traffic disposition',
    trafficDisposition => {
      const active = trafficDisposition === 'masq';
      const mode =
        trafficDisposition === 'off' || trafficDisposition === 'blocked'
          ? 'off'
          : 'wholeDevice';
      expect(
        decodeSystemTunnelStatus(
          JSON.stringify({
            active,
            lastError: null,
            mode,
            phase: active
              ? 'active'
              : trafficDisposition === 'off'
                ? 'off'
                : 'blocked',
            selectedApps: [],
            supported: true,
            trafficDisposition,
          }),
        ).trafficDisposition,
      ).toBe(trafficDisposition);
    },
  );

  it('rejects missing or unknown dispositions in schema-v2 status', () => {
    const base = {
      active: false,
      lastError: null,
      mode: 'off',
      phase: 'off',
      schemaVersion: 2,
      selectedApps: [],
      supported: true,
    };
    expect(() =>
      decodeSystemTunnelStatus(JSON.stringify(base)),
    ).toThrow('invalid data');
    expect(() =>
      decodeSystemTunnelStatus(
        JSON.stringify({ ...base, trafficDisposition: 'unknown' }),
      ),
    ).toThrow('invalid data');
  });

  it.each(['blocked', 'stopping'] as const)(
    'never infers MASQ from a legacy active=%s lifecycle status',
    phase => {
      const status = decodeSystemTunnelStatus(
        JSON.stringify({
          active: true,
          lastError: null,
          mode: 'wholeDevice',
          phase,
          selectedApps: [],
          supported: true,
        }),
      );
      expect(systemTunnelTrafficDisposition(status)).toBe('blocked');
      expect(systemTunnelTrafficDisposition(status)).not.toBe('masq');
    },
  );

  it('uses applied revision and scope as the stable draft identity', () => {
    const first = decodeSystemTunnelStatus(
      JSON.stringify({
        active: false,
        appliedMode: 'selectedApps',
        appliedRevision: 7,
        appliedSelectedApps: ['org.example.video', 'org.example.browser'],
        lastError: null,
        mode: 'wholeDevice',
        phase: 'starting',
        selectedApps: [],
        supported: true,
      }),
    );
    const reorderedPoll = {
      ...first,
      appliedSelectedApps: ['org.example.browser', 'org.example.video'],
      lastError: 'A polling diagnostic.',
    };

    expect(appliedSystemTunnelScope(first)).toEqual({
      mode: 'selectedApps',
      revision: 7,
      selectedApps: ['org.example.video', 'org.example.browser'],
    });
    expect(appliedSystemTunnelScopeKey(reorderedPoll)).toBe(
      appliedSystemTunnelScopeKey(first),
    );
    expect(
      appliedSystemTunnelScopeKey({
        ...reorderedPoll,
        appliedRevision: 8,
      }),
    ).not.toBe(appliedSystemTunnelScopeKey(first));
  });

  it('uses desired scope for the editable draft while applied capture changes', () => {
    const starting = decodeSystemTunnelStatus(
      JSON.stringify({
        active: false,
        alwaysOn: false,
        appliedMode: 'off',
        appliedRevision: null,
        appliedSelectedApps: [],
        coreRouteReady: false,
        desiredMode: 'wholeDevice',
        desiredRevision: 12,
        desiredSelectedApps: [],
        failClosedDesired: false,
        lastError: null,
        lockdown: false,
        mode: 'wholeDevice',
        phase: 'starting',
        routingPhase: 'startingBlocking',
        schemaVersion: 2,
        selectedApps: [],
        supported: true,
        trafficDisposition: 'directRisk',
        trafficObserved: false,
        translatorReady: false,
        tunPresent: false,
      }),
    );
    const captured = {
      ...starting,
      appliedMode: 'wholeDevice' as const,
      appliedRevision: 12,
      routingPhase: 'reconnecting' as const,
      trafficDisposition: 'blocked' as const,
      tunPresent: true,
    };

    expect(desiredSystemTunnelScope(starting)).toEqual({
      mode: 'wholeDevice',
      revision: 12,
      selectedApps: [],
    });
    expect(desiredSystemTunnelScopeKey(captured)).toBe(
      desiredSystemTunnelScopeKey(starting),
    );
    expect(appliedSystemTunnelScopeKey(captured)).not.toBe(
      appliedSystemTunnelScopeKey(starting),
    );
  });

  it('downgrades an inconsistent native MASQ claim to blocked or direct risk', () => {
    const fixture = JSON.parse(
      readFileSync(
        path.resolve(
          __dirname,
          '../android/app/src/test/resources/system-routing-status-v2.json',
        ),
        'utf8',
      ),
    );
    const translatorStopped = decodeSystemTunnelStatus(
      JSON.stringify({ ...fixture, translatorReady: false }),
    );
    const captureGone = decodeSystemTunnelStatus(
      JSON.stringify({ ...fixture, tunPresent: false }),
    );

    expect(isVerifiedSystemTunnelRoute(translatorStopped)).toBe(false);
    expect(systemTunnelTrafficDisposition(translatorStopped)).toBe('blocked');
    expect(systemTunnelTrafficDisposition(captureGone)).toBe('directRisk');
  });

  it('rejects an incomplete applied native scope', () => {
    expect(() =>
      decodeSystemTunnelStatus(
        JSON.stringify({
          active: false,
          appliedRevision: 3,
          lastError: null,
          mode: 'off',
          phase: 'off',
          selectedApps: [],
          supported: true,
        }),
      ),
    ).toThrow('invalid data');
  });
});
