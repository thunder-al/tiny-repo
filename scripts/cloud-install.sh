#!/usr/bin/env bash
# Install step for cloud agent VMs/containers (Cursor Cloud via .cursor/environment.json,
# Claude Code via .claude/hooks/session-start.sh). Runs on every environment build or session,
# so every step is safe to repeat and never overwrites local state.
set -euo pipefail

cd "$(dirname "$0")/.."

# Root containers usually have no sudo; VMs with a normal user do.
SUDO=""
if [ "$(id -u)" -ne 0 ] && command -v sudo >/dev/null 2>&1; then
  SUDO="sudo"
fi

echo "=== rustup (toolchain from rust-toolchain.toml) ==="
if ! command -v rustup >/dev/null 2>&1; then
  curl --proto '=https' --tlsv1.2 -fsSL https://sh.rustup.rs | sh -s -- -y --default-toolchain none --profile minimal
fi

export PATH="$HOME/.cargo/bin:$PATH"

# Reads rust-toolchain.toml and installs that version with its components.
rustup toolchain install

# rust-toolchain.toml applies only inside the repo; `cargo install` run by mise works elsewhere
# and would otherwise get the image's default toolchain, which may be too old.
rustup default "$(rustup show active-toolchain | cut -d' ' -f1)"

echo "=== mise (trivy, cargo-audit from mise.toml) ==="
if ! command -v mise >/dev/null 2>&1; then
  curl -fsSL https://mise.run | sh
fi

export PATH="$HOME/.local/bin:$HOME/.local/share/mise/shims:$PATH"

mise trust
mise install
mise reshim

# Agent shells may start with a reset PATH, so expose the tools system-wide.
for bin in "$(command -v mise)" "$HOME"/.local/share/mise/shims/* "$HOME"/.cargo/bin/*; do
  $SUDO ln -sf "$bin" "/usr/local/bin/$(basename "$bin")"
done

echo "=== dependencies ==="
cargo fetch --locked

# Optional per-user overlay: private skills and notes that must not live in the public repo.
# Set TINY_REPO_PRIVATE_AGENTS_REPO to a git URL the environment can read; it lands in
# .agents/local/private (gitignored). Without the variable, or without access, it is skipped.
if [ -n "${TINY_REPO_PRIVATE_AGENTS_REPO:-}" ]; then
  echo "=== private agent overlay ==="
  mkdir -p .agents/local

  if [ -d .agents/local/private/.git ]; then
    git -C .agents/local/private pull --ff-only || echo "private overlay: pull failed, keeping the current copy"
  else
    git clone --depth 1 "$TINY_REPO_PRIVATE_AGENTS_REPO" .agents/local/private || echo "private overlay: clone failed, skipping"
  fi
fi
