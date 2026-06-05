# ─── Zentra seed/full node — isolated, lightweight container ──────────────────
# Build stage: compile only the headless daemon (no GUI deps needed).
FROM rust:bookworm AS build
RUN apt-get update && apt-get install -y --no-install-recommends \
    clang libclang-dev pkg-config libssl-dev \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY . .
RUN cargo build --release -p zentrad

# Runtime stage: tiny image with just the binary.
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates libssl3 \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/target/release/zentrad /usr/local/bin/zentrad

# 16110 = P2P (the only port that must be public for a seed node)
# 16111 = JSON-RPC (admin — keep private), 16112 = web dashboard (optional)
EXPOSE 16110
VOLUME ["/data"]

# Devnet seed node: mines to keep the chain alive 24/7 and serves peers.
ENTRYPOINT ["zentrad", "--network", "devnet", "--mine", "--data-dir", "/data"]
