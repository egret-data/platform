# ---- Stage 1: build the egret binary ----
FROM rust:bookworm AS builder
WORKDIR /src
# Dep-layer cache: copy manifests first. (Workspace has many crates; copying the
# whole tree is acceptable for a local image — keep it simple and correct.)
COPY . .
RUN cargo build --release -p egret-bin --locked

# ---- Stage 2: slim runtime ----
FROM debian:bookworm-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates \
 && rm -rf /var/lib/apt/lists/* \
 && useradd -r -u 10001 egret
COPY --from=builder /src/target/release/egret /usr/local/bin/egret
USER 10001
EXPOSE 8080
ENTRYPOINT ["egret"]
CMD ["serve"]
