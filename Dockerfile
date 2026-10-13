# Multi-stage Dockerfile for Layer MCP server and CLI
FROM rust:1.85-slim AS builder

WORKDIR /usr/src/layer

# Install C/C++ compiler required for compiling embedded Tree-sitter grammars
RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential \
    && rm -rf /var/lib/apt/lists/*

COPY Cargo.toml Cargo.lock ./
COPY src ./src

RUN cargo build --release

# Runtime stage
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    git \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /usr/src/layer/target/release/layer /usr/local/bin/layer

WORKDIR /workspace

ENTRYPOINT ["layer"]
CMD ["mcp"]
