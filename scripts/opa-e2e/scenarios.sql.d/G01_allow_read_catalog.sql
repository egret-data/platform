-- G01: allow read on the whole catalog, two users (carol has no grant).
-- users: ann,bob,carol
-- setup: POST /api/control-plane/perm/grants {"effect":"allow","actions":["read"],"scope":{"catalog_id":"@CAT@"},"principals":{"include":[{"kind":"user","ref":"ann"},{"kind":"user","ref":"bob"}],"exclude":[]}}
-- expect: ann | SELECT count(*) FROM lake.s1.t1 | [[1000]]
-- expect: carol | SELECT count(*) FROM lake.s1.t1 | denied
SELECT count(*) FROM lake.s1.t1
SELECT id, email, ssn, region, name FROM lake.s1.t1 ORDER BY id LIMIT 5
SELECT id, salary, dept FROM lake.s2.t3 ORDER BY id LIMIT 5
SELECT count(*) FROM lake.s1.t2
