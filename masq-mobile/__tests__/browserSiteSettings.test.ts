import {
  browserSiteHostname,
  decodeBrowserSiteSettings,
} from '../src/core/browserSiteSettings';

describe('browser site settings boundary', () => {
  const serialized = JSON.stringify({
    hostname: 'www.youtube.com',
    mode: 'masq',
    persistentSessionsSupported: true,
    protectionDisabled: false,
    rememberSignIn: true,
  });

  it('strictly decodes a supported native response', () => {
    expect(decodeBrowserSiteSettings(serialized)).toEqual({
      hostname: 'www.youtube.com',
      mode: 'masq',
      persistentSessionsSupported: true,
      protectionDisabled: false,
      rememberSignIn: true,
    });
  });

  it.each([
    '{',
    JSON.stringify({ hostname: 'example.com' }),
    JSON.stringify({
      ...JSON.parse(serialized),
      unexpected: true,
    }),
    JSON.stringify({
      ...JSON.parse(serialized),
      mode: 'blocked',
    }),
    JSON.stringify({
      ...JSON.parse(serialized),
      persistentSessionsSupported: false,
    }),
  ])('rejects malformed or contradictory site settings', value => {
    expect(() => decodeBrowserSiteSettings(value)).toThrow(/site settings/i);
  });

  it('extracts only a safe HTTPS hostname', () => {
    expect(browserSiteHostname('https://www.youtube.com/watch?v=1')).toBe(
      'www.youtube.com',
    );
    expect(() => browserSiteHostname('http://example.com')).toThrow();
    expect(() =>
      browserSiteHostname('https://user:pass@example.com'),
    ).toThrow();
    expect(() => browserSiteHostname('https://localhost')).toThrow();
  });
});
