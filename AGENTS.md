# AGENTS.md

**tiny-repo** is a small self-hosted OCI registry: a single Rust crate at the repository root. It is at an early stage: the server answers health probes and opens its storage; the OCI routes come next.

Standard commands live in `README.md`. Tool versions are pinned in `rust-toolchain.toml` (Rust) and `mise.toml` (trivy, cargo-audit); tasks are mise tasks (`mise tasks ls`).

> **Cloud agents (Cursor Cloud, Claude Code on the web):** read [`AGENTS_CLOUD.md`](./AGENTS_CLOUD.md) first. It covers what the environment installs and how to run things there.

## Where things are

| Path | What |
|---|---|
| `src/` | the `tiny-repo` binary: CLI (`clap`), config (`figment`), HTTP routes (`axum`) |
| `src/oci/` | OCI protocol types (digest, repository name, tag), parsed at the boundary |
| `src/store/` | storage contract: `BlobStore`, `RegistryMeta`, `AuthMeta`; `Staging` (every upload is staged in a local file, then `BlobStore::put`); the shared meta `Index` behind single-writer backends; backends `fs/` and in-memory (tests only), chosen by config through `BlobBackend`/`MetaBackend`; the conformance suite every backend runs (`blob_store_suite!`, `meta_suite!`) |
| `scripts/` | cloud install/start, `security-scan.sh` |
| [`STYLEGUIDE.md`](./STYLEGUIDE.md) | how code is written and how dependencies are added. Read before writing code or adding a package. |
| [`SECURITY.md`](./SECURITY.md) | how to report a vulnerability |
| [`.agents/skills/`](./.agents/skills/) | task workflows. Each `SKILL.md` says when it applies. |

Everything is one crate: new parts (storage backends, OCI routes, auth) are modules under `src/`, not separate crates.

## Process

- No `git commit` or `git push` unless the user asks for it in the same message.
- Work on a tracker issue carries its identifier: branch `abc-123-short-slug`, commit subject `ABC-123 Short summary`. Work without an issue gets no prefix.
- Dependencies only through the package manager (`cargo add` / `cargo remove`), never by editing `Cargo.toml` by hand; latest version by default. Read the Dependencies section of `STYLEGUIDE.md` first.
- Generated files only through the project's commands; never hand-edit `Cargo.lock`.
- After changing code, run `cargo fmt`, `cargo clippy --all-targets`, `cargo test`. After adding or updating a dependency, also `mise run security:scan`.

## Task tracking

If a Linear MCP server is connected, task work goes through `.agents/skills/linear/SKILL.md`; read it before any Linear call. The skill finds the Linear project bound to this repository on its own. Findings, traps, and rejected options go into the issue, not only into the PR or commit message. No secrets and no local machine paths in issues.

## Gotchas

### Config keys are strict

Config is defaults, then `tiny-repo.toml`, then `TINY_REPO_*` env vars (`__` separates nested keys: `TINY_REPO_LOG__FORMAT=json`). Unknown keys fail at startup. An env var with the `TINY_REPO_` prefix that is not a config key has to be added to the ignore list in `src/config.rs`.
