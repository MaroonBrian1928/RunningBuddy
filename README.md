# RunningBuddy

Local-first Rust + Vite/React training dashboard for one athlete, with SQLite
persistence, Strava OAuth/webhook sync, and configurable LLM training advice.

## Development

Copy `.env.example` into your shell or local environment and set the secrets you
need. The app defaults to `sqlite://runningbuddy.db` and `127.0.0.1:3000`.

Use `mise` for project commands. `mise` installs the Rust, Node, and Bun toolchain
declared by this repo; Bun is used for frontend dependency install and Vite
scripts.

```sh
mise run api:dev
mise run web:install
mise run web:dev
mise run test
```

The frontend dev server proxies `/api` and `/strava` requests to the Rust API.

## Docker

Build and run the combined API/frontend image:

```sh
docker build -t runningbuddy .
docker run --rm -p 7317:3000 \
  -v runningbuddy-data:/data \
  --env-file .env \
  runningbuddy
```

Or use Compose for the normal hosted path:

```sh
docker compose up --build
```

Compose maps the app to `http://127.0.0.1:7317` by default. Override it with
`RUNNINGBUDDY_PORT` if needed.

Pushes to `main` publish an image to GitHub Container Registry as
`ghcr.io/<owner>/<repo>:latest` and `ghcr.io/<owner>/<repo>:sha-<commit>`.
