import {execFileSync, spawnSync} from 'node:child_process';
import {fileURLToPath} from 'node:url';
import path from 'node:path';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const toolchain = process.env.MASQ_RUST_TOOLCHAIN ?? '1.77.2';
const which = tool =>
  execFileSync('rustup', ['which', tool, '--toolchain', toolchain], {
    encoding: 'utf8',
  }).trim();

const result = spawnSync(
  which('cargo'),
  [
    'test',
    '--offline',
    '--manifest-path',
    path.join(root, 'native/masq-mobile-core/Cargo.toml'),
  ],
  {
    cwd: root,
    env: {
      ...process.env,
      RUSTC: which('rustc'),
      RUSTDOC: which('rustdoc'),
    },
    stdio: 'inherit',
  },
);

if (result.error) {
  throw result.error;
}
process.exit(result.status ?? 1);
