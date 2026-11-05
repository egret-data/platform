-- G08: a classification mask. l2-pii on column s1.t1.email under MASK_HASH by the tag, ann exempt.
-- l2-pii on the whole table s2.t3 (column_name = '') is refused while that mask exists (measured:
-- 400 table_tag_mask_conflict, either order), so the table-level attachment carries a second,
-- unmasked classification l2-conf instead; table-level clearance is G11.
-- users: ann,bob
-- setup: T_PII = POST /api/control-plane/tags {"key":"l2-pii","name":"L2 PII"}
-- setup: T_CONF = POST /api/control-plane/tags {"key":"l2-conf","name":"L2 confidential"}
-- setup: PUT /api/control-plane/warehouses/@WH@/resource-tags {"schema":"s1","table":"t1","column":"email","tag_id":"@T_PII@"}
-- setup: PUT /api/control-plane/warehouses/@WH@/resource-tags {"schema":"s2","table":"t3","tag_id":"@T_CONF@"}
-- setup: POST /api/control-plane/perm/grants {"effect":"allow","actions":["read"],"scope":{"catalog_id":"@CAT@"},"principals":{"include":[{"kind":"group","ref":"public"}],"exclude":[]}}
-- setup: POST /api/control-plane/perm/rules {"name":"l2-g08-mask","kind":{"type":"mask","mask_type":"MASK_HASH"},"selector":{"type":"tag","key":"l2-pii"},"exempt":{"include":[{"kind":"user","ref":"ann"}],"exclude":[]}}
-- refused: PUT /api/control-plane/warehouses/@WH@/resource-tags {"schema":"s2","table":"t3","tag_id":"@T_PII@"}
SELECT id, email, name FROM lake.s1.t1 ORDER BY id LIMIT 3
SELECT count(DISTINCT email) FROM lake.s1.t1
SELECT id, salary, dept FROM lake.s2.t3 ORDER BY id LIMIT 3
