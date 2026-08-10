declare const __dirname: string;

export {};

const {
  copyFileSync,
  mkdirSync,
  mkdtempSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} = require('fs');
const { tmpdir } = require('os');
const path = require('path');
const { spawnSync } = require('child_process');

describe('source privacy verification', () => {
  it('ignores the root .git pointer used by a linked worktree', () => {
    const fixture = privacyFixture();
    try {
      writeFileSync(
        path.join(fixture.root, '.git'),
        `gitdir: ${localUserPath()}/.git/worktrees/preview6\n`,
      );
      writeFileSync(path.join(fixture.root, 'README.md'), 'Safe public source\n');

      const result = runPrivacyCheck(fixture.script);

      expect(result.status).toBe(0);
      expect(result.stdout).toContain('Source privacy check passed.');
    } finally {
      rmSync(fixture.root, { force: true, recursive: true });
    }
  });

  it('still rejects a local user path in tracked source content', () => {
    const fixture = privacyFixture();
    try {
      writeFileSync(
        path.join(fixture.root, 'unsafe.txt'),
        `Local build path: ${localUserPath()}\n`,
      );

      const result = runPrivacyCheck(fixture.script);

      expect(result.status).toBe(1);
      expect(result.stderr).toContain('a local user path');
    } finally {
      rmSync(fixture.root, { force: true, recursive: true });
    }
  });

  it('rejects an absolute symbolic-link target without printing that target', () => {
    const fixture = privacyFixture();
    const privateTarget = localUserPath();
    try {
      symlinkSync(
        privateTarget,
        path.join(fixture.root, 'masq-mobile', 'native-dependencies'),
      );

      const result = runPrivacyCheck(fixture.script);

      expect(result.status).toBe(1);
      expect(result.stderr).toContain('an absolute symbolic-link target');
      expect(result.stderr).not.toContain(privateTarget);
    } finally {
      rmSync(fixture.root, { force: true, recursive: true });
    }
  });

  it('ignores a relative dependency symlink in an excluded node_modules directory', () => {
    const fixture = privacyFixture();
    try {
      mkdirSync(path.join(fixture.root, 'shared-dependencies'), {
        recursive: true,
      });
      symlinkSync(
        '../../shared-dependencies',
        path.join(fixture.root, 'masq-mobile', 'node_modules'),
      );

      const result = runPrivacyCheck(fixture.script);

      expect(result.status).toBe(0);
      expect(result.stdout).toContain('Source privacy check passed.');
    } finally {
      rmSync(fixture.root, { force: true, recursive: true });
    }
  });

  it('still rejects an absolute node_modules symlink target', () => {
    const fixture = privacyFixture();
    const privateTarget = localUserPath();
    try {
      symlinkSync(
        privateTarget,
        path.join(fixture.root, 'masq-mobile', 'node_modules'),
      );

      const result = runPrivacyCheck(fixture.script);

      expect(result.status).toBe(1);
      expect(result.stderr).toContain('an absolute symbolic-link target');
      expect(result.stderr).not.toContain(privateTarget);
    } finally {
      rmSync(fixture.root, { force: true, recursive: true });
    }
  });

  it('fails closed when ripgrep is unavailable and disables user config', () => {
    const fixture = privacyFixture();
    try {
      const source = require('fs').readFileSync(fixture.script, 'utf8');

      expect(source).toContain('if ! command -v rg');
      expect(source).toContain('rg --no-config');
    } finally {
      rmSync(fixture.root, { force: true, recursive: true });
    }
  });
});

function privacyFixture() {
  const root = mkdtempSync(path.join(tmpdir(), 'masq-source-privacy-'));
  const scripts = path.join(root, 'masq-mobile', 'scripts');
  mkdirSync(scripts, { recursive: true });
  const script = path.join(scripts, 'verify-source-privacy.sh');
  copyFileSync(
    path.resolve(__dirname, '../scripts/verify-source-privacy.sh'),
    script,
  );
  return { root, script };
}

function runPrivacyCheck(script: string) {
  return spawnSync('bash', [script], {
    encoding: 'utf8',
  });
}

function localUserPath() {
  return ['', 'Users', 'private-user', 'project'].join('/');
}
