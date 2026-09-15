# Shortcoming: `.env` token seeding outranks the stored Strava grant

Known limitation. Both fixes below are identified and **not implemented**.

## Behavior

`bootstrap_env_token()` (`src/strava.rs:186`) copies
`STRAVA_ACCESS_TOKEN` / `STRAVA_REFRESH_TOKEN` / `STRAVA_TOKEN_SCOPE` /
`STRAVA_TOKEN_EXPIRES_AT` into `strava_tokens` on every process start,
unconditionally replacing the stored row. It compares nothing — not scope, not
expiry, not recency — and no-ops only when both the access and refresh vars are
unset (`config.rs:84` `nonempty_env()` treats blank as unset).

A stale environment value therefore always wins over a live database token.

## Why that is dangerous

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

Useful diagnostic property: `refresh_token_if_needed` (`src/sync.rs:436`) writes
only `access_token`, `refresh_token` and `expires_at`, never `scopes`. A changed
`scopes` column therefore means an OAuth exchange ran, not a refresh.

## Current mitigation

The `.env` token variables are blank on both hosts, which disables seeding and
leaves the database authoritative. Change the grant by reconnecting through
`/api/strava/connect`.

This is a convention, not a guard: repopulating those variables silently re-arms
the overwrite.

## Deferred fix 1 — refuse to downgrade

Guard `bootstrap_env_token()` so env seeding applies only to first-run
provisioning: when a row already exists, overwrite only if the env token is
broader in scope or later in expiry, otherwise warn and return `Ok(false)`.

```rust
if let Some(existing) = load_token_for_athlete(state, athlete_id).await? {
    if !scope_is_superset(&env_scope, &existing.scopes) {
        tracing::warn!("ignoring env token: narrower than the stored grant");
        return Ok(false);
    }
}
```

Requires a `runningbuddy:local` rebuild on winbook.

## Deferred fix 2 — alert on failed syncs

Nothing surfaces a broken sync. `/api/strava/status` already returns
`failed_jobs`, `queued_jobs` and `last_completed_sync_at`
(`src/strava.rs:149-161`), so an Uptime Kuma keyword monitor against
`failed_jobs`, or a staleness check on `last_completed_sync_at`, is sufficient.

Stricter alternative: alert when the stored `scopes` lacks `activity:read`,
which catches the downgrade rather than its symptom.
