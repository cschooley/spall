# taphttp has no Dockerfile or published image yet (see docs/taphttp-requests.md),
# so build it from source at a pinned commit. Bump TAPHTTP_REF deliberately.
FROM rust:1.90-slim-bookworm AS build
RUN apt-get update \
 && apt-get install -y --no-install-recommends git ca-certificates \
 && rm -rf /var/lib/apt/lists/*
ARG TAPHTTP_REPO=https://github.com/cschooley/taphttp.git
ARG TAPHTTP_REF=51eb9bc3fc606d2a3652dccaf7b70a2293e55f53
WORKDIR /src
RUN git init -q . \
 && git remote add origin "$TAPHTTP_REPO" \
 && git fetch -q --depth 1 origin "$TAPHTTP_REF" \
 && git checkout -q FETCH_HEAD
RUN cargo build --release --locked \
 && install -m 0755 target/release/taphttp /usr/local/bin/taphttp

FROM debian:bookworm-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates \
 && rm -rf /var/lib/apt/lists/* \
 && useradd --system --uid 10001 --user-group --no-create-home taphttp \
 && mkdir -p /data /ca \
 && chown taphttp:taphttp /data /ca
COPY --from=build /usr/local/bin/taphttp /usr/local/bin/taphttp
USER taphttp
# CA cert + key live in /ca (own volume, never shared with spall).
ENV TAPHTTP_DATA=/ca
EXPOSE 8080
ENTRYPOINT ["taphttp"]
CMD ["start", "--listen", "0.0.0.0:8080", "--log-file", "/data/traffic.jsonl"]
