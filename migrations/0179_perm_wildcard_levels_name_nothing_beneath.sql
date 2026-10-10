-- 0179_perm_wildcard_levels_name_nothing_beneath.sql
--
-- SP4d debt Task 14, and it is the half Task 12 (`0178`) deliberately left:
-- a `perm.grants` or `perm.rules` path level that Ranger reads as "every value
-- at this level" may not have a NAMED level beneath it.
--
-- THIS IS NOT "`*` IS ILLEGITIMATE", and the predicate below refuses nothing a
-- steward writes on purpose. A grant scoped to a whole catalog, a grant scoped
-- to a whole schema, `('*','*')`, `('sales','orders','*')` — every shape where
-- a `*` has nothing named under it — stays writable, and the accept-controls in
-- `crates/egret-perm/tests/store.rs` pin each of them. `scope_to_resources`
-- emits `*` ITSELF at every level a scope does not name, so refusing `*`
-- outright would refuse the compiler's own ordinary output.
--
-- WHAT IS WRONG IS A GAP IN THE PATH, AND THIS TABLE ALREADY SAID SO.
-- `0154_perm_model.sql` gave both tables
--
--     CHECK (schema_name IS NOT NULL OR table_name IS NULL)
--     CHECK (table_name  IS NOT NULL OR column_name IS NULL)
--
-- because "a path cannot skip a level: a column implies a table implies a
-- schema". `schema_name = '*'` IS that skip: `*` names no schema — Ranger's
-- matcher reads it as match-any — so `('*', 'orders')` is "a table with no
-- schema", the very scope `grants_check` refuses when it is spelled NULL. So
-- two guards in one table disagreed about whether that scope exists. egret's
-- model had already decided `*/orders` is not expressible; `*` was a way around
-- that decision rather than a feature of it. And `scope_to_resources`' own
-- `wildcard_from_here` latch is the same decision in the emission: once a level
-- is unnamed every level below it is rendered `*`, so NO scope the compiler can
-- render ever carries a named level under an unnamed one. This CHECK is that
-- latch stated over the AUTHORED values.
--
-- THE MEASUREMENT, AS A PAIR, on `egret_test` with `0178`'s two constraints
-- applied inside the same (rolled-back) transaction so the probe sees exactly
-- the branch tip. `scratchpad/t14_probe.sql`; `egret_test` is untouched
-- afterwards (0 rows, constraint absent) so it re-runs as recorded:
--
--   | insert into `perm.grants`                              | result                                    |
--   |--------------------------------------------------------|-------------------------------------------|
--   | `schema_name = '*',  table_name = 'orders'`             | **ACCEPTED**                              |
--   | `schema_name = NULL, table_name = 'orders'`             | rejected by `grants_check`                |
--   | `schema_name = '',   table_name = 'orders'`             | rejected by `grants_scope_levels_are_named`|
--   | `schema_name = 'sales', table_name = '*', col = 'email'`| **ACCEPTED**                              |
--   | `schema_name = '*', table_name = '*', col = 'email'`    | **ACCEPTED**                              |
--   | `perm.rules`: `('*', 'orders', 'email')`                | **ACCEPTED**                              |
--
-- The two rejections are the control, and they are what make this a
-- measurement rather than a probe that could not fail: the SAME level skip,
-- spelled NULL and spelled `''`, is refused on the same connection in the same
-- transaction by two live constraints on these very columns — while spelled
-- `'*'` it was accepted. Row 1 is not a missing guard; it is a third hole in
-- the one guard that exists, and `0178` closed only the second.
--
-- WHAT THE MATCH-ANY LEVEL DOES, measured on the compiler's own artifact rather
-- than asserted. `crates/egret-perm-compile/tests/compile_grants.rs`'s
-- `an_empty_scope_level_would_be_enforced_in_every_schema` compiles the LEGAL
-- grant for `sales.orders`, renders it with `access_to_ranger_json`, replaces
-- the one token under test in the schema level's values, and asks
-- `egret_ranger_eval::resource::match_resource` about it against the real Trino
-- service-def and the request `catalog=warehouse, schema=finance, table=orders,
-- column=amount`:
--
--   | policy `schema` value | MatchType   |
--   |-----------------------|-------------|
--   | `"sales"` (unedited)  | `None`      | <- control
--   | `""`                  | `Self_`     | <- `0178`
--   | `"*"`                 | `Self_`     | <- THIS migration
--
-- Identical reach. A grant written for one schema was enforced across every
-- schema of the catalog.
--
-- AND UNLIKE `''` IT IS REACHABLE FROM THE BROWSER, which is why this is the
-- more reachable half. Stated BY CODE PATH and not by authorship, because five
-- authorship claims in this programme have been wrong:
-- `frontend/src/components/perm/ScopePicker.tsx`'s three levels are free-text
-- `<Input>`s whose `onChange` is `e.target.value || undefined`. That maps `""`
-- to `undefined` — which is exactly why `''` never arrived from the UI — while
-- `"*"` is truthy and passes straight through. `canPickTable = !!value.schema`,
-- so typing `*` into Schema ENABLES the Table field, and Table's own `onChange`
-- clears only `column`, never `schema`. `('*','orders')` is therefore two
-- ordinary keystrokes in the Governance dialogs.
--
-- A RUN OF STARS, NOT THE SINGLE CHARACTER — and that is the mechanism rather
-- than a list of spellings. `RangerAbstractResourceMatcher.buildResourceMatchers()`
-- treats a value that is a pure run of `*` as match-any and CLEARS every other
-- matcher on the level; `egret_ranger_eval::resource::ValueMatcher::classify`
-- reproduces it (`classify("**")` and `classify("***")` are the same match-any
-- as `classify("*")`, pinned by `classify_reproduces_rangers_four_matcher_shapes`),
-- so keying this on the literal `'*'` alone would have closed one spelling and
-- left `'**'` reaching the identical enforcement from the identical free-text
-- field. `^[*]+$` is the whole class, and the Rust twin
-- (`egret_perm_compile::grants::ensure_a_wildcard_level_has_nothing_named_beneath_it`)
-- keys on the same class so the two cannot drift.
--
-- WHAT IS DELIBERATELY NOT IN THE CLASS. `'ord*'` is a PREFIX matcher
-- (`ValueMatcher::StartsWith`), not match-any: it names a set of schemas, the
-- level is not unnamed, and a table under it is not a gap. It stays legal, as
-- does `' '` (a literal resource name — `init()` drops `''` and nothing else).
-- `egret_perm::eval::glob_match` exists precisely because such values are
-- storable and the engine glob-matches them. Every level this compiler emits on
-- BOTH engines declares `wildCard: true` in its service-def
-- (`crates/egret-policy/src/service-defs/{trino,starrocks}.json`: all four
-- access levels, all four `dataMaskDef` levels, all three `rowFilterDef`
-- levels), so "a run of stars is match-any" is engine-independent — which is
-- what lets this rule live at an authoring boundary at all.
--
-- `catalog` IS NOT IN THE PREDICATE, same as in `0178`, and for the same
-- measured reason: the value emitted at that level is `registry.catalogs.name`,
-- and `0147_catalog_registry.sql` gives that column
-- `CHECK (name ~ '^[a-z][a-z0-9_]{0,62}$')` — an allow-list under which `*` is
-- impossible. Weakening that regex re-opens the catalog level here.
--
-- WHY A CHECK **AND** A RUST REFUSAL, when either alone looks sufficient. Tasks
-- 2, 3 and 12 of this plan each mutation-measured that question and neither
-- guard was redundant: with only the Rust call removed, the route returned
-- **500 instead of 400** three separate times, because the constraint caught
-- the write with a message naming a schema internal. So the Rust refusal is
-- what makes a bad POST/PATCH a 400 that names both levels, and this CHECK is
-- what holds for every writer that is not one of those paths: a route added
-- later, `psql`, a test's raw `sqlx::query`. Stated by code path: `perm.grants`
-- has exactly one `INSERT` and one `UPDATE` in any `src/`
-- (`egret_perm::store::create_grant_in_tx`/`update_grant_in_tx`, plus
-- `egret_perm::assignment`, which names no level at all), and create, patch and
-- the rolled-back `/perm/preview` all funnel through `routes::write_grant`.
--
-- VALID, AND NO CLEARING STATEMENT — `0177`/`0178`'s case, not `0176`'s, and
-- measured rather than copied. Across the 29 databases on this host that have
-- `perm.grants` and `perm.rules` (10 more have no `perm` schema at all), the
-- two tables hold **4 grant rows and 3 rule rows in total**, all of them in the
-- three `egret_sp4*` scratch databases, and **not one row anywhere carries `*`
-- at ANY level** — so the violating population is empty by a wider margin than
-- `0178`'s was. A row this CHECK rejects is ENFORCED (that is the finding), so
-- a clearing statement would change what an engine enforces: deleting an
-- `allow` fails closed, but deleting a `deny` written `('*','orders')` WIDENS
-- access, and no one statement is right for both. Nor is a repair one
-- statement — `'*' -> NULL` at the gapped level is not enforcement-equivalent
-- (it makes the level match-any AND forces every level below it to `*`, which
-- is a different, wider scope than the named table the author wrote), and a
-- correct change has to be followed by `recompile_service_in_tx` so the engine
-- stops enforcing the old artifact, which is Rust a migration cannot call. With
-- the measured population empty, `NOT VALID` would buy nothing and would leave
-- a dormant row to surface later on an unrelated write.
--
-- IF A DEPLOYMENT DOES HOLD ONE, this `ALTER TABLE` fails naming the constraint
-- and `migrate()` aborts before the process serves. That is intended. The
-- operator's remedy is a product action: read the predicate from
-- `pg_get_constraintdef` (so this comment is not a second copy of it), list the
-- offenders with `SELECT * FROM perm.grants WHERE NOT (<that predicate>)`, and
-- re-author each through `PATCH /api/control-plane/perm/grants/{id}` — which
-- recompiles every profile it affects in the same transaction — either naming
-- the gapped level or dropping the levels beneath it. `DELETE
-- /perm/grants/{id}` carries no write-side refusal, so removing a row that
-- should never have existed is always possible too.
-- Read it as the Rust twin's two loops: for each level that IS a run of stars,
-- every level BELOW it must be a run of stars or absent. Each disjunct opens
-- with its own `IS NULL` test, so the predicate is total — it is never NULL,
-- and the operator's `WHERE NOT (<predicate>)` above lists exactly the
-- offenders.
ALTER TABLE perm.grants
    ADD CONSTRAINT grants_wildcard_levels_name_nothing_beneath
    CHECK (
        (schema_name IS NULL
            OR schema_name !~ '^[*]+$'
            OR ((table_name IS NULL OR table_name ~ '^[*]+$')
                AND (column_name IS NULL OR column_name ~ '^[*]+$')))
        AND (table_name IS NULL
            OR table_name !~ '^[*]+$'
            OR column_name IS NULL
            OR column_name ~ '^[*]+$')
    );

