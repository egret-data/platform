# P1 — classify a recompile's lock timeout by SQLSTATE (#793)

## Row

GitHub #793: `compile_is_lock_timeout` (`egret-catalog-engine/src/view_hold.rs`)
matched `CompileError::Db(m)` on the text "lock timeout". Postgres translates the
message under a non-English `lc_messages` (PostgreSQL 16's German catalogue sends
"storniere Anfrage wegen Zeitüberschreitung einer Sperre"), so a busy refusal was
answered as an overlap refusal (view hold), or as a plain refusal (table hook
dry run, table-move copy).

Premise on `origin/main` (99270626): reproduces. Every `CompileError::Db` was
built from `e.to_string()`, so the SQLSTATE was lost before the classifier saw it.

## Shape (owner-approved)

- `egret_perm::CompileError::LockTimeout(String)`: same rendered text and the same
  `Display` as `Db`; `CompileError` stays `Clone + PartialEq + Eq`.
- ONE conversion reads the SQLSTATE: `CompileError::from_db(sqlx::Error)` and
  `CompileError::from_db_in(context, sqlx::Error)` (both through one private
  `classified`), 55P03 → `LockTimeout`, anything else → `Db`.
  `CompileError::from_perm(PermError)` and `overlap::store_error(PolicyStoreError)`
  send their `Db(sqlx::Error)` arm through `from_db`.
- Construction sites from a `sqlx::Error` are converted mechanically. Sites that
  wrap a non-database error (a JSON parse, a missing field, an internal invariant)
  stay `Db`.
- `compile_is_lock_timeout` matches the variant only.
- Every `match` that answers `Db` (500 / `Internal`) answers `LockTimeout` the same
  way, so HTTP behaviour outside the hold/hook busy paths is unchanged.

## Decisions

- `PermError` is NOT changed: its `Db` variant already carries the `sqlx::Error`,
  and `table_hook` already classifies it by SQLSTATE (`is_lock_timeout` on the
  `sqlx::Error`); no text match on `PermError` exists. Where a `PermError` becomes
  a `CompileError` it now goes through `from_perm`, so the code survives.
- A `LockTimeout` reaching a route outside the hold/hook keeps today's answer
  (500 / internal), not a new 503: conservative, no behaviour change in scope.

## Tests

- `view_hold::tests::a_recompile_lock_timeout_is_read_from_its_sqlstate_not_its_message`:
  a real Postgres error with SQLSTATE 55P03 and the German PG16 text is a lock
  timeout; the English text under 40P01 is not. The test Postgres has no German
  locale (`SET lc_messages = 'de_DE.utf8'` is refused), so the translated error is
  raised with `RAISE ... USING ERRCODE`.
- `view_hold::tests::a_recompile_that_runs_out_of_lock_timeout_is_the_busy_refusal`:
  a real `lock_timeout` on a held profile row; `recompile_services_in_tx` returns
  `LockTimeout` and `bump_and_recompile` answers the busy refusal.
