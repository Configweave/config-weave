# Runtime-only image for weave-server (the config-weave web GUI).
#
# Binaries are pre-built on the host (`just docker-build` runs the cross
# musl build + server release build first) because the crates path-dep on
# sibling repos (../WCL, ../wscript, ../forge). The image just assembles:
#  - dist/config-weave-linux-x86_64 — static musl CLI; also the binary
#    the testlab copies into test containers,
#  - dist/weave-server — the GUI server with the frontend embedded,
#  - a static docker CLI for the testlab's docker backend (the socket is
#    mounted at runtime; test containers are siblings on the host daemon),
#  - git + ca-certificates for the remote package repositories
#    (repos.wcl clones, e.g. the stdlib seeded on first start).
#
# vmlab-backed tests and VNC are unavailable inside the container (they
# need host KVM + vmlab daemons); the UI degrades to docker + terminal.
#
# Run:
#   docker run --rm -p 8765:8765 \
#     -v /var/run/docker.sock:/var/run/docker.sock \
#     -v /path/to/runbooks:/runbooks \
#     -e FORGE_JWT_SECRET=… -e FORGE_AUTH_USERS=admin:… \
#     weave-server
# (append --no-auth instead of the -e's for a trusted network — the
# terminal widget gives shell access to test containers).

# weave-server is glibc-linked and built on the host, so the base image's
# glibc must be at least the host's (rolling-release hosts: prefer the
# newest debian). The CLI is static musl and doesn't care.
FROM debian:13-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends git ca-certificates \
    && rm -rf /var/lib/apt/lists/*

ARG DOCKER_CLI_VERSION=27.4.1
ADD https://download.docker.com/linux/static/stable/x86_64/docker-${DOCKER_CLI_VERSION}.tgz /tmp/docker.tgz
RUN tar -xzf /tmp/docker.tgz -C /tmp docker/docker \
    && mv /tmp/docker/docker /usr/local/bin/docker \
    && rm -rf /tmp/docker /tmp/docker.tgz

COPY dist/config-weave-linux-x86_64 /usr/local/bin/config-weave
COPY dist/weave-server /usr/local/bin/weave-server

ENV FORGE_HOST=0.0.0.0 \
    CONFIG_WEAVE_TEST_BINARY=/usr/local/bin/config-weave

VOLUME /runbooks
EXPOSE 8765

ENTRYPOINT ["weave-server", "--dir", "/runbooks", "--bind", "0.0.0.0"]
