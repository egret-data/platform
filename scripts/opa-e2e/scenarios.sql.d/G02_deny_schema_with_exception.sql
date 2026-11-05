-- G02: deny read on a schema to public except carol (denyExceptions), under a public catalog read.
-- users: ann,carol
-- setup: POST /api/control-plane/perm/grants {"effect":"allow","actions":["read"],"scope":{"catalog_id":"@CAT@"},"principals":{"include":[{"kind":"group","ref":"public"}],"exclude":[]}}
-- setup: POST /api/control-plane/perm/grants {"effect":"deny","actions":["read"],"scope":{"catalog_id":"@CAT@","schema":"s2"},"principals":{"include":[{"kind":"group","ref":"public"}],"exclude":[{"kind":"user","ref":"carol"}]}}
-- expect: carol | SELECT count(*) FROM lake.s2.t3 | [[100]]
-- expect: ann | SELECT count(*) FROM lake.s2.t3 | denied
SELECT id, salary, dept FROM lake.s2.t3 ORDER BY id LIMIT 5
SELECT count(*) FROM lake.s2.t3
SELECT count(*) FROM lake.s1.t1
SHOW TABLES FROM lake.s2
