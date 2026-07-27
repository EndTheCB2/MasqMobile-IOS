#!/usr/bin/env node
'use strict';

const { execFileSync } = require('child_process');
const {
  existsSync,
  mkdtempSync,
  readdirSync,
  rmSync,
  statSync,
} = require('fs');
const { tmpdir } = require('os');
const path = require('path');

const API_LEVEL = '24';
const LIBRARIES = {
  'libmasq_mobile_core.so': new Set(['libc.so', 'libdl.so', 'libm.so']),
  'libmasq_packet_tunnel.so': new Set([
    'libc.so',
    'libdl.so',
    'liblog.so',
    'libm.so',
  ]),
};
const ABIS = {
  'arm64-v8a': {
    machine: 'AArch64',
    triple: 'aarch64-linux-android',
  },
  x86_64: {
    machine: 'Advanced Micro Devices X86-64',
    triple: 'x86_64-linux-android',
  },
};

const normalizeSymbol = symbol => symbol.split('@')[0];

const parseDynamicSection = output => {
  const needed = [];
  const forbiddenTags = [];

  output.split(/\r?\n/).forEach(line => {
    const neededMatch = line.match(/\(NEEDED\).*Shared library: \[([^\]]+)\]/);
    if (neededMatch) {
      needed.push(neededMatch[1]);
    }
    const forbiddenMatch = line.match(/\((RPATH|RUNPATH|TEXTREL)\)/);
    if (forbiddenMatch) {
      forbiddenTags.push(forbiddenMatch[1]);
    }
  });

  return { needed, forbiddenTags };
};

const parseElfHeader = output => {
  const classMatch = output.match(/^\s*Class:\s+(.+)$/m);
  const machineMatch = output.match(/^\s*Machine:\s+(.+)$/m);
  return {
    elfClass: classMatch?.[1].trim() ?? '',
    machine: machineMatch?.[1].trim() ?? '',
  };
};

const parseDynamicSymbols = output => {
  const defined = new Set();
  const strongUndefined = new Set();

  output.split(/\r?\n/).forEach(line => {
    const fields = line.trim().split(/\s+/);
    if (fields.length < 8 || !/^\d+:$/.test(fields[0]) || fields[7] === '') {
      return;
    }

    const bind = fields[4];
    const index = fields[6];
    const symbol = normalizeSymbol(fields[7]);
    if (!symbol || bind === 'LOCAL') {
      return;
    }
    if (index === 'UND') {
      if (bind !== 'WEAK') {
        strongUndefined.add(symbol);
      }
      return;
    }
    defined.add(symbol);
  });

  return { defined, strongUndefined };
};

const validateHeader = ({ header, expectedMachine, label }) => {
  if (header.elfClass !== 'ELF64' || header.machine !== expectedMachine) {
    throw new Error(
      `${label} has unexpected ELF class or machine ` +
        `(${header.elfClass}, ${header.machine}).`,
    );
  }
};

const validateDynamicPolicy = ({ dynamic, allowedDependencies, label }) => {
  if (dynamic.forbiddenTags.length > 0) {
    throw new Error(
      `${label} contains forbidden dynamic tags: ` +
        dynamic.forbiddenTags.join(', '),
    );
  }
  const unexpectedDependencies = dynamic.needed.filter(
    dependency => !allowedDependencies.has(dependency),
  );
  const missingDependencies = [...allowedDependencies].filter(
    dependency => !dynamic.needed.includes(dependency),
  );
  if (unexpectedDependencies.length > 0 || missingDependencies.length > 0) {
    throw new Error(
      `${label} has an unexpected dependency set. ` +
        `Needed: ${dynamic.needed.join(', ') || '(none)'}.`,
    );
  }
};

const unresolvedStrongSymbols = ({ strongUndefined, availableSymbols }) =>
  [...strongUndefined].filter(symbol => !availableSymbols.has(symbol)).sort();

const findNdkRoot = env => {
  const configured = [env.ANDROID_NDK_HOME, env.ANDROID_NDK_ROOT].find(
    candidate => candidate && existsSync(candidate),
  );
  if (configured) {
    return configured;
  }

  const sdkRoot = env.ANDROID_SDK_ROOT || env.ANDROID_HOME;
  const ndkDirectory = sdkRoot ? path.join(sdkRoot, 'ndk') : '';
  if (!ndkDirectory || !existsSync(ndkDirectory)) {
    throw new Error(
      'Set ANDROID_NDK_HOME, ANDROID_NDK_ROOT, ANDROID_SDK_ROOT, or ANDROID_HOME.',
    );
  }
  const versions = readdirSync(ndkDirectory)
    .filter(entry => statSync(path.join(ndkDirectory, entry)).isDirectory())
    .sort((left, right) =>
      left.localeCompare(right, undefined, { numeric: true }),
    );
  if (versions.length === 0) {
    throw new Error(`No Android NDK is installed under ${ndkDirectory}.`);
  }
  return path.join(ndkDirectory, versions[versions.length - 1]);
};

const findLlvmTools = ndkRoot => {
  const prebuiltRoot = path.join(ndkRoot, 'toolchains', 'llvm', 'prebuilt');
  if (!existsSync(prebuiltRoot)) {
    throw new Error(`Invalid Android NDK: ${ndkRoot}.`);
  }
  const toolchain = readdirSync(prebuiltRoot)
    .map(entry => path.join(prebuiltRoot, entry))
    .find(candidate => existsSync(path.join(candidate, 'bin', 'llvm-readelf')));
  if (!toolchain) {
    throw new Error(`Android NDK LLVM tools were not found under ${ndkRoot}.`);
  }
  return {
    readelf: path.join(toolchain, 'bin', 'llvm-readelf'),
    sysroot: path.join(toolchain, 'sysroot'),
  };
};

