#!/bin/bash
# Claude Code cloud session setup: reuses the cloud install script (see AGENTS_CLOUD.md).
# Every step of cloud-install.sh is idempotent, so a cached container only re-checks.
set -euo pipefail

if [ "${CLAUDE_CODE_REMOTE:-}" != "true" ]; then
  exit 0
fi

cd "$CLAUDE_PROJECT_DIR"

bash scripts/cloud-install.sh

# Later tool calls run in fresh shells; make the installed tools visible to them.
echo "export PATH=\"\$HOME/.cargo/bin:\$HOME/.local/bin:\$HOME/.local/share/mise/shims:\$PATH\"" >>"$CLAUDE_ENV_FILE"
