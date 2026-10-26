# --- Build Stage ---
FROM rust:1.80-bookworm AS builder

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

# Copy realtgbot source and compile release binary
COPY realtgbot .
RUN cargo build --release --bin realtgbot

# --- Runtime Stage ---
FROM debian:bookworm-slim

# Install runtime SSL certificates
RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl3 \
    curl \
    && rm -rf /var/lib/apt/lists/*

# User with UID 1000 for cloud container runners
RUN useradd -m -u 1000 user
ENV HOME=/home/user \
    PORT=7860

WORKDIR /app

RUN chown -R user:user /app
USER user

# Copy compiled binary from builder
COPY --from=builder --chown=user:user /usr/src/realtgbot/target/release/realtgbot /app/realtgbot

# Ensure executable permissions
RUN chmod +x /app/realtgbot

EXPOSE 7860

# Run the trading bot
CMD ["/app/realtgbot"]
