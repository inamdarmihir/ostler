# Ostler container image: the prebuilt release binary plus Chromium, running as a non-root user.
# Built by the release workflow from the already-built Linux tarballs (no compile here), one per CPU.
#   docker run -i --rm ghcr.io/inamdarmihir/ostler            # MCP server on stdio
#   docker run -i --rm ghcr.io/inamdarmihir/ostler doctor     # check that Chromium launches
FROM debian:bookworm-slim

ARG TARGETARCH
RUN apt-get update \
 && apt-get install -y --no-install-recommends chromium fonts-liberation ca-certificates tini \
 && rm -rf /var/lib/apt/lists/*

# docker/bin/<amd64|arm64>/ostler is placed by the release workflow (or by hand, see docs/INSTALL.md).
COPY docker/bin/${TARGETARCH}/ostler /usr/local/bin/ostler

RUN useradd --create-home --uid 10001 ostler
USER ostler
WORKDIR /home/ostler

# Containers rarely allow Chromium's own sandbox; the container is the sandbox.
ENV OSTLER_CHROME=/usr/bin/chromium \
    OSTLER_NO_SANDBOX=1
ENTRYPOINT ["/usr/bin/tini", "--", "ostler"]
CMD ["serve"]
