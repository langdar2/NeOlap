# syntax=docker/dockerfile:1
#
# NeOlap server image.
#   docker compose up -d          (see docker-compose.yml)
#   docker build -t neolap .      (manual build)
#
# Multi-arch: builds natively for linux/amd64 and linux/arm64 (Apple Silicon).

ARG RUST_VERSION=1.99

# ---- build -----------------------------------------------------------------
FROM rust:${RUST_VERSION}-slim-trixie AS build
WORKDIR /src

# Copy manifests first so that dependency downloads are cached between code changes.
# (rust-toolchain.toml is deliberately not copied: the image's toolchain is used.)
COPY Cargo.toml Cargo.lock ./
COPY crates/neolap-core/Cargo.toml   crates/neolap-core/
COPY crates/neolap-server/Cargo.toml crates/neolap-server/
COPY crates/neolap-cli/Cargo.toml    crates/neolap-cli/
COPY crates/neolap-py/Cargo.toml     crates/neolap-py/
RUN mkdir -p crates/neolap-core/src crates/neolap-server/src crates/neolap-cli/src crates/neolap-py/src ui \
 && echo "" > crates/neolap-core/src/lib.rs \
 && echo "" > crates/neolap-server/src/lib.rs \
 && echo "fn main() {}" > crates/neolap-cli/src/main.rs \
 && echo "" > crates/neolap-py/src/lib.rs \
 && cargo fetch --locked

COPY crates crates
COPY ui ui
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked -p neolap-cli \
 && cp target/release/neolap /neolap \
 && mkdir /data-skeleton

# ---- runtime ---------------------------------------------------------------
# distroless/cc: glibc + libgcc only, no shell, no package manager; runs as non-root (uid 65532).
FROM gcr.io/distroless/cc-debian13:nonroot

LABEL org.opencontainers.image.title="NeOlap" \
      org.opencontainers.image.description="Lightweight multidimensional database with write-back and multi-user web UI" \
      org.opencontainers.image.source="https://github.com/langdar2/NeOlap" \
      org.opencontainers.image.licenses="MIT"

COPY --from=build /neolap /usr/local/bin/neolap
COPY --from=build --chown=65532:65532 /data-skeleton /data

ENV NEOLAP_LISTEN=0.0.0.0:8080 \
    NEOLAP_DATA_DIR=/data \
    NEOLAP_HEALTHCHECK_ADDR=127.0.0.1:8080 \
    RUST_LOG=info

VOLUME ["/data"]
EXPOSE 8080

HEALTHCHECK --interval=30s --timeout=5s --start-period=60s --retries=3 \
  CMD ["/usr/local/bin/neolap", "healthcheck"]

ENTRYPOINT ["/usr/local/bin/neolap"]
CMD ["serve"]
