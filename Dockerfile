# Stage 1: build the UI (ui/ -> ui/dist). Node never reaches the final image.
FROM node:26.13.0-bookworm-slim AS ui
WORKDIR /ui
# Lockfile only, no install scripts (ui/.npmrc), so deps are cached as a layer.
COPY ui/package.json ui/package-lock.json ui/.npmrc ./
RUN npm ci
COPY ui/ ./
RUN npm run lint && npm test && npm run build

# Stage 2: build spall with the UI embedded.
FROM rust:1.90-slim-bookworm AS build
WORKDIR /src
# Build dependencies in their own layer so source edits rebuild fast.
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo 'fn main() {}' > src/main.rs && touch src/lib.rs \
 && cargo build --release --locked \
 && rm -rf src
COPY src ./src
COPY --from=ui /ui/dist ./ui/dist
RUN touch src/main.rs src/lib.rs \
 && cargo build --release --locked \
 && install -m 0755 target/release/spall /usr/local/bin/spall

# Stage 3: runtime. Neither toolchain, just the binary.
FROM debian:bookworm-slim
RUN useradd --system --uid 10001 --user-group --no-create-home spall \
 && mkdir -p /data /token && chown spall:spall /data /token
COPY --from=build /usr/local/bin/spall /usr/local/bin/spall
USER spall
# The binary defaults to 127.0.0.1:3000. docker-compose.yml sets
# SPALL_LISTEN=0.0.0.0:3000 *inside* the container and publishes the port
# to 127.0.0.1 on the host only.
ENV SPALL_LOG_FILE=/data/traffic.jsonl
EXPOSE 3000
ENTRYPOINT ["spall"]
