-- G07: path row filters: on a table (region = 'us', ann exempt) and on a schema (wildcard
-- table `*`: id % 2 = 0 on every table of s2; a schema-only scope is refused at write, measured).
-- users: ann,bob
-- setup: POST /api/control-plane/perm/grants {"effect":"allow","actions":["read"],"scope":{"catalog_id":"@CAT@"},"principals":{"include":[{"kind":"group","ref":"public"}],"exclude":[]}}
-- setup: POST /api/control-plane/perm/rules {"name":"l2-g07-table","kind":{"type":"row_filter","filter_expr":"region = 'us'"},"selector":{"type":"path","catalog_id":"@CAT@","schema":"s1","table":"t1"},"exempt":{"include":[{"kind":"user","ref":"ann"}],"exclude":[]}}
-- setup: POST /api/control-plane/perm/rules {"name":"l2-g07-schema","kind":{"type":"row_filter","filter_expr":"id % 2 = 0"},"selector":{"type":"path","catalog_id":"@CAT@","schema":"s2","table":"*"},"exempt":{"include":[],"exclude":[]}}
SELECT count(*) FROM lake.s1.t1
SELECT region, count(*) FROM lake.s1.t1 GROUP BY region ORDER BY region
SELECT count(*), min(id) FROM lake.s2.t3
SELECT count(*) FROM lake.s1.t2
