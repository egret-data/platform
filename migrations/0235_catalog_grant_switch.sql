-- 0235_catalog_grant_switch.sql
--
-- SP5b Task 5 (catalog merge / one grant store): the per-catalog switch that
-- selects which store the embedded-catalog ENFORCEMENT reads. The request guard
-- (`egret-bin`'s `EgretCatalogGuard::check`) and the lakekeeper authorizer
-- (`egret_catalog_engine::authorizer::EgretAuthorizer::client_may`) consult it
-- per request:
--
--   * `catalog_rbac` (the DEFAULT, and the state of every catalog with no row):
--     decide from `catalog.catalog_grants` by the role's grants, exactly as
--     before this task — the legacy path, untouched.
--   * `unified`: decide the caller's service-account principal via `perm-eval`
--     over the folded `perm.grants` + `perm.securable_owners` of the catalog.
--
-- The switch is behaviour-preserving: Task 2's fold proved the two stores
-- equivalent, and flipping `source` to `unified` changes nothing a caller may
-- do (Task 6 gates the flip on a live equivalence check). A catalog with no row
-- reads `catalog_rbac`, so shipping this migration enables nothing on its own.
--
-- `switched_at` / `switched_by` are audit columns, filled when a row is written
-- or flipped; a defaulted row (never flipped) leaves them NULL.

CREATE TABLE perm.catalog_grant_switch (
    catalog_id   uuid PRIMARY KEY REFERENCES registry.catalogs(id) ON DELETE CASCADE,
    source       text NOT NULL DEFAULT 'catalog_rbac'
                 CHECK (source IN ('catalog_rbac', 'unified')),
    switched_at  timestamptz,
    switched_by  text
);
