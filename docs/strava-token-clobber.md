# `.env` Strava token seeding vs. the stored grant

## Behavior

`bootstrap_env_token()` (`src/strava.rs`) seeds `strava_tokens` from
`STRAVA_ACCESS_TOKEN` / `STRAVA_REFRESH_TOKEN` / `STRAVA_TOKEN_SCOPE` /
`STRAVA_TOKEN_EXPIRES_AT` at process start. It no-ops when either token
variable is unset — `config.rs` `nonempty_env()` treats blank as unset — and
when a token row already exists whose scopes the env token does not cover.
Seeding therefore provisions a first run but cannot narrow a live grant.

The env variables are blank on both hosts, leaving the database authoritative.
Change the grant by reconnecting through `/api/strava/connect`.

## Why the guard matters

A downgraded grant cannot recover on its own:

- **Refresh never widens scope.** A refresh token permanently carries the scope
  of the exchange that minted it, so a narrow token chain renews itself
  indefinitely. Only a new OAuth exchange replaces it.
- **The symptom is silent.** Activity endpoints answer `401` with
  `activity:read_permission missing`, while `/athlete` still answers `200`, so
  the app reports itself connected. The failure is recorded only in
  `sync_jobs.last_error`; the container stays healthy.
- **Another app on the same `client_id` is unaffected**, because scope lives on
  the issued token pair rather than the app registration. Its continued success
  is not evidence that this app's token is valid.

Useful diagnostic property: `refresh_token_if_needed` (`src/sync.rs`) writes
only `access_token`, `refresh_token` and `expires_at`, never `scopes`. A changed
`scopes` column therefore means an OAuth exchange ran, not a refresh.

## Known gap — nothing alerts on failed syncs

A broken sync surfaces nowhere. `/api/strava/status` already returns
`failed_jobs`, `queued_jobs` and `last_completed_sync_at`, so an Uptime Kuma
keyword monitor against `failed_jobs`, or a staleness check on
`last_completed_sync_at`, is sufficient. Stricter alternative: alert when the
stored `scopes` lacks `activity:read`, which catches a downgrade rather than its
symptom.