-- `perm.rules` carries the same three columns, written by the same kind of
-- author through `routes::write_rule`, rendered by the same
-- `scope_to_resources`, and it has the SAME two level-skip CHECKs from `0154`
-- that this predicate is making consistent. It is not the same severity — a
-- match-any level on a mask or a row filter makes the control cover every
-- sibling at that level, so it withholds MORE than the author wrote rather than
-- granting more — and it is included for the same reason `0178` included it:
-- the predicate, the Rust refusal and the test are the same ones, probe row 6
-- above shows the write was accepted, and
-- `rules::ensure_row_filter_scope_is_a_table` — whose whole job is to refuse a
-- row-filter scope that is not exactly catalog+schema+table — counts
-- `schema_name = '*'` as a NAMED schema, exactly as `0178` found it counting
-- `''` as one.
--
-- The Rust twin runs at BOTH rule-side positions — `rules::validate_rule_shape`
-- and `rules::compile_rule` — which is one more than `0178` shipped with. Task
-- 12 left `compile_rule` unguarded on a fail-closed severity argument; that
-- argument is sound about severity and does not answer what a `pub` entry point
-- should emit, so Task 14 added the call. `rules::compile_rule`'s doc carries the
-- reasoning, including why `ensure_row_filter_scope_is_a_table` staying at the
-- authoring boundary is a DIFFERENT case: the shapes it refuses are still
-- storable under `0154`, and `scope_to_resources`' `RowFilter` arm exists to keep
-- such a stored row compiling to something matchable.
--
-- Note which rule shapes this still allows, because they are not gaps: a mask
-- on `('sales','orders','*')` (every column of one table) and a row filter on
-- `('*','*')` (every table of every schema) both leave nothing named under a
-- match-any level, and both stay writable.
ALTER TABLE perm.rules
    ADD CONSTRAINT rules_wildcard_levels_name_nothing_beneath
    CHECK (
        (schema_name IS NULL
            OR schema_name !~ '^[*]+$'
            OR ((table_name IS NULL OR table_name ~ '^[*]+$')
                AND (column_name IS NULL OR column_name ~ '^[*]+$')))
        AND (table_name IS NULL
            OR table_name !~ '^[*]+$'
            OR column_name IS NULL
            OR column_name ~ '^[*]+$')
    );
