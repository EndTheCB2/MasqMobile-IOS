#!/bin/bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT_DIR"

scan() {
  local expression="$1"
  local description="$2"
  if rg -n --hidden \
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

echo "Source privacy check passed."
