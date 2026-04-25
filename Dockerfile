# syntax=docker/dockerfile:1

FROM node:22-bookworm-slim AS frontend
WORKDIR /app/frontend
ARG VITE_MAP_STYLE_URL
ARG VITE_MAPTILER_KEY
ARG VITE_STADIA_API_KEY
ENV VITE_MAP_STYLE_URL=${VITE_MAP_STYLE_URL} \
    VITE_MAPTILER_KEY=${VITE_MAPTILER_KEY} \
    VITE_STADIA_API_KEY=${VITE_STADIA_API_KEY}
COPY frontend/package.json frontend/package-lock.json ./
RUN npm ci
COPY frontend ./
RUN npm run build

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
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

ENV RUNNINGBUDDY_BIND_ADDR=0.0.0.0:3000 \
    RUNNINGBUDDY_DATABASE_URL=sqlite:///data/runningbuddy.db

COPY --from=backend /app/target/release/runningbuddy /usr/local/bin/runningbuddy
COPY --from=backend /app/migrations ./migrations
COPY --from=frontend /app/frontend/dist ./frontend/dist

VOLUME ["/data"]
EXPOSE 3000
CMD ["runningbuddy"]
