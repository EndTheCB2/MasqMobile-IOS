export {};

const {
  parseArguments,
  parseDynamicSection,
  parseDynamicSymbols,
  parseElfHeader,
  unresolvedStrongSymbols,
  validateDynamicPolicy,
  validateHeader,
} = require('../scripts/verify-android-native-elf');

describe('Android native ELF verifier', () => {
  it('parses ELF identity and rejects the wrong architecture', () => {
    const header = parseElfHeader(`
      Class:                             ELF64
      Machine:                           AArch64
    `);

    expect(header).toEqual({ elfClass: 'ELF64', machine: 'AArch64' });
    expect(() =>
      validateHeader({
        header,
        expectedMachine: 'Advanced Micro Devices X86-64',
        label: 'x86_64/libmasq_mobile_core.so',
      }),
    ).toThrow('unexpected ELF class or machine');
  });

  it('requires only the declared Android system dependencies', () => {
    const dynamic = parseDynamicSection(`
      0x0000000000000001 (NEEDED) Shared library: [libdl.so]
      0x0000000000000001 (NEEDED) Shared library: [libm.so]
      0x0000000000000001 (NEEDED) Shared library: [libc.so]
    `);

    expect(dynamic).toEqual({
      needed: ['libdl.so', 'libm.so', 'libc.so'],
      forbiddenTags: [],
    });
    expect(() =>
      validateDynamicPolicy({
        dynamic,
        allowedDependencies: new Set(['libc.so', 'libdl.so', 'libm.so']),
        label: 'arm64-v8a/libmasq_mobile_core.so',
      }),
    ).not.toThrow();
    expect(() =>
      validateDynamicPolicy({
        dynamic: {
          needed: [...dynamic.needed, 'libcrypto.so'],
          forbiddenTags: [],
        },
        allowedDependencies: new Set(['libc.so', 'libdl.so', 'libm.so']),
        label: 'arm64-v8a/libmasq_mobile_core.so',
      }),
    ).toThrow('unexpected dependency set');
  });

  it.each(['RPATH', 'RUNPATH', 'TEXTREL'])(
    'rejects the %s dynamic tag',
    forbiddenTag => {
      expect(() =>
        validateDynamicPolicy({
          dynamic: {
            needed: ['libc.so'],
            forbiddenTags: [forbiddenTag],
          },
          allowedDependencies: new Set(['libc.so']),
          label: 'native.so',
        }),
      ).toThrow('forbidden dynamic tags');
    },
  );

  it('distinguishes strong unresolved imports from weak optional imports', () => {
    const symbols = parseDynamicSymbols(`
      1: 0000000000000000 0 FUNC GLOBAL DEFAULT UND memcpy@LIBC
      2: 0000000000000000 0 FUNC GLOBAL DEFAULT UND BIO_meth_new
      3: 0000000000000000 0 FUNC WEAK DEFAULT UND getrandom
      4: 0000000000001234 8 FUNC GLOBAL DEFAULT 10 memcpy@@LIBC
    `);

    expect(symbols.strongUndefined).toEqual(
      new Set(['memcpy', 'BIO_meth_new']),
    );
    expect(symbols.defined).toEqual(new Set(['memcpy']));
    expect(
      unresolvedStrongSymbols({
        strongUndefined: symbols.strongUndefined,
        availableSymbols: new Set(['memcpy']),
      }),
    ).toEqual(['BIO_meth_new']);
  });

  it.each(['SSL_new', 'crypto_box_seal', 'sodium_memzero', '__addtf3'])(
    'flags unavailable strong import %s',
    symbol => {
      expect(
        unresolvedStrongSymbols({
          strongUndefined: new Set([symbol]),
          availableSymbols: new Set(['memcpy']),
        }),
      ).toEqual([symbol]);
    },
  );

  it('requires exactly one supported input mode', () => {
    expect(parseArguments(['--jni-dir', '/tmp/jni'])).toEqual({
      apk: '',
      jniDirectory: '/tmp/jni',
    });
    expect(parseArguments(['--apk', '/tmp/app.apk'])).toEqual({
      apk: '/tmp/app.apk',
      jniDirectory: '',
    });
    expect(() => parseArguments([])).toThrow('exactly one');
    expect(() =>
      parseArguments(['--apk', '/tmp/app.apk', '--jni-dir', '/tmp/jni']),
    ).toThrow('exactly one');
  });
});
