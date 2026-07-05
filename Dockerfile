# syntax=docker/dockerfile:1

FROM oven/bun:1-debian AS frontend
WORKDIR /app/frontend
ARG VITE_MAP_STYLE_URL
ENV VITE_MAP_STYLE_URL=${VITE_MAP_STYLE_URL}
COPY frontend/package.json frontend/bun.lock ./
RUN bun install --frozen-lockfile
COPY frontend ./
RUN bun run build

FROM rust:1-bookworm AS backend
WORKDIR /app

# Cache dependencies
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && \
    echo "fn main() { println!(\"dummy main\"); }" > src/main.rs && \
    echo "pub fn dummy() {}" > src/lib.rs && \
    cargo build --release && \
    rm -rf src && \
    rm -rf target/release/deps/runningbuddy* target/release/runningbuddy* target/release/.fingerprint/runningbuddy*

# Build application
COPY src ./src
COPY migrations ./migrations
RUN cargo build --release

FROM debian:bookworm-slim AS runtime
WORKDIR /app
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates tzdata \
    && rm -rf /var/lib/apt/lists/*

ENV RUNNINGBUDDY_BIND_ADDR=0.0.0.0:3000 \
    RUNNINGBUDDY_DATABASE_URL=sqlite:///data/runningbuddy.db

COPY --from=backend /app/target/release/runningbuddy /usr/local/bin/runningbuddy
COPY --from=backend /app/migrations ./migrations
COPY --from=frontend /app/frontend/dist ./frontend/dist

VOLUME ["/data"]
EXPOSE 3000
CMD ["runningbuddy"]
