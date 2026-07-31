import {
  appliedSystemTunnelScope,
  appliedSystemTunnelScopeKey,
  decodeRoutableApps,
  decodeSystemTunnelStatus,
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
            schemaVersion: 2,
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
