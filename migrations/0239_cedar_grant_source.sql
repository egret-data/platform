-- 0239_cedar_grant_source.sql
--
-- EO4 (one grant store for egret objects): the GLOBAL, single-row switch that
-- selects which table the control-plane PDP reads per-request grant links from.
-- `AccessGrantStore::grants_for` reads it on every reload path:
--
--   * `legacy` (the DEFAULT, and the state with no row): read `access_grants`
--     exactly as before EO4 — the proven path.
--   * `unified`: read the one grant store's mirror `perm.cedar_grants` (kept
--     live-consistent by the trigger in 0240). Decisions are identical by
--     construction (same rows → same links), proved by the equivalence gate
--     before the flip.
--
-- Shipping this with no row keeps every PDP on `legacy`, so it enables nothing
-- on its own. Singleton via the `id = true` primary key; audit columns record
-- the last flip.
CREATE TABLE perm.cedar_grant_source (
    id           boolean PRIMARY KEY DEFAULT true CHECK (id),
    source       text NOT NULL DEFAULT 'legacy'
                 CHECK (source IN ('legacy', 'unified')),
    switched_at  timestamptz,
    switched_by  text
);
