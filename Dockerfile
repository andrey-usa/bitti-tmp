# bitti with a headless Chromium, for CI and Kubernetes/OpenShift Jobs.
# Build from the directory that holds both checkouts (bitti links
# navigera from ../navigera):
#
#   docker build -f bitti/Dockerfile -t bitti .
#   docker run --rm -v "$PWD/tests:/work" bitti run . --base-url http://host:3000
#
# amd64 and arm64 (Debian's chromium-headless-shell; Chrome for Testing has
# no Linux arm64 build). Runs as an unprivileged user (uid 10001, any uid
# works: OpenShift's random ones too) without Chrome's sandbox: in a
# container without user namespaces Chrome can't start one, and the
# container is the boundary.

FROM rust:1-trixie AS build
WORKDIR /src
COPY navigera/Cargo.toml navigera/Cargo.lock navigera/build.rs navigera/
COPY navigera/src navigera/src
COPY navigera/skills navigera/skills
COPY navigera/vendor navigera/vendor
COPY bitti/Cargo.toml bitti/Cargo.lock bitti/
COPY bitti/src bitti/src
COPY bitti/skills bitti/skills
RUN cd bitti && cargo build --release --locked && strip target/release/bitti

FROM debian:trixie-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends chromium-headless-shell fonts-liberation fonts-noto-color-emoji ca-certificates \
 && rm -rf /var/lib/apt/lists/* \
 && useradd --uid 10001 --create-home --home-dir /home/bt bt \
 && mkdir /work && chmod 1777 /work
COPY --from=build /src/bitti/target/release/bitti /usr/local/bin/bitti
ENV NAVIGERA_CHROMIUM=/usr/bin/chromium-headless-shell \
    NAVIGERA_NO_SANDBOX=1 \
    HOME=/home/bt
USER 10001
WORKDIR /work
ENTRYPOINT ["bitti"]
CMD ["run", "."]
