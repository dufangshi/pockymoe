FROM ubuntu:24.04
ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
       ca-certificates curl git iproute2 libgcc-s1 procps python3 tini \
    && rm -rf /var/lib/apt/lists/*
COPY acp_agent.py fixture_service.py stress_probe.py /opt/pockymoe-test/
# These aliases exercise adapters with the deterministic protocol fixture.
# They do not install the native Codex or Claude harnesses.
RUN printf '#!/bin/sh\nexec python3 /opt/pockymoe-test/acp_agent.py "$@"\n' \
      > /usr/local/bin/pocky-env-acp \
    && chmod 755 /usr/local/bin/pocky-env-acp \
    && ln -s pocky-env-acp /usr/local/bin/codex-acp \
    && ln -s pocky-env-acp /usr/local/bin/codex \
    && ln -s pocky-env-acp /usr/local/bin/claude-agent-acp \
    && ln -s pocky-env-acp /usr/local/bin/claude
ENV PYTHONDONTWRITEBYTECODE=1 PYTHONUNBUFFERED=1
WORKDIR /workspace
