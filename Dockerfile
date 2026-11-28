# Stage 1: build the UI (ui/ -> ui/dist). Node never reaches the final image.
FROM node:22.16.0-bookworm-slim AS ui
WORKDIR /ui
# Lockfile only, no install scripts (ui/.npmrc), so deps are cached as a layer.
COPY ui/package.json ui/package-lock.json ui/.npmrc ./
RUN npm ci
COPY ui/ ./
RUN npm run lint && npm test && npm run build

# Stage 2: build spall as a static musl binary with the UI embedded. No
# OpenSSL: TLS to a model API (#63) is rustls on ring, whose C and assembly
# are compiled in statically, so it needs no libc at run time.
FROM rust:1.90-slim-bookworm AS build
WORKDIR /src
# musl-gcc compiles ring's C for the musl target (build stage only).
RUN apt-get update \
 && apt-get install -y --no-install-recommends musl-tools \
 && rm -rf /var/lib/apt/lists/*
RUN TARGET="$(uname -m)-unknown-linux-musl" \
 && rustup target add "$TARGET" \
 && echo "$TARGET" > /target
ENV CC_x86_64_unknown_linux_musl=musl-gcc \
    CC_aarch64_unknown_linux_musl=musl-gcc
# Build dependencies in their own layer so source edits rebuild fast.
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo 'fn main() {}' > src/main.rs && touch src/lib.rs \
 && cargo build --release --locked --target "$(cat /target)" \
 && rm -rf src
COPY src ./src
COPY --from=ui /ui/dist ./ui/dist
RUN touch src/main.rs src/lib.rs \
 && cargo build --release --locked --target "$(cat /target)" \
 && install -m 0755 "target/$(cat /target)/release/spall" /spall \
 && (ldd /spall 2>&1 | grep -qE "statically linked|not a dynamic executable") \
 && mkdir -p /mnt/data /mnt/token

# The sandbox sensor (#13, `spall sandboxd`): the same binary with the
# Docker CLI, the only image that is given the Docker socket. Not the
# default target: compose builds it with `target: sandboxd`.
FROM docker:29.9.3-cli@sha256:5a7af0dcb0e46c803efe9b45371dd21cb500e5b4453250394ac6f06d49b6e6f1 AS sandboxd
COPY --from=build /spall /spall
ENTRYPOINT ["/spall", "sandboxd"]

# Stage 3: runtime. Nothing but the binary: no shell, no package manager, no
# libc, so no OS packages to carry CVEs (#28).
# - No CA bundle: the lever API is plain HTTP on the compose network, and
#   the one outbound TLS connection (a model API, #63) checks against the
#   Mozilla roots compiled into the binary (webpki-roots).
# - DNS for `taphttp` works: Docker mounts /etc/resolv.conf and /etc/hosts.
FROM scratch
COPY --from=build /spall /spall
# Mount points for the named volumes, owned by uid 10001 so a fresh volume
# initializes with that owner. spall must run as 10001: taphttp writes the
# API token as 0600 owned by 10001.
COPY --from=build --chown=10001:10001 /mnt/data /data
COPY --from=build --chown=10001:10001 /mnt/token /token
USER 10001:10001
# The binary defaults to 127.0.0.1:3000. docker-compose.yml sets
# SPALL_LISTEN=0.0.0.0:3000 *inside* the container and publishes the port
# to 127.0.0.1 on the host only.
ENV SPALL_LOG_FILE=/data/traffic.jsonl
EXPOSE 3000
ENTRYPOINT ["/spall"]
