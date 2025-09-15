# Use cargo-chef for better dependency caching
FROM lukemathwalker/cargo-chef:latest-rust-1.81-slim AS chef
WORKDIR /app

# Plan stage - prepare dependency information
FROM chef AS planner
COPY Cargo.toml Cargo.lock ./
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

# Build stage - build dependencies separately from source
FROM chef AS builder

# Install required dependencies for building
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

# Copy and build dependencies first (cached unless Cargo.toml/lock changes)
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json

# Copy source code and build the application
COPY Cargo.toml Cargo.lock ./
COPY . .
RUN cargo build --release

# Runtime stage
FROM debian:bookworm-slim

# Install runtime dependencies
RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl3 \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copy the binary from builder
COPY --from=builder /app/target/release/margonem-monitor /usr/local/bin/margonem-monitor

# Create non-root user
RUN useradd -m -u 1000 appuser && chown -R appuser:appuser /app
USER appuser

# Set environment variables
ENV RUST_LOG=info

# Run the application
CMD ["margonem-monitor"]