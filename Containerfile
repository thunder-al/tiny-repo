FROM docker.io/library/rust:1.97.1-trixie AS build

WORKDIR /app

COPY ./Cargo.toml ./Cargo.lock ./
COPY ./src/ ./src/

RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/app/target \
    set -xe; \
    cargo build --release --locked; \
    cp target/release/tiny-repo /tiny-repo; \
    mkdir /data;

# distroless + glibc
FROM gcr.io/distroless/cc-debian13:nonroot AS release

WORKDIR /app

COPY --from=build /tiny-repo /app/tiny-repo
# The runtime user cannot create directories in /; a mounted volume replaces this one.
COPY --from=build --chown=nonroot:nonroot /data /data

ENV TINY_REPO_STORAGE__TYPE=fs \
    TINY_REPO_STORAGE__ROOT=/data

EXPOSE 8000

ENTRYPOINT ["/app/tiny-repo"]
CMD ["serve"]
