# syntax=docker/dockerfile:1
# ---- Stage 1: build the egret binary ----
# BuildKit cache mounts keep the cargo registry + the workspace `target/` warm
# across builds, so a source change recompiles only the changed crates instead of
# the whole ~20-40min workspace (a bare `COPY . . && cargo build` rebuilt
# everything on any file change). cargo-chef doesn't work here — this workspace's
# vendored `third_party/` patch crates + build scripts break its dummy-project
# reconstruction — so we cache the real target dir instead.
FROM rust:bookworm AS builder
WORKDIR /src
COPY . .
ENV CARGO_NET_GIT_FETCH_WITH_CLI=true
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/src/target \
    cargo build --release -p egret-bin --locked \
 && cp target/release/egret /usr/local/bin/egret

# ---- Demo runtime: egret + the Data Apps payload, for `apps.pod.launcher = "subprocess"` ----
# DEMO ONLY. The subprocess launcher runs marimo as the same uid as the server and gives no
# isolation (crates/egret-apps/src/subprocess_pod.rs). Production uses the Kubernetes launcher and
# the separate egret-apps pod image; the default (last) stage below is unchanged.
FROM python:3.12-slim AS demo
ARG MARIMO_VERSION=0.25.0
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates tini \
 && rm -rf /var/lib/apt/lists/* \
 && useradd -r -u 10001 egret
WORKDIR /opt/egret-apps/src
COPY apps-image/requirements.txt apps-image/constraints.txt ./
RUN pip install --no-cache-dir -r requirements.txt -c constraints.txt
COPY apps-image/pyproject.toml ./
COPY apps-image/egret_marimo ./egret_marimo
RUN pip install --no-cache-dir --no-deps . \
 && cd / && rm -rf /opt/egret-apps/src
WORKDIR /opt/egret-apps
COPY apps-image/venv-shim ./venv-shim
COPY apps-image/config ./config
RUN chmod +x venv-shim/bin/python \
 && mkdir -p /var/lib/egret-apps && chown 10001:10001 /var/lib/egret-apps \
 && python -c "import marimo,sys; sys.exit(marimo.__version__ != '${MARIMO_VERSION}')" \
 && cd /tmp && python -I -c "import egret_marimo,sys; sys.exit('site-packages' not in egret_marimo.__file__)"
ENV MARIMO_SKIP_UPDATE_CHECK=1 \
    PYTHONNOUSERSITE=1 \
    EGRET_REAL_PYTHON=/usr/local/bin/python \
    EGRET_MARIMO_VERSION=${MARIMO_VERSION} \
    EGRET_VENV_SHIM=/opt/egret-apps/venv-shim \
    XDG_CONFIG_HOME=/opt/egret-apps/config
COPY --from=builder /usr/local/bin/egret /usr/local/bin/egret
WORKDIR /
USER 10001
EXPOSE 8080
ENTRYPOINT ["egret"]
CMD ["serve"]

# ---- Stage 2: slim runtime ----
FROM debian:bookworm-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates \
 && rm -rf /var/lib/apt/lists/* \
 && useradd -r -u 10001 egret
COPY --from=builder /usr/local/bin/egret /usr/local/bin/egret
USER 10001
EXPOSE 8080
ENTRYPOINT ["egret"]
CMD ["serve"]
