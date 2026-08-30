# DB-gated tests: a skip is no longer a pass

Canonical. Vietnamese reference: `docs/vi/testing/db-gated-tests.md`.

Most integration suites in this workspace need Postgres. Until 2026-08-30 they all
handled its absence the same way: `egret_test_db::url()` returned `None`, the suite's
`skip_without_db!` macro turned that into an early `return`, and libtest printed `ok`.

A suite that asserted nothing was **indistinguishable from one that passed**, except by
its duration. Measured on `cargo test -p egret-dataapi`, same machine, same commit:

| run | binaries | reported | wall clock |
| --- | --- | --- | --- |
| `DATABASE_URL` set | 23 | `373 passed; 0 failed` | 282.69 s |
| `DATABASE_URL` unset | 23 | `373 passed; 0 failed` | 11.31 s |

Identical output. 315 of those 373 tests executed no assertion at all in the second run
(only `serving_auth.rs`'s 9 and `template_render.rs`'s 49 are database-free by design).
An audit of the Data API had to caveat nearly every finding with "the suite reports `ok`
but may have asserted nothing".

## The rule now

`egret_test_db::url()` **panics** when it cannot obtain a test database. Skipping is still
possible, but it has to be asked for:

```bash
EGRET_ALLOW_DB_SKIP=1 cargo test --workspace     # or: just test-no-db
```

The skip is now in the command line that produced the green, instead of being inferable
only from the wall clock. The same panic covers `DATABASE_URL` set but *unreachable* —
that case skipped silently too, and it is the shape a CI run takes when its database
service failed to start.

Two things the opt-out does **not** do, both worth knowing before reading a green:

- It does not cover `#[sqlx::test]`. Those tests never call `egret_test_db::url()`; they
  resolve `DATABASE_URL` through dotenvy themselves and create their own scratch databases.
  They are unaffected by this flag and may still fail without a database.
- It does not unset `DATABASE_URL` for you. If one is exported — or supplied by `.env`
  under `just`'s `dotenv-load` — the suites will still run against it. `just test-no-db`
  passes `env -u DATABASE_URL` for exactly this reason.

## Running with a database (what CI does)

```bash
just up                                                          # Postgres on :5432
export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test
cargo test --workspace
```

`egret_test_db` clones a private, already-migrated database per test process, so the URL
above is only a server address plus a name it derives the admin connection from.

> **Never point `DATABASE_URL` at the dev database `egret`.** `#[sqlx::test]` resolves the
> variable through dotenvy **unconditionally**, so an *unexported* variable does not make
> those tests skip — it makes them read `.env`, which points at the dev database, and they
> create `_sqlx_test` bookkeeping schemas inside it.
> `egret_test_db::assert_database_url_is_exported()` exists to catch exactly that.

## CI

`.github/workflows/ci.yml` runs a `postgres:16` service and sets `DATABASE_URL` at job
level, so **this change does not alter what CI does**. It changes what a laptop with no
database does: previously green-and-empty, now a failure with both ways forward in the
message.

## Engine-gated tests are a separate axis

Tests that need a live query engine gate on their own variables and still self-skip,
because CI has no engine to give them:

| variable | what it unlocks |
| --- | --- |
| `EGRET_TEST_STARROCKS_URL` | StarRocks over the MySQL protocol, e.g. `mysql://localhost:9030` |
| `EGRET_TEST_TRINO_URL` | a Trino coordinator, e.g. `https://localhost:8453` |
| `EGRET_TEST_S3_ENDPOINT` | MinIO |
| `EGRET_TEST_RANGER_E2E` | the Ranger enforcement gate |

See `just test-dataapi-starrocks` for the one Data API path that runs against a real
engine rather than a fake `QueryExecPort`.
