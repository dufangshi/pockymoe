# Native binaries are built on Ubuntu 26.04; keep the container glibc compatible.
FROM node:22-bookworm-slim AS node
FROM ubuntu:26.04
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates curl socat procps hostname git libssl3t64 libsqlite3-0 \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --create-home --uid 1001 lab \
    && mkdir -p /workspace && chown lab:lab /workspace
COPY --from=node /usr/local/bin/node /usr/local/bin/node
COPY bin/ /usr/local/bin/
COPY lab/ /opt/lab/
COPY web/ /opt/pockymoe-web/
USER lab
ENV PATH="/usr/local/bin:/usr/bin:/bin" TREER_NETWORK_MODE=plain
WORKDIR /workspace
ENTRYPOINT ["/opt/lab/device-entrypoint.sh"]
