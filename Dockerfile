# Stage 1: build the UI (ui/ -> ui/dist). Node never reaches the final image.
FROM node:22.16.0-bookworm-slim AS ui
WORKDIR /ui
# Lockfile only, no install scripts (ui/.npmrc), so deps are cached as a layer.
COPY ui/package.json ui/package-lock.json ui/.npmrc ./
RUN npm ci
COPY ui/ ./
RUN npm run lint && npm test && npm run build

# Stage 2: build spall as a static musl binary with the UI embedded. spall
# is pure Rust (no OpenSSL or other C dependencies), so it needs no libc at
# run time.
FROM rust:1.90-slim-bookworm AS build
WORKDIR /src
RUN TARGET="$(uname -m)-unknown-linux-musl" \
 && rustup target add "$TARGET" \
 && echo "$TARGET" > /target
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

# Stage 3: runtime. Nothing but the binary: no shell, no package manager, no
# libc, so no OS packages to carry CVEs (#28).
# - No CA bundle: spall makes no outbound TLS connections (the lever API is
#   plain HTTP on the compose network). Add one if that ever changes.
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
