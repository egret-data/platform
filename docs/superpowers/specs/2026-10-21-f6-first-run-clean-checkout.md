# F6: First run from a clean checkout (KI-136, KI-140, KI-141)

Package F6 of the known-issues fix programme
(`docs/superpowers/plans/2026-10-21-known-issues-fix-programme.md`).

## Premise, per row

- **KI-136 (S1)**: does not reproduce on current `main`. #704 (the Docker demo stack, work package A) added
  `EGRET_STORAGE__*` and `EGRET_SECRETS__MASTER_KEY` to `.env.example`. With the file as it was before #704,
  the new test `env_example_loads_and_validates` fails with
  `the apps API requires [storage] bucket, access_key, secret_key`; with the current file it passes.
  What was missing is a guard so the file cannot rot again: that is what this package adds.
- **KI-140 (S3)**: reproduces. `Config::figment()` merges every `EGRET_*` variable and every `egret.toml`
  key, and serde drops the ones no field takes, so `EGRET_SERVER__PORTT=9090` leaves port 8080 with no sign.
- **KI-141 (S3)**: reproduces. `Config::default()` sets `catalog.public_url = Some("http://localhost:8080")`
  as a literal, so `EGRET_SERVER__PORT=9090` leaves the URL, and the machine-IdP issuer that falls back to
  it, on 8080.

## What each row does

- **KI-136**: two `egret-config` tests read the repository's `.env.example` at compile time. One loads it
  through the real loader (`Config::load`, in a `figment::Jail` whose environment holds only the file's
  variables) and runs `Config::validate`. The other checks the storage credentials equal the
  `RUSTFS_ACCESS_KEY` / `RUSTFS_SECRET_KEY` the dev compose starts RustFS with, and that the endpoint is the
  port it publishes. Both run in the normal `cargo test` CI job. KI-137 (nothing creates the bucket) and
  KI-138 (the compose Trino has no catalogs) stay open and are not touched.
- **KI-140**: `Config::unknown_keys()` deserialises the merged figment value into `Config` once more
  through `serde_ignored` and returns every key serde skipped: an `EGRET_*` variable by its exact name, a
  file key as a dotted path with the file it came from, each with the closest real key within two edits
  (`EGRET_SERVER__PORTT` -> `EGRET_SERVER__PORT`, `EGRET_DATABASE_URL` -> `EGRET_DATABASE__URL`).
  `main.rs` logs each as a `WARN` line right after tracing starts, for every subcommand. A warning, not an
  error, so deployments carrying a stale key keep starting. The variables egret reads with
  `std::env::var` outside the struct are allow-listed in one place, `ENV_OUTSIDE_CONFIG` (plus the
  test-harness prefixes `EGRET_TEST_`, `EGRET_E2E_`, `EGRET_ALLOW_`, because `just` loads `.env` into every
  recipe); a test scans `crates/*/src` for literal `env::var("EGRET_...")` reads and fails if one is not on
  the list.
- **KI-141**: decision: derive. The default of `catalog.public_url` becomes `None`, and `Config::load`
  fills an unset or blank value with `http://localhost:<server.port>`. An explicit value is kept whatever
  the port. The doc comment on `CatalogSettings::public_url` says so.

## Dependency

`serde_ignored` 0.1.14 (dtolnay), new in the workspace, only in `egret-config`.
