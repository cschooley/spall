# The agent's toolset (#13, docs/sandbox.md): what runs inside a sandbox.
# spall starts it under gVisor (`--runtime=runsc`), read-only, no network
# until taphttp's per-task credentials are wired (slice 2), as uid 10001,
# with a tmpfs /scratch that dies with the task.
#
# Supply chain: the base is pinned by digest; packages come from Alpine's
# signed repositories for that release. Keep the list short and add a tool
# only when a task needs it. Build: docker build -t spall-sandbox:dev sandbox/
FROM alpine:3.24.4@sha256:f59abe656b692d901e184be3445e084786602108599d2a51d229c0f5a48c71bf

RUN apk add --no-cache \
      bind-tools \
      ca-certificates \
      curl \
      jq \
      nmap \
      openssl \
      python3 \
 && adduser -D -H -u 10001 -s /bin/sh agent

USER 10001
WORKDIR /scratch
# A sandbox idles between commands; spall runs each with `docker exec`.
CMD ["sleep", "infinity"]
