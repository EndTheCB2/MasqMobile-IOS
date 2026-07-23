import {execFileSync, spawnSync} from 'node:child_process';
import {fileURLToPath} from 'node:url';
import path from 'node:path';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const toolchain = process.env.MASQ_RUST_TOOLCHAIN ?? '1.77.2';
const which = tool =>
  execFileSync('rustup', ['which', tool, '--toolchain', toolchain], {
    encoding: 'utf8',
  }).trim();

const cargo = which('cargo');
const environment = {
  ...process.env,
  RUSTC: which('rustc'),
  RUSTDOC: which('rustdoc'),
};
const baseArguments = [
  'test',
  '--offline',
  '--manifest-path',
  path.join(root, 'native/masq-mobile-core/Cargo.toml'),
  '--features',
  'node-engine',
];

const run = arguments_ =>
  spawnSync(cargo, arguments_, {
    cwd: root,
    env: environment,
    stdio: 'inherit',
  });

const requireSuccess = result => {
  if (result.error) {
    throw result.error;
  }
  if (result.status !== 0) {
    process.exit(result.status ?? 1);
  }
};

requireSuccess(run(baseArguments));
requireSuccess(
  run([
    ...baseArguments,
    'engine::tests::embedded_node_starts_consume_only_and_stops_without_terminating_the_app',
    '--',
    '--ignored',
    '--exact',
  ]),
);
