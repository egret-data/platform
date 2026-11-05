-- G14: metadata tables (#821) next to a column mask (ann exempt), a row filter, a deny read
-- (ann), a classification mask and a clearance (bob cleared).
-- users: ann,bob,carol
-- setup: R_CLEARED = POST /api/control-plane/access/roles {"scope":"platform","name":"l2_cleared"}
-- setup: POST /api/control-plane/users/@UID_bob@/roles {"roleId":"@R_CLEARED@"}
-- setup: T_PII = POST /api/control-plane/tags {"key":"l2-pii","name":"L2 PII"}
-- setup: T_SECRET = POST /api/control-plane/tags {"key":"l2-secret","name":"L2 secret"}
-- setup: PUT /api/control-plane/warehouses/@WH@/resource-tags {"schema":"s1","table":"t2","column":"v","tag_id":"@T_PII@"}
-- setup: PUT /api/control-plane/warehouses/@WH@/resource-tags {"schema":"s2","table":"t3","column":"salary","tag_id":"@T_SECRET@"}
-- setup: PUT /api/control-plane/tags/@T_SECRET@/access-policy {"required_roles":["l2_cleared"],"required_groups":[]}
-- setup: POST /api/control-plane/perm/grants {"effect":"allow","actions":["read"],"scope":{"catalog_id":"@CAT@"},"principals":{"include":[{"kind":"group","ref":"public"}],"exclude":[]}}
-- setup: POST /api/control-plane/perm/grants {"effect":"deny","actions":["read"],"scope":{"catalog_id":"@CAT@","schema":"s2","table":"w"},"principals":{"include":[{"kind":"user","ref":"ann"}],"exclude":[]}}
-- setup: POST /api/control-plane/perm/rules {"name":"l2-g14-mask","kind":{"type":"mask","mask_type":"MASK_NULL"},"selector":{"type":"path","catalog_id":"@CAT@","schema":"s1","table":"t1","column":"email"},"exempt":{"include":[{"kind":"user","ref":"ann"}],"exclude":[]}}
-- setup: POST /api/control-plane/perm/rules {"name":"l2-g14-filter","kind":{"type":"row_filter","filter_expr":"id < 100"},"selector":{"type":"path","catalog_id":"@CAT@","schema":"s1","table":"t2"},"exempt":{"include":[],"exclude":[]}}
-- setup: POST /api/control-plane/perm/rules {"name":"l2-g14-tagmask","kind":{"type":"mask","mask_type":"MASK_HASH"},"selector":{"type":"tag","key":"l2-pii"},"exempt":{"include":[],"exclude":[]}}
-- expect: bob | SELECT count(*) FROM lake.s1.t2 | [[99]]
-- expect: bob | SELECT salary FROM lake.s2.t3 WHERE id = 1 | [[100]]
-- expect: carol | SELECT salary FROM lake.s2.t3 WHERE id = 1 | denied
-- expect: carol | SELECT email FROM lake.s1.t1 WHERE id = 1 | [[null]]
SELECT count(*) FROM lake.s1."t2$files"
SELECT count(*) FROM lake.s2."t3$files"
SELECT count(*) FROM lake.s2."w$snapshots"
SELECT count(*) FROM lake.s1."t1$snapshots"
SELECT count(*) FROM lake.s1."t2$snapshots"
SHOW TABLES FROM lake.s2
SELECT id, salary FROM lake.s2.t3 ORDER BY id LIMIT 3
SELECT count(*) FROM lake.s1.t2