const runReadelf = (readelf, args, filePath) =>
  execFileSync(readelf, [...args, filePath], {
    encoding: 'utf8',
    maxBuffer: 32 * 1024 * 1024,
  });

const verifyLibrary = ({ abi, libraryName, libraryPath, readelf, sysroot }) => {
  const abiPolicy = ABIS[abi];
  const label = `${abi}/${libraryName}`;
  const header = parseElfHeader(runReadelf(readelf, ['-hW'], libraryPath));
  validateHeader({ header, expectedMachine: abiPolicy.machine, label });

  const dynamic = parseDynamicSection(
    runReadelf(readelf, ['-dW'], libraryPath),
  );
  const allowedDependencies = LIBRARIES[libraryName];
  validateDynamicPolicy({ dynamic, allowedDependencies, label });

  const availableSymbols = new Set();
  dynamic.needed.forEach(dependency => {
    const stubPath = path.join(
      sysroot,
      'usr',
      'lib',
      abiPolicy.triple,
      API_LEVEL,
      dependency,
    );
    if (!existsSync(stubPath)) {
      throw new Error(
        `Android API ${API_LEVEL} stub is missing for ${abi}/${dependency}.`,
      );
    }
    const symbols = parseDynamicSymbols(
      runReadelf(readelf, ['--dyn-syms', '-W'], stubPath),
    );
    symbols.defined.forEach(symbol => availableSymbols.add(symbol));
  });

  const librarySymbols = parseDynamicSymbols(
    runReadelf(readelf, ['--dyn-syms', '-W'], libraryPath),
  );
  const unresolved = unresolvedStrongSymbols({
    strongUndefined: librarySymbols.strongUndefined,
    availableSymbols,
  });
  if (unresolved.length > 0) {
    const visible = unresolved.slice(0, 20).join(', ');
    const suffix =
      unresolved.length > 20 ? ` (+${unresolved.length - 20} more)` : '';
    throw new Error(
      `${abi}/${libraryName} has strong unresolved symbols that Android ` +
        `API ${API_LEVEL} cannot provide: ${visible}${suffix}.`,
    );
  }
};

const verifyJniDirectory = ({ jniDirectory, ndkRoot }) => {
  const { readelf, sysroot } = findLlvmTools(ndkRoot);
  Object.entries(ABIS).forEach(([abi]) => {
    const abiDirectory = path.join(jniDirectory, abi);
    if (!existsSync(abiDirectory)) {
      throw new Error(
        `Missing required native ABI directory: ${abiDirectory}.`,
      );
    }
    const unexpectedLibraries = readdirSync(abiDirectory)
      .filter(entry => entry.endsWith('.so') && !LIBRARIES[entry])
      .sort();
    if (unexpectedLibraries.length > 0) {
      throw new Error(
        `${abi} contains undeclared native dependencies: ` +
          unexpectedLibraries.join(', '),
      );
    }
    Object.keys(LIBRARIES).forEach(libraryName => {
      const libraryPath = path.join(abiDirectory, libraryName);
      if (!existsSync(libraryPath)) {
        throw new Error(`Missing required native library: ${libraryPath}.`);
      }
      verifyLibrary({
        abi,
        libraryName,
        libraryPath,
        readelf,
        sysroot,
      });
    });
  });
};

const parseArguments = argv => {
  const args = { apk: '', jniDirectory: '' };
  for (let index = 0; index < argv.length; index += 1) {
    if (argv[index] === '--apk') {
      args.apk = argv[(index += 1)] ?? '';
    } else if (argv[index] === '--jni-dir') {
      args.jniDirectory = argv[(index += 1)] ?? '';
    } else {
      throw new Error(`Unknown argument: ${argv[index]}.`);
    }
  }
  if (Boolean(args.apk) === Boolean(args.jniDirectory)) {
    throw new Error('Specify exactly one of --apk or --jni-dir.');
  }
  return args;
};

const main = argv => {
  const args = parseArguments(argv);
  const ndkRoot = findNdkRoot(process.env);
  let temporaryDirectory = '';
  let jniDirectory = args.jniDirectory;

  try {
    if (args.apk) {
      if (!existsSync(args.apk)) {
        throw new Error(`APK does not exist: ${args.apk}.`);
      }
      temporaryDirectory = mkdtempSync(
        path.join(tmpdir(), 'masq-android-elf-'),
      );
      const entries = Object.keys(ABIS).flatMap(abi =>
        Object.keys(LIBRARIES).map(libraryName => `lib/${abi}/${libraryName}`),
      );
      execFileSync('unzip', [
        '-q',
        args.apk,
        ...entries,
        '-d',
        temporaryDirectory,
      ]);
      jniDirectory = path.join(temporaryDirectory, 'lib');
    }

    verifyJniDirectory({ jniDirectory, ndkRoot });
    process.stdout.write('Android native ELF linkage checks passed.\n');
  } finally {
    if (temporaryDirectory) {
      rmSync(temporaryDirectory, { recursive: true, force: true });
    }
  }
};

if (require.main === module) {
  try {
    main(process.argv.slice(2));
  } catch (error) {
    process.stderr.write(`error: ${error.message}\n`);
    process.exitCode = 1;
  }
}

module.exports = {
  parseArguments,
  parseDynamicSection,
  parseDynamicSymbols,
  parseElfHeader,
  unresolvedStrongSymbols,
  validateDynamicPolicy,
  validateHeader,
};
