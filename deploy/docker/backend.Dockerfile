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
