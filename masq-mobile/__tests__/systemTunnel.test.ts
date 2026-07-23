import {
  decodeRoutableApps,
  decodeSystemTunnelStatus,
} from '../src/core/systemTunnel';

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
});
