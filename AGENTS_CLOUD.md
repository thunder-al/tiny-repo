# AGENTS_CLOUD.md -- cloud agent setup

Setup notes for agents running in a cloud VM or container: Cursor Cloud and Claude Code on the web. General guidance lives in `AGENTS.md`; read that too.

## Entry points

- **Cursor Cloud:** [`.cursor/environment.json`](./.cursor/environment.json) runs `scripts/cloud-install.sh` on every environment build and `scripts/cloud-start.sh` on every session. A saved environment in the Cursor dashboard takes precedence over the file: either have none for this repository, or point its "environment.json location" at the file.
- **Claude Code:** [`.claude/hooks/session-start.sh`](./.claude/hooks/session-start.sh), registered in `.claude/settings.json`, runs `scripts/cloud-install.sh` when `CLAUDE_CODE_REMOTE=true` and adds the tool paths to the session environment. Locally the hook does nothing.

## What the install step does

[`scripts/cloud-install.sh`](./scripts/cloud-install.sh) is idempotent and keeps existing state:

- rustup, if missing, then the toolchain and components from `rust-toolchain.toml`;
- mise, if missing, then the tools from `mise.toml` (trivy, cargo-audit). mise, its shims, and the cargo binaries are linked into `/usr/local/bin`, because agent shells may start with a reset `PATH`;
- `cargo fetch --locked`;
- optionally, a private overlay (below).

The project needs no databases or other services yet; `cloud-start.sh` only checks that the install step has run. A session that starts before any environment build has finished boots without the tools; run `bash scripts/cloud-install.sh` once by hand.

## Private overlay

Per-user skills and notes that must not live in the public repository can come from a private git repository. Set `TINY_REPO_PRIVATE_AGENTS_REPO` in the environment's secrets to a git URL the environment can read; the install step clones or updates it into `.agents/local/private/` (gitignored). Without the variable, or without access, the step is skipped and nothing else changes.

## Running things

```bash
cargo run -- serve                 # http://127.0.0.1:8000/probe/live
cargo test
```
