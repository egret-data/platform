-- SP5c (iceberg-rest EP): the per-catalog, opt-in switch that turns on
-- RULE enforcement (deny READ on a table carrying a mask / row filter /
-- withhold) at the embedded catalog. INDEPENDENT of 0235's
-- catalog_grant_switch: grant enforcement and rule enforcement switch
-- separately. A catalog with no row enforces NO rules (opt-in), so shipping
-- this migration enables nothing on its own. `undo` sets enforce=false.
CREATE TABLE perm.catalog_rule_enforcement_switch (
    catalog_id   uuid PRIMARY KEY REFERENCES registry.catalogs(id) ON DELETE CASCADE,
    enforce      boolean NOT NULL DEFAULT false,
    switched_at  timestamptz,
    switched_by  text
);
