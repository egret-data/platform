-- 0238_pdp_policy_source.sql
--
-- EO3 (PDP consumes the IR-compiled policy): the GLOBAL, single-row switch that
-- selects which assembler produces the Cedar `PolicySet` the control-plane PDP
-- serves. `CedarPdp::load_inner` reads it on every reload (initial build and
-- every RolesChanged):
--
--   * `legacy` (the DEFAULT, and the state with no row): assemble the set from
--     `load_inner`'s bespoke SQL exactly as before EO3 — the proven path.
--   * `shadow`: also build the egret-cedar-compiled set, compare the two, log /
--     metric any divergence, but still SERVE legacy. A production-data canary.
--   * `compiled`: serve the egret-cedar-compiled set (still comparing each
--     reload); fail-safe to legacy if the compiled build errors.
--
-- Behaviour-preserving: EO2's differential proved `compile()` decision-equivalent
-- to `load_inner()`; shipping this migration with no row keeps every PDP on
-- `legacy`, so it enables nothing on its own. One row only (singleton via the
-- `id = true` primary key). `switched_at` / `switched_by` are audit columns.
CREATE TABLE perm.pdp_policy_source (
    id           boolean PRIMARY KEY DEFAULT true CHECK (id),
    source       text NOT NULL DEFAULT 'legacy'
                 CHECK (source IN ('legacy', 'shadow', 'compiled')),
    switched_at  timestamptz,
    switched_by  text
);
