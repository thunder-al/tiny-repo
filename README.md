# tiny-repo

A small self-hosted OCI registry: one Rust binary, meant to run on a scale-to-zero platform with S3-compatible storage behind it.

**Status:** early. Blobs and meta live on the local filesystem; the server takes blob uploads (`/v2/<name>/blobs/uploads/`). Pulling and manifests come next.

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
| `storage.type` | `TINY_REPO_STORAGE__TYPE` | `fs` |
| `storage.root` | `TINY_REPO_STORAGE__ROOT` | `data` (for `fs`: blobs in `<root>/blobs`) |
| `meta.type` | `TINY_REPO_META__TYPE` | `storage`: the meta backend of `storage` (for `fs`: `<root>/meta`) |
| `uploads.dir` | `TINY_REPO_UPLOADS__DIR` | for `fs`: `<root>/uploads`; must be on the same filesystem as `storage.root` |
| `uploads.max_blob_size` | `TINY_REPO_UPLOADS__MAX_BLOB_SIZE` | `10GiB`: largest single blob (layer), bytes or `KiB`/`MiB`/`GiB`/`TiB`/`KB`/`MB`/`GB`/`TB`; `0` = no limit |
| `uploads.session_ttl_secs` | `TINY_REPO_UPLOADS__SESSION_TTL_SECS` | `3600`: an upload idle this long is dropped on the next new upload |

`[storage]` and `[meta]` are taken whole: setting any of their keys means setting `type` too, and nothing is inherited from the defaults of another type.

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
docker run --rm -p 8000:8000 -v tiny-repo-data:/data tiny-repo
```

The image stores everything in `/data` (a volume on Fly).

Multi-stage: cargo builds the binary, the result runs on `gcr.io/distroless/cc-debian13:nonroot`.

## License

[Apache-2.0](./LICENSE)
