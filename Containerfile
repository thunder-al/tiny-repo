FROM docker.io/library/rust:1.97.1-trixie AS build

WORKDIR /app

COPY ./Cargo.toml ./Cargo.lock ./
COPY ./src/ ./src/

RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/app/target \
    set -xe; \
    cargo build --release --locked; \
    cp target/release/tiny-repo /tiny-repo;

# distroless + glibc
FROM gcr.io/distroless/cc-debian13:nonroot AS release

WORKDIR /app

COPY --from=build /tiny-repo /app/tiny-repo

EXPOSE 8000

ENTRYPOINT ["/app/tiny-repo"]
CMD ["serve"]
