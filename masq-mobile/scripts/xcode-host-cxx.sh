#!/bin/bash
set -euo pipefail

exec xcrun --sdk macosx clang++ "$@"
