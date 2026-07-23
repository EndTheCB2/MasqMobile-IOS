import {
  isDescriptorForChain,
  normalizeBrowserUrl,
  normalizeWalletSecret,
  parseNeighborList,
  validateConfig,
} from '../src/core/config';
import { DEFAULT_SETUP } from '../src/core/types';

describe('mobile consume configuration', () => {
  const descriptor =
    'masq://base-mainnet:ZjPLnb9RrgsRM1D9edqH8jx9DkbPZSWqqFqLnmdKhsk@example.org:4433';

  it('uses the MASQ-documented Nodies endpoint by default', () => {
    expect(DEFAULT_SETUP.rpcUrl).toBe('https://base-pokt.nodies.app');
  });

  it('accepts descriptors only for the configured chain', () => {
    expect(isDescriptorForChain(descriptor, 'base-mainnet')).toBe(true);
    expect(isDescriptorForChain(descriptor, 'base-sepolia')).toBe(false);
  });

  it('accepts MASQ descriptors that advertise multiple entry ports', () => {
    expect(
      isDescriptorForChain(`${descriptor}/4434/4435`, 'base-mainnet'),
    ).toBe(true);
  });

  it('splits newline and comma separated entry nodes', () => {
    expect(parseNeighborList(` ${descriptor}\n${descriptor}, `)).toEqual([
      descriptor,
      descriptor,
    ]);
  });

  it('requires HTTPS RPC and a 32-byte private key', () => {
    const errors = validateConfig({
      chain: 'base-mainnet',
      rpcUrl: 'http://rpc.example',
      neighbors: [descriptor],
      minHops: 1,
      exitCountry: null,
      exitCountryFallback: true,
      walletImportMode: 'privateKey',
      walletSecret: 'abc',
    });

    expect(errors.rpcUrl).toMatch(/HTTPS/);
    expect(errors.walletSecret).toMatch(/64/);
  });

  it('accepts and normalizes an English 12-word recovery phrase', () => {
    const draft = {
      chain: 'base-mainnet' as const,
      rpcUrl: 'https://rpc.example',
      neighbors: [descriptor],
      minHops: 3,
      exitCountry: 'BE',
      exitCountryFallback: false,
      walletImportMode: 'seedPhrase' as const,
      walletSecret:
        ' Test   test test test test test test test test test test JUNK ',
    };

    expect(validateConfig(draft)).toEqual({});
    expect(normalizeWalletSecret(draft)).toBe(
      'test test test test test test test test test test test junk',
    );
  });

  it('requires exactly 12 recovery words', () => {
    const errors = validateConfig({
      chain: 'base-mainnet',
      rpcUrl: 'https://rpc.example',
      neighbors: [descriptor],
      minHops: 1,
      exitCountry: null,
      exitCountryFallback: true,
      walletImportMode: 'seedPhrase',
      walletSecret: 'one two three',
    });

    expect(errors.walletSecret).toMatch(/12/);
  });

  it('accepts route length and exit-country preferences', () => {
    const draft = {
      ...DEFAULT_SETUP,
      neighbors: [descriptor],
      minHops: 4,
      exitCountry: 'NL',
      exitCountryFallback: false,
      walletSecret: '',
    };

    expect(validateConfig(draft, { walletRequired: false })).toEqual({});
  });

  it('rejects unsupported hop counts and malformed country codes', () => {
    const errors = validateConfig(
      {
        ...DEFAULT_SETUP,
        neighbors: [descriptor],
        minHops: 7,
        exitCountry: 'Belgium',
        walletSecret: '',
      },
      { walletRequired: false },
    );

    expect(errors.minHops).toMatch(/six/);
    expect(errors.exitCountry).toMatch(/two-letter/);
  });
});

describe('fail-closed browser URL handling', () => {
  it('adds HTTPS to ordinary hostnames', () => {
    expect(normalizeBrowserUrl('example.com')).toBe('https://example.com/');
  });

  it.each([
    'file:///etc/passwd',
    // eslint-disable-next-line no-script-url -- verifies that script URLs are blocked
    'javascript:alert(1)',
    'http://example.com',
    'http://localhost',
    'https://127.0.0.1',
    'https://[fd00::1]',
    'https://[fe80::1]',
    'https://100.64.0.1',
    'http://192.168.1.10',
    'https://service.local',
  ])('rejects non-web or local destination %s', value => {
    expect(() => normalizeBrowserUrl(value)).toThrow();
  });
});
