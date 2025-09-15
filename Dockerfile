FROM messense/rust-musl-cross:x86_64-musl AS chef

# Configure Cargo
RUN mkdir -p /root/.cargo && echo '[net]\ngit-fetch-with-cli = true' > /root/.cargo/config.toml

# Install cargo-chef
RUN cargo install cargo-chef

WORKDIR /app

# Plan stage - prepare dependency information
FROM chef AS planner
COPY Cargo.toml Cargo.lock ./
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

# Build stage - build dependencies separately from source
FROM chef AS builder

# Install required dependencies for building
# RUN apt-get update && apt-get install -y \
#     pkg-config \
#     libssl-dev \
#     && rm -rf /var/lib/apt/lists/*

# Copy and build dependencies first (cached unless Cargo.toml/lock changes)
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --target x86_64-unknown-linux-musl --recipe-path recipe.json

# Copy source code and build the application
COPY Cargo.toml Cargo.lock ./
COPY . .
RUN cargo build --release --target x86_64-unknown-linux-musl

# Runtime stage
FROM alpine:latest AS runtime

# Install runtime dependencies
RUN apk --no-cache add ca-certificates

WORKDIR /app

# Copy the binary from builder
COPY --from=builder /app/target/x86_64-unknown-linux-musl/release/margonem-monitor /usr/local/bin/margonem-monitor

# Create non-root user
RUN useradd -m -u 1000 appuser && chown -R appuser:appuser /app
USER appuser

# Set environment variables
ENV RUST_LOG=info

# Run the application
CMD ["margonem-monitor"]