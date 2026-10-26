# tiny-repo

A small self-hosted OCI registry: one Rust binary, meant to run on a scale-to-zero platform with S3-compatible storage behind it.

**Status:** boilerplate. The server answers health probes only.

## Layout

| Path | What |
|---|---|
| `src/` | the `tiny-repo` binary (axum) |
| `scripts/` | cloud agent setup, security scan |

## Requirements

- [rustup](https://rustup.rs/) -- the Rust version comes from `rust-toolchain.toml`
- [mise](https://mise.jdx.dev/) -- installs trivy and cargo-audit from `mise.toml` (`mise install`)

## Quick start

```bash
cargo run -- serve     # http://127.0.0.1:8000/probe/live
```

## Configuration

Defaults, then `tiny-repo.toml` (path via `--config` or `TINY_REPO_CONFIG`), then environment variables. Unknown keys are an error.

| Key | Env | Default |
|---|---|---|
| `listen` | `TINY_REPO_LISTEN` | `0.0.0.0:8000` |
| `log.level` | `TINY_REPO_LOG__LEVEL` | `info` (a `tracing` filter) |
| `log.format` | `TINY_REPO_LOG__FORMAT` | `text` (`text` or `json`) |

## Commands

| Command | Purpose |
|---|---|
| `cargo run -- serve` | run the server |
| `cargo test` | tests |
| `cargo clippy --all-targets` | lints |
| `cargo fmt` | format |
| `mise run security:scan [image]` | cargo audit, trivy fs (and the image, if given) |

## Container

```bash
docker build -f Containerfile -t tiny-repo .
docker run --rm -p 8000:8000 tiny-repo
```

Multi-stage: cargo builds the binary, the result runs on `gcr.io/distroless/cc-debian13:nonroot`.

## License

[Apache-2.0](./LICENSE)
