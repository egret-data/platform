-- G09: a classification row filter. l2-region on column s1.t1.region and on the whole table s2.t3
-- (column_name = ''); row filter id % 2 = 0 by the tag, ann exempt (KI-287).
-- users: ann,bob
-- setup: T_REGION = POST /api/control-plane/tags {"key":"l2-region","name":"L2 region"}
-- setup: PUT /api/control-plane/warehouses/@WH@/resource-tags {"schema":"s1","table":"t1","column":"region","tag_id":"@T_REGION@"}
-- setup: PUT /api/control-plane/warehouses/@WH@/resource-tags {"schema":"s2","table":"t3","tag_id":"@T_REGION@"}
-- setup: POST /api/control-plane/perm/grants {"effect":"allow","actions":["read"],"scope":{"catalog_id":"@CAT@"},"principals":{"include":[{"kind":"group","ref":"public"}],"exclude":[]}}
-- setup: POST /api/control-plane/perm/rules {"name":"l2-g09-rf","kind":{"type":"row_filter","filter_expr":"id % 2 = 0"},"selector":{"type":"tag","key":"l2-region"},"exempt":{"include":[{"kind":"user","ref":"ann"}],"exclude":[]}}
SELECT count(*) FROM lake.s1.t1
SELECT count(*) FROM lake.s2.t3
SELECT count(*) FROM lake.s1.t2
