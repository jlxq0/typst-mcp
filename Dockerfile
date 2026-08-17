# syntax=docker/dockerfile:1.7

# Multi-stage build → distroless runtime.
# RFC 9728: advertise resource as {origin}/mcp (see src/config.rs mcp_resource_url).
# Do not invent Entra app IDs here; they live in deploy config, not this image.
# Compatible with fondue PSS-restricted namespace conventions.
# The same binary is the HTTP server and the compile worker (`--compile-worker`).

ARG RUST_VERSION=1.93
# Digest pinned to rust:1.93-bookworm (OCI index). Same pin as matrix-mcp. Update via:
#   TOKEN=$(curl -s "https://auth.docker.io/token?service=registry.docker.io&scope=repository:library/rust:pull" | jq -r .token)
#   curl -sI -H "Accept: application/vnd.oci.image.index.v1+json" -H "Authorization: Bearer $TOKEN" \
#     "https://registry-1.docker.io/v2/library/rust/manifests/${RUST_VERSION}-bookworm" | grep docker-content-digest
FROM rust:${RUST_VERSION}-bookworm@sha256:7c4ae649a84014c467d79319bbf17ce2632ae8b8be123ac2fb2ea5be46823f31 AS builder

WORKDIR /build

# Cache dependencies separately from source: copy manifest first, build a
# stub, then copy real source. This means `cargo build` only re-runs the
# slow dependency compile if Cargo.toml / Cargo.lock change.
COPY Cargo.toml Cargo.lock* ./
RUN mkdir src && \
    echo 'fn main() { println!("dep stub"); }' > src/main.rs && \
    cargo build --release --locked && \
    rm -rf src target/release/deps/typst_mcp* target/release/typst-mcp*

# Now the real source.
COPY src ./src
RUN cargo build --release --locked

# Distroless runtime: no shell, no apt, no package manager.
# `cc` variant includes glibc + ca-certs which we need for HTTPS to Entra.
# Digest pinned to gcr.io/distroless/cc-debian12:nonroot (OCI index). Same pin as matrix-mcp.
FROM gcr.io/distroless/cc-debian12:nonroot@sha256:e2d29aec8061843706b7e484c444f78fafb05bfe47745505252b1769a05d14f1

WORKDIR /app
COPY --from=builder /build/target/release/typst-mcp /app/typst-mcp
COPY templates /usr/share/typst-mcp/templates
COPY fonts /usr/share/fonts/typst

# Non-root by default (distroless `nonroot` user, UID 65532). Matches
# a typical PSS-restricted Kubernetes security context (no privilege
# escalation, drop ALL capabilities, read-only root filesystem).
USER nonroot:nonroot

EXPOSE 3000
ENTRYPOINT ["/app/typst-mcp"]
