#!/usr/bin/env bash
# Start step for cloud agent VMs (.cursor/environment.json). The project needs no services yet,
# so this only checks that the install step has run.
set -euo pipefail

for tool in cargo mise; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "$tool is missing: the environment build has not run. Run: bash scripts/cloud-install.sh" >&2
    exit 1
  fi
done
