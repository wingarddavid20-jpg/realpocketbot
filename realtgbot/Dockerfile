# --- Build Stage ---
FROM rust:1.80-bullseye AS builder

WORKDIR /usr/src/realtgbot

# Install native compilation dependencies for fastcrypto and SSL
RUN apt-get update && apt-get install -y \
    cmake \
    clang \
    build-essential \
    libssl-dev \
    pkg-config \
    git \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Copy full source and compile release binary
COPY . .
RUN cargo build --release --bin realtgbot

# --- Runtime Stage ---
FROM debian:bullseye-slim

WORKDIR /app

# Install runtime SSL certificates
RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl1.1 \
    curl \
    && rm -rf /var/lib/apt/lists/*

# Copy compiled binary from builder
COPY --from=builder /usr/src/realtgbot/target/release/realtgbot /app/realtgbot

# Ensure executable permissions
RUN chmod +x /app/realtgbot

# Run the trading bot
CMD ["/app/realtgbot"]
