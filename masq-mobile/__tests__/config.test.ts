import {
  isDescriptorForChain,
  isValidSavedConfig,
  normalizeBrowserUrl,
  normalizeWalletSecret,
  parseNeighborList,
  validateConfig,
} from '../src/core/config';
import {
  decodeSavedConfiguration,
  isSavedProfileError,
  SavedProfileError,
} from '../src/core/masqCore';
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

  it('requires a normalized, current and semantically valid saved profile', () => {
    const saved = {
      configVersion: 2,
      chain: 'base-mainnet' as const,
      rpcUrl: 'https://rpc.example',
      neighbors: [descriptor],
      minHops: 4,
      exitCountry: 'NL',
      exitCountryFallback: false,
    };

    expect(isValidSavedConfig(saved)).toBe(true);
    expect(isValidSavedConfig({ ...saved, configVersion: 1 })).toBe(false);
    expect(isValidSavedConfig({ ...saved, rpcUrl: ' http://rpc.example' })).toBe(
      false,
    );
    expect(isValidSavedConfig({ ...saved, neighbors: [] })).toBe(false);
    expect(isValidSavedConfig({ ...saved, minHops: 0 })).toBe(false);
    expect(isValidSavedConfig({ ...saved, exitCountry: 'Netherlands' })).toBe(
      false,
    );
    expect(decodeSavedConfiguration(JSON.stringify(saved))).toEqual(saved);
  });

  it('normalizes malformed serialized profiles to a stable coded error', () => {
    for (const serialized of [
      '{not-json',
      JSON.stringify({ configVersion: 2 }),
    ]) {
      try {
        decodeSavedConfiguration(serialized);
        throw new Error('Expected the saved profile to be rejected.');
      } catch (caught) {
        expect(caught).toBeInstanceOf(SavedProfileError);
        expect(isSavedProfileError(caught)).toBe(true);
        expect((caught as SavedProfileError).code).toBe(
          'E_SAVED_CONFIG_INVALID',
        );
      }
    }
    expect(isSavedProfileError({ code: 'E_SAVED_CONFIG' })).toBe(true);
    expect(isSavedProfileError({ code: 'E_SAVED_CONFIG_INVALID' })).toBe(true);
    expect(isSavedProfileError({ code: 'E_BRIDGE_TIMEOUT' })).toBe(false);
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
