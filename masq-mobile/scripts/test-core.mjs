import {execFileSync, spawnSync} from 'node:child_process';
import {fileURLToPath} from 'node:url';
import path from 'node:path';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const which = tool =>
  execFileSync('rustup', ['which', tool, '--toolchain', 'stable'], {
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
