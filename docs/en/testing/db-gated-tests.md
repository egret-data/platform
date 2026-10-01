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

### The opt-out may not come from `.env`

That whole claim — *the skip appears in the invocation* — is worth exactly as much as the
guarantee that the variable could not have arrived from somewhere nobody looks. A `.env`
breaks it in one untracked line: `justfile` has `set dotenv-load := true`, so `.env` enters
the environment of **every** recipe, and `#[sqlx::test]`'s `dotenvy::dotenv()` loads the
whole file into the process environment besides. Either route would make `just check`
report a full green having asserted nothing, and nothing would be visible on the terminal —
the skip's message is swallowed by libtest precisely *because* the test passes.

So `egret_test_db` reads the **file**, not the environment, and **refuses** an opt-out a
`.env` at or above the working directory could have supplied:

```
EGRET_ALLOW_DB_SKIP is set by /path/to/.env, so this run cannot show that anyone ASKED
to skip the DB-gated suites. […] Delete the EGRET_ALLOW_DB_SKIP line from that file.
```

This is structural rather than per-recipe: it holds for a bare `cargo test`, and it cannot
be forgotten when someone adds the next recipe. What it *cannot* see is a variable exported
in a shell profile — inside the process that is indistinguishable from one the operator
typed. So the recipes that mean "everything ran" also carry `{{ no_db_skip }}`
(`env -u EGRET_ALLOW_DB_SKIP`), and
`crates/egret-test-db/tests/every_test_recipe_refuses_the_skip.rs` fails if a `justfile`
recipe runs `cargo test` without either scrubbing the variable or setting it deliberately.

### Two things the opt-out does not do

- It does not cover `#[sqlx::test]`. Those tests never call `egret_test_db::url()`; they
  resolve `DATABASE_URL` through dotenvy themselves and create their own scratch databases.
  They are unaffected by this flag and may still fail without a database — see below.
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

### The template is named after the migration set, not fixed

The clone comes from a template, and that template's name is `egret_test_tmpl_` plus a
digest of the **`(version, checksum)` pairs of every embedded migration** — so two branches
at different migration sets use different templates and neither can meet the other's schema.

It used to be the single name `egret_test_template`, which meant every worktree on every
branch shared one template and whichever branch migrated it last decided the schema the
others tested against. Measured twice on 2026-10-01 on one machine: a tree at `0159` met a
template another checkout had taken to `0160` and 84 tests failed with one identical
`VersionMissing(160)`; then, with `main` at `0162`, a suite went 6/6 green and the next
binary hit `VersionMissing(163)` **mid-run** because a third worktree advanced the template
while the measurement was in flight. That message reads like a missing migration — a product
defect — which is why the fix is a name that cannot be wrong rather than a rule to remember.

Two consequences:

- **"Drop `egret_test_template` and re-run" is obsolete.** Older plan documents in
  `docs/en/plans/` tell you to do that after adding or editing a migration. A new or edited
  migration now changes the digest, so it asks for a template of its own and builds it from
  empty. Nothing needs dropping. (The old `egret_test_template` database, if your server
  still has one, is simply unused; drop it by hand whenever no suite is running.)
- **Templates are swept, not accumulated.** A template whose database comment says it has
  not been used for 14 days, and which has no sessions on it, is dropped by the next run —
  under the same advisory lock as the create, so a template another process is about to
  clone from cannot be caught. A template with no readable marker is never dropped.

> **Never point `DATABASE_URL` at the dev database `egret`.** `#[sqlx::test]` resolves the
> variable through dotenvy **unconditionally**, so an *unexported* variable does not make
> those tests skip — it makes them read `.env`, which points at the dev database, and they
> create `_sqlx_test` bookkeeping schemas inside it.
> `egret_test_db::assert_database_url_is_exported()` exists to catch exactly that.

### Every `#[sqlx::test]` binary carries that guard

Not a style rule — a measured hole. With `DATABASE_URL` unset,
`egret-automation/tests/report_tick.rs` (7 tests) and `tick_end_to_end.rs` (11) **passed**,
having written their scratch databases into the dev `egret`: `_sqlx_test.databases` inside
it showed `n_tup_ins = 338`. The guard existed, in 5 of the 24 files that needed it.

It is now in all 24 — one per **binary**, since the guard has to share a process with the
tests it protects: each `tests/<name>.rs` carries its own, and the four crates with
`#[sqlx::test]` cases in `src/` carry a `dotenv_fallback_guard` module in `src/lib.rs` for
their `--lib` binary.
`crates/egret-test-db/tests/every_sqlx_test_binary_guards_the_dotenv_fallback.rs` scans the
tree and fails, naming the file, when a binary using `#[sqlx::test]` has no guard — so the
25th file cannot be added without one.

This is detection, not prevention: libtest gives no ordering, so a run that trips the guard
still creates its scratch databases before going red. Prevention would need a hook running
before any test in the process; sqlx has none, a `.cargo/config.toml` `[env]` default for
`DATABASE_URL` would redirect `cargo run -p egret-bin` (the dev server) too, and a pre-main
constructor only runs if the linker keeps a crate nothing references. Red-and-noisy beats
green-and-silent.

## CI

`.github/workflows/ci.yml` runs a `postgres:16` service and sets `DATABASE_URL` at
**workflow** level — the top-level `env:` block, which every job inherits — so **this
change does not alter what CI does**. It changes what a laptop with no database does:
previously green-and-empty, now a failure with both ways forward in the message.

## The shared store is gated the same way, and for the same reason

Package 4 moved the Data API's rate-limit buckets and response cache into a shared
Redis/Valkey. `crates/egret-dataapi/tests/serving_shared_store.rs` is the only suite that
can tell whether that actually happened — it inspects the store from an **independent**
connection, so an implementation that had quietly answered from a `HashMap` reddens it.

A suite with that job may not self-skip, so it follows the rule above rather than the
engine table below:

| variable | behaviour when unset |
| --- | --- |
| `EGRET_TEST_SHARED_STORE_URL` | **panics** — e.g. `redis://127.0.0.1:6399` |
| `EGRET_ALLOW_STORE_SKIP=1` | opts out, in the invocation |

```bash
docker run -d --name egret-test-valkey -p 6399:6379 valkey/valkey:8
EGRET_TEST_SHARED_STORE_URL=redis://127.0.0.1:6399 cargo test -p egret-dataapi
```

CI runs a `valkey/valkey:8` service and sets the variable at workflow level, so CI is
never the environment where this goes quiet. Nothing else in the workspace reads it: the
suite needs no database, and every other Data API suite drives the in-process store.

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
