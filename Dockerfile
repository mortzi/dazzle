# Build Stage
FROM rust:1.94.1-slim-bookworm AS builder
WORKDIR /app

RUN apt-get update && \
    apt-get install -y protobuf-compiler && \
    rm -rf /var/lib/apt/lists/*

COPY Cargo.toml Cargo.lock ./

# Cache dependencies
RUN mkdir src && echo "fn main() {}" > src/main.rs
RUN cargo build --release
RUN rm -f target/release/dazzle*
# Build app
COPY src ./src
RUN cargo build --release

# Runtime stage
FROM debian:bookworm-slim AS runner

ARG PORT
ENV SERVICE_PORT=${PORT}

WORKDIR /app
RUN apt-get update && \
    apt-get install -y libssl3 ca-certificates && \
    rm -rf /var/lib/apt/lists/* \

COPY --from=builder /app/target/release/dazzle ./dazzle
EXPOSE ${SERVICE_PORT}
CMD ["./dazzle"]
