#!/usr/bin/env bash
# Runs every security check, then exits non-zero if any of them had findings.
# Usage: mise run security:scan [image-ref]
set -uo pipefail

cd "$(dirname "$0")/.."

failed=()

echo "=== cargo audit (RustSec advisories for Cargo.lock) ==="
cargo audit || failed+=("cargo audit")

echo
echo "=== trivy fs (secrets, Containerfile) ==="
trivy fs . --scanners=secret,misconfig --severity=MEDIUM,HIGH,CRITICAL --exit-code=1 \
  --skip-dirs=target || failed+=("trivy fs")

if [ -n "${1:-}" ]; then
  echo
  echo "=== trivy image $1 (OS packages of the base image) ==="
  trivy image "$1" --severity=HIGH,CRITICAL --exit-code=1 || failed+=("trivy image $1")
fi

if [ "${#failed[@]}" -gt 0 ]; then
  echo
  echo "Findings in:" >&2
  printf -- '- %s\n' "${failed[@]}" >&2
  exit 1
fi

echo
echo "No findings"
