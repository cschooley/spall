FROM rust:1.90-slim-bookworm AS build
WORKDIR /src
# Build dependencies in their own layer so source edits rebuild fast.
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo 'fn main() {}' > src/main.rs && touch src/lib.rs \
 && cargo build --release --locked \
 && rm -rf src
COPY src ./src
COPY static ./static
RUN touch src/main.rs src/lib.rs \
 && cargo build --release --locked \
 && install -m 0755 target/release/spall /usr/local/bin/spall

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
