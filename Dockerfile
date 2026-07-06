# syntax=docker/dockerfile:1.9
#
# Production image for the Quarry server.
#
# The default target builds the browser UI and Rust binary from source, which
# keeps local `docker build .` self-contained. Release CI targets
# `runtime-prebuilt` instead, reusing the already-packaged Linux binaries whose
# browser UI was embedded during the release-build workflow.
#
# Runtime state lives under /storage, and the container binds to $PORT when
# provided by a PaaS, falling back to Quarry's default 7831.

FROM oven/bun:1 AS ui-builder
WORKDIR /app/ui

COPY ui/package.json ui/bun.lock ./
RUN bun install --frozen-lockfile

COPY ui/ ./
RUN bun run build

FROM rust:1-bookworm AS builder
WORKDIR /app

RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      build-essential \
      ca-certificates \
      cmake \
      fuse3 \
      libssl-dev \
      pkg-config \
 && rm -rf /var/lib/apt/lists/*

COPY Cargo.toml Cargo.lock ./
COPY crates/ ./crates/
COPY --from=ui-builder /app/ui/dist ./ui/dist

ARG QUARRY_FEATURES=
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    if [ -n "$QUARRY_FEATURES" ]; then \
      cargo build --locked --release -p quarry --features "$QUARRY_FEATURES"; \
    else \
      cargo build --locked --release -p quarry; \
    fi

FROM debian:bookworm-slim AS runtime-base

RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      ca-certificates \
      curl \
      gosu \
      tini \
 && rm -rf /var/lib/apt/lists/* \
 && groupadd --gid 1000 quarry \
 && useradd --uid 1000 --gid quarry --home-dir /var/quarry --shell /usr/sbin/nologin --no-create-home quarry \
 && install -d -o quarry -g quarry -m 0755 /var/quarry /storage

ENV QUARRY_ROOT=/storage \
    QUARRY_LOG_FORMAT=json \
    RUST_LOG=info,quarry=info

VOLUME ["/storage"]
EXPOSE 7831

HEALTHCHECK --interval=10s --timeout=5s --start-period=20s --retries=12 \
  CMD curl -fsS http://127.0.0.1:${PORT:-7831}/v1/health >/dev/null || exit 1

ENTRYPOINT ["/usr/bin/tini", "--"]
CMD ["sh", "-c", "chown -R quarry:quarry \"$QUARRY_ROOT\" && exec gosu quarry quarry server start --root \"$QUARRY_ROOT\" --addr 0.0.0.0:${PORT:-7831}"]

FROM runtime-base AS runtime-prebuilt

ARG TARGETARCH

COPY --chmod=0755 tmp/docker-context/${TARGETARCH}/quarry /usr/local/bin/quarry

FROM runtime-base AS runtime-built

COPY --from=builder /app/target/release/quarry /usr/local/bin/quarry
