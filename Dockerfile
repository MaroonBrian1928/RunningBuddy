# syntax=docker/dockerfile:1

FROM node:22-bookworm-slim AS frontend
WORKDIR /app/frontend
COPY frontend/package.json frontend/package-lock.json ./
RUN npm ci
COPY frontend ./
RUN npm run build

FROM rust:1-bookworm AS backend
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
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
