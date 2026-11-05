-- G18 (adapted): a table renamed under a grant and a classification. ann reads s2.t3; l2-pii on
-- s2.t3.salary under a MASK_NULL rule; then the admin renames s2.t3 to s2.t3_v2 through the
-- catalog. Grants and classifications follow the rename (#757 and its grant twin).
-- users: ann,bob
-- setup: T_PII = POST /api/control-plane/tags {"key":"l2-pii","name":"L2 PII"}
-- setup: PUT /api/control-plane/warehouses/@WH@/resource-tags {"schema":"s2","table":"t3","column":"salary","tag_id":"@T_PII@"}
-- setup: POST /api/control-plane/perm/rules {"name":"l2-g18-mask","kind":{"type":"mask","mask_type":"MASK_NULL"},"selector":{"type":"tag","key":"l2-pii"},"exempt":{"include":[],"exclude":[]}}
-- setup: POST /api/control-plane/perm/grants {"effect":"allow","actions":["read"],"scope":{"catalog_id":"@CAT@","schema":"s2","table":"t3"},"principals":{"include":[{"kind":"user","ref":"ann"}],"exclude":[]}}
-- setup-sql: ALTER TABLE lake.s2.t3 RENAME TO lake.s2.t3_v2
SELECT id, salary, dept FROM lake.s2.t3_v2 ORDER BY id LIMIT 3
SELECT count(*) FROM lake.s2.t3_v2
SHOW TABLES FROM lake.s2
