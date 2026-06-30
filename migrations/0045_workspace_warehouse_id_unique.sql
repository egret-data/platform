-- F2-P2 hardening: the lakekeeper warehouse UUID must be globally unique so the
-- reverse resolver `warehouse_by_lakekeeper_id` (used by catalog OIDC-JWT auth to
-- map a request path prefix to exactly one workspace) is deterministic. lakekeeper
-- already generates unique warehouse ids; this makes it a DB guarantee, turning the
-- cross-workspace IDOR closure from an assumption into an enforced invariant.
CREATE UNIQUE INDEX IF NOT EXISTS workspace_warehouses_warehouse_id_uq
    ON workspace_warehouses (warehouse_id);
