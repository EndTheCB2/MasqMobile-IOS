#!/bin/bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT_DIR"

if ! command -v rg >/dev/null 2>&1; then
  echo "error: source privacy check requires ripgrep (rg)" >&2
  exit 1
fi

scan() {
  local expression="$1"
  local description="$2"
  # Ordinary checkouts use a .git directory; linked worktrees use a .git pointer file.
  if rg --no-config -n --hidden \
      --glob '!.git' \
      --glob '!.git/**' \
      --glob '!**/build/**' \
      --glob '!**/target/**' \
      --glob '!**/node_modules/**' \
      --glob '!**/Pods/**' \
      --glob '!masq-node-mobile/.github/workflows/ci-matrix.yml' \
      --glob '!masq-node-mobile/node/src/daemon/daemon_initializer.rs' \
      --glob '!masq-mobile/scripts/archive-ios-app-store.sh' \
      --glob '!masq-mobile/scripts/verify-source-privacy.sh' \
      "$expression" . >/dev/null; then
    echo "error: source privacy check found $description" >&2
    return 1
  fi
}

scan '/Users/[A-Za-z0-9._-]+([/[:space:]]|$)' 'a local user path'
scan 'DEVELOPMENT_TEAM[[:space:]]*=[[:space:]]*[A-Z0-9]{10}' 'an Apple team identifier'
scan 'PROVISIONING_PROFILE_SPECIFIER[[:space:]]*=[[:space:]]*[^;[:space:]]+' 'a provisioning profile'
scan 'Apple Development:[[:space:]]+[^;]+' 'a personal signing identity'
scan 'BEGIN (RSA |EC |OPENSSH )?PRIVATE KEY' 'private key material'

while IFS= read -r -d '' link_path; do
  if [[ "$(readlink "$link_path")" = /* ]]; then
    echo "error: source privacy check found an absolute symbolic-link target" >&2
    exit 1
  fi
done < <(
  find . \
    -type d \
    \( -name .git -o -name build -o -name target -o -name node_modules -o -name Pods \) \
    -prune \
    -o -type l -print0
)

echo "Source privacy check passed."
