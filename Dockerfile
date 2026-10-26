# Dot Home: one container with the server, QEMU and OSX-KVM's firmware.
#
#   docker build -t dot-home .
#   docker run -d --name dot-home --device /dev/kvm --group-add "$(getent group kvm | cut -d: -f3)" \
#     -p 443:8443 -v dot-home:/var/lib/dot-home \
#     -e DOT_HOME_PUBLIC_URL=https://home.example.com -e DOT_HOME_ACME_EMAIL=you@example.com \
#     --stop-timeout 300 dot-home
#
# See docs/home/SETUP.md.

# Portal
FROM node:22.20-trixie-slim AS web
WORKDIR /src/home-web
COPY home-web/package.json home-web/package-lock.json ./
RUN npm ci --no-audit --no-fund
COPY home-web/ ./
RUN npm run build

# Server
FROM rust:1.90-slim-trixie AS server
# Release identity and the update key, set by the release workflow (empty for local builds,
# which then report the crate version and leave auto-updates off).
ARG DOT_BUILD_VERSION=
ARG DOT_BUILD_COMMIT=
ARG DOT_BUILD_CHANNEL=
ARG DOT_UPDATE_REPOSITORY=
ARG DOT_UPDATE_PUBLIC_KEYS=
RUN apt-get update && apt-get install -y --no-install-recommends pkg-config cmake clang perl make && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY . .
COPY --from=web /src/crates/dot-home/web-dist crates/dot-home/web-dist
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    CARGO_TARGET_DIR=/src/target \
    DOT_BUILD_VERSION="$DOT_BUILD_VERSION" DOT_BUILD_COMMIT="$DOT_BUILD_COMMIT" DOT_BUILD_CHANNEL="$DOT_BUILD_CHANNEL" \
    DOT_UPDATE_REPOSITORY="$DOT_UPDATE_REPOSITORY" DOT_UPDATE_PUBLIC_KEYS="$DOT_UPDATE_PUBLIC_KEYS" \
    cargo build --release --locked -p dot-home \
    && cp /src/target/release/dot-home /usr/local/bin/dot-home

# OSX-KVM (OpenCore, OVMF, fetch-macOS-v2.py). Pin OSX_KVM_REF to a commit for reproducible builds.
FROM debian:trixie-slim AS osxkvm
ARG OSX_KVM_REF=4c378a4b5e0b219783683012bec680325eb40719
RUN apt-get update && apt-get install -y --no-install-recommends git ca-certificates && rm -rf /var/lib/apt/lists/*
RUN git clone --filter=blob:none https://github.com/kholia/OSX-KVM.git /opt/osx-kvm \
    && git -C /opt/osx-kvm checkout "$OSX_KVM_REF" \
    && rm -rf /opt/osx-kvm/.git

FROM debian:trixie-slim
RUN apt-get update && apt-get install -y --no-install-recommends \
      qemu-system-x86 qemu-utils python3 ca-certificates tini \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --home /var/lib/dot-home --shell /usr/sbin/nologin dot \
    && mkdir -p /var/lib/dot-home /etc/dot-home \
    && chown dot:dot /var/lib/dot-home
COPY --from=osxkvm /opt/osx-kvm /opt/osx-kvm
COPY --from=server /usr/local/bin/dot-home /usr/local/bin/dot-home
COPY deploy/home/config.docker.toml /etc/dot-home/config.toml
ENV DOT_HOME_CONFIG=/etc/dot-home/config.toml \
    DOT_HOME_ON_SHUTDOWN=hibernate \
    DOT_HOME_IN_CONTAINER=1
USER dot
VOLUME ["/var/lib/dot-home"]
EXPOSE 8443
STOPSIGNAL SIGTERM
HEALTHCHECK --interval=30s --timeout=5s --start-period=20s CMD ["dot-home", "health"]
# tini reaps the daemonized QEMU processes, which re-parent to PID 1.
ENTRYPOINT ["/usr/bin/tini", "--", "dot-home"]
CMD ["serve"]
