# Both digests select Linux amd64 manifests. Build with --platform linux/amd64.
# The full official Node stage supplies a fixed CA bundle for apt's first HTTPS
# request; the slim variant removes that bundle during its package cleanup.
FROM node:24.22.2-bookworm@sha256:6b9d4ea31e614e29f364bf75758458731eb1e105b80c99667438dd6b3fe262e0 AS node_runtime

FROM ubuntu:24.04@sha256:78d4744ead1f5b19b04d0df558ab182a5300903900755e515b2ca4d10eb2044d

COPY --from=node_runtime /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/ca-certificates.crt
COPY --from=node_runtime /usr/local/bin/node /usr/local/bin/node
COPY --from=node_runtime /usr/local/lib/node_modules/npm /usr/local/lib/node_modules/npm
COPY ubuntu.sources /etc/apt/sources.list.d/ubuntu.sources

# Package versions are fixed by the signed Ubuntu snapshot and base digest.
# hadolint ignore=DL3008
RUN test "$(dpkg --print-architecture)" = amd64 \
    && rm -f /etc/apt/sources.list \
    && apt-get update \
    && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
        ca-certificates curl git python3 ripgrep \
        libatomic1 libgcc-s1 libssl3t64 libstdc++6 \
    && rm -rf /var/lib/apt/lists/* \
    && ln -s ../lib/node_modules/npm/bin/npm-cli.js /usr/local/bin/npm \
    && ln -s ../lib/node_modules/npm/bin/npx-cli.js /usr/local/bin/npx \
    && test "$(node --version)" = v24.22.2 \
    && test "$(npm --version)" = 11.19.1

WORKDIR /opt/pockymoe/harnesses
COPY package.json package-lock.json verify-install.mjs ./
RUN touch /tmp/pockymoe-npm-globalrc \
    && npm ci --omit=dev --include=optional --os=linux --cpu=x64 --libc=glibc \
        --registry=https://registry.npmjs.org --no-audit --no-fund \
        --userconfig=/dev/null --globalconfig=/tmp/pockymoe-npm-globalrc \
        --cache=/tmp/pockymoe-npm-cache \
    && node verify-install.mjs \
    && rm -rf /tmp/pockymoe-npm-cache /tmp/pockymoe-npm-globalrc \
    && mkdir -p /opt/pockymoe/bin /pockymoe/state /pockymoe/bridge /pockymoe/workspace \
    && chown 1000:1000 /pockymoe/state /pockymoe/bridge /pockymoe/workspace

ENV HOME=/pockymoe/state \
    CODEX_HOME=/pockymoe/state/codex \
    CLAUDE_CONFIG_DIR=/pockymoe/state/claude \
    XDG_CONFIG_HOME=/pockymoe/state/config \
    XDG_CACHE_HOME=/pockymoe/state/cache \
    LANG=C.UTF-8 \
    DISABLE_AUTOUPDATER=1 \
    CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1

# The provider supplies the checkout owner, read-only rootfs, network none,
# resource limits, writable private state/tmpfs and the read-only worker bind.
# No Supervisor executable, database, CLI home or credential is copied here.
USER 1000:1000
WORKDIR /pockymoe/workspace
CMD ["/bin/sh"]
